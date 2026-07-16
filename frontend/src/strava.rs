//! Minimal Strava API client for the browser (WASM).
//!
//! Fetches the athlete's most recent runs and returns them as a GeoJSON `FeatureCollection` of `
//! LineString`s ready to hand to Mapbox.

use crate::{BACKEND_BASE_URL, session};
use geojson::{Feature, GeoJson, Geometry, Value};
use gloo_net::http::Request;
use http::status::StatusCode;
use serde::de::DeserializeOwned;
use web_sys::RequestCredentials;

/// Strava encoded polylines use a precision of 5 decimal places.
const POLYLINE_PRECISION: u32 = 5;

enum CompleteDownload {
    Yes,
    No,
}

#[derive(PartialEq)]
pub enum LoadState {
    /// Continue loading. Includes the next after_id.
    Continue(Option<i32>),
    /// Finished loading.
    Finished,
}

pub struct LoadedRuns {
    pub features: Vec<GeoJson>,
    pub load_state: LoadState,
}

pub enum LoadError {
    Unauthorized,
    Other(String),
}

/// Generic fetch helper.
async fn fetch_json<T: DeserializeOwned>(url: &str, error_name: &str) -> Result<(Vec<T>, CompleteDownload), LoadError> {
    let session_id = match session::get_session_id() {
        Some(session_id) => session_id,
        None => return Ok((Vec::new(), CompleteDownload::Yes)),
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
    if resp.status() == StatusCode::NO_CONTENT {
        // There will be no body to parse.
        return Ok((Vec::new(), CompleteDownload::No));
    }

    let data = resp.json()
        .await
        .map_err(|e| LoadError::Other(format!("Failed to parse {error_name}: {e}")))?;
    let load_state = if resp.status() == StatusCode::PARTIAL_CONTENT {CompleteDownload::No} else {CompleteDownload::Yes};
    Ok((data, load_state))
}

/// Fetch the most recent activities for the authenticated athlete.
///
/// `after_id` pages through results: only runs with a `strava_activity_id`
/// at or beyond it are returned by the backend.
async fn fetch_runs(after_id: Option<i32>) -> Result<(Vec<comms::runs::RunResponse>, CompleteDownload), LoadError> {
    let mut url = format!("{BACKEND_BASE_URL}/api/runs");
    if let Some(after_id) = after_id {
        url = format!("{url}?after_id={after_id}");
    }
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
///
/// Pass `after_id` to fetch the following page or pass `None` for the initial load.
pub async fn load_run_lines(after_id: Option<i32>) -> Result<LoadedRuns, LoadError> {
    let (runs, complete_download) = fetch_runs(after_id).await?;

    let features = runs.iter().map(|run| {
        let mut properties = serde_json::Map::new();
        properties.insert("strava_activity_id".to_string(), serde_json::Value::Number(run.strava_activity_id.into()));
        properties.insert("name".to_string(), serde_json::Value::String(run.name.clone()));
        let coords = decode_line(&run.summary_map);

        GeoJson::Feature(Feature {
            bbox: None,
            geometry: Some(Geometry::new(Value::LineString(coords))),
            id: None,
            properties: Some(properties),
            foreign_members: None,
        })
    }).collect();
    let load_state = match complete_download {
        CompleteDownload::No => {
            let next_after_id = match runs.last() {
                Some(last_run) => Some(last_run.strava_activity_id),
                None => after_id,
            };
            LoadState::Continue(next_after_id)
        }
        CompleteDownload::Yes => LoadState::Finished,
    };

    Ok(LoadedRuns {
        features,
        load_state,
    })
}
