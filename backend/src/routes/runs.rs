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
    loop {
        let activities = match services::strava::fetch_activities(&access_token, before_epoch).await
        {
            Ok(activities) => activities,
            Err(FetchError::Backoff) => {
                current_wait *= 2;
                if current_wait > MAX_WAIT {
                    return Err("time out during backoff".to_string());
                }
                sleep(current_wait).await;
                continue;
            }
            Err(FetchError::Other(message)) => return Err(message),
        };
        // Reset wait since we weren't told to backoff.
        current_wait = START_WAIT;
        before_epoch = match activities.last() {
            Some(final_activity) => Some(final_activity.start_date),
            // No activities.
            None => break,
        };
        let runs: Vec<_> = activities
            .iter()
            .filter(|activity| activity.is_run())
            .map(|activity| {
                // TODO: can clones be avoided?
                models::run::ActiveModel {
                    strava_activity_id: Set(activity.id),
                    athlete_id: Set(athlete_id),
                    name: Set(activity.name.clone()),
                    start_date: Set(activity.start_date.into()),
                    summary_map: Set(activity.map.summary_polyline.clone()),
                    is_first_run: Set(false), // this will updated afterwards.
                }
            })
            .collect();
        // In practice it seems like the "before_epoch" is inclusive, so there will be some conflicts while paging.
        models::run::Entity::insert_many(runs)
            .on_conflict_do_nothing()
            .exec(database)
            .await
            .map_err(|err| format!("Error while inserting runs: {err}"))?;
        sleep(current_wait).await;
    }
    // If the loop above finished without returning an Err, then we know all the previous runs have been downloaded.
    // Update the final run in the database to know it is the final one.
    if let Some(final_run) = find_final_downloaded_run(database, athlete_id).await {
        let mut final_run_active: models::run::ActiveModel = final_run.into();
        final_run_active.is_first_run = Set(true);
        final_run_active
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

/// Return a query to find the final downloaded run for a given athlete, as per the start date.
///
/// This run does not necessarily have `is_first_run=true` (if not all runs have been downloaded).
async fn find_final_downloaded_run(
    database: &DatabaseConnection,
    athlete_id: i64,
) -> Option<models::run::Model> {
    find_runs(athlete_id)
        .order_by_asc(models::run::COLUMN.start_date)
        .one(database)
        .await
        .unwrap()
}

#[derive(Deserialize)]
pub struct RunQuery {
    pub before: Option<i64>,
}

// Get the latest runs for an athlete.
pub async fn get_runs(
    State(state): State<AppState>,
    athlete: AuthedAthlete,
    Query(params): Query<RunQuery>,
) -> Response {
    // TODO: fetch newer activities.
    let mut athlete_runs = find_runs(athlete.athlete_id);
    match params.before {
        Some(before_epoch) => {
            athlete_runs = athlete_runs.filter(models::run::COLUMN.start_date.lt(before_epoch))
        }
        None => {
            // Assume this is the first of multiple paginated requests from the frontend.
            let final_run = find_final_downloaded_run(&state.database, athlete.athlete_id).await;
            // Only need to fetch older runs if there isn't the "final" activity.
            let has_first_run = match &final_run {
                Some(run) => run.is_first_run,
                None => false
            };
            if !has_first_run {
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
            } else if runs[runs.len() - 1].is_first_run {
                StatusCode::OK
            } else {
                StatusCode::PARTIAL_CONTENT
            };
            // Is there a simpler way to do this?
            let runs_response: Vec<RunResponse> = runs
                .iter()
                .filter_map(|run| {
                    run.summary_map.clone().map(|summary_map| RunResponse {
                        strava_activity_id: run.strava_activity_id,
                        name: run.name.clone(),
                        start_date: *run.start_date,
                        summary_map,
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
