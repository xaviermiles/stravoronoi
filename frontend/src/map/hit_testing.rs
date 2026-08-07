use geojson::{Feature, Value};

fn mercator_point(coordinates: &[f64]) -> Option<(f64, f64)> {
    let [lng, lat, ..] = coordinates else {
        return None;
    };
    let lat = lat.clamp(-85.051_128_78, 85.051_128_78).to_radians();
    Some((lng.to_radians(), (lat.tan() + 1.0 / lat.cos()).ln()))
}

fn distance_to_segment_squared(point: (f64, f64), start: (f64, f64), end: (f64, f64)) -> f64 {
    let segment = (end.0 - start.0, end.1 - start.1);
    let segment_length_squared = segment.0 * segment.0 + segment.1 * segment.1;
    if segment_length_squared == 0.0 {
        return (point.0 - start.0).powi(2) + (point.1 - start.1).powi(2);
    }

    let projection = (((point.0 - start.0) * segment.0 + (point.1 - start.1) * segment.1)
        / segment_length_squared)
        .clamp(0.0, 1.0);
    let nearest = (
        start.0 + projection * segment.0,
        start.1 + projection * segment.1,
    );
    (point.0 - nearest.0).powi(2) + (point.1 - nearest.1).powi(2)
}

fn distance_to_line_squared(point: (f64, f64), coordinates: &[Vec<f64>]) -> Option<f64> {
    coordinates
        .windows(2)
        .filter_map(|segment| {
            let start = mercator_point(&segment[0])?;
            let end = mercator_point(&segment[1])?;
            Some(distance_to_segment_squared(point, start, end))
        })
        .min_by(f64::total_cmp)
}

pub(super) fn distance_to_run_squared(feature: &Feature, tap_lng_lat: &[f64; 2]) -> Option<f64> {
    let tap = mercator_point(tap_lng_lat)?;
    let geometry = feature.geometry.as_ref()?;
    let Value::LineString(coordinates) = &geometry.value else {
        return None;
    };
    distance_to_line_squared(tap, coordinates)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a Feature for run coordinates.
    fn run_feature(coordinates: Vec<Vec<f64>>) -> Feature {
        Feature {
            geometry: Some(geojson::Geometry::new(Value::LineString(coordinates))),
            ..Default::default()
        }
    }

    #[test]
    fn segment_distance_uses_nearest_point_on_segment() {
        let distance = distance_to_segment_squared((1.0, 1.0), (0.0, 0.0), (2.0, 0.0));

        assert_eq!(distance, 1.0);
    }

    #[test]
    fn closer_parallel_run_has_smaller_distance() {
        let closer = run_feature(vec![vec![172.0, -43.5001], vec![173.0, -43.5001]]);
        let farther = run_feature(vec![vec![172.0, -43.51], vec![173.0, -43.51]]);
        let tap = [172.5, -43.5];

        assert!(
            distance_to_run_squared(&closer, &tap).unwrap()
                < distance_to_run_squared(&farther, &tap).unwrap()
        );
    }
}
