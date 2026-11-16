/// Spatial index for efficient nearest-neighbor queries on road segments.
use crate::models::grid_way;
use geo::{Distance, Haversine, Point};
use geo_types::Coord;
use geojson::Value;
use sea_orm::EntityTrait;
use std::collections::HashMap;

/// A single road segment (edge) in the network.
#[derive(Clone, Debug)]
pub struct RoadSegment {
    pub way_id: i64,
    /// Consecutive vertices of this segment from the road network.
    pub vertices: Vec<Coord<f64>>,
}

impl RoadSegment {
    /// Find the closest point on this segment to the given coordinate.
    /// Returns (closest_point, distance_in_meters, segment_parameter).
    pub fn closest_point(&self, target: Coord<f64>) -> (Coord<f64>, f64, f64) {
        let mut min_dist = f64::INFINITY;
        let mut closest_point = self.vertices[0];
        let mut closest_segment_parameter = 0.0;

        for i in 0..self.vertices.len() - 1 {
            let point0 = self.vertices[i];
            let point1 = self.vertices[i + 1];

            let (point_on_segment, distance, segment_parameter) =
                closest_point_on_segment(point0, point1, target);

            if distance < min_dist {
                min_dist = distance;
                closest_point = point_on_segment;
                closest_segment_parameter = segment_parameter;
            }
        }

        (closest_point, min_dist, closest_segment_parameter)
    }
}

/// Spatial index for road segments using grid-based partitioning.
pub struct RoadSegmentIndex {
    segments: Vec<RoadSegment>,
    /// Grid-based spatial index: (grid_x, grid_y) -> segment indices
    grid: HashMap<(i32, i32), Vec<usize>>,
    grid_size: f64, // degrees
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
                        segments.push(RoadSegment {
                            way_id: way.way_id,
                            vertices,
                        });
                    }
                }
            }
        }

        let mut index = RoadSegmentIndex {
            segments,
            grid: HashMap::new(),
            grid_size: 0.01, // ~1km at equator; adjust as needed
        };

        index.build_grid();
        Ok(index)
    }

    /// Build the spatial grid index for fast neighbor queries.
    fn build_grid(&mut self) {
        for (seg_idx, segment) in self.segments.iter().enumerate() {
            for vertex in &segment.vertices {
                let grid_x = (vertex.x / self.grid_size).floor() as i32;
                let grid_y = (vertex.y / self.grid_size).floor() as i32;

                self.grid
                    .entry((grid_x, grid_y))
                    .or_insert_with(Vec::new)
                    .push(seg_idx);
            }
        }
    }

    /// Find road segments near the given coordinate within search radius.
    pub fn find_nearby_segments(&self, coord: Coord<f64>, radius_m: f64) -> Vec<&RoadSegment> {
        // Convert radius from meters to degrees (rough approximation)
        let radius_deg = radius_m / 111_000.0;

        let grid_radius = (radius_deg / self.grid_size).ceil() as i32 + 1;
        let grid_x = (coord.x / self.grid_size).floor() as i32;
        let grid_y = (coord.y / self.grid_size).floor() as i32;

        let mut candidates = std::collections::HashSet::new();

        for dx in -grid_radius..=grid_radius {
            for dy in -grid_radius..=grid_radius {
                if let Some(indices) = self.grid.get(&(grid_x + dx, grid_y + dy)) {
                    for &idx in indices {
                        candidates.insert(idx);
                    }
                }
            }
        }

        candidates
            .into_iter()
            .map(|idx| &self.segments[idx])
            .collect()
    }

    /// Find the single closest segment to a coordinate.
    pub fn find_closest_segment(
        &self,
        coord: Coord<f64>,
        radius_m: f64,
    ) -> Option<(&RoadSegment, f64)> {
        let candidates = self.find_nearby_segments(coord, radius_m);

        candidates
            .into_iter()
            .map(|seg| {
                let (_, dist, _) = seg.closest_point(coord);
                (seg, dist)
            })
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    }

    pub fn segments(&self) -> &[RoadSegment] {
        &self.segments
    }
}

/// Find the closest point on a line segment to a target point.
/// Returns (point, distance_in_meters, segment_parameter where segment_parameter=0 is point0, segment_parameter=1 is point1).
fn closest_point_on_segment(
    point0: Coord<f64>,
    point1: Coord<f64>,
    target: Coord<f64>,
) -> (Coord<f64>, f64, f64) {
    let dx = point1.x - point0.x;
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

    let segment_parameter = ((target.x - point0.x) * dx + (target.y - point0.y) * dy) / len_sq;
    let segment_parameter_clamped = segment_parameter.max(0.0).min(1.0);

    let closest = Coord {
        x: point0.x + segment_parameter_clamped * dx,
        y: point0.y + segment_parameter_clamped * dy,
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

        let (closest, _, segment_parameter) = closest_point_on_segment(point0, point1, target);
        assert_eq!(closest.x, 1.0);
        assert_eq!(closest.y, 0.0);
        assert_eq!(segment_parameter, 0.5);
    }

    #[test]
    fn test_closest_point_clamping() {
        let point0 = Coord { x: 0.0, y: 0.0 };
        let point1 = Coord { x: 1.0, y: 0.0 };
        let target = Coord { x: 2.0, y: 0.0 };

        let (closest, _, segment_parameter) = closest_point_on_segment(point0, point1, target);
        assert_eq!(closest.x, 1.0);
        assert_eq!(segment_parameter, 1.0);
    }
}
