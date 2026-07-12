//! Minimal Strava API client for the browser.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use comms::runs::RunResponse;
use geo_types::Coord;
use sea_orm::ActiveModelTrait;
use sea_orm::ActiveValue::Set;
use sea_orm::DatabaseConnection;
use sea_orm::EntityTrait;
use sea_orm::QueryFilter;
use sea_orm::QuerySelect;
use serde::Deserialize;

use crate::session::AuthedAthlete;
use crate::{AppState, models, services};

/// Strava encoded polylines use a precision of 5 decimal places.
const POLYLINE_PRECISION: u32 = 5;

/// Decode a Strava encoded polyline into GeoJSON coordinates.
fn decode_line(encoded: &str) -> Vec<Coord<f64>> {
    match polyline::decode_polyline(encoded, POLYLINE_PRECISION) {
        Ok(line) => line.into_inner(),
        Err(_) => Vec::new(),
    }
}

/// Start fetching runs.
async fn start_fetching_runs(database: &DatabaseConnection, athlete_id: i64) -> Result<(), String> {
    let access_token = match models::athlete::Entity::find_by_id(athlete_id)
        .one(database)
        .await
    {
        Ok(Some(athlete)) => athlete.access_token,
        Ok(None) => return Err("Cannot find athlete for given session ID.".to_string()),
        Err(err) => return Err(format!("Error while finding athlete: {err}")),
    };

    let mut final_activity_id: Option<i32> = None;
    let mut activities = services::strava::fetch_activities(&access_token, None).await?;
    while !activities.is_empty() {
        final_activity_id = Some(activities.last().expect("checked is_empty() above").id);
        for activity in activities.iter() {
            if !(activity.sport_type.contains("Run")) {
                continue;
            }
            let encoded_coords = match &activity.map.summary_polyline {
                Some(p) => p,
                None => continue,
            };

            let raw_coords = decode_line(encoded_coords);
            if raw_coords.len() < 2 {
                // Less than 2 points means it isn't a line.
                continue;
            }
            // TODO: Currently it is mapping the snapped lines. I want the snapped lines to be primarily used for the voronoi calculations.
            //       They could be mapped in addition to the raw coordinates but they should be different colour and possibly more transparent.
            let snapped_coords = services::mapbox::map_match(&raw_coords).await;
            if snapped_coords.len() < 2 {
                // Less than 2 points means it isn't a line.
                continue;
            }

            let encoded_polyline_map =
                polyline::encode_coordinates(snapped_coords, POLYLINE_PRECISION)
                    .map_err(|err| format!("Failed to encoded polyline: {err}"))?;
            let run = models::run::ActiveModel {
                strava_activity_id: Set(activity.id),
                name: Set(activity.name.clone()),
                summary_map: Set(Some(encoded_polyline_map)),
                is_final_activity: Set(false), // to be modified afterwards
            };
            models::run::Entity::insert(run)
                .exec(database)
                .await
                .map_err(|err| format!("Error while inserting run: {err}"))?;
        }
        let after_epoch = activities
            .last()
            .expect("checked is_empty() above")
            .start_date;
        activities = services::strava::fetch_activities(&access_token, Some(after_epoch)).await?;
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

#[derive(Deserialize)]
pub struct RunQuery {
    pub after_id: Option<u64>,
}

// Get the latest runs for an athelete.
pub async fn get_runs(
    State(state): State<AppState>,
    athlete: AuthedAthlete,
    Query(params): Query<RunQuery>,
) -> impl IntoResponse {
    let after_id = params.after_id.unwrap_or(0);
    match models::run::Entity::find()
        .filter(models::run::COLUMN.strava_activity_id.gte(after_id))
        .limit(10)
        .all(&state.database)
        .await
    {
        Ok(runs) => {
            let status_code = if runs.is_empty() {
                if params.after_id.is_none() {
                    // Assume it hasn't been fetched before.
                    tokio::spawn(async move {
                        if let Err(err) =
                            start_fetching_runs(&state.database, athlete.athlete_id).await
                        {
                            log::error!("{err}");
                        };
                    });
                }
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
            (
                status_code,
                Json(serde_json::json!({"runs": runs_response})),
            )
        }
        Err(err) => {
            log::warn!(
                "Getting runs with athelete_id={}: {err}",
                athlete.athlete_id
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "failed to find runs"})),
            )
        }
    }
}
