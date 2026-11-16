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

impl RoadSegment {
    fn new(way_id: i64, vertices: Vec<Coord<f64>>) -> Self {
        let points: Vec<[f64; 2]> = vertices.iter().map(|v| [v.x, v.y]).collect();
        let envelope = AABB::from_points(&points);
        RoadSegment {
            way_id,
            vertices,
            envelope,
        }
    }

    /// Find the closest point on this segment to the given coordinate.
    /// Returns (closest_point, distance_in_meters).
    pub fn closest_point(&self, target: Coord<f64>) -> (Coord<f64>, f64) {
        let mut min_dist = f64::INFINITY;
        let mut closest_point = self.vertices[0];

        for i in 0..self.vertices.len() - 1 {
            let point0 = self.vertices[i];
            let point1 = self.vertices[i + 1];

            let (point_on_segment, distance) = closest_point_on_segment(point0, point1, target);

            if distance < min_dist {
                min_dist = distance;
                closest_point = point_on_segment;
            }
        }

        (closest_point, min_dist)
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

        Ok(RoadSegmentIndex {
            rtree: RTree::bulk_load(segments),
        })
    }

    /// Find the single closest segment to a coordinate within the snap radius.
    pub fn find_closest_segment(
        &self,
        coord: Coord<f64>,
        radius_m: f64,
    ) -> Option<(&RoadSegment, f64)> {
        // A degree of longitude shrinks with latitude, so the search box is wider in degrees of longitude.
        let radius_lat_deg = radius_m / METERS_PER_DEGREE_LAT;
        let radius_lon_deg = radius_lat_deg / coord.y.to_radians().cos();
        let search_box = AABB::from_corners(
            [coord.x - radius_lon_deg, coord.y - radius_lat_deg],
            [coord.x + radius_lon_deg, coord.y + radius_lat_deg],
        );

        self.rtree
            .locate_in_envelope_intersecting(search_box)
            .map(|segment| (segment, segment.closest_point(coord).1))
            .filter(|(_, distance)| *distance <= radius_m)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    pub fn is_empty(&self) -> bool {
        self.rtree.size() == 0
    }
}

/// Find the closest point on a line segment to a target point.
/// Returns (point, distance_in_meters).
fn closest_point_on_segment(
    point0: Coord<f64>,
    point1: Coord<f64>,
    target: Coord<f64>,
) -> (Coord<f64>, f64) {
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

        let (closest, distance) = closest_point_on_segment(point0, point1, target);
        assert_eq!(closest.x, 1.0);
        assert_eq!(closest.y, 0.0);
        assert_eq!(distance, 111195.0802335329);
    }

    #[test]
    fn test_closest_point_clamping() {
        let point0 = Coord { x: 0.0, y: 0.0 };
        let point1 = Coord { x: 1.0, y: 0.0 };
        let target = Coord { x: 2.0, y: 0.0 };

        let (closest, distance) = closest_point_on_segment(point0, point1, target);
        assert_eq!(closest.x, 1.0);
        assert_eq!(distance, 111195.0802335329);
    }
}
