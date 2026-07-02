//! Minimal Strava API client for the browser (WASM).
//!
//! Uses a pre-configured refresh token (baked in at build time via `build.rs`)
//! to obtain a short-lived access token, then fetches the athlete's most recent
//! runs and returns them as a GeoJSON `FeatureCollection` of `LineString`s ready
//! to hand to Mapbox.
//!
//! NOTE: the client secret and refresh token are embedded in the WASM binary.
//! This is acceptable for local development only. For a real deployment, move
//! the token exchange behind a backend proxy so the secret is never shipped to
//! the browser.

use geojson::{Feature, FeatureCollection, GeoJson, Geometry, Value};
use gloo_net::http::Request;
use serde::{Deserialize, de::DeserializeOwned};

use crate::map;

const CLIENT_ID: &str = env!("STRAVA_CLIENT_ID");
const CLIENT_SECRET: &str = env!("STRAVA_CLIENT_SECRET");
const REFRESH_TOKEN: &str = env!("STRAVA_REFRESH_TOKEN");

const TOKEN_URL: &str = "https://www.strava.com/oauth/token";
const ACTIVITIES_URL: &str = "https://www.strava.com/api/v3/athlete/activities";

/// Number of most-recent activities to request.
// TODO: this is so low because the map matching API takes a while. Could it stream the individual runs? Or cache the results?
const PER_PAGE: u32 = 5;
/// Strava encoded polylines use a precision of 5 decimal places.
const POLYLINE_PRECISION: u32 = 5;

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct PolylineMap {
    #[serde(default)]
    summary_polyline: Option<String>,
}

#[derive(Deserialize)]
struct SummaryActivity {
    #[serde(default)]
    name: String,
    #[serde(default)]
    sport_type: String,
    map: PolylineMap,
}

/// Exchange the stored refresh token for a fresh short-lived access token.
async fn refresh_access_token() -> Result<String, String> {
    let url = format!(
        "{TOKEN_URL}?client_id={CLIENT_ID}&client_secret={CLIENT_SECRET}\
         &grant_type=refresh_token&refresh_token={REFRESH_TOKEN}"
    );

    let resp = Request::post(&url)
        .send()
        .await
        .map_err(|e| format!("Token request failed: {e}"))?;

    if !resp.ok() {
        return Err(format!("Token request returned HTTP {}", resp.status()));
    }

    let token: TokenResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse token response: {e}"))?;

    Ok(token.access_token)
}

/// Generic fetch helper.
async fn fetch_json<T: DeserializeOwned>(
    url: &str,
    access_token: &str,
    error_name: &str,
) -> Result<Vec<T>, String> {
    let resp = Request::get(url)
        .header("Authorization", &format!("Bearer {access_token}"))
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
async fn fetch_activities(access_token: &str) -> Result<Vec<SummaryActivity>, String> {
    let url = format!("{ACTIVITIES_URL}?per_page={PER_PAGE}&page=1");
    fetch_json(&url, access_token, "Activities").await
}

/// Fetch the segment efforts for a given activity.
// TODO: was here for voronoi but might be unnecessary if the map matching works and is quick enough.
#[allow(dead_code)]
async fn fetch_segment_efforts(
    access_token: &str,
    activity_id: u64,
) -> Result<Vec<SummaryActivity>, String> {
    let url = format!("https://www.strava.com/api/v3/activities/{activity_id}/segment_efforts");
    fetch_json(&url, access_token, "Segment efforts").await
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

/// Refresh the token, fetch recent runs, and return them as a GeoJSON
/// `FeatureCollection` of `LineString`s.
pub async fn load_run_lines() -> Result<GeoJson, String> {
    let access_token = refresh_access_token().await?;
    let activities = fetch_activities(&access_token).await?;

    let mut features = Vec::new();
    for activity in activities.into_iter() {
        if !(activity.sport_type.contains("Run")) {
            continue;
        }
        let encoded_coords = match activity.map.summary_polyline {
            Some(p) => p,
            None => continue,
        };

        let raw_coords = decode_line(&encoded_coords);
        if raw_coords.len() < 2 {
            // Not a line with less than 2 points.
            continue;
        }
        // TODO: Currently it is mapping the snapped lines. I want the snapped lines to be primarily used for the voronoi calculations.
        //       They could be mapped in addition to the raw coordinates but they should be different colour and possibly more transparent.
        let snapped_coords = map::map_match(&raw_coords).await;
        if snapped_coords.len() < 2 {
            // Not a line with less than 2 points.
            continue;
        }

        let mut properties = serde_json::Map::new();
        properties.insert("name".to_string(), serde_json::Value::String(activity.name));

        features.push(Feature {
            bbox: None,
            geometry: Some(Geometry::new(Value::LineString(snapped_coords))),
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
