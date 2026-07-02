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
use serde::Deserialize;

const CLIENT_ID: &str = env!("STRAVA_CLIENT_ID");
const CLIENT_SECRET: &str = env!("STRAVA_CLIENT_SECRET");
const REFRESH_TOKEN: &str = env!("STRAVA_REFRESH_TOKEN");

const TOKEN_URL: &str = "https://www.strava.com/oauth/token";
const ACTIVITIES_URL: &str = "https://www.strava.com/api/v3/athlete/activities";

/// Number of most-recent activities to request.
const PER_PAGE: u32 = 120;
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

/// Fetch the most recent activities for the authenticated athlete.
async fn fetch_activities(access_token: &str) -> Result<Vec<SummaryActivity>, String> {
    let url = format!("{ACTIVITIES_URL}?per_page={PER_PAGE}&page=1");

    let resp = Request::get(&url)
        .header("Authorization", &format!("Bearer {access_token}"))
        .send()
        .await
        .map_err(|e| format!("Activities request failed: {e}"))?;

    if !resp.ok() {
        return Err(format!(
            "Activities request returned HTTP {}",
            resp.status()
        ));
    }

    resp.json()
        .await
        .map_err(|e| format!("Failed to parse activities: {e}"))
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

    let features: Vec<Feature> = activities
        .into_iter()
        .filter(|a| a.sport_type.contains("Run"))
        .filter_map(|a| {
            let encoded = a.map.summary_polyline?;
            let coords = decode_line(&encoded);
            if coords.len() < 2 {
                return None;
            }

            let mut properties = serde_json::Map::new();
            properties.insert("name".to_string(), serde_json::Value::String(a.name));

            Some(Feature {
                bbox: None,
                geometry: Some(Geometry::new(Value::LineString(coords))),
                id: None,
                properties: Some(properties),
                foreign_members: None,
            })
        })
        .collect();

    Ok(GeoJson::FeatureCollection(FeatureCollection {
        bbox: None,
        features,
        foreign_members: None,
    }))
}
