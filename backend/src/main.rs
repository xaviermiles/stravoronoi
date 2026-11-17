use axum::{
    Router,
    http::{HeaderValue, Method, header},
    routing::{get, post},
};
use clap::Parser;
use sea_orm::DatabaseConnection;
use sea_orm::DbErr;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::sync::Notify;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use url::Url;

mod models;
mod road_grid;
mod routes;
mod services;
mod session;
mod snapping;

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

type FillAthleteSet = Arc<Mutex<HashSet<i64>>>;

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
    backfilling_athletes: FillAthleteSet,
    /// Athlete IDs that currently have a background run forwardfill in progress.
    ///
    /// No data will be returned while a forwardfill is in progress, since the frontend
    /// only paginates backwards in time, so it needs to wait for the most recent run
    /// before it can start paginating without losing any data.
    forwardfilling_athletes: FillAthleteSet,
    /// Channel for notifying that there is new runs to be snapped.
    snap_notify: Arc<Notify>,
    /// Spatial index of road segments for snapping runs.
    /// Only populated after the road grid is seeded.
    snapping_index: Arc<tokio::sync::RwLock<Option<snapping::RoadSegmentIndex>>>,
    /// If true, the "runs" endpoint will return snapped runs instead of the original polylines.
    ///
    /// This is useful for debugging.
    show_snapped_runs: bool,
}

async fn init_app_state(clean_database: bool, show_snapped_runs: bool) -> Result<AppState, DbErr> {
    if clean_database {
        models::clean_database();
    }
    let state = AppState {
        database: models::connect_database().await?,
        is_grid_ready: Arc::new(AtomicBool::new(false)),
        backfilling_athletes: Arc::new(Mutex::new(HashSet::new())),
        forwardfilling_athletes: Arc::new(Mutex::new(HashSet::new())),
        snap_notify: Arc::new(Notify::new()),
        snapping_index: Arc::new(tokio::sync::RwLock::new(None)),
        show_snapped_runs,
    };
    road_grid::start_seed(&state);
    snapping::start(&state);
    Ok(state)
}

#[derive(Parser)]
struct Args {
    /// Clean the database before starting.
    #[arg(long)]
    clean: bool,
    /// Return snapped polylines from the "runs" endpoint.
    #[arg(long)]
    show_snapped_runs: bool,
}

#[tokio::main]
async fn main() -> Result<(), DbErr> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::filter::LevelFilter::INFO)
        .with(tracing_subscriber::fmt::layer())
        .init();

    let args = Args::parse();
    let state = init_app_state(args.clean, args.show_snapped_runs).await?;

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
        .route("/api/grid/cells", get(routes::grid::get_cells))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        // CORS layer goes last so it executes first for incoming requests and wraps everything else.
        .layer(cors);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
    Ok(())
}
