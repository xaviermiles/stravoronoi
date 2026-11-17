/// R-tree spatial index for efficient nearest-neighbor queries on road segments.
use crate::models::grid_way;
use geo::{Distance, Haversine, Point};
use geo_types::Coord;
use geojson::Value;
use rstar::{AABB, RTree, RTreeObject};
use sea_orm::EntityTrait;

const METERS_PER_DEGREE_LAT: f64 = 111_320.0;

/// A single road segment (edge) in the network.
#[derive(Clone, Debug)]
pub struct RoadSegment {
    pub way_id: i64,
    /// Consecutive vertices of this segment from the road network.
    pub vertices: Vec<Coord<f64>>,
    envelope: AABB<[f64; 2]>,
}

impl RTreeObject for RoadSegment {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

/// Result of projecting a coordinate onto a road segment.
#[derive(Clone, Copy, Debug)]
pub struct Projection {
    pub point: Coord<f64>,
    pub distance_m: f64,
    /// Location along the segment: the index of the edge plus the fraction along that edge.
    /// It increases monotonically from the first vertex (0.0) to the last (vertices.len() - 1).
    pub position: f64,
}

impl RoadSegment {
    pub fn new(way_id: i64, vertices: Vec<Coord<f64>>) -> Self {
        let points: Vec<[f64; 2]> = vertices.iter().map(|v| [v.x, v.y]).collect();
        let envelope = AABB::from_points(&points);
        RoadSegment {
            way_id,
            vertices,
            envelope,
        }
    }

    /// Find the closest point on this segment to the given coordinate.
    pub fn closest_point(&self, target: Coord<f64>) -> Projection {
        let mut closest = Projection {
            point: self.vertices[0],
            distance_m: f64::INFINITY,
            position: 0.0,
        };

        for (i, edge) in self.vertices.windows(2).enumerate() {
            let (point, distance_m, edge_fraction) =
                closest_point_on_segment(edge[0], edge[1], target);

            if distance_m < closest.distance_m {
                closest = Projection {
                    point,
                    distance_m,
                    position: i as f64 + edge_fraction,
                };
            }
        }

        closest
    }

    /// Position of the last vertex of this segment.
    pub fn end_position(&self) -> f64 {
        (self.vertices.len() - 1) as f64
    }

    /// The coordinate at a position along this segment.
    pub fn point_at(&self, position: f64) -> Coord<f64> {
        let edge = (position.floor().max(0.0) as usize).min(self.vertices.len() - 2);
        let edge_fraction = position - edge as f64;
        let point0 = self.vertices[edge];
        let point1 = self.vertices[edge + 1];

        // Return exact vertices at the ends so shared nodes compare equal across segments.
        if edge_fraction <= 0.0 {
            point0
        } else if edge_fraction >= 1.0 {
            point1
        } else {
            Coord {
                x: point0.x + edge_fraction * (point1.x - point0.x),
                y: point0.y + edge_fraction * (point1.y - point0.y),
            }
        }
    }

    /// The part of this segment between two positions, including any vertices in between.
    /// The result is ordered from `start` to `end`.
    pub fn slice(&self, start: f64, end: f64) -> Vec<Coord<f64>> {
        let (low, high) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };

        let mut points = vec![self.point_at(low)];
        let first_interior_vertex = low.floor() as usize + 1;
        let end_vertex_exclusive = high.ceil() as usize;
        points.extend(
            &self.vertices[first_interior_vertex..end_vertex_exclusive.max(first_interior_vertex)],
        );
        points.push(self.point_at(high));
        points.dedup();

        if start > end {
            points.reverse();
        }
        points
    }
}

/// Spatial index for road segments. The R-tree narrows candidates by bounding box,
/// then the exact distance is computed for each candidate.
pub struct RoadSegmentIndex {
    rtree: RTree<RoadSegment>,
}

impl RoadSegmentIndex {
    /// Load all road segments from the database.
    pub async fn from_database(db: &sea_orm::DatabaseConnection) -> Result<Self, String> {
        let ways = grid_way::Entity::find()
            .all(db)
            .await
            .map_err(|e| format!("Failed to load road grid: {e}"))?;

        let mut segments = Vec::new();

        for way in ways {
            let feature: geojson::Feature = way
                .geojson
                .parse()
                .map_err(|e| format!("Invalid GeoJSON in way {}: {e}", way.way_id))?;

            if let Some(geometry) = feature.geometry {
                if let Value::LineString(coords_array) = geometry.value {
                    let vertices: Vec<Coord<f64>> = coords_array
                        .iter()
                        .filter_map(|c| {
                            if c.len() >= 2 {
                                Some(Coord { x: c[0], y: c[1] })
                            } else {
                                None
                            }
                        })
                        .collect();

                    if vertices.len() >= 2 {
                        segments.push(RoadSegment::new(way.way_id, vertices));
                    }
                }
            }
        }

        Ok(Self::from_segments(segments))
    }

    pub fn from_segments(segments: Vec<RoadSegment>) -> Self {
        RoadSegmentIndex {
            rtree: RTree::bulk_load(segments),
        }
    }

    /// Find the single closest segment to a coordinate within the snap radius.
    pub fn find_closest_segment(
        &self,
        coord: Coord<f64>,
        radius_m: f64,
    ) -> Option<(&RoadSegment, Projection)> {
        // A degree of longitude shrinks with latitude, so the search box is wider in degrees of longitude.
        let radius_lat_deg = radius_m / METERS_PER_DEGREE_LAT;
        let radius_lon_deg = radius_lat_deg / coord.y.to_radians().cos();
        let search_box = AABB::from_corners(
            [coord.x - radius_lon_deg, coord.y - radius_lat_deg],
            [coord.x + radius_lon_deg, coord.y + radius_lat_deg],
        );

        self.rtree
            .locate_in_envelope_intersecting(search_box)
            .map(|segment| (segment, segment.closest_point(coord)))
            .filter(|(_, projection)| projection.distance_m <= radius_m)
            .min_by(|a, b| a.1.distance_m.total_cmp(&b.1.distance_m))
    }

    pub fn is_empty(&self) -> bool {
        self.rtree.size() == 0
    }
}

/// Find the closest point on a line segment to a target point.
/// Returns (point, distance_in_meters, fraction along the line from point0 (0.0) to point1 (1.0)).
fn closest_point_on_segment(
    point0: Coord<f64>,
    point1: Coord<f64>,
    target: Coord<f64>,
) -> (Coord<f64>, f64, f64) {
    // Project in a locally-equirectangular space: scale longitude by cos(latitude)
    // so that perpendicular projection is correct away from the equator.
    let lon_scale = target.y.to_radians().cos();
    let dx = (point1.x - point0.x) * lon_scale;
    let dy = point1.y - point0.y;
    let len_sq = dx * dx + dy * dy;

    if len_sq == 0.0 {
        return (
            point0,
            Haversine.distance(
                Point::new(point0.x, point0.y),
                Point::new(target.x, target.y),
            ),
            0.0,
        );
    }

    let segment_parameter =
        ((target.x - point0.x) * lon_scale * dx + (target.y - point0.y) * dy) / len_sq;
    let segment_parameter_clamped = segment_parameter.clamp(0.0, 1.0);

    let closest = Coord {
        x: point0.x + segment_parameter_clamped * (point1.x - point0.x),
        y: point0.y + segment_parameter_clamped * (point1.y - point0.y),
    };

    (
        closest,
        Haversine.distance(
            Point::new(closest.x, closest.y),
            Point::new(target.x, target.y),
        ),
        segment_parameter_clamped,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_closest_point_on_segment_midpoint() {
        let point0 = Coord { x: 0.0, y: 0.0 };
        let point1 = Coord { x: 2.0, y: 0.0 };
        let target = Coord { x: 1.0, y: 1.0 };

        let (closest, distance, fraction) = closest_point_on_segment(point0, point1, target);
        assert_eq!(closest.x, 1.0);
        assert_eq!(closest.y, 0.0);
        assert_eq!(distance, 111195.0802335329);
        assert_eq!(fraction, 0.5);
    }

    #[test]
    fn test_closest_point_clamping() {
        let point0 = Coord { x: 0.0, y: 0.0 };
        let point1 = Coord { x: 1.0, y: 0.0 };
        let target = Coord { x: 2.0, y: 0.0 };

        let (closest, distance, fraction) = closest_point_on_segment(point0, point1, target);
        assert_eq!(closest.x, 1.0);
        assert_eq!(distance, 111195.0802335329);
        assert_eq!(fraction, 1.0);
    }

    #[test]
    fn test_slice_includes_interior_vertices() {
        let segment = RoadSegment::new(
            1,
            vec![
                Coord { x: 0.0, y: 0.0 },
                Coord { x: 1.0, y: 0.0 },
                Coord { x: 1.0, y: 1.0 },
            ],
        );

        // From halfway along the first edge to halfway along the second edge.
        let points = segment.slice(0.5, 1.5);
        assert_eq!(
            points,
            vec![
                Coord { x: 0.5, y: 0.0 },
                Coord { x: 1.0, y: 0.0 },
                Coord { x: 1.0, y: 0.5 },
            ]
        );

        // Reversed positions give the reversed order.
        let mut reversed = segment.slice(1.5, 0.5);
        reversed.reverse();
        assert_eq!(reversed, points);

        // The full length reaches both end vertices.
        let full = segment.slice(0.0, segment.end_position());
        assert_eq!(full.first(), Some(&Coord { x: 0.0, y: 0.0 }));
        assert_eq!(full.last(), Some(&Coord { x: 1.0, y: 1.0 }));
        assert_eq!(full.len(), 3);
    }
}
