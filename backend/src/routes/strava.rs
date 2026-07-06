use crate::models;
use crate::services;
use crate::{AppState, BACKEND_BASE_URL, FRONTEND_URL};
use axum::{
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use sea_orm::ActiveValue::Set;
use serde::Deserialize;
use sea_orm::EntityTrait;
use tower_sessions::Session;

/// Start the OAuth flow: generate a `state` value and redirect the user to
/// Strava's authorize page (scope `activity:read`). The CSRF `state` is stored
/// in a cookie so we can verify it on the callback.
pub async fn auth_login(State(_state): State<AppState>) -> Response {
    let (url, csrf) = services::strava::authorize_url();
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
pub struct AuthCallback {
    code: String,
    state: String,
}

/// Strava redirects here after the user approves. Verify `state`, exchange the
/// `code` for tokens, upsert them keyed by athlete id, set a session cookie,
/// then redirect back to the app.
pub async fn auth_callback(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Query(params): Query<AuthCallback>,
) -> Response {
    // Verify the CSRF `state` against the value we stored in the login cookie.
    match cookie_value(&headers, "oauth_state") {
        Some(expected) if expected == params.state => {}
        _ => return (StatusCode::BAD_REQUEST, "invalid OAuth state").into_response(),
    }

    match services::strava::exchange_code(&params.code).await {
        Ok(tokens) => {
            let user = models::athlete::ActiveModel {
                strava_id: Set(tokens.athlete.id),
                strava_username: Set(tokens.athlete.username),
                access_token: Set(tokens.access_token.to_owned()),
                refresh_token: Set(tokens.refresh_token.to_owned()),
                expires_at: Set(tokens.expires_at.to_owned()),
            };
            if let Err(err) = session.insert("athlete_id", tokens.athlete.id).await {
                return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
            };
            match models::athlete::Entity::insert(user).on_conflict_do_nothing().exec(&state.database).await {
                Ok(_) => Redirect::to(FRONTEND_URL).into_response(),
                Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
            }
        }
        // The code exchange failed. The most common cause is a single-use
        // authorization code that has already been consumed or expired (e.g. a
        // refreshed callback page). Send the user back through login to mint a
        // fresh code rather than stranding them on a dead one.
        Err(e) => {
            eprintln!("code exchange failed, restarting login: {e}");
            Redirect::to(&format!("{BACKEND_BASE_URL}/auth/login")).into_response()
        }
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
pub async fn auth_logout(State(_state): State<AppState>) -> Response {
    todo!()
}
