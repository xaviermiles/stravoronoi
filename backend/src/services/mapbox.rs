/// Map matching using the mapbox Map Matching API.
///
/// https://docs.mapbox.com/api/navigation/map-matching/
use axum::http::StatusCode;
use geo::{LineString, MultiLineString};
use geo_types::geometry::Coord;
use geojson::{LineStringType, Value};
use serde::{self, Deserialize};
use std::time::Duration;

const MAPBOX_TOKEN: &str = env!("MAPBOX_TOKEN");
// The /driving endpoint only includes roads, while /walking matches to paths if they are closer.
const MATCHING_URL: &str = "https://api.mapbox.com/matching/v5/mapbox/driving";
/// Mapbox Map Matching accepts at most 100 coordinates per request.
const MAX_MATCH_COORDS: usize = 100;

pub enum MatchError {
    /// An error that is consistently reproducible with the same data.
    Permanent(String),
    /// An error that should be recoverable immediately.
    Transient(String),
    /// An error that should be recoverable after waiting.
    Backoff(Duration),
}

/// https://docs.mapbox.com/api/navigation/map-matching/#response-retrieve-a-match
#[derive(Deserialize)]
struct MatchResponse {
    #[serde(default)]
    code: String,
    #[serde(default)]
    matchings: Vec<Matching>,
    /// Human-readable explanation of the error.
    ///
    /// Will only be present on responses with HTTP status codes that are errors (!= 200) and lower than 500.
    #[serde(default)]
    message: String,
}

/// https://docs.mapbox.com/api/navigation/map-matching/#match-object
#[derive(Deserialize)]
struct Matching {
    geometry: geojson::Geometry, // geometries=geojson => a LineString
}

fn as_line(geom: &geojson::Geometry) -> LineStringType {
    match &geom.value {
        Value::LineString(coords) => coords.clone(),
        _ => Vec::new(),
    }
}

/// Snap coordinates to the road network.
async fn match_chunk(coords: &[Coord<f64>]) -> Result<Vec<LineStringType>, MatchError> {
    let path = coords
        .iter()
        .map(|coord: &Coord<f64>| format!("{},{}", coord.x, coord.y)) // lng,lat
        .collect::<Vec<_>>()
        .join(";");
    // Per-point search radius (m). One value per coordinate is required.
    let radiuses = vec!["25"; coords.len()].join(";");

    let url = format!(
        "{MATCHING_URL}/{path}?geometries=geojson&overview=simplified&tidy=true\
         &radiuses={radiuses}&access_token={MAPBOX_TOKEN}"
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|err| MatchError::Transient(format!("Map matching failed: {err}")))?;
    let status = resp.status();
    let matched: MatchResponse = resp.json().await.map_err(|err| {
        MatchError::Transient(format!("Failed to parse matching response: {err}"))
    })?;
    if status == StatusCode::TOO_MANY_REQUESTS {
        // The rate limit is "300 requests per minute" so waiting 1 minute should fix this.
        // https://docs.mapbox.com/api/guides/#rate-limits
        return Err(MatchError::Backoff(Duration::from_mins(1)));
    } else if status != StatusCode::OK {
        return Err(MatchError::Permanent(format!(
            "Matching code: {}, message: {}",
            matched.code, matched.message
        )));
    }
    Ok(matched
        .matchings
        .into_iter()
        .map(|matching| as_line(&matching.geometry))
        .collect())
}

/// Map-match a full run.
pub async fn map_match(coords: &[Coord<f64>]) -> Result<MultiLineString, MatchError> {
    if coords.len() < 2 {
        return Err(MatchError::Permanent(
            "Less than 2 points means it isn't a line.".into(),
        ));
    }

    let mut polylines: Vec<LineString> = Vec::new();
    let mut current_polyline: LineStringType = Vec::new();
    let mut start: usize = 0;
    while start < coords.len() - 1 {
        let end = (start + MAX_MATCH_COORDS).min(coords.len());
        let chunk = &coords[start..end];

        for seg in match_chunk(chunk).await? {
            if let Some(last_current_polyline) = current_polyline.last()
                && let Some(first_next_seg) = seg.first()
                && last_current_polyline == first_next_seg
            {
                // Chunks overlap by one input point; drop the seam vertex to reduce duplication.
                current_polyline.extend(seg.into_iter().skip(1));
            } else {
                // The matched chunks are distinct, so finish the current polyline and start a new one.
                polylines.push(LineString::new(
                    current_polyline
                        .iter()
                        .map(|p| Coord { x: p[0], y: p[1] })
                        .collect(),
                ));
                current_polyline = seg;
            }
        }

        if end == coords.len() {
            break;
        }
        start = end - 1; // overlap for continuity
    }
    Ok(MultiLineString::new(polylines))
}
