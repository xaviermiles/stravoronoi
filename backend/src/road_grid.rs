/// Imports and processes the road grid.
use crate::models::{grid_cell, grid_node, grid_way};
use crate::services::overpass::{self, OsmElement};
use boostvoronoi::prelude::*;
use geojson::Feature;
use geojson::Geometry;
use geojson::Value;
use geojson::feature::Id;
use sea_orm::ActiveValue::Set;
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

async fn get_ways(database: &DatabaseConnection) -> Vec<Feature> {
    // Join each way segment to its node so coordinates are resolved in the query.
    let ways = match grid_way::Entity::find()
        .order_by_id_asc()
        .find_also_related(grid_node::Entity)
        .all(database)
        .await
    {
        Ok(ways) => ways,
        Err(err) => {
            tracing::error!("Error finding grid ways: {err}");
            return Vec::new();
        }
    };

    // Chunk by way_id to create a LineString feature for each way.
    ways.chunk_by(|a, b| a.0.way_id == b.0.way_id)
        .filter_map(|chunk| {
            let coords: Vec<Vec<f64>> = chunk
                .iter()
                .filter_map(|(_, node)| node.as_ref())
                .map(|node| {
                    vec![
                        (node.longitude as f64) / COORD_SCALE,
                        (node.latitude as f64) / COORD_SCALE,
                    ]
                })
                .collect();
            let way = &chunk[0].0;
            let mut properties = geojson::JsonObject::new();
            if let Some(name) = &way.name {
                properties.insert("name".to_string(), serde_json::Value::String(name.clone()));
            }
            Some(Feature {
                id: Some(Id::Number(way.way_id.into())),
                geometry: Some(Geometry::new(Value::LineString(coords))),
                properties: Some(properties),
                ..Default::default()
            })
        })
        .collect()
}

/// Create boostvoronoi point from coordinate.
///
/// boostvoronoi only supports integer types so cast the f64 to i64.
fn point_i64(point_f64: &[f64]) -> Point<i64> {
    Point::new(
        (point_f64[0] * COORD_SCALE) as i64,
        (point_f64[1] * COORD_SCALE) as i64,
    )
}

fn line_i64(start: &[f64], end: &[f64]) -> Line<i64> {
    Line::new(point_i64(start), point_i64(end))
}

fn get_segment_pairs(segment_coords: &[Vec<f64>]) -> Vec<Line<i64>> {
    segment_coords
        .iter()
        .zip(segment_coords.iter().skip(1))
        .map(|(start, end)| line_i64(start, end))
        .collect()
}

/// Extract the numeric OSM way id carried on a way feature.
fn feature_way_id(way: &Feature) -> Option<i64> {
    match way.id.as_ref()? {
        Id::Number(number) => number.as_i64(),
        Id::String(_) => None,
    }
}

async fn seed_voronoi(database: &DatabaseConnection) -> Result<(), String> {
    // TODO: this is lazy way to get them (code copied from router).
    let ways = get_ways(database).await;
    let mut segment_pairs = Vec::new();
    // `segment_ways[i]` records the way that produced `segment_pairs[i]`. Because
    // the diagram is built from segments only, a cell's `source_index()` indexes
    // straight back into these vectors, giving cell -> way.
    let mut segment_ways: Vec<i64> = Vec::new();
    for way in ways {
        let Some(way_id) = feature_way_id(&way) else {
            continue;
        };
        if way.property("name").and_then(|name| name.as_str()) != Some("Oxford Terrace") {
            continue;
        }
        let Some(geometry) = &way.geometry else {
            continue;
        };
        let geojson::Value::LineString(coords) = &geometry.value else {
            continue;
        };
        for segment in get_segment_pairs(coords) {
            segment_pairs.push(segment);
            segment_ways.push(way_id);
        }
    }

    let diagram = Builder::<i64>::default()
        .with_segments(segment_pairs)
        .unwrap()
        .build()
        .unwrap();
    let mut polygons: HashMap<i64, geo_types::Polygon<f64>> = HashMap::new();
    for (index, cell) in diagram.cells().iter().enumerate() {
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
                vec![vertex.x() / SCALING_FACTOR, vertex.y() / SCALING_FACTOR]
            })
            .collect();
        // Map the cell back to the way whose segment created it. All cells a
        // segment spawns (its body and two endpoints) share this source index.
        let Some(way_id) = segment_ways.get(cell.source_index().usize()).copied() else {
            tracing::error!("Unrecognised cell source index.");
            continue;
        };
        let polygon = geo_types::Polygon::new(vec![cell_coords], vec![]);
        polygons.entry(way_id).or_default().push(polygon);
    }

    let polygons_database = polygons.iter().map(|(way_id, polygon_cells)|) {
        grid_cell::ActiveModel {
            way_id: Set(way_id),
            geojson: geo::algorithm::unary_union(polygon_cells),
        }
        }).collect();
    for cell_chunk in polygons.chunks(INSERT_CHUNK) {
        grid_cell::Entity::insert_many(cell_chunk.to_vec())
            .exec(database)
            .await
            .map_err(|err| format!("Failed to insert polygon cells: {err}"))?;
    }

    Ok(())
}

/// Fetch the Christchurch road network from Overpass and persist it as grid
/// nodes and ordered way-node segments.
async fn load(database: &DatabaseConnection) -> Result<(), String> {
    let response = overpass::get_overpass_data().await?;

    let mut nodes = Vec::new();
    let mut ways = Vec::new();
    let mut node_to_nodes: HashMap<i64, HashSet<i64>> = HashMap::new();
    for element in response.elements {
        match element {
            OsmElement::Node { id, lat, lon } => {
                nodes.push(grid_node::ActiveModel {
                    id: Set(id),
                    latitude: Set((lat * COORD_SCALE) as i32),
                    longitude: Set((lon * COORD_SCALE) as i32),
                    is_intersection: Set(false),
                });
            }
            OsmElement::Way {
                id,
                nodes: node_ids,
                tags,
            } => {
                // A way is an ordered list of node references: each consecutive
                // pair forms a segment of the polyline, captured here as one
                // (way_id, sequence, node_id) row.
                let tags = tags.unwrap_or_default();
                for (sequence, node_id) in node_ids.iter().enumerate() {
                    ways.push(grid_way::ActiveModel {
                        way_id: Set(id as i64),
                        name: Set(tags.get("name").cloned()),
                        sequence: Set(sequence as i32),
                        node_id: Set(*node_id as i64),
                    });
                }
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
            }
        }
    }

    tracing::info!(
        "Persisting {} grid nodes and {} way-node segments",
        nodes.len(),
        ways.len()
    );

    // Insert nodes first so the way-node references always resolve.
    for nodes_chunk in nodes.chunks(INSERT_CHUNK) {
        grid_node::Entity::insert_many(nodes_chunk.to_vec())
            .exec(database)
            .await
            .map_err(|err| format!("Failed to insert grid nodes: {err}"))?;
    }
    for ways_chunk in ways.chunks(INSERT_CHUNK) {
        grid_way::Entity::insert_many(ways_chunk.to_vec())
            .exec(database)
            .await
            .map_err(|err| format!("Failed to insert way nodes: {err}"))?;
    }

    let intersection_ids: Vec<_> = node_to_nodes
        .into_iter()
        .filter(|(_node, other_nodes)| other_nodes.len() > 2)
        .map(|(node, _other_nodes)| node)
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
