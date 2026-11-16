/// Core snapping algorithm that matches run coordinates to road grid.
use super::error::SnapError;
use super::segment_index::RoadSegmentIndex;
use geo::{Distance, Haversine, LineString, MultiLineString};
use geo_types::{Coord, Point};

const SNAP_RADIUS_M: f64 = 25.0;
/// Break the polyline only after this many consecutive out-of-range points.
const MAX_CONSECUTIVE_OUTLIERS: usize = 5;
/// Maximum plausible distance between consecutive snapped points (meters).
/// Gaps larger than this indicate a jump between disconnected road segments.
const MAX_SNAP_DISTANCE_M: f64 = 100.0;

/// Snaps coordinates to the road grid.
pub struct Snapper<'a> {
    index: &'a RoadSegmentIndex,
}

impl<'a> Snapper<'a> {
    pub fn new(index: &'a RoadSegmentIndex) -> Self {
        Snapper { index }
    }

    /// Snap a polyline to the road grid.
    ///
    /// Returns a MultiLineString where each LineString represents a continuous
    /// segment of the snapped path. Breaks occur only after multiple consecutive
    /// points are too far from any road (true gaps).
    pub fn snap_polyline(&self, coords: &[Coord<f64>]) -> Result<MultiLineString, SnapError> {
        if coords.len() < 2 {
            return Err(SnapError::InsufficientPoints);
        }

        if self.index.segments().is_empty() {
            return Err(SnapError::NoRoadSegmentsLoaded);
        }

        let mut polylines: Vec<LineString> = Vec::new();
        let mut current_line: Vec<Coord<f64>> = Vec::new();
        let mut consecutive_outliers = 0;

        for coord in coords {
            match self.index.find_closest_segment(*coord, SNAP_RADIUS_M) {
                Some((segment, _distance)) => {
                    let (snapped_point, _, _) = segment.closest_point(*coord);
                    consecutive_outliers = 0;

                    // Check if this snapped point is suspiciously far from the previous one
                    if !current_line.is_empty() {
                        let last_point = current_line[current_line.len() - 1];
                        let snap_distance = Haversine.distance(
                            Point::new(last_point.x, last_point.y),
                            Point::new(snapped_point.x, snapped_point.y),
                        );

                        if snap_distance > MAX_SNAP_DISTANCE_M {
                            // Large jump detected - likely a disconnected road segment
                            // Break the polyline here
                            polylines.push(LineString::new(current_line.clone()));
                            current_line.clear();
                        }
                    }

                    current_line.push(snapped_point);
                }
                None => {
                    consecutive_outliers += 1;

                    // Only break polyline if we have too many consecutive outliers
                    if consecutive_outliers >= MAX_CONSECUTIVE_OUTLIERS && !current_line.is_empty()
                    {
                        polylines.push(LineString::new(current_line.clone()));
                        current_line.clear();
                    }
                }
            }
        }

        // Finish any remaining line
        if !current_line.is_empty() {
            polylines.push(LineString::new(current_line));
        }

        if polylines.is_empty() {
            return Err(SnapError::AllPointsOutOfRange);
        }

        Ok(MultiLineString::new(polylines))
    }
}
