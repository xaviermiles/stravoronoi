use axum::{
    Router,
    http::{HeaderValue, Method, header},
    routing::{get, post},
};
use sea_orm::DatabaseConnection;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use url::Url;

mod models;
mod road_grid;
mod routes;
mod services;
mod session;

const FRONTEND_URL: &str = if cfg!(debug_assertions) {
    "http://localhost:8080"
} else {
    "https://xaviermiles.github.io/stravoronoi"
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
    /// Flag to indicate whether the road grid is ready to be read.
    ///
    /// If false, it is still seeding.
    is_grid_ready: Arc<AtomicBool>,
    /// Athlete IDs that currently have a background run backfill in progress.
    ///
    /// Used to ensure only one backfill runs per athlete at a time, so repeated
    /// initial requests from the frontend don't each spawn a duplicate fetch.
    backfilling_athletes: Arc<Mutex<HashSet<i64>>>,
}

async fn init_app_state() -> AppState {
    let database = models::connect_database()
        .await
        .expect("need a database connection");
    let is_grid_ready = Arc::new(AtomicBool::new(false));
    let state = AppState {
        database,
        is_grid_ready: is_grid_ready.clone(),
        backfilling_athletes: Arc::new(Mutex::new(HashSet::new())),
    };

    // Seed the road grid without blocking.
    let seed_database = state.database.clone();
    tokio::spawn(async move {
        match road_grid::seed(&seed_database).await {
            Ok(()) => is_grid_ready.store(true, Ordering::Release),
            Err(err) => tracing::warn!("Failed to seed road grid: {err}"),
        }
    });
    state
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::filter::LevelFilter::INFO)
        .with(tracing_subscriber::fmt::layer())
        .init();

    let state = init_app_state().await;

    let frontend_base_url = Url::parse(FRONTEND_URL)
        .expect("Defined statically")
        .origin()
        .unicode_serialization();
    let cors = CorsLayer::new()
        .allow_origin(frontend_base_url.parse::<HeaderValue>().unwrap())
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .expose_headers([header::CONTENT_DISPOSITION])
        .allow_credentials(true)
        .max_age(Duration::from_secs(3600));
    let app = Router::new()
        .route("/auth/login", get(routes::strava::auth_login))
        .route("/auth/callback", get(routes::strava::auth_callback))
        .route("/auth/logout", post(routes::strava::auth_logout))
        .route("/api/me", get(routes::strava::get_me))
        .route("/api/runs", get(routes::runs::get_runs))
        .route("/api/grid/ways", get(routes::grid::get_ways))
        .route(
            "/api/grid/intersections",
            get(routes::grid::get_intersections),
        )        
        .route(
            "/api/grid/cells",
            get(routes::grid::get_cells),
        )
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        // CORS layer goes last so it executes first for incoming requests and wraps everything else.
        .layer(cors);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
