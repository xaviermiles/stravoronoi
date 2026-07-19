//! Minimal Strava API client for the browser.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use comms::runs::RunResponse;
use sea_orm::ActiveValue::Set;
use sea_orm::DatabaseConnection;
use sea_orm::EntityTrait;
use sea_orm::QueryFilter;
use sea_orm::QuerySelect;
use sea_orm::{ActiveModelTrait, QueryOrder, Select};
use serde::Deserialize;
use tokio::time::{Duration, sleep};

use crate::services::strava::FetchError;
use crate::session::AuthedAthlete;
use crate::{AppState, models, services};

/// Strava encoded polylines use a precision of 5 decimal places.
const POLYLINE_PRECISION: u32 = 5;

// This will retry 5 times. This backoff usually won't work since the rate limits are per 15
// minutes and per 1 day, but it doesn't hurt since we will wait between requests anyway.
const START_WAIT: Duration = Duration::from_millis(100);
const MAX_WAIT: Duration = Duration::from_secs(2);

// TODO: Currently it is mapping the snapped lines. I want the snapped lines to be primarily used for the voronoi calculations.
//       They could be mapped in addition to the raw coordinates but they should be different colour and possibly more transparent.
/// Snap a Strava encoded polyline using map matching.
#[allow(dead_code)]
async fn snap_line(encoded_coords: &str) -> Result<String, String> {
    let raw_coords = polyline::decode_polyline(encoded_coords, POLYLINE_PRECISION)
        .map_err(|err| format!("Failed to decoded polyline {err}"))?
        .into_inner();

    if raw_coords.len() < 2 {
        return Err("Less than 2 points means it isn't a line.".into());
    }
    let snapped_coords = services::mapbox::map_match(&raw_coords).await;
    if snapped_coords.len() < 2 {
        return Err("Less than 2 points means it isn't a line.".into());
    }

    polyline::encode_coordinates(snapped_coords, POLYLINE_PRECISION)
        .map_err(|err| format!("Failed to encode polyline: {err}"))
}

/// Start fetching older runs before a given time.
///
/// If no time is given then all runs will be fetched.
async fn fetch_older_runs(
    database: &DatabaseConnection,
    athlete_id: i64,
    mut before_epoch: Option<DateTime<Utc>>,
) -> Result<(), String> {
    let access_token = match models::athlete::Entity::find_by_id(athlete_id)
        .one(database)
        .await
    {
        Ok(Some(athlete)) => athlete.access_token,
        Ok(None) => return Err("Cannot find athlete for given session ID.".to_string()),
        Err(err) => return Err(format!("Error while finding athlete: {err}")),
    };
    tracing::info!("Start fetching runs for athlete ID: {athlete_id}");

    let mut current_wait = START_WAIT;
    let mut final_activity_id: Option<i64> = None;
    loop {
        let activities = match services::strava::fetch_activities(&access_token, before_epoch).await
        {
            Ok(activities) => activities,
            Err(FetchError::Backoff) => {
                current_wait *= 2;
                if current_wait > MAX_WAIT {
                    break;
                }
                sleep(current_wait).await;
                continue;
            }
            Err(FetchError::Other(message)) => {
                tracing::error!("{message}");
                break;
            }
        };
        // Reset wait since we weren't told to backoff.
        current_wait = START_WAIT;
        if activities.is_empty() {
            break;
        }
        final_activity_id = Some(activities.last().expect("checked is_empty() above").id);
        for activity in activities.iter() {
            if !(activity.sport_type.contains("Run")) {
                continue;
            }
            // TODO: can clones be avoided?
            let run = models::run::ActiveModel {
                strava_activity_id: Set(activity.id),
                athlete_id: Set(athlete_id),
                name: Set(activity.name.clone()),
                start_date: Set(activity.start_date.into()),
                summary_map: Set(activity.map.summary_polyline.clone()),
                is_final_activity: Set(false), // to be modified afterwards
            };
            models::run::Entity::insert(run)
                .exec(database)
                .await
                .map_err(|err| format!("Error while inserting run: {err}"))?;
        }
        before_epoch = Some(
            activities
                .last()
                .expect("checked is_empty() above")
                .start_date,
        );
        sleep(current_wait).await;
    }
    // Update the final activity in the database to know it is the final activity.
    if let Some(activity_id) = final_activity_id {
        let mut final_activity: models::run::ActiveModel =
            models::run::Entity::find_by_id(activity_id)
                .one(database)
                .await
                .map_err(|err| format!("Finding final activity: {err}"))?
                .expect("final activity should be present")
                .into();
        final_activity.is_final_activity = Set(true);
        final_activity
            .update(database)
            .await
            .map_err(|err| format!("Updating final activity: {err}"))?;
    }
    Ok(())
}

/// Return a query to find the runs for a given athlete.
fn find_runs(athlete_id: i64) -> Select<models::run::Entity> {
    models::run::Entity::find().filter(models::run::COLUMN.athlete_id.eq(athlete_id))
}

#[derive(Deserialize)]
pub struct RunQuery {
    pub after_id: Option<u64>,
}

// Get the latest runs for an athlete.
pub async fn get_runs(
    State(state): State<AppState>,
    athlete: AuthedAthlete,
    Query(params): Query<RunQuery>,
) -> Response {
    // TODO: fetch newer activities.
    let mut athlete_runs = find_runs(athlete.athlete_id);
    match params.after_id {
        Some(after_id) => {
            athlete_runs = athlete_runs.filter(models::run::COLUMN.strava_activity_id.gt(after_id))
        }
        None => {
            // Assume this is the first of multiple paginated requests from the frontend.
            let final_run = find_runs(athlete.athlete_id)
                .order_by_asc(models::run::COLUMN.start_date)
                .one(&state.database)
                .await
                .unwrap();
            // Only need to fetch older runs if there isn't the "final" activity.
            if final_run.is_none() || !final_run.as_ref().unwrap().is_final_activity {
                let before_epoch = final_run.map(|run| *run.start_date);
                tokio::spawn(async move {
                    let database = models::connect_database()
                        .await
                        .expect("need a database connection");
                    if let Err(err) =
                        fetch_older_runs(&database, athlete.athlete_id, before_epoch).await
                    {
                        tracing::error!("{err}");
                    };
                });
            }
        }
    }
    match athlete_runs
        .order_by_id_asc()
        .limit(10)
        .all(&state.database)
        .await
    {
        Ok(runs) => {
            let status_code = if runs.is_empty() {
                StatusCode::NO_CONTENT
            } else if runs[runs.len() - 1].is_final_activity {
                StatusCode::OK
            } else {
                StatusCode::PARTIAL_CONTENT
            };
            // Is there a simpler way to do this?
            let runs_response: Vec<RunResponse> = runs
                .iter()
                .filter_map(|run| {
                    run.summary_map.clone().map(|map| RunResponse {
                        strava_activity_id: run.strava_activity_id,
                        name: run.name.clone(),
                        summary_map: map,
                    })
                })
                .collect();
            (status_code, Json(runs_response)).into_response()
        }
        Err(err) => {
            tracing::error!("Getting runs for athlete_id={}: {err}", athlete.athlete_id);
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to find runs").into_response()
        }
    }
}
