//! Minimal Strava API client for the browser (WASM).
//!
//! Fetches the athlete's most recent runs and returns them as a GeoJSON `FeatureCollection` of `
//! LineString`s ready to hand to Mapbox.

use crate::{BACKEND_BASE_URL, session};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use comms::runs::RunResponse;
use geojson::{Feature, Geometry, Value};
use gloo_net::http::Request;
use http::status::StatusCode;
use serde::de::DeserializeOwned;

/// Strava encoded polylines use a precision of 5 decimal places.
const POLYLINE_PRECISION: u32 = 5;

enum CompleteDownload {
    Yes,
    No,
}

#[derive(PartialEq)]
pub enum LoadState {
    /// Continue loading. Includes the next after_id.
    Continue(Option<DateTime<Utc>>),
    /// Finished loading.
    Finished,
}

/// A batch of runs loaded from the backend.
pub struct LoadedRuns {
    /// Pairs of (strava activity ID, polyline feature).
    pub features: Vec<(i64, Feature)>,
    /// Loaded state of the runs for this athlete.
    pub load_state: LoadState,
}

pub enum LoadError {
    Unauthorized,
    Other(String),
}

/// Generic fetch helper.
async fn fetch_json<T: DeserializeOwned>(
    url: &str,
    error_name: &str,
) -> Result<(Vec<T>, CompleteDownload), LoadError> {
    let request = match session::authed(Request::get(url)) {
        Some(request) => request,
        None => return Ok((Vec::new(), CompleteDownload::Yes)),
    };
    let resp = request
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

    let data = resp
        .json()
        .await
        .map_err(|e| LoadError::Other(format!("Failed to parse {error_name}: {e}")))?;
    let load_state = if resp.status() == StatusCode::PARTIAL_CONTENT {
        CompleteDownload::No
    } else {
        CompleteDownload::Yes
    };
    Ok((data, load_state))
}

/// Fetch the most recent activities for the authenticated athlete.
///
/// `after_id` pages through results: only runs with a `strava_activity_id`
/// at or beyond it are returned by the backend.
async fn fetch_runs(
    before: Option<DateTime<Utc>>,
) -> Result<(Vec<comms::runs::RunResponse>, CompleteDownload), LoadError> {
    let mut url = format!("{BACKEND_BASE_URL}/api/runs");
    if let Some(before) = before {
        url = format!("{url}?before={}", before.timestamp());
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

fn format_time(seconds: i64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let remaining_seconds = seconds % 60;
    if hours >= 1 {
        if minutes > 0 {
            format!("{}h {}m", hours, minutes)
        } else {
            format!("{}h", hours)
        }
    } else if minutes >= 1 {
        if remaining_seconds > 0 {
            format!("{}m {}s", minutes, remaining_seconds)
        } else {
            format!("{}m", minutes)
        }
    } else {
        // unlikely lol
        format!("{}s", remaining_seconds)
    }
}

fn get_properties(run: &RunResponse) -> serde_json::Map<String, serde_json::Value> {
    // Future improvements to datetime:
    // - Use "Today" & "Yesterday" instead of date, if appropriate.
    // - Don't assume the run is in NZ. Use the timezone that corresponds to the start coordinate.
    let nz_tz: Tz = "Pacific/Auckland".parse().unwrap();
    let formatted_time = run
        .start_date
        .with_timezone(&nz_tz)
        .format("%A, %d %b %Y at %l:%M%P")
        .to_string();
    let distance_km = (run.distance as f64) / 1e3;
    // let formatted_moving_time =
    //     humantime::format_duration(Duration::from_secs(run.moving_time.try_into().unwrap()));
    let formatted_moving_time = format_time(run.moving_time);
    let popup_text = format!(
        "<h3>{formatted_time}</h3><h1>{}</h1>{:.2}km {}",
        run.name, distance_km, formatted_moving_time
    );

    let mut properties = serde_json::Map::new();
    properties.insert("popup_text".into(), serde_json::Value::String(popup_text));
    properties
}

/// Fetch recent runs and return them as a GeoJSON `FeatureCollection` of `LineString`s.
///
/// Pass `before` to fetch the following page or pass `None` for the initial load.
pub async fn load_run_lines(before: Option<DateTime<Utc>>) -> Result<LoadedRuns, LoadError> {
    let (runs, complete_download) = fetch_runs(before).await?;

    let features = runs
        .iter()
        .map(|run| {
            let coords = decode_line(&run.summary_map);

            let run_line = Feature {
                bbox: None,
                geometry: Some(Geometry::new(Value::LineString(coords))),
                id: None,
                properties: Some(get_properties(run)),
                foreign_members: None,
            };
            (run.strava_activity_id, run_line)
        })
        .collect();
    let load_state = match complete_download {
        CompleteDownload::No => {
            let next_before = match runs.last() {
                Some(last_run) => Some(last_run.start_date),
                None => before,
            };
            LoadState::Continue(next_before)
        }
        CompleteDownload::Yes => LoadState::Finished,
    };

    Ok(LoadedRuns {
        features,
        load_state,
    })
}

/// Fetch the authenticated athlete's profile picture URL from the backend.
pub async fn load_profile() -> Result<comms::athlete::AthleteResponse, LoadError> {
    let url = format!("{BACKEND_BASE_URL}/api/me");
    let request =
        session::authed(Request::get(&url)).expect("should only be called when logged in");
    let resp = request
        .send()
        .await
        .map_err(|e| LoadError::Other(format!("Profile request failed: {e}")))?;

    if !resp.ok() {
        if resp.status() == StatusCode::UNAUTHORIZED {
            session::delete_session_id();
            return Err(LoadError::Unauthorized);
        }
        return Err(LoadError::Other(format!(
            "Profile request returned HTTP {}",
            resp.status()
        )));
    }

    resp.json()
        .await
        .map_err(|err| LoadError::Other(format!("Failed to parse profile: {err}")))
}

#[cfg(test)]
mod tests {
    use super::format_time;

    #[test]
    fn format_time_seconds() {
        assert_eq!(format_time(0), "0s");
        assert_eq!(format_time(30), "30s");
    }

    #[test]
    fn format_time_minutes() {
        // Seconds shown.
        assert_eq!(format_time(90), "1m 30s");
        assert_eq!(format_time(125), "2m 5s");
        assert_eq!(format_time(3599), "59m 59s");
        // Exact minute, so no seconds shown.
        assert_eq!(format_time(120), "2m");
    }

    #[test]
    fn format_time_hours() {
        // 7325s = 2 hours, 2 minutes, 5 seconds. Minutes shown.
        assert_eq!(format_time(7325), "2h 2m");
        // Exact hours, so no minutes shown.
        assert_eq!(format_time(3600), "1h");
        assert_eq!(format_time(7200), "2h");
    }
}
