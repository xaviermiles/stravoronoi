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

use crate::services::strava::{FetchEpoch, FetchError, SummaryActivity};
use crate::session::AuthedAthlete;
use crate::{AppState, FillAthleteSet, models, services};

// This will retry 5 times. This backoff usually won't work since the rate limits are per 15
// minutes and per 1 day, but it doesn't hurt since we will wait between requests anyway.
const START_WAIT: Duration = Duration::from_millis(100);
const MAX_WAIT: Duration = Duration::from_secs(2);

/// Removes an athlete from the in-flight fill athlete set when dropped.
///
/// Holding this in the spawned *fill task guarantees the athlete is cleared
/// once the task finishes, whether it returns normally, errors, or panics.
struct FillAthleteGuard {
    athletes: FillAthleteSet,
    athlete_id: i64,
}

impl Drop for FillAthleteGuard {
    fn drop(&mut self) {
        if let Ok(mut athletes) = self.athletes.lock() {
            athletes.remove(&self.athlete_id);
        }
    }
}

/// Return a currently-valid Strava access token for `athlete_id`.
///
/// Strava access tokens expire after ~6 hours. If the stored one has expired (or
/// is about to) refresh it using the stored refresh token.
async fn valid_access_token(
    database: &DatabaseConnection,
    athlete_id: i64,
) -> Result<String, String> {
    let athlete = match models::athlete::Entity::find_by_id(athlete_id)
        .one(database)
        .await
    {
        Ok(Some(athlete)) => athlete,
        Ok(None) => return Err("Cannot find athlete for given session ID.".to_string()),
        Err(err) => return Err(format!("Error while finding athlete: {err}")),
    };

    // Refresh slightly ahead of the expiry to avoid racing it.
    const EXPIRY_BUFFER_SECS: i64 = 60;
    if athlete.expires_at > Utc::now().timestamp() + EXPIRY_BUFFER_SECS {
        return Ok(athlete.access_token);
    }

    tracing::info!("Refreshing Strava access token for athlete ID: {athlete_id}");
    let tokens = services::strava::refresh_access_token(&athlete.refresh_token).await?;

    let access_token = tokens.access_token.clone();
    let mut athlete_active: models::athlete::ActiveModel = athlete.into();
    athlete_active.access_token = Set(tokens.access_token);
    athlete_active.refresh_token = Set(tokens.refresh_token);
    athlete_active.expires_at = Set(tokens.expires_at);
    athlete_active
        .update(database)
        .await
        .map_err(|err| format!("Failed to store refreshed tokens: {err}"))?;

    Ok(access_token)
}

/// Insert activities into the 'runs' table of the database.
async fn insert_activities(
    activities: Vec<SummaryActivity>,
    athlete_id: i64,
    database: &DatabaseConnection,
) -> Result<(), String> {
    let runs: Vec<_> = activities
        .iter()
        .filter(|activity| activity.is_run())
        .map(|activity| {
            models::run::ActiveModel {
                strava_activity_id: Set(activity.id),
                athlete_id: Set(athlete_id),
                name: Set(activity.name.clone()),
                distance: Set(activity.distance as i64),
                moving_time: Set(activity.moving_time),
                start_datetime: Set(activity.start_date.into()),
                summary_map: Set(activity.map.summary_polyline.clone()),
                is_first_run: Set(false), // this will updated afterwards.
            }
        })
        .collect();
    match models::run::Entity::insert_many(runs).exec(database).await {
        Ok(_) => Ok(()),
        Err(err) => Err(format!("Error while inserting runs: {err}")),
    }
}

#[derive(Clone, Copy, Debug)]
enum FetchCursor {
    Older(Option<DateTime<Utc>>),
    Newer(DateTime<Utc>),
}

impl FetchCursor {
    fn fetch_epoch(self) -> FetchEpoch {
        match self {
            Self::Older(Some(before)) => FetchEpoch::Before(before),
            Self::Older(None) => FetchEpoch::All,
            Self::Newer(after) => FetchEpoch::After(after),
        }
    }

    fn next(self, activities: &[SummaryActivity]) -> Option<Self> {
        match self {
            Self::Older(_) => activities
                .last()
                .map(|activity| Self::Older(Some(activity.start_date))),
            Self::Newer(_) => activities
                .first()
                .map(|activity| Self::Newer(activity.start_date)),
        }
    }
}

/// Fetch and insert activity pages, advancing the cursor in the requested direction.
async fn fetch_activity_pages(
    database: &DatabaseConnection,
    athlete_id: i64,
    mut cursor: FetchCursor,
) -> Result<(), String> {
    let access_token = valid_access_token(database, athlete_id).await?;
    tracing::info!("Start fetching runs for athlete_id={athlete_id:?} cursor={cursor:?}");

    let mut current_wait = START_WAIT;
    loop {
        let activities =
            match services::strava::fetch_activities(&access_token, &cursor.fetch_epoch()).await {
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
        tracing::info!(
            "Fetched {} activities for athlete_id={athlete_id} and cursor={cursor:?}",
            activities.len(),
        );
        // Reset wait since we weren't told to backoff.
        current_wait = START_WAIT;
        let Some(next_cursor) = cursor.next(&activities) else {
            break;
        };
        insert_activities(activities, athlete_id, database).await?;
        cursor = next_cursor;
        sleep(current_wait).await;
    }
    tracing::info!("Finish fetching runs for athlete_id={athlete_id:?} cursor={cursor:?}");
    Ok(())
}

/// Fetch older runs before a given time.
///
/// If no time is given then all runs will be fetched.
async fn fetch_older_runs(
    database: &DatabaseConnection,
    athlete_id: i64,
    before_epoch: Option<DateTime<Utc>>,
) -> Result<(), String> {
    fetch_activity_pages(database, athlete_id, FetchCursor::Older(before_epoch)).await?;

    // If the fetching above finished without returning an Err, then we know all the previous runs have been downloaded.
    // Update the final run in the database to know it is the final one.
    if let Some(final_run) = find_oldest_downloaded_run(database, athlete_id).await {
        let mut final_run_active: models::run::ActiveModel = final_run.into();
        final_run_active.is_first_run = Set(true);
        final_run_active
            .update(database)
            .await
            .map_err(|err| format!("Updating final activity: {err}"))?;
    } else {
        tracing::warn!("No final downloaded run found - does this user have no runs?")
    }
    Ok(())
}

/// Fetch newer runs before the most recent run in the database.
///
/// This panics if there is no downloaded runs.
async fn fetch_newer_runs(
    database: &DatabaseConnection,
    athlete_id: i64,
    after_epoch: DateTime<Utc>,
) -> Result<(), String> {
    fetch_activity_pages(database, athlete_id, FetchCursor::Newer(after_epoch)).await?;
    Ok(())
}

/// Return a query to find the runs for a given athlete.
fn find_runs(athlete_id: i64) -> Select<models::run::Entity> {
    models::run::Entity::find().filter(models::run::COLUMN.athlete_id.eq(athlete_id))
}

/// Return a query to find the final downloaded run for a given athlete, as per the start date.
///
/// This run does not necessarily have `is_first_run=true` (if not all runs have been downloaded).
async fn find_oldest_downloaded_run(
    database: &DatabaseConnection,
    athlete_id: i64,
) -> Option<models::run::Model> {
    find_runs(athlete_id)
        .order_by_asc(models::run::COLUMN.start_datetime)
        .one(database)
        .await
        .unwrap()
}

/// Maybe start backfilling and forwardfilling runs.
///
/// Return True if the backend should not return any data yet.
async fn maybe_start_filling_runs(
    state: &AppState,
    athlete_id: i64,
    oldest_downloaded_run: Option<models::run::Model>,
    is_backfill_complete: bool,
) -> bool {
    let is_forward_applicable = oldest_downloaded_run.is_some();
    // Only need to fetch older runs if the backfill hasn't already completed and there
    // isn't an existing backfill in-flight.
    let should_backfill = !is_backfill_complete
        && state
            .backfilling_athletes
            .lock()
            .expect("backfill set mutex poisoned")
            .insert(athlete_id);
    if should_backfill {
        let before_epoch = oldest_downloaded_run.map(|run| *run.start_datetime);
        let guard = FillAthleteGuard {
            athletes: state.backfilling_athletes.clone(),
            athlete_id,
        };
        tokio::spawn(async move {
            let _guard = guard;
            let database = models::connect_database()
                .await
                .expect("need a database connection");
            if let Err(err) = fetch_older_runs(&database, athlete_id, before_epoch).await {
                tracing::error!("{err}");
            };
        });
    }

    if !is_forward_applicable {
        // There is no runs so the backfill (from the present time) will find all runs and
        // forwardfilling is unnecessary.
        return false;
    }

    let database = models::connect_database()
        .await
        .expect("need a database connection");
    let access_token = valid_access_token(&database, athlete_id).await.unwrap();
    // Similarly, only need to forwardfill if there isn't an existing one in-flight.
    let after_epoch = *find_runs(athlete_id)
        .order_by_desc(models::run::COLUMN.start_datetime)
        .one(&database)
        .await
        .unwrap()
        .expect("there is an oldest_downloaded_run")
        .start_datetime;
    tracing::info!("peeking after_epoch {:?}", after_epoch);
    // This needs to check for any newer activities otherwise this always forwardfill and never complete.
    if !services::strava::any_newer_activities(&access_token, after_epoch)
        .await
        .unwrap()
    {
        tracing::info!("Skipping forwardfill as there is no newer activities.");
        return false;
    }

    if !state
        .forwardfilling_athletes
        .lock()
        .expect("forwardfill set mutex poisoned")
        .insert(athlete_id)
    {
        tracing::info!("Need forwardfill but skipping as one already in-flight.");
        return false;
    }

    tracing::info!("Starting forwardfill.");
    let guard = FillAthleteGuard {
        athletes: state.forwardfilling_athletes.clone(),
        athlete_id,
    };
    tokio::spawn(async move {
        let _guard = guard;
        if let Err(err) = fetch_newer_runs(&database, athlete_id, after_epoch).await {
            tracing::error!("{err}");
        }
    });
    // See AppState.forwardfilling_athletes for why no data should be returned if forwardfilling.
    true
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
    if state
        .forwardfilling_athletes
        .lock()
        .expect("forwardfill set mutex poisoned")
        .contains(&athlete.athlete_id)
    {
        tracing::info!("Returning NO_CONTENT as forwardfilling in progress.");
        return StatusCode::NO_CONTENT.into_response();
    }

    // Runs that couldn't be snapped have a NULL polyline and are excluded.
    let mut athlete_runs = find_runs(athlete.athlete_id)
        .find_also_related(models::snapped_run::Entity)
        .filter(models::snapped_run::COLUMN.polyline.is_not_null())
        .order_by_desc(models::run::COLUMN.start_datetime);

    // The oldest downloaded run tells us whether the full history has been
    // backfilled: its `is_first_run` flag is only set once Strava has returned
    // an empty page, meaning there is nothing older left to fetch. We read it up
    // front so the status code below can be decided from athlete-wide state
    // rather than from whichever page happens to be returned (which races with
    // the background backfill flagging the oldest run).
    let oldest_downloaded_run =
        find_oldest_downloaded_run(&state.database, athlete.athlete_id).await;
    let is_backfill_complete = oldest_downloaded_run
        .as_ref()
        .is_some_and(|run| run.is_first_run);

    match params.before {
        Some(before_epoch) => {
            athlete_runs = athlete_runs.filter(models::run::COLUMN.start_datetime.lt(before_epoch))
        }
        None => {
            if maybe_start_filling_runs(
                &state,
                athlete.athlete_id,
                oldest_downloaded_run,
                is_backfill_complete,
            )
            .await
            {
                return StatusCode::NO_CONTENT.into_response();
            }
        }
    }
    match athlete_runs.limit(10).all(&state.database).await {
        Ok(runs) => {
            let status_code = if runs.is_empty() {
                if is_backfill_complete {
                    // There is nothing more to poll for.
                    StatusCode::OK
                } else {
                    StatusCode::NO_CONTENT
                }
            } else if runs[runs.len() - 1].0.is_first_run {
                StatusCode::OK
            } else {
                StatusCode::PARTIAL_CONTENT
            };
            let runs_response: Vec<_> = runs
                .into_iter()
                .filter_map(|(run, snapped_run)| {
                    snapped_run
                        .and_then(|snapped| snapped.polyline)
                        .map(|polyline| RunResponse {
                            strava_activity_id: run.strava_activity_id,
                            name: run.name,
                            distance: run.distance,
                            moving_time: run.moving_time,
                            start_datetime: *run.start_datetime,
                            summary_map: polyline,
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
