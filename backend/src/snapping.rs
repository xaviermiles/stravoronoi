use crate::models;
use sea_orm::DatabaseConnection;
use std::sync::Arc;
use tokio::sync::Notify;

use crate::services::mapbox::{MatchError, map_match};

/// Strava encoded polylines use a precision of 5 decimal places.
const POLYLINE_PRECISION: u32 = 5;

#[allow(unused_variables)]
async fn find_next_run(database: &DatabaseConnection) -> Option<models::run::Model> {
    todo!("fetch run from database which hasn't been snapped")
}

/// Snap a Strava encoded polyline using map matching.
#[allow(dead_code)]
async fn snap_line(encoded_coords: &str) -> Result<String, String> {
    let raw_coords = polyline::decode_polyline(encoded_coords, POLYLINE_PRECISION)
        .map_err(|err| format!("Failed to decoded polyline {err}"))?
        .into_inner();

    let snapped_coords = match map_match(&raw_coords).await {
        Ok(polyline) => polyline,
        Err(MatchError::Permanent(err)) => return Err(format!("Permanent: {err}")),
        Err(MatchError::Transient(err)) => return Err(format!("Transient: {err}")),
    };

    polyline::encode_coordinates(snapped_coords, POLYLINE_PRECISION)
        .map_err(|err| format!("Failed to encode polyline: {err}"))
}

#[allow(unused_variables)]
async fn process_run(database: &DatabaseConnection, run: models::run::Model) {}

/// Spawn a worker to snap runs.
pub fn start(database: DatabaseConnection, notify: Arc<Notify>) {
    tokio::spawn(async move {
        loop {
            while let Some(run) = find_next_run(&database).await {
                process_run(&database, run).await;
            }
            notify.notified().await;
        }
    });
}
