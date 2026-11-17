/// Core snapping algorithm that matches run coordinates to road grid.
use super::segment_index::{RoadSegment, RoadSegmentIndex};
use geo::{Distance, Haversine, LineString, MultiLineString};
use geo_types::{Coord, Point};

const SNAP_RADIUS_M: f64 = 15.0;
/// Maximum plausible distance between consecutive snapped points (meters).
/// Gaps larger than this indicate a jump between disconnected road segments.
const MAX_SNAP_DISTANCE_M: f64 = 100.0;

/// The part of a road segment that the run travelled along, as positions along that segment.
struct SegmentRun<'a> {
    segment: &'a RoadSegment,
    start: f64,
    end: f64,
    /// The off-segment sample the run entered from, if it didn't enter from a connected segment.
    entered_from: Option<Coord<f64>>,
}

/// Which way a run is travelling along its segment.
enum Direction {
    /// Towards the segment's last vertex.
    Forward,
    /// Towards the segment's first vertex.
    Backward,
    /// No direction yet (a single sample).
    None,
}

impl SegmentRun<'_> {
    /// The segment's own geometry between the run's start and end.
    /// None if the run covers no length (e.g. a single sample that wasn't connected to a neighbour).
    fn into_line(mut self) -> Option<LineString> {
        // Extending the start needs the run's direction, which is only known once it is complete.
        if let Some(previous_sample) = self.entered_from {
            self.extend_start_to_node(previous_sample);
        }
        let points = self.segment.slice(self.start, self.end);
        (points.len() >= 2).then(|| LineString::new(points))
    }

    /// Which way the run is travelling along its segment.
    fn direction(&self) -> Direction {
        if self.end > self.start {
            Direction::Forward
        } else if self.end < self.start {
            Direction::Backward
        } else {
            Direction::None
        }
    }

    /// The run was left at `next_sample`, which is off this segment (e.g. on a path or in a park).
    /// If the node ahead is nearer than that sample, the run passed through the node, so extend to it.
    fn extend_end_to_node(&mut self, next_sample: Coord<f64>) {
        let node_position = match self.direction() {
            Direction::Forward => self.segment.end_position(),
            Direction::Backward => 0.0,
            Direction::None => return,
        };
        let exit = self.segment.point_at(self.end);

        if distance_m(exit, self.segment.point_at(node_position)) <= distance_m(exit, next_sample) {
            self.end = node_position;
        }
    }

    /// The run was entered from `previous_sample`, which is off this segment. If the node behind
    /// is nearer than that sample, the run came through the node, so extend back to it.
    fn extend_start_to_node(&mut self, previous_sample: Coord<f64>) {
        let node_position = match self.direction() {
            Direction::Forward => 0.0,
            Direction::Backward => self.segment.end_position(),
            Direction::None => return,
        };
        let entry = self.segment.point_at(self.start);

        if distance_m(entry, self.segment.point_at(node_position))
            <= distance_m(entry, previous_sample)
        {
            self.start = node_position;
        }
    }
}

/// Snap a polyline to the road grid.
///
/// Returns a MultiLineString where each LineString follows the geometry of a single road
/// segment (way_id) between where the run entered and left it. When the run moves on to a
/// neighbouring segment, both lines are extended to the node the segments share. When the run
/// leaves or joins the road grid elsewhere (e.g. at a dead end onto a path), the line is extended
/// to the segment's end node if the run passed it.
/// Breaks occur when:
/// - Switching to a segment that doesn't connect to the previous one
/// - Consecutive points on one segment are implausibly far apart
/// - A point is too far from any road (a gap)
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
    let mut previous_coord: Option<Coord<f64>> = None;

    for coord in coords {
        let previous = previous_coord.replace(*coord);

        let Some((segment, projection)) = index.find_closest_segment(*coord, SNAP_RADIUS_M) else {
            if let Some(mut run) = current.take() {
                run.extend_end_to_node(*coord);
                lines.extend(run.into_line());
            }
            continue;
        };

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
            entered_from: None,
        };
        match current.take() {
            Some(mut previous_run) => {
                if previous_run.segment.way_id != segment.way_id
                    && !connect_at_shared_node(&mut previous_run, &mut next)
                {
                    previous_run.extend_end_to_node(*coord);
                    next.entered_from = previous;
                }
                lines.extend(previous_run.into_line());
            }
            None => next.entered_from = previous,
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
/// so the snapped lines meet at the intersection. Returns whether the runs were connected.
fn connect_at_shared_node(previous: &mut SegmentRun, next: &mut SegmentRun) -> bool {
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

    match best {
        Some((previous_position, next_position, _)) => {
            previous.end = previous_position;
            next.start = next_position;
            true
        }
        None => false,
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

    /// A sample in a park: on the road's line but far enough east/west of the dead end to be off-grid.
    fn off_grid(lon: f64) -> Coord<f64> {
        Coord { x: lon, y: -43.5 }
    }

    #[test]
    fn line_reaches_the_dead_end_when_the_run_continues_off_the_road_grid() {
        let index = RoadSegmentIndex::from_segments(vec![straight_segment(101, 172.600, 172.601)]);
        let dead_end = Coord {
            x: 172.601,
            y: -43.5,
        };

        let coords = [
            sample(172.6003),
            sample(172.6006),
            sample(172.6009),
            off_grid(172.6015),
            off_grid(172.6020),
        ];
        let lines = snap_polyline(&coords, &index).unwrap().0;

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0.last(), Some(&dead_end));
    }

    #[test]
    fn line_starts_at_the_dead_end_when_the_run_joins_from_off_the_road_grid() {
        let index = RoadSegmentIndex::from_segments(vec![straight_segment(101, 172.600, 172.601)]);
        let dead_end = Coord {
            x: 172.600,
            y: -43.5,
        };

        let coords = [
            off_grid(172.5985),
            off_grid(172.5990),
            sample(172.6002),
            sample(172.6005),
        ];
        let lines = snap_polyline(&coords, &index).unwrap().0;

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].0.first(), Some(&dead_end));
    }

    #[test]
    fn line_stops_where_the_run_left_the_road_mid_segment() {
        let index = RoadSegmentIndex::from_segments(vec![straight_segment(101, 172.600, 172.601)]);

        // The run leaves into a park well before the end of the segment.
        let coords = [
            sample(172.6001),
            sample(172.6003),
            Coord {
                x: 172.6004,
                y: -43.5005,
            },
        ];
        let lines = snap_polyline(&coords, &index).unwrap().0;

        assert_eq!(lines.len(), 1);
        assert!(lines[0].0.last().unwrap().x < 172.6004);
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
