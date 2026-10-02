use crate::models;
use sea_orm::QueryOrder;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::time;

use crate::models::{run, snapped_run};
use sea_orm::ActiveValue::Set;

use crate::services::mapbox::{MatchError, map_match};

/// Strava encoded polylines use a precision of 5 decimal places.
const POLYLINE_PRECISION: u32 = 5;

/// Get next run which has no corresponding `snapped_run`.
async fn get_next_unsnapped_run(database: &DatabaseConnection) -> Option<run::Model> {
    run::Entity::find()
        .left_join(snapped_run::Entity)
        .filter(snapped_run::Column::StravaActivityId.is_null())
        .order_by_desc(run::Column::StartDatetime)
        .one(database)
        .await
        .unwrap_or_else(|err| {
            tracing::error!("Failed to query unsnapped runs: {err}");
            None
        })
}

async fn insert_snapped_run(
    database: &DatabaseConnection,
    strava_activity_id: i64,
    polyline: Option<String>,
) -> Result<(), String> {
    match snapped_run::Entity::insert(snapped_run::ActiveModel {
        strava_activity_id: Set(strava_activity_id),
        polyline: Set(polyline),
        processed_time: Set(chrono::Utc::now().into()),
    })
    .exec(database)
    .await
    {
        Ok(_) => Ok(()),
        Err(err) => Err(format!("Failed to insert run: {err}")),
    }
}

/// Snap a Strava encoded polyline using map matching.
///
/// Returns an error if the run could not be processed, but should be attempted again.
async fn process_run(database: &DatabaseConnection, run: models::run::Model) -> Result<(), String> {
    let Some(summary_map) = run.summary_map else {
        return insert_snapped_run(database, run.strava_activity_id, None).await;
    };
    let raw_coords = polyline::decode_polyline(&summary_map, POLYLINE_PRECISION)
        .map_err(|err| format!("Failed to decoded polyline {err}"))?
        .into_inner();

    let snapped_coords = match map_match(&raw_coords).await {
        Ok(polyline) => polyline,
        Err(MatchError::Permanent(err)) => {
            tracing::error!("Permanent error map matching: {err}");
            return insert_snapped_run(database, run.strava_activity_id, None).await;
        }
        Err(MatchError::Transient(err)) => return Err(format!("Transient: {err}")),
        Err(MatchError::Backoff(backoff_time)) => {
            time::sleep(backoff_time).await;
            return Err(format!("Backed off for {backoff_time:?}"));
        }
    };

    match polyline::encode_coordinates(snapped_coords, POLYLINE_PRECISION) {
        Ok(encoded_snapped_coords) => {
            insert_snapped_run(
                database,
                run.strava_activity_id,
                Some(encoded_snapped_coords),
            )
            .await
        }
        Err(err) => Err(format!("Failed to encode polyline: {err}")),
    }
}

/// Spawn a worker to snap runs.
pub fn start(database: DatabaseConnection, notify: Arc<Notify>) {
    tokio::spawn(async move {
        loop {
            while let Some(run) = get_next_unsnapped_run(&database).await {
                if let Err(err) = process_run(&database, run).await {
                    tracing::error!("Error processing run: {err}");
                };
            }
            notify.notified().await;
        }
    });
}
