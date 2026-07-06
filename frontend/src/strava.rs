//! Minimal Strava API client for the browser (WASM).
//!
//! Asks the backend for a short-lived Strava access token, then fetches the athlete's most recent
//! runs and returns them as a GeoJSON `FeatureCollection` of `LineString`s ready to hand to
//! Mapbox.

use crate::BACKEND_BASE_URL;
use geojson::{Feature, FeatureCollection, GeoJson, Geometry, Value};
use gloo_net::http::Request;
use serde::{Deserialize, de::DeserializeOwned};
use web_sys::RequestCredentials;
use gloo_storage::{LocalStorage, Storage, errors::StorageError};

/// Strava encoded polylines use a precision of 5 decimal places.
const POLYLINE_PRECISION: u32 = 5;

// TODO: put frontend-backend API structs in shared crate
#[derive(Deserialize)]
struct SummaryActivity {
    name: String,
    polyline_map: String,
}

/// Generic fetch helper.
async fn fetch_json<T: DeserializeOwned>(url: &str, error_name: &str) -> Result<Vec<T>, String> {
    let session_id: String = match LocalStorage::get("session_id") {
        Ok(session_id) => session_id,
        Err(StorageError::KeyNotFound(_)) => return Ok(Vec::new()),
        Err(err) => {
            // Log unexpected errors.
            log::info!("{}", err.to_string());
            return Ok(Vec::new());
        }
    };
    let resp = Request::get(&url)
        .header("Authorization", &format!("Bearer {session_id}"))
        .credentials(RequestCredentials::Include)
        .send()
        .await
        .map_err(|e| format!("{error_name} request failed: {e}"))?;

    if !resp.ok() {
        return Err(format!(
            "{error_name} request returned HTTP {}",
            resp.status()
        ));
    }

    resp.json()
        .await
        .map_err(|e| format!("Failed to parse {error_name}: {e}"))
}

/// Fetch the most recent activities for the authenticated athlete.
async fn fetch_activities() -> Result<Vec<SummaryActivity>, String> {
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
pub async fn load_run_lines() -> Result<GeoJson, String> {
    let activities = fetch_activities().await?;

    let mut features = Vec::new();
    for activity in activities.into_iter() {
        let mut properties = serde_json::Map::new();
        properties.insert("name".to_string(), serde_json::Value::String(activity.name));
        let coords = decode_line(&activity.polyline_map);

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
