use axum::{
    Router,
    http::{HeaderValue, Method, header},
    routing::get,
};
use sea_orm::DatabaseConnection;
use std::time::Duration;
use tower_http::cors::CorsLayer;
use url::Url;

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

#[tokio::main]
async fn main() {
    let database = models::connect_database()
        .await
        .expect("need a database connection");
    let state = AppState { database };

    let frontend_base_url = Url::parse(FRONTEND_URL)
        .expect("Defined statically")
        .origin()
        .unicode_serialization();
    let cors = CorsLayer::new()
        .allow_origin(frontend_base_url.parse::<HeaderValue>().unwrap())
        .allow_methods([Method::GET])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .expose_headers([header::CONTENT_DISPOSITION])
        .allow_credentials(true)
        .max_age(Duration::from_secs(3600));
    let app = Router::new()
        .route("/auth/login", get(routes::strava::auth_login))
        .route("/auth/callback", get(routes::strava::auth_callback))
        .route("/auth/logout", get(routes::strava::auth_logout))
        .route("/api/runs", get(routes::runs::list_runs))
        .with_state(state)
        // CORS layer goes last so it executes first for incoming requests and wraps everything else.
        .layer(cors);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
