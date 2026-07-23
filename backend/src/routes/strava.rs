use crate::models;
use crate::services;
use crate::{AppState, BACKEND_BASE_URL, FRONTEND_URL};
use axum::{
    Json,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use sea_orm::ActiveValue::Set;
use sea_orm::{ActiveModelTrait, EntityTrait};
use serde::Deserialize;
use url::Url;

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
            let athlete_id = tokens.athlete.id;
            let existing_athlete = match models::athlete::Entity::find_by_id(athlete_id)
                .one(&state.database)
                .await
            {
                Ok(existing_athlete) => existing_athlete,
                Err(err) => {
                    tracing::error!("Failed to search for athlete: {err}");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "Find failed.").into_response();
                }
            };
            // Fetch the profile picture for existing users so that it gets updated if they've
            // changed it in Strava.
            let Ok(strava_athlete) = services::strava::get_athlete(&tokens.access_token).await
            else {
                return (StatusCode::INTERNAL_SERVER_ERROR, "Getting athlete failed.")
                    .into_response();
            };
            let upsert = match existing_athlete {
                Some(existing) => {
                    // Overwrite the stored tokens otherwise they'd be stuck on stale credentials
                    // as Strava rotates the refresh token on each refresh.
                    let mut user: models::athlete::ActiveModel = existing.into();
                    user.strava_username = Set(tokens.athlete.username);
                    user.profile_url = Set(strava_athlete.profile);
                    user.access_token = Set(tokens.access_token.to_owned());
                    user.refresh_token = Set(tokens.refresh_token.to_owned());
                    user.expires_at = Set(tokens.expires_at.to_owned());
                    user.update(&state.database).await
                }
                None => {
                    let user = models::athlete::ActiveModel {
                        strava_id: Set(athlete_id),
                        strava_username: Set(tokens.athlete.username),
                        profile_url: Set(strava_athlete.profile),
                        access_token: Set(tokens.access_token.to_owned()),
                        refresh_token: Set(tokens.refresh_token.to_owned()),
                        expires_at: Set(tokens.expires_at.to_owned()),
                    };
                    user.insert(&state.database).await
                }
            };
            if let Err(err) = upsert {
                tracing::error!("Failed to upsert athlete: {err}");
                return (StatusCode::INTERNAL_SERVER_ERROR, "Upsert failed.").into_response();
            }
            // Mint an opaque session and hand its id to the frontend, which
            // stores it and sends it back as a bearer token.
            match crate::session::create_session(&state.database, athlete_id).await {
                Ok(session_id) => {
                    let mut callback_url = Url::parse(FRONTEND_URL).expect("Defined statically");
                    callback_url
                        .query_pairs_mut()
                        .append_pair("session_id", &session_id);
                    Redirect::to(callback_url.as_str()).into_response()
                }
                Err(err) => {
                    tracing::error!("Failed to create session: {err}");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Creating session failed.",
                    )
                        .into_response();
                }
            }
        }
        // The code exchange failed. The most common cause is a single-use authorization code that
        // has already been consumed or expired (e.g. a refreshed callback page). Send the user
        // back through login to mint a fresh code rather than stranding them on a dead one.
        Err(err) => {
            tracing::error!("code exchange failed, restarting login: {err}");
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

/// Logout the session by deleting it from the database.
pub async fn auth_logout(
    State(state): State<AppState>,
    athlete: crate::session::AuthedAthlete,
) -> Response {
    match models::session::Entity::delete_by_id(athlete.session_id)
        .exec(&state.database)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            tracing::error!("Failed to delete session ID: {err}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to delete session ID.",
            )
                .into_response()
        }
    }
}

/// Return the authenticated athlete's profile information.
pub async fn get_me(
    State(state): State<AppState>,
    athlete: crate::session::AuthedAthlete,
) -> Response {
    match models::athlete::Entity::find_by_id(athlete.athlete_id)
        .one(&state.database)
        .await
    {
        Ok(Some(athlete)) => Json(comms::athlete::AthleteResponse {
            username: athlete.strava_username,
            profile_url: athlete.profile_url,
        })
        .into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Athlete not found.").into_response(),
        Err(err) => {
            tracing::error!("Failed to load athlete: {err}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to load athlete.").into_response()
        }
    }
}
