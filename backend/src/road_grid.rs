/// Imports and processes the road grid.
use crate::models::{grid_cell, grid_node, grid_way};
use crate::services::overpass::{self, OsmElement};
use boostvoronoi::prelude::*;
use geo::BooleanOps;
use geo_types::{Coord, LineString, Polygon};
use geojson::Feature;
use geojson::Geometry;
use geojson::Value;
use geojson::feature::Id;
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::EntityTrait;
use sea_orm::QueryFilter;
use sea_orm::prelude::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, PaginatorTrait};
use std::collections::{HashMap, HashSet};

/// OSM coordinates are persisted as fixed-point integers scaled by 1e7 (E7),
/// the same convention OpenStreetMap uses. This keeps full precision without
/// storing floats.
pub const COORD_SCALE: f64 = 1e7;

/// Maximum rows per bulk insert, kept well under SQLite's bound-parameter limit.
const INSERT_CHUNK: usize = 300;

const SCALING_FACTOR: f64 = 10_000_000.;

/// Create boostvoronoi point from coordinate.
///
/// boostvoronoi only supports integer types so cast the f64 to i64.
fn point_i64(point_f64: &[f64]) -> Option<Point<i64>> {
    let [x, y] = point_f64 else {
        return None;
    };
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    Some(Point::new(
        (point_f64[0] * COORD_SCALE) as i64,
        (point_f64[1] * COORD_SCALE) as i64,
    ))
}

fn line_i64(start: &[f64], end: &[f64]) -> Option<Line<i64>> {
    let start = point_i64(start)?;
    let end = point_i64(end)?;
    if start == end {
        // Skip zero-length segments.
        return None;
    }
    Some(Line::new(start, end))
}

fn get_segment_pairs(segment_coords: &[Vec<f64>]) -> Vec<Line<i64>> {
    segment_coords
        .iter()
        .zip(segment_coords.iter().skip(1))
        .filter_map(|(start, end)| line_i64(start, end))
        .collect()
}

fn get_bounding_box(minimum: Coord, maximum: Coord) -> Polygon {
    Polygon::new(
        LineString(vec![
            Coord {
                x: minimum.x,
                y: minimum.y,
            },
            Coord {
                x: minimum.x,
                y: maximum.y,
            },
            Coord {
                x: maximum.x,
                y: maximum.y,
            },
            Coord {
                x: maximum.x,
                y: minimum.y,
            },
        ]),
        vec![],
    )
}

/// Reason a pair of input segments violates boostvoronoi's precondition that
/// segments only touch at shared endpoints.
enum SegmentConflict {
    /// Interiors cross at a point.
    Crossing,
    /// Collinear and overlapping along a shared span.
    CollinearOverlap,
    /// An endpoint of one segment lands in the interior of the other.
    TJunction,
}

/// Signed area sign of triangle (o, a, b): 1 left turn, -1 right turn, 0 collinear.
fn orientation(o: (i128, i128), a: (i128, i128), b: (i128, i128)) -> i32 {
    let value = (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    value.signum() as i32
}

/// Whether collinear point `c` lies within the bounding box of segment `a`-`b`.
fn within_bbox(a: (i128, i128), b: (i128, i128), c: (i128, i128)) -> bool {
    c.0 >= a.0.min(b.0) && c.0 <= a.0.max(b.0) && c.1 >= a.1.min(b.1) && c.1 <= a.1.max(b.1)
}

/// Classify how two segments touch, returning `None` when they are disjoint or
/// only meet at a mutual endpoint (both legal for boostvoronoi).
fn classify_conflict(
    a0: (i128, i128),
    a1: (i128, i128),
    b0: (i128, i128),
    b1: (i128, i128),
) -> Option<SegmentConflict> {
    let d1 = orientation(b0, b1, a0);
    let d2 = orientation(b0, b1, a1);
    let d3 = orientation(a0, a1, b0);
    let d4 = orientation(a0, a1, b1);

    // Interiors cross when each segment straddles the other's supporting line.
    if d1 * d2 < 0 && d3 * d4 < 0 {
        return Some(SegmentConflict::Crossing);
    }

    // Fully collinear: flag when the 1-D overlap is longer than a single point.
    if d1 == 0 && d2 == 0 && d3 == 0 && d4 == 0 {
        let horizontal = (a1.0 - a0.0).abs() >= (a1.1 - a0.1).abs();
        let key = |p: (i128, i128)| if horizontal { p.0 } else { p.1 };
        let (a_lo, a_hi) = (key(a0).min(key(a1)), key(a0).max(key(a1)));
        let (b_lo, b_hi) = (key(b0).min(key(b1)), key(b0).max(key(b1)));
        if a_lo.max(b_lo) < a_hi.min(b_hi) {
            return Some(SegmentConflict::CollinearOverlap);
        }
        return None;
    }

    // An endpoint sitting on the other segment's interior (not a shared vertex).
    if d1 == 0 && within_bbox(b0, b1, a0) && a0 != b0 && a0 != b1 {
        return Some(SegmentConflict::TJunction);
    }
    if d2 == 0 && within_bbox(b0, b1, a1) && a1 != b0 && a1 != b1 {
        return Some(SegmentConflict::TJunction);
    }
    if d3 == 0 && within_bbox(a0, a1, b0) && b0 != a0 && b0 != a1 {
        return Some(SegmentConflict::TJunction);
    }
    if d4 == 0 && within_bbox(a0, a1, b1) && b1 != a0 && b1 != a1 {
        return Some(SegmentConflict::TJunction);
    }
    None
}

/// Log every input segment pair that breaks boostvoronoi's precondition, using
/// an x-interval sweep so only overlapping-x candidates are pairwise tested.
/// This proves which geometry causes the `is_finite()` panic before the build.
fn report_invalid_segments(segments: &[Line<i64>], segment_ways: &[i64]) {
    let coords: Vec<((i128, i128), (i128, i128))> = segments
        .iter()
        .map(|line| {
            (
                (line.start.x as i128, line.start.y as i128),
                (line.end.x as i128, line.end.y as i128),
            )
        })
        .collect();

    // Order by minimum x so the sweep only compares x-overlapping segments.
    let mut order: Vec<usize> = (0..coords.len()).collect();
    order.sort_by_key(|&i| coords[i].0.0.min(coords[i].1.0));

    let mut active: Vec<usize> = Vec::new();
    let mut crossing = 0usize;
    let mut collinear = 0usize;
    let mut t_junction = 0usize;
    const MAX_LOGGED: usize = 50;
    let mut logged = 0usize;

    for &i in &order {
        let (ai0, ai1) = coords[i];
        let i_min_x = ai0.0.min(ai1.0);
        // Drop segments whose x-range ended before this one begins.
        active.retain(|&j| coords[j].0.0.max(coords[j].1.0) >= i_min_x);
        for &j in &active {
            let (bj0, bj1) = coords[j];
            let Some(conflict) = classify_conflict(ai0, ai1, bj0, bj1) else {
                continue;
            };
            match conflict {
                SegmentConflict::Crossing => crossing += 1,
                SegmentConflict::CollinearOverlap => collinear += 1,
                SegmentConflict::TJunction => t_junction += 1,
            }
            if logged < MAX_LOGGED {
                let kind = match conflict {
                    SegmentConflict::Crossing => "crossing",
                    SegmentConflict::CollinearOverlap => "collinear-overlap",
                    SegmentConflict::TJunction => "t-junction",
                };
                tracing::warn!(
                    "Invalid segment pair ({kind}): way {} [{:?}->{:?}] vs way {} [{:?}->{:?}]",
                    segment_ways.get(i).copied().unwrap_or(-1),
                    ai0,
                    ai1,
                    segment_ways.get(j).copied().unwrap_or(-1),
                    bj0,
                    bj1
                );
                logged += 1;
            }
        }
        active.push(i);
    }

    let total = crossing + collinear + t_junction;
    if total == 0 {
        tracing::info!("Segment validation passed: no illegal segment intersections found.");
    } else {
        tracing::warn!(
            "Segment validation found {total} illegal intersections \
             ({crossing} crossings, {collinear} collinear overlaps, {t_junction} t-junctions). \
             boostvoronoi requires segments to touch only at shared endpoints."
        );
    }
}

/// Intersection point of two segments, rounded to the integer grid. Returns
/// `None` for parallel/collinear pairs where there is no single crossing point.
fn intersection_point(
    a0: (i128, i128),
    a1: (i128, i128),
    b0: (i128, i128),
    b1: (i128, i128),
) -> Option<(i128, i128)> {
    let (a0x, a0y) = (a0.0 as f64, a0.1 as f64);
    let (rx, ry) = ((a1.0 - a0.0) as f64, (a1.1 - a0.1) as f64);
    let (sx, sy) = ((b1.0 - b0.0) as f64, (b1.1 - b0.1) as f64);
    let denominator = rx * sy - ry * sx;
    if denominator == 0.0 {
        return None;
    }
    let t = (((b0.0 - a0.0) as f64) * sy - ((b0.1 - a0.1) as f64) * sx) / denominator;
    Some((
        (a0x + t * rx).round() as i128,
        (a0y + t * ry).round() as i128,
    ))
}

/// Planarize the input so no two segments cross mid-span: every crossing or
/// t-junction is split at its intersection point, which becomes a shared
/// endpoint of both segments. Grade-separated OSM crossings (bridges/tunnels
/// without a shared node) are what boostvoronoi rejects, so this makes the
/// road network safe to feed to the builder while preserving cell -> way ids.
fn planarize_segments(
    segments: Vec<Line<i64>>,
    segment_ways: Vec<i64>,
) -> (Vec<Line<i64>>, Vec<i64>) {
    let coords: Vec<((i128, i128), (i128, i128))> = segments
        .iter()
        .map(|line| {
            (
                (line.start.x as i128, line.start.y as i128),
                (line.end.x as i128, line.end.y as i128),
            )
        })
        .collect();

    let mut cuts: Vec<Vec<(i128, i128)>> = vec![Vec::new(); coords.len()];
    let mut order: Vec<usize> = (0..coords.len()).collect();
    order.sort_by_key(|&i| coords[i].0.0.min(coords[i].1.0));

    let mut active: Vec<usize> = Vec::new();
    for &i in &order {
        let (ai0, ai1) = coords[i];
        let i_min_x = ai0.0.min(ai1.0);
        active.retain(|&j| coords[j].0.0.max(coords[j].1.0) >= i_min_x);
        for &j in &active {
            let (bj0, bj1) = coords[j];
            let split = matches!(
                classify_conflict(ai0, ai1, bj0, bj1),
                Some(SegmentConflict::Crossing | SegmentConflict::TJunction)
            );
            if split && let Some(point) = intersection_point(ai0, ai1, bj0, bj1) {
                cuts[i].push(point);
                cuts[j].push(point);
            }
        }
        active.push(i);
    }

    let mut new_segments = Vec::with_capacity(segments.len());
    let mut new_ways = Vec::with_capacity(segment_ways.len());
    for (index, &(a0, a1)) in coords.iter().enumerate() {
        let direction = (a1.0 - a0.0, a1.1 - a0.1);
        let full = direction.0 * direction.0 + direction.1 * direction.1;
        // Keep only cut points strictly interior to the segment, ordered along it.
        let mut interior: Vec<(i128, (i128, i128))> = cuts[index]
            .iter()
            .filter_map(|&point| {
                let projected = (point.0 - a0.0) * direction.0 + (point.1 - a0.1) * direction.1;
                (projected > 0 && projected < full).then_some((projected, point))
            })
            .collect();
        interior.sort_by_key(|(projected, _)| *projected);

        let mut points = vec![a0];
        for (_, point) in interior {
            if *points.last().expect("seeded with a0") != point {
                points.push(point);
            }
        }
        if *points.last().expect("seeded with a0") != a1 {
            points.push(a1);
        }

        for pair in points.windows(2) {
            let (start, end) = (pair[0], pair[1]);
            new_segments.push(Line::new(
                Point::new(start.0 as i64, start.1 as i64),
                Point::new(end.0 as i64, end.1 as i64),
            ));
            new_ways.push(segment_ways[index]);
        }
    }

    (new_segments, new_ways)
}

async fn seed_voronoi(database: &DatabaseConnection) -> Result<(), String> {
    let ways = grid_way::Entity::find()
        .order_by_id_asc()
        .all(database)
        .await
        .map_err(|err| format!("Failed to get merged ways: {err}"))?;
    let mut segment_pairs = Vec::new();
    // `segment_ways[i]` records the way that produced `segment_pairs[i]`. Because
    // the diagram is built from segments only, a cell's `source_index()` indexes
    // straight back into these vectors, giving cell -> way.
    let mut segment_ways: Vec<i64> = Vec::new();
    for way_model in ways {
        let way: Feature = way_model.geojson.parse().expect("load() creates features");
        let Some(geometry) = &way.geometry else {
            continue;
        };
        let Value::LineString(coords) = &geometry.value else {
            continue;
        };
        for segment in get_segment_pairs(coords) {
            segment_pairs.push(segment);
            segment_ways.push(way_model.way_id);
        }
    }

    tracing::info!(
        "Using {} segment pairs and {} segment ways for voronoi.",
        segment_pairs.len(),
        segment_ways.len()
    );

    let (segment_pairs, segment_ways) = planarize_segments(segment_pairs, segment_ways);
    report_invalid_segments(&segment_pairs, &segment_ways);

    // boostvoronoi panics (rather than errors) on illegal geometry, so contain it.
    let diagram = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Builder::<i64>::default()
            .with_segments(segment_pairs.iter())
            .map_err(|err| format!("Failed to add segments to voronoi builder: {err}"))?
            .build()
            .map_err(|err| format!("Failed to build voronoi diagram: {err}"))
    }))
    .map_err(|_| {
        "Voronoi build panicked on illegal segment geometry. See the invalid segment pairs logged above.".to_string()
    })??;
    let mut polygons: HashMap<i64, Vec<Polygon<f64>>> = HashMap::new();
    for cell in diagram.cells().iter() {
        // combine continue/filter & map into a single iter operation?
        if diagram
            .cell_edge_iterator(cell.id())
            .any(|edge_index| diagram.edge(edge_index).unwrap().vertex0().is_none())
        {
            continue;
        }
        let cell_coords: Vec<_> = diagram
            .cell_edge_iterator(cell.id())
            .map(|edge_index| {
                let edge = diagram.edge(edge_index).unwrap();
                let vertex_index = edge.vertex0().expect("filtered out None above");
                let vertex = diagram.vertex(vertex_index).unwrap();
                Coord {
                    x: vertex.x() / SCALING_FACTOR,
                    y: vertex.y() / SCALING_FACTOR,
                }
            })
            .collect();
        // Map the cell back to the way whose segment created it. All cells a
        // segment spawns (its body and two endpoints) share this source index.
        let Some(way_id) = segment_ways.get(cell.source_index().usize()).copied() else {
            tracing::error!("Unrecognised cell source index.");
            continue;
        };
        let polygon = Polygon::new(LineString(cell_coords), vec![]);
        polygons.entry(way_id).or_default().push(polygon);
    }

    // The voronoi cells are unbounded and some cells spike off far into the distance. Crop by a
    // bounding box so they look more sensible.
    let christchurch_bounding_box = get_bounding_box(
        Coord {
            x: 172.50,
            y: -43.60,
        },
        Coord {
            x: 172.75,
            y: -43.45,
        },
    );
    let polygons_database: Vec<_> = polygons
        .into_iter()
        .map(|(way_id, polygon_cells)| {
            let merged_polygons = geo::algorithm::unary_union(&polygon_cells);
            let cropped_merged_polygons = merged_polygons.intersection(&christchurch_bounding_box);
            let feature = Feature {
                id: Some(Id::Number(way_id.into())),
                geometry: Some(Geometry::new(Value::from(&cropped_merged_polygons))),
                ..Default::default()
            };
            grid_cell::ActiveModel {
                way_id: Set(way_id),
                geojson: Set(feature.to_string()),
            }
        })
        .collect();
    for cell_chunk in polygons_database.chunks(INSERT_CHUNK) {
        grid_cell::Entity::insert_many(cell_chunk.to_vec())
            .exec(database)
            .await
            .map_err(|err| format!("Failed to insert polygon cells: {err}"))?;
    }

    Ok(())
}

/// Union-find root lookup with path halving, used to group adjoining ways into connected components.
fn find_root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// A component line string tagged with the node ids of its two endpoints, so
/// stitching can join pieces at a shared node without welding at intersections.
struct StitchPiece {
    coords: Vec<Coord>,
    start_node: i64,
    end_node: i64,
}

/// Stitch a component's line strings into one continuous line string, grafting
/// pieces end-to-end only where they meet at a shared non-intersection node so a
/// merged way never runs through an intersection.
fn stitch_line_strings(
    mut pieces: Vec<StitchPiece>,
    intersection_nodes: &HashSet<i64>,
) -> LineString {
    let Some(first) = pieces.pop() else {
        return LineString(Vec::new());
    };
    let mut chain = first.coords;
    let mut front_node = first.start_node;
    let mut back_node = first.end_node;
    let mut progress = true;
    while progress {
        progress = false;
        let mut index = 0;
        while index < pieces.len() {
            let piece = &pieces[index];
            // The shared endpoint is dropped from the grafted piece to avoid a
            // duplicated coordinate at the join.
            if piece.start_node == back_node && !intersection_nodes.contains(&back_node) {
                chain.extend_from_slice(&piece.coords[1..]);
                back_node = piece.end_node;
            } else if piece.end_node == back_node && !intersection_nodes.contains(&back_node) {
                chain.extend(piece.coords.iter().rev().skip(1).copied());
                back_node = piece.start_node;
            } else if piece.end_node == front_node && !intersection_nodes.contains(&front_node) {
                let mut prefix = piece.coords[..piece.coords.len() - 1].to_vec();
                prefix.extend_from_slice(&chain);
                chain = prefix;
                front_node = piece.start_node;
            } else if piece.start_node == front_node && !intersection_nodes.contains(&front_node) {
                let mut prefix: Vec<Coord> = piece
                    .coords
                    .iter()
                    .rev()
                    .take(piece.coords.len() - 1)
                    .copied()
                    .collect();
                prefix.extend_from_slice(&chain);
                chain = prefix;
                front_node = piece.end_node;
            } else {
                index += 1;
                continue;
            }
            pieces.swap_remove(index);
            progress = true;
        }
    }
    LineString(chain)
}

/// Bit-pattern key for a coordinate. Stitching only copies and reorders existing
/// coordinates, so identical nodes keep byte-identical f64 values that hash equal.
fn coord_key(coord: &Coord) -> (u64, u64) {
    (coord.x.to_bits(), coord.y.to_bits())
}

/// Split a stitched line string at any interior intersection coordinate so none of the lines run
/// through an intersection. The boundary coordinate is repeated as the shared endpoint of both
/// adjoining pieces to keep the geometry contiguous.
fn split_line_at_intersections(
    line: LineString,
    intersection_coords: &HashSet<(u64, u64)>,
) -> Vec<LineString> {
    let coords = line.0;
    let mut pieces = Vec::new();
    let mut current: Vec<Coord> = Vec::new();
    for (index, coord) in coords.iter().enumerate() {
        current.push(*coord);
        let is_interior = index > 0 && index + 1 < coords.len();
        if is_interior && intersection_coords.contains(&coord_key(coord)) {
            pieces.push(LineString(std::mem::take(&mut current)));
            current.push(*coord);
        }
    }
    if current.len() > 1 {
        pieces.push(LineString(current));
    }
    pieces
}

/// Fetch the Christchurch road network from Overpass and persist it as grid
/// nodes and ordered way-node segments.
async fn load(database: &DatabaseConnection) -> Result<(), String> {
    let response = overpass::get_overpass_data().await?;

    let mut nodes = Vec::new();
    let mut nodes_by_id = HashMap::new();
    for element in &response.elements {
        let OsmElement::Node { id, lat, lon } = element else {
            continue;
        };
        nodes.push(grid_node::ActiveModel {
            id: Set(*id),
            latitude: Set((lat * COORD_SCALE) as i32),
            longitude: Set((lon * COORD_SCALE) as i32),
            is_intersection: Set(false),
        });
        nodes_by_id.insert(id, Coord { x: *lon, y: *lat });
    }

    // Track each way's node ids alongside its line string so adjoining ways can be
    // joined by shared nodes rather than an expensive geometric adjacency test.
    let mut named_ways_geo: HashMap<&String, Vec<(LineString, &Vec<i64>)>> = HashMap::new();
    let mut merged_ways_geo = Vec::new();
    // Unnamed ways can't be merged, but they are still split at intersections below.
    let mut unnamed_ways_geo: Vec<LineString> = Vec::new();
    let mut node_to_nodes: HashMap<i64, HashSet<i64>> = HashMap::new();
    for element in &response.elements {
        let OsmElement::Way {
            nodes: node_ids,
            tags,
        } = element
        else {
            continue;
        };

        // A way is an ordered list of node references: each consecutive
        // pair forms a segment of the polyline, captured here as one
        // (way_id, sequence, node_id) row.
        let name = match &tags {
            Some(tags) => tags.get("name"),
            None => None,
        };
        for (node_id1, node_id2) in node_ids.iter().zip(node_ids.iter().skip(1)) {
            node_to_nodes
                .entry(*node_id1)
                .or_default()
                .insert(*node_id2);
            node_to_nodes
                .entry(*node_id2)
                .or_default()
                .insert(*node_id1);
        }

        let way_coords: Vec<_> = node_ids
            .iter()
            .map(|node_id| {
                nodes_by_id
                    .get(&node_id)
                    .expect("should be referencing a node that exists")
                    .to_owned()
            })
            .collect();
        let way_geo = LineString(way_coords);
        match name {
            Some(name) => named_ways_geo
                .entry(name)
                .or_default()
                .push((way_geo, node_ids)),
            None => {
                // Unnamed ways can't be merged with anything, but they are still
                // split at intersections below like the merged named ways.
                unnamed_ways_geo.push(way_geo);
            }
        };
    }

    tracing::info!("Persisting {} grid nodes", nodes.len());

    for nodes_chunk in nodes.chunks(INSERT_CHUNK) {
        grid_node::Entity::insert_many(nodes_chunk.to_vec())
            .exec(database)
            .await
            .map_err(|err| format!("Failed to insert grid nodes: {err}"))?;
    }

    // A node is an intersection (or dead end) when it doesn't connect exactly two neighbours.
    let intersection_nodes: HashSet<i64> = node_to_nodes
        .iter()
        .filter(|(_, others)| others.len() != 2)
        .map(|(node, _)| *node)
        .collect();
    let intersection_ids: Vec<_> = intersection_nodes.iter().copied().collect();
    let intersection_coords: HashSet<(u64, u64)> = intersection_nodes
        .iter()
        .filter_map(|node_id| nodes_by_id.get(&node_id).map(coord_key))
        .collect();

    tracing::info!("Found {} intersection nodes", intersection_ids.len());

    for intersection_ids_chunk in intersection_ids.chunks(INSERT_CHUNK) {
        grid_node::Entity::update_many()
            .col_expr(grid_node::Column::IsIntersection, Expr::value(true))
            .filter(grid_node::Column::Id.is_in(intersection_ids_chunk.iter().copied()))
            .exec(database)
            .await
            .map_err(|err| format!("Failed to flag intersection nodes: {err}"))?;
    }

    // Split each unnamed way at intersections and emit it as a merged way.
    for way_geo in unnamed_ways_geo {
        for piece in split_line_at_intersections(way_geo, &intersection_coords) {
            let feature = Feature {
                geometry: Some(Geometry::new(Value::from(&piece))),
                ..Default::default()
            };
            merged_ways_geo.push(grid_way::ActiveModel {
                way_id: NotSet,
                geojson: Set(feature.to_string()),
            });
        }
    }

    // Merge adjoining same-named ways into connected components using union-find:
    // two ways join only when they share a non-intersection node.
    for (name, ways_geo) in named_ways_geo.into_iter() {
        let mut parent: Vec<usize> = (0..ways_geo.len()).collect();
        // Map each non-intersection node to the first way that touched it; a
        // second visitor unions the two ways into one component.
        let mut node_owner: HashMap<i64, usize> = HashMap::new();
        for (way_index, (_, node_ids)) in ways_geo.iter().enumerate() {
            for node_id in node_ids.iter() {
                if intersection_nodes.contains(node_id) {
                    continue;
                }
                match node_owner.get(node_id) {
                    Some(&other) => {
                        let a = find_root(&mut parent, way_index);
                        let b = find_root(&mut parent, other);
                        parent[a] = b;
                    }
                    None => {
                        node_owner.insert(*node_id, way_index);
                    }
                }
            }
        }

        // Bucket ways by their component root, then union each bucket.
        let mut components: HashMap<usize, Vec<StitchPiece>> = HashMap::new();
        for (way_index, (way_geo, node_ids)) in ways_geo.into_iter().enumerate() {
            let root = find_root(&mut parent, way_index);
            let (Some(&start_node), Some(&end_node)) = (node_ids.first(), node_ids.last()) else {
                continue;
            };
            components.entry(root).or_default().push(StitchPiece {
                coords: way_geo.0,
                start_node,
                end_node,
            });
        }

        for (_, pieces) in components {
            let mut properties = geojson::JsonObject::new();
            properties.insert(
                "name".to_string(),
                serde_json::Value::String(name.to_string()),
            );
            // Union each bucket into one merged way.
            let merged_way = stitch_line_strings(pieces, &intersection_nodes);
            // The original (unmerged) ways may have contained an intersection partway through them, so it
            // is necessary to split at intersections afterwards, even though the union-find and stitching
            // also check for them.
            for piece in split_line_at_intersections(merged_way, &intersection_coords) {
                let feature = Feature {
                    geometry: Some(Geometry::new(Value::from(&piece))),
                    properties: Some(properties.clone()),
                    ..Default::default()
                };
                merged_ways_geo.push(grid_way::ActiveModel {
                    way_id: NotSet,
                    geojson: Set(feature.to_string()),
                });
            }
        }
    }

    for merged_ways_chunk in merged_ways_geo.chunks(INSERT_CHUNK) {
        grid_way::Entity::insert_many(merged_ways_chunk.to_vec())
            .exec(database)
            .await
            .map_err(|err| format!("Failed to insert way nodes: {err}"))?;
    }

    Ok(())
}

/// Populate the road grid only when it is empty, so a cold database gets seeded
/// while repeat startups are cheap no-ops.
pub async fn seed(database: &DatabaseConnection) -> Result<(), String> {
    let existing_node_count = grid_node::Entity::find()
        .count(database)
        .await
        .map_err(|err| format!("Failed to count grid nodes: {err}"))?;
    if existing_node_count > 0 {
        tracing::info!("Skipping populating road grid as already filled.");
    } else {
        load(database).await?;
    }
    // TODO: should this be 2 separate seeding jobs?
    let existing_cell_count = grid_cell::Entity::find()
        .count(database)
        .await
        .map_err(|err| format!("Failed to count grid cells: {err}"))?;
    if existing_cell_count > 0 {
        tracing::info!("Skipping populating road cells as already filled.");
    } else {
        seed_voronoi(database).await?;
    }

    Ok(())
}
