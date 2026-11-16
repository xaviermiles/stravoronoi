use crate::AppState;
use crate::models;
use geo::MultiLineString;
use geojson::Feature;
use geojson::Geometry;
use geojson::Value;
use sea_orm::QueryOrder;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use std::sync::Arc;

use crate::models::{run, snapped_run};
use sea_orm::ActiveValue::Set;

use super::segment_index::RoadSegmentIndex;
use super::snapper::Snapper;

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
    polyline: Option<MultiLineString>,
) -> Result<(), String> {
    let geojson = polyline.map(|polyline| {
        let feature = Feature {
            geometry: Some(Geometry::new(Value::from(&polyline))),
            ..Default::default()
        };
        feature.to_string()
    });
    match snapped_run::Entity::insert(snapped_run::ActiveModel {
        strava_activity_id: Set(strava_activity_id),
        polyline: Set(geojson),
        processed_time: Set(chrono::Utc::now().into()),
    })
    .exec(database)
    .await
    {
        Ok(_) => Ok(()),
        Err(err) => Err(format!("Failed to insert run: {err}")),
    }
}

/// Snap a Strava encoded polyline to the road grid.
///
/// Returns an error if the run could not be processed, but should be attempted again.
async fn process_run(
    database: &DatabaseConnection,
    run: models::run::Model,
    index: &Arc<tokio::sync::RwLock<Option<RoadSegmentIndex>>>,
) -> Result<(), String> {
    let Some(summary_map) = run.summary_map else {
        return insert_snapped_run(database, run.strava_activity_id, None).await;
    };
    let raw_coords = polyline::decode_polyline(&summary_map, POLYLINE_PRECISION)
        .map_err(|err| format!("Failed to decode polyline: {err}"))?
        .into_inner();

    let index_guard = index.read().await;
    let Some(idx) = index_guard.as_ref() else {
        // This will need to be processed again, but is a normal state so don't need log an error.
        return Ok(());
    };

    match Snapper::new(idx).snap_polyline(&raw_coords) {
        Ok(polyline) => insert_snapped_run(database, run.strava_activity_id, Some(polyline)).await,
        Err(err) => {
            tracing::error!("Error snapping run {}: {}", run.strava_activity_id, err);
            insert_snapped_run(database, run.strava_activity_id, None).await
        }
    }
}

/// Spawn a worker to snap runs.
pub fn start(state: &AppState) {
    let database = state.database.clone();
    let snapping_index = state.snapping_index.clone();
    let snap_notify = state.snap_notify.clone();
    tokio::spawn(async move {
        loop {
            while let Some(run) = get_next_unsnapped_run(&database).await {
                if let Err(err) = process_run(&database, run, &snapping_index).await {
                    tracing::error!("Error processing run: {err}");
                };
            }
            snap_notify.notified().await;
        }
    });
}
