/// Endpoints for the road grid.
use crate::AppState;
use crate::models::{grid_node, grid_way};
use crate::road_grid;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
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

    // Join each way segment to its node so coordinates are resolved in the query.
    let ways = match grid_way::Entity::find()
        .order_by_id_asc()
        .find_also_related(grid_node::Entity)
        .all(&state.database)
        .await
    {
        Ok(ways) => ways,
        Err(err) => {
            tracing::error!("Error finding grid ways: {err}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Chunk by way_id to create a LineString feature for each way.
    let features = ways
        .chunk_by(|a, b| a.0.way_id == b.0.way_id)
        .map(|chunk| {
            let coords: Vec<Vec<f64>> = chunk
                .iter()
                .filter_map(|(_, node)| node.as_ref())
                .map(|node| {
                    vec![
                        (node.longitude as f64) / road_grid::COORD_SCALE,
                        (node.latitude as f64) / road_grid::COORD_SCALE,
                    ]
                })
                .collect();
            let way = &chunk[0].0;
            let mut properties = geojson::JsonObject::new();
            if let Some(name) = &way.name {
                properties.insert("name".to_string(), serde_json::Value::String(name.clone()));
            }
            Feature {
                id: Some(Id::Number(way.way_id.into())),
                geometry: Some(Geometry::new(Value::LineString(coords))),
                properties: Some(properties),
                ..Default::default()
            }
        })
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
