/// Core snapping algorithm that matches run coordinates to road grid.
use super::segment_index::{RoadSegment, RoadSegmentIndex};
use geo::{Distance, Haversine, LineString, MultiLineString};
use geo_types::{Coord, Point};

const SNAP_RADIUS_M: f64 = 30.0;
/// Break the polyline only after this many consecutive out-of-range points.
const MAX_CONSECUTIVE_OUTLIERS: usize = 5;
/// Maximum plausible distance between consecutive snapped points (meters).
/// Gaps larger than this indicate a jump between disconnected road segments.
const MAX_SNAP_DISTANCE_M: f64 = 100.0;

/// The part of a road segment that the run travelled along, as positions along that segment.
struct SegmentRun<'a> {
    segment: &'a RoadSegment,
    start: f64,
    end: f64,
}

impl SegmentRun<'_> {
    /// The segment's own geometry between the run's start and end.
    /// None if the run covers no length (e.g. a single sample that wasn't connected to a neighbour).
    fn into_line(self) -> Option<LineString> {
        let points = self.segment.slice(self.start, self.end);
        (points.len() >= 2).then(|| LineString::new(points))
    }
}

/// Snap a polyline to the road grid.
///
/// Returns a MultiLineString where each LineString follows the geometry of a single road
/// segment (way_id) between where the run entered and left it. When the run moves on to a
/// neighbouring segment, both lines are extended to the node the segments share.
/// Breaks occur when:
/// - Switching to a segment that doesn't connect to the previous one
/// - Consecutive points on one segment are implausibly far apart
/// - Multiple consecutive points are too far from any road (true gaps)
pub fn snap_polyline(coords: &[Coord<f64>], index: &RoadSegmentIndex) -> Option<MultiLineString> {
    if coords.len() < 2 {
        // Less than 2 points means it isn't a line.
        return None;
    }

    if index.is_empty() {
        return None;
    }

    let mut lines: Vec<LineString> = Vec::new();
    let mut current: Option<SegmentRun> = None;
    let mut consecutive_outliers = 0;

    for coord in coords {
        let Some((segment, projection)) = index.find_closest_segment(*coord, SNAP_RADIUS_M) else {
            consecutive_outliers += 1;

            if consecutive_outliers >= MAX_CONSECUTIVE_OUTLIERS {
                lines.extend(current.take().and_then(SegmentRun::into_line));
            }
            continue;
        };
        consecutive_outliers = 0;

        let continues_current_run = current.as_ref().is_some_and(|run| {
            run.segment.way_id == segment.way_id
                && distance_m(run.segment.point_at(run.end), projection.point)
                    <= MAX_SNAP_DISTANCE_M
        });

        if continues_current_run {
            if let Some(run) = current.as_mut() {
                run.end = projection.position;
            }
            continue;
        }

        let mut next = SegmentRun {
            segment,
            start: projection.position,
            end: projection.position,
        };
        if let Some(mut previous) = current.take() {
            if previous.segment.way_id != segment.way_id {
                connect_at_shared_node(&mut previous, &mut next);
            }
            lines.extend(previous.into_line());
        }
        current = Some(next);
    }

    lines.extend(current.and_then(SegmentRun::into_line));

    if lines.is_empty() {
        // No run covered any length of road.
        return None;
    }

    Some(MultiLineString::new(lines))
}

/// If two consecutive runs are on segments that share an end node, extend both runs to that node
/// so the snapped lines meet at the intersection.
fn connect_at_shared_node(previous: &mut SegmentRun, next: &mut SegmentRun) {
    let previous_exit = previous.segment.point_at(previous.end);
    let next_entry = next.segment.point_at(next.start);

    // (position on previous, position on next, distance from previous exit to the node)
    let mut best: Option<(f64, f64, f64)> = None;

    for previous_position in [0.0, previous.segment.end_position()] {
        let node = previous.segment.point_at(previous_position);
        let exit_distance = distance_m(previous_exit, node);

        for next_position in [0.0, next.segment.end_position()] {
            let is_shared_node = next.segment.point_at(next_position) == node;
            let is_plausible = exit_distance <= MAX_SNAP_DISTANCE_M
                && distance_m(node, next_entry) <= MAX_SNAP_DISTANCE_M;
            let is_closer = best.is_none_or(|(_, _, best_distance)| exit_distance < best_distance);

            if is_shared_node && is_plausible && is_closer {
                best = Some((previous_position, next_position, exit_distance));
            }
        }
    }

    if let Some((previous_position, next_position, _)) = best {
        previous.end = previous_position;
        next.start = next_position;
    }
}

fn distance_m(a: Coord<f64>, b: Coord<f64>) -> f64 {
    Haversine.distance(Point::from(a), Point::from(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn straight_segment(way_id: i64, from_lon: f64, to_lon: f64) -> RoadSegment {
        RoadSegment::new(
            way_id,
            vec![
                Coord {
                    x: from_lon,
                    y: -43.5,
                },
                Coord {
                    x: to_lon,
                    y: -43.5,
                },
            ],
        )
    }

    /// A sample about 5m north of the road at the given longitude.
    fn sample(lon: f64) -> Coord<f64> {
        Coord {
            x: lon,
            y: -43.50005,
        }
    }

    #[test]
    fn lines_meet_at_the_shared_intersection_when_continuing_straight() {
        let index = RoadSegmentIndex::from_segments(vec![
            straight_segment(101, 172.600, 172.601),
            straight_segment(102, 172.601, 172.602),
        ]);
        let intersection = Coord {
            x: 172.601,
            y: -43.5,
        };

        let coords = [
            sample(172.6003),
            sample(172.6006),
            sample(172.6013),
            sample(172.6016),
        ];
        let lines = snap_polyline(&coords, &index).unwrap().0;

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].0.last(), Some(&intersection));
        assert_eq!(lines[1].0.first(), Some(&intersection));
    }

    #[test]
    fn lines_do_not_connect_across_unconnected_segments() {
        let index = RoadSegmentIndex::from_segments(vec![
            straight_segment(101, 172.600, 172.601),
            straight_segment(102, 172.6011, 172.602),
        ]);

        let coords = [
            sample(172.6003),
            sample(172.6006),
            sample(172.6013),
            sample(172.6016),
        ];
        let lines = snap_polyline(&coords, &index).unwrap().0;

        assert_eq!(lines.len(), 2);
        assert_ne!(lines[0].0.last(), lines[1].0.first());
    }
}
