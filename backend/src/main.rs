use axum::{
    Router,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use sea_orm::ActiveModelTrait;
use sea_orm::{ActiveValue::Set, DatabaseConnection};
use serde::Deserialize;
use std::time::Duration;
use tower_cookies::{Cookie, CookieManagerLayer, Cookies};
use tower_http::cors::CorsLayer;

mod models;
mod session;
mod strava;

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

/// Start the OAuth flow: generate a `state` value and redirect the user to
/// Strava's authorize page (scope `activity:read`). The CSRF `state` is stored
/// in a cookie so we can verify it on the callback.
async fn auth_login(State(_state): State<AppState>) -> Response {
    let (url, csrf) = strava::authorize_url();
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, url.to_string())
        .header(
            header::SET_COOKIE,
            format!(
                "oauth_state={}; HttpOnly; SameSite=Lax; Path=/",
                csrf.secret()
            ),
        )
        .body(Body::empty())
        .expect("failed to build redirect response")
}

#[derive(Deserialize)]
struct AuthCallback {
    code: String,
    state: String,
}

/// Strava redirects here after the user approves. Verify `state`, exchange the
/// `code` for tokens, upsert them keyed by athlete id, set a session cookie,
/// then redirect back to the app.
async fn auth_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<AuthCallback>,
    cookies: Cookies,
) -> Response {
    // Verify the CSRF `state` against the value we stored in the login cookie.
    match cookie_value(&headers, "oauth_state") {
        Some(expected) if expected == params.state => {}
        _ => return (StatusCode::BAD_REQUEST, "invalid OAuth state").into_response(),
    }

    log::info!("{}", params.code);
    log::info!("{}", params.state);
    match strava::exchange_code(&params.code).await {
        Ok(tokens) => {
            // TODO: upsert `_tokens` keyed by athlete id, create a session, and
            // set a session cookie before redirecting.
            let user = models::athlete::ActiveModel {
                strava_athlete_id: Set(0), // TODO: populate this correctly
                access_token: Set(tokens.access_token.to_owned()),
                refresh_token: Set(tokens.refresh_token.to_owned()),
                expires_at: Set(tokens.expires_at.to_owned()),
            };
            let mut cookie = Cookie::new("session_token", "blah blah blah");
            cookie.set_path("/");
            cookie.set_http_only(true);
            cookie.set_same_site(tower_cookies::cookie::SameSite::Lax);
            cookie.set_secure(true); // Enable in production over HTTPS

            cookies.add(cookie);
            match user.insert(&state.database).await {
                Ok(_) => Redirect::to(FRONTEND_URL).into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            }
        }
        Err(e) => (StatusCode::BAD_GATEWAY, e).into_response(),
    }
}

/// Read a single cookie value from the request headers.
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|pair| {
        let (key, value) = pair.trim().split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

/// Clear the session cookie and optionally deauthorize the athlete on Strava.
async fn auth_logout(State(_state): State<AppState>) -> Response {
    todo!()
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
        .route("/auth/login", get(auth_login))
        .route("/auth/callback", get(auth_callback))
        .route("/auth/logout", get(auth_logout))
        // .route("/api/runs", get(list_runs))
        .with_state(state)
        .layer(cors)
        .layer(CookieManagerLayer::new())
        .layer(session::get_session_layer());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
