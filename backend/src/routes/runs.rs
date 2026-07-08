//! Minimal Strava API client for the browser (WASM).
//!
//! Asks the backend for a short-lived Strava access token, then fetches the athlete's most recent
//! runs and returns them as a GeoJSON `FeatureCollection` of `LineString`s ready to hand to
//! Mapbox.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};
use geo_types::Coord;

use crate::session::AuthedAthlete;
use crate::{AppState, models, services};

const ACTIVITIES_URL: &str = "https://www.strava.com/api/v3/athlete/activities";

/// Number of most-recent activities to request.
// TODO: this is so low because the map matching API takes a while. Could it stream the individual runs? Or cache the results?
const PER_PAGE: u32 = 10;
/// Strava encoded polylines use a precision of 5 decimal places.
const POLYLINE_PRECISION: u32 = 5;

#[derive(Debug, Deserialize)]
struct PolylineMap {
    #[serde(default)]
    summary_polyline: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SummaryActivity {
    #[serde(default)]
    name: String,
    #[serde(default)]
    sport_type: String,
    map: PolylineMap,
}

#[derive(Serialize, Deserialize)]
struct Run {
    name: String,
    polyline_map: String,
}

/// Fetch the most recent activities for the authenticated athlete.
async fn fetch_activities(
    access_token: &str,
) -> Result<Vec<SummaryActivity>, (StatusCode, String)> {
    let url = format!("{ACTIVITIES_URL}?per_page={PER_PAGE}&page=1");
    let client = reqwest::Client::new();
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access_token}")).unwrap(),
    );
    // fetch_json(&url, access_token, "Activities").await
    let response = client.get(url).headers(headers).send().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get activities: {e}"),
        )
    })?;
    let json = response
        .json::<Vec<SummaryActivity>>()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(json)
}

/// Decode a Strava encoded polyline into GeoJSON positions (`[lng, lat]`).
///
/// The `polyline` crate returns `geo-types` coordinates in `(x = lng, y = lat)`
/// order, which is exactly the order GeoJSON expects.
fn decode_line(encoded: &str) -> Vec<Coord<f64>> {
    match polyline::decode_polyline(encoded, POLYLINE_PRECISION) {
        Ok(line) => line.into_inner(),
        Err(_) => Vec::new(),
    }
}

/// Fetch recent runs.
pub async fn list_runs(State(state): State<AppState>, athlete: AuthedAthlete) -> Response {
    let athlete_id = athlete.athlete_id;
    let access_token = match models::athlete::Entity::find_by_id(athlete_id)
        .one(&state.database)
        .await
    {
        Ok(Some(athlete)) => athlete.access_token,
        Ok(None) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Missing athlete ID: {}", athlete_id),
            )
                .into_response();
        }
        Err(err) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Error while finding athlete: {}", err.to_string()),
            )
                .into_response();
        }
    };

    let activities = match fetch_activities(&access_token).await {
        Ok(activities) => activities,
        Err(err) => return err.into_response(),
    };

    let mut runs = Vec::new();
    for activity in activities.into_iter() {
        if !(activity.sport_type.contains("Run")) {
            continue;
        }
        let encoded_coords = match activity.map.summary_polyline {
            Some(p) => p,
            None => continue,
        };
        // runs.push(Run {
        //     name: activity.name,
        //     polyline_map: encoded_coords,
        // });
        // continue;

        let raw_coords = decode_line(&encoded_coords);
        if raw_coords.len() < 2 {
            // Not a line with less than 2 points.
            continue;
        }
        // TODO: Currently it is mapping the snapped lines. I want the snapped lines to be primarily used for the voronoi calculations.
        //       They could be mapped in addition to the raw coordinates but they should be different colour and possibly more transparent.
        let snapped_coords = services::mapbox::map_match(&raw_coords).await;
        if snapped_coords.len() < 2 {
            // Not a line with less than 2 points.
            continue;
        }

        match polyline::encode_coordinates(snapped_coords, POLYLINE_PRECISION) {
            Ok(encoded_polyline_map) => runs.push(Run {
                name: activity.name,
                polyline_map: encoded_polyline_map,
            }),
            Err(err) => {
                log::error!("{}", err.to_string())
            }
        }
    }

    (StatusCode::OK, Json(runs)).into_response()
}
