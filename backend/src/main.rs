use axum::{
    Router,
    extract::State,
    http::{HeaderValue, Method, header},
    response::Response,
    routing::get,
};
use sea_orm::DatabaseConnection;
use std::time::Duration;
use tower_cookies::CookieManagerLayer;
use tower_http::cors::CorsLayer;

mod models;
mod routes;
mod services;
mod session;

const FRONTEND_URL: &str = if cfg!(debug_assertions) {
    "http://localhost:8080"
} else {
    "https://xaviermiles.github.io/stravoronoi/"
};
pub const BACKEND_BASE_URL: &str = if cfg!(debug_assertions) {
    "http://localhost:3000"
} else {
    "https://stravoronoi-production.up.railway.app"
};

/// Shared state handed to every request handler.
#[derive(Clone)]
struct AppState {
    database: DatabaseConnection,
    // TODO: reqwest::Client, Strava OAuth config (client id / secret / redirect
    // uri), and a session signing key.
}

/// Return the signed-in athlete's runs as GeoJSON. Resolve the caller from the
/// session cookie, refresh their access token if expired, fetch their
/// activities, then decode and return them.
#[allow(dead_code)]
async fn list_runs(State(_state): State<AppState>) -> Response {
    todo!()
}

#[tokio::main]
async fn main() {
    let database = models::connect_database()
        .await
        .expect("need a database connection");
    let state = AppState { database };

    let cors = CorsLayer::new()
        .allow_origin(FRONTEND_URL.parse::<HeaderValue>().unwrap())
        .allow_methods([Method::GET])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .expose_headers([header::CONTENT_DISPOSITION])
        .allow_credentials(true)
        .max_age(Duration::from_secs(3600));
    let app = Router::new()
        .route("/auth/login", get(routes::strava::auth_login))
        .route("/auth/callback", get(routes::strava::auth_callback))
        .route("/auth/logout", get(routes::strava::auth_logout))
        // .route("/api/runs", get(list_runs))
        .with_state(state)
        .layer(cors)
        .layer(CookieManagerLayer::new())
        .layer(session::get_session_layer());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
