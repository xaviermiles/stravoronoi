//! Server-side session management and request authentication.
//!
//! Sessions live in the database (not a cookie), so they work across the cross-site frontend/
//! backend split. The frontend stores the opaque `session_id` and sends it as 
//! `Authorization: Bearer <session_id>`; the [`AuthedAthlete`] extractor resolves that header back
//! to an athlete.

use crate::AppState;
use crate::models;
use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::http::StatusCode;
use sea_orm::ActiveValue::Set;
use sea_orm::{ActiveModelTrait, DatabaseConnection, DbErr, EntityTrait};

// TODO: Security ideas: (a) add a sliding expiry and rotate the id. (b) a tight CSP.
/// How long a freshly minted session is valid for (seconds). 7 days.
const SESSION_TTL_SECS: i64 = 7 * 24 * 60 * 60;

/// Create a new session for `athlete_id` and return its opaque id.
pub async fn create_session(
    database: &DatabaseConnection,
    athlete_id: i64,
) -> Result<String, DbErr> {
    let session_id = nanoid::nanoid!();
    let expires_at = chrono::Utc::now().timestamp() + SESSION_TTL_SECS;
    let session = models::session::ActiveModel {
        session_id: Set(session_id.clone()),
        athlete_id: Set(athlete_id),
        expires_at: Set(expires_at),
    };
    session.insert(database).await?;
    Ok(session_id)
}

/// An authenticated caller, resolved from the `Authorization: Bearer <session_id>` header.
pub struct AuthedAthlete {
    pub athlete_id: i64,
    pub session_id: String,
}

impl FromRequestParts<AppState> for AuthedAthlete {
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // Pull the bearer token out of the Authorization header.
        let session_id = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .map(|token| token.trim().to_string())
            .ok_or((StatusCode::UNAUTHORIZED, "missing bearer token"))?;

        // Look the session up in the database.
        let session = models::session::Entity::find_by_id(session_id)
            .one(&state.database)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "session lookup failed"))?
            .ok_or((StatusCode::UNAUTHORIZED, "unknown session"))?;

        // Reject expired sessions.
        if session.expires_at < chrono::Utc::now().timestamp() {
            return Err((StatusCode::UNAUTHORIZED, "session expired"));
        }

        Ok(AuthedAthlete {
            athlete_id: session.athlete_id,
            session_id: session.session_id,
        })
    }
}
