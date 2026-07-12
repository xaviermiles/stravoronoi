//! Minimal Strava API client for the browser (WASM).
//!
//! Fetches the athlete's most recent runs and returns them as a GeoJSON `FeatureCollection` of `
//! LineString`s ready to hand to Mapbox.

use crate::{BACKEND_BASE_URL, session};
use geojson::{Feature, FeatureCollection, GeoJson, Geometry, Value};
use gloo_net::http::Request;
use http::status::StatusCode;
use serde::de::DeserializeOwned;
use web_sys::RequestCredentials;

/// Strava encoded polylines use a precision of 5 decimal places.
const POLYLINE_PRECISION: u32 = 5;

pub enum LoadError {
    Unauthorized,
    Other(String),
}

/// Generic fetch helper.
async fn fetch_json<T: DeserializeOwned>(url: &str, error_name: &str) -> Result<Vec<T>, LoadError> {
    let session_id = match session::get_session_id() {
        Some(session_id) => session_id,
        None => return Ok(Vec::new()),
    };
    let resp = Request::get(url)
        .header("Authorization", &format!("Bearer {session_id}"))
        .credentials(RequestCredentials::Include)
        .send()
        .await
        .map_err(|e| LoadError::Other(format!("{error_name} request failed: {e}")))?;

    if !resp.ok() {
        if resp.status() == StatusCode::UNAUTHORIZED {
            session::delete_session_id();
            return Err(LoadError::Unauthorized);
        }
        return Err(LoadError::Other(format!(
            "{error_name} request returned HTTP {}",
            resp.status()
        )));
    }

    resp.json()
        .await
        .map_err(|e| LoadError::Other(format!("Failed to parse {error_name}: {e}")))
}

/// Fetch the most recent activities for the authenticated athlete.
async fn fetch_activities() -> Result<Vec<comms::runs::RunResponse>, LoadError> {
    let url = format!("{BACKEND_BASE_URL}/api/runs");
    fetch_json(&url, "Activities").await
}

/// Decode a Strava encoded polyline into GeoJSON positions (`[lng, lat]`).
///
/// The `polyline` crate returns `geo-types` coordinates in `(x = lng, y = lat)`
/// order, which is exactly the order GeoJSON expects.
fn decode_line(encoded: &str) -> Vec<Vec<f64>> {
    match polyline::decode_polyline(encoded, POLYLINE_PRECISION) {
        Ok(line) => line.coords().map(|c| vec![c.x, c.y]).collect(),
        Err(_) => Vec::new(),
    }
}

/// Fetch recent runs and return them as a GeoJSON `FeatureCollection` of `LineString`s.
pub async fn load_run_lines() -> Result<GeoJson, LoadError> {
    let activities = fetch_activities().await?;

    let mut features = Vec::new();
    for activity in activities.into_iter() {
        let mut properties = serde_json::Map::new();
        properties.insert("name".to_string(), serde_json::Value::String(activity.name));
        let coords = decode_line(&activity.summary_map);

        features.push(Feature {
            bbox: None,
            geometry: Some(Geometry::new(Value::LineString(coords))),
            id: None,
            properties: Some(properties),
            foreign_members: None,
        })
    }

    Ok(GeoJson::FeatureCollection(FeatureCollection {
        bbox: None,
        features,
        foreign_members: None,
    }))
}
