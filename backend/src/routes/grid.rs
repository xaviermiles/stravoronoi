/// Endpoints for the road grid.
use crate::AppState;
use crate::models::{grid_cell, grid_way, grid_node};
use crate::road_grid;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use geojson::Geometry;
use geojson::Value;
use geojson::feature::Id;
use geojson::{Feature, FeatureCollection};
use sea_orm::EntityTrait;
use sea_orm::QueryFilter;
use std::sync::atomic::Ordering;

fn as_response(features: Vec<Feature>) -> Response {
    let intersections = FeatureCollection {
        features,
        ..Default::default()
    };
    Json(intersections).into_response()
}

pub async fn get_ways(State(state): State<AppState>) -> Response {
    if !state.is_grid_ready.load(Ordering::Acquire) {
        return StatusCode::NO_CONTENT.into_response();
    }

    let merged_ways = match grid_way::Entity::find()
        .order_by_id_asc()
        .all(&state.database)
        .await
    {
        Ok(ways) => ways,
        Err(err) => {
            tracing::error!("Error finding grid ways: {err}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let features: Vec<_> = merged_ways
        .iter()
        .map(|way_model| way_model.geojson.parse().expect("load() creates features"))
        .collect();

    as_response(features)
}

pub async fn get_intersections(State(state): State<AppState>) -> Response {
    if !state.is_grid_ready.load(Ordering::Acquire) {
        return StatusCode::NO_CONTENT.into_response();
    }
    let intersections = match grid_node::Entity::find()
        .filter(grid_node::COLUMN.is_intersection.eq(true))
        .order_by_id_asc()
        .all(&state.database)
        .await
    {
        Ok(intersections) => intersections,
        Err(err) => {
            tracing::error!("Error finding intersection nodes: {err}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let features = intersections
        .iter()
        .map(|grid_node| {
            let point = Value::Point(vec![
                (grid_node.longitude as f64) / road_grid::COORD_SCALE,
                (grid_node.latitude as f64) / road_grid::COORD_SCALE,
            ]);
            Feature {
                id: Some(Id::Number(grid_node.id.into())),
                geometry: Some(Geometry::new(point)),
                ..Default::default()
            }
        })
        .collect();
    as_response(features)
}

pub async fn get_cells(State(state): State<AppState>) -> Response {
    if !state.is_grid_ready.load(Ordering::Acquire) {
        return StatusCode::NO_CONTENT.into_response();
    }
    let cells = match grid_cell::Entity::find().all(&state.database).await {
        Ok(cells) => cells,
        Err(err) => {
            tracing::error!("Error finding grid cells: {err}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Each `geojson` column already holds a serialized Feature, so splice the
    // stored strings straight into a FeatureCollection body instead of parsing
    // them into geojson types and re-serializing via `Json`.
    let mut body = String::from(r#"{"type":"FeatureCollection","features":["#);
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            body.push(',');
        }
        body.push_str(&cell.geojson);
    }
    body.push_str("]}");

    ([(header::CONTENT_TYPE, "application/geo+json")], body).into_response()
}
