//! Strava OAuth + API client (server-side, per user).
//!
//! The client secret lives here and never leaves the backend. Each user
//! authorizes the app, and we store *their* tokens keyed by athlete id.
//!
//! Note: Strava does not support PKCE, and it expects the client credentials in
//! the request body (not as an HTTP Basic auth header).

use crate::BACKEND_BASE_URL;
use chrono::{DateTime, Utc};
use oauth2::basic::{
    BasicErrorResponse, BasicRevocationErrorResponse, BasicTokenIntrospectionResponse,
    BasicTokenType,
};
use oauth2::url::Url;
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, Client, ClientId, ClientSecret, CsrfToken,
    EndpointNotSet, EndpointSet, ErrorResponse, ExtraTokenFields, RedirectUrl, RefreshToken,
    RequestTokenError, Scope, StandardRevocableToken, StandardTokenResponse, TokenResponse as _,
    TokenUrl,
};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::fmt;

const AUTHORIZE_URL: &str = "https://www.strava.com/oauth/authorize";
const TOKEN_URL: &str = "https://www.strava.com/oauth/token";
/// URL to fetch activities for a given athelete.
const ACTIVITIES_URL: &str = "https://www.strava.com/api/v3/athlete/activities";

/// The extra fields Strava tacks onto its OAuth token response, on top of the
/// standard OAuth fields. We only model the `athlete` object here.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct StravaExtraFields {
    athlete: StravaAthlete,
}

impl ExtraTokenFields for StravaExtraFields {}

/// A Strava token response: the standard OAuth fields plus [`StravaExtraFields`].
type StravaTokenResponse = StandardTokenResponse<StravaExtraFields, BasicTokenType>;

/// A Strava OAuth client, like `BasicClient` but returning [`StravaTokenResponse`]
/// so we can read the athlete out of the token exchange. The two type parameters
/// track whether the authorize and token endpoints have been configured.
type StravaClient<HasAuthUrl = EndpointNotSet, HasTokenUrl = EndpointNotSet> = Client<
    BasicErrorResponse,
    StravaTokenResponse,
    BasicTokenIntrospectionResponse,
    StandardRevocableToken,
    BasicRevocationErrorResponse,
    HasAuthUrl,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    HasTokenUrl,
>;

/// The athlete Strava returns inside the OAuth token response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StravaAthlete {
    pub id: i64,
    pub username: Option<String>,
}
/// The subset of Strava's token response that we care about.
pub struct StravaTokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix timestamp (seconds) at which `access_token` expires.
    pub expires_at: i64,
    pub athlete: StravaAthlete,
}

/// Build a Strava OAuth client from environment configuration.
fn oauth_client() -> StravaClient<EndpointSet, EndpointSet> {
    let client_id = std::env::var("STRAVA_CLIENT_ID").unwrap();
    let client_secret = std::env::var("STRAVA_CLIENT_SECRET").unwrap();

    StravaClient::new(ClientId::new(client_id))
        .set_client_secret(ClientSecret::new(client_secret))
        .set_auth_uri(AuthUrl::new(AUTHORIZE_URL.to_string()).expect("valid authorize url"))
        .set_token_uri(TokenUrl::new(TOKEN_URL.to_string()).expect("valid token url"))
        .set_redirect_uri(
            RedirectUrl::new(format!("{BACKEND_BASE_URL}/auth/callback"))
                .expect("valid redirect url"),
        )
        // Strava wants the client credentials in the request body.
        .set_auth_type(AuthType::RequestBody)
}

/// An HTTP client for talking to Strava oauth. Redirects are disabled to avoid SSRF.
fn http_client() -> oauth2::reqwest::Client {
    oauth2::reqwest::ClientBuilder::new()
        .redirect(oauth2::reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build HTTP client")
}

/// Build the Strava authorize URL to redirect the user to, plus the CSRF token
/// that must be echoed back on the callback.
pub fn authorize_url() -> (Url, CsrfToken) {
    oauth_client()
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("activity:read_all".to_string()))
        .url()
}

/// Exchange an authorization `code` (from the OAuth callback) for a new user's
/// tokens. The only place the client secret is used for a brand-new user.
pub async fn exchange_code(code: &str) -> Result<StravaTokens, String> {
    let token = oauth_client()
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .request_async(&http_client())
        .await
        .map_err(|e| format!("code exchange failed: {}", format_token_error(e)))?;
    Ok(into_tokens(&token))
}

/// Refresh one user's expired access token using their stored refresh token.
#[allow(dead_code)]
pub async fn refresh_access_token(refresh_token: &str) -> Result<StravaTokens, String> {
    let token = oauth_client()
        .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
        .request_async(&http_client())
        .await
        .map_err(|e| format!("token refresh failed: {}", format_token_error(e)))?;
    Ok(into_tokens(&token))
}

/// Turn an oauth2 request error into a message that actually says what went
/// wrong. The default `Display` for a parse failure is just "Failed to parse
/// server response", which hides the real cause. The `Parse` variant carries
/// the exact field path that failed plus the raw response body, so surface both.
fn format_token_error<RE, T>(e: RequestTokenError<RE, T>) -> String
where
    RE: std::error::Error,
    T: ErrorResponse,
{
    match e {
        RequestTokenError::Parse(err, raw) => {
            let body = String::from_utf8_lossy(&raw);
            format!(
                "could not parse response at `{}`: {} — raw body: {body}",
                err.path(),
                err.inner()
            )
        }
        other => other.to_string(),
    }
}

/// Extract the fields we care about from an OAuth token response.
fn into_tokens(token: &StravaTokenResponse) -> StravaTokens {
    let expires_at = token
        .expires_in()
        .map(|d| chrono::Utc::now().timestamp() + d.as_secs() as i64)
        .unwrap_or_default();
    StravaTokens {
        access_token: token.access_token().secret().to_string(),
        refresh_token: token
            .refresh_token()
            .map(|t| t.secret().to_string())
            .unwrap_or_default(),
        expires_at,
        athlete: token.extra_fields().athlete.clone(),
    }
}

/// https://developers.strava.com/docs/reference/#api-models-Error
#[derive(Deserialize)]
pub struct Error {
    /// The code associated with this error.
    pub code: String,
    /// The specific field or aspect of the resource associated with this error.
    pub field: String,
    /// The type of resource associated with this error.
    pub resource: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Customize so only `x` and `y` are denoted.
        write!(
            f,
            "Error(code={}, field={}, resource={})",
            self.code, self.field, self.resource
        )
    }
}

/// https://developers.strava.com/docs/reference/#api-models-Fault
#[derive(Deserialize)]
pub struct Fault {
    /// The message of the fault.
    pub message: String,
    /// The set of specific errors associated with this fault, if any.
    pub errors: Vec<Error>,
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let joined_errors = self
            .errors
            .iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        write!(f, "{}, errors=<{}>", self.message, joined_errors)
    }
}

/// https://developers.strava.com/docs/reference/#api-models-PolylineMap
#[derive(Clone, Debug, Deserialize)]
pub struct PolylineMap {
    /// The summary polyline of the map.
    #[serde(default)]
    pub summary_polyline: Option<String>,
}

/// https://developers.strava.com/docs/reference/#api-models-SummaryActivity
#[derive(Clone, Debug, Deserialize)]
pub struct SummaryActivity {
    /// The unique identifier of the activity.
    pub id: i64,
    /// The name of the activity.
    pub name: String,
    /// An instance of SportType. TODO: enumerate
    pub sport_type: String,
    /// The time at which the activity was started.
    pub start_date: DateTime<Utc>,
    /// An instance of PolylineMap.
    pub map: PolylineMap,
}

pub enum FetchError {
    Backoff,
    Other(String),
}

/// Fetch the most recent activities for the authenticated athlete.
///
/// before_epoch: An epoch timestamp to use for filtering activities that have taken place before a certain time.
pub async fn fetch_activities(
    access_token: &str,
    before_epoch: Option<DateTime<Utc>>,
) -> Result<Vec<SummaryActivity>, FetchError> {
    let url = match before_epoch {
        Some(before_epoch) => format!("{ACTIVITIES_URL}&before={}", before_epoch.timestamp()),
        None => ACTIVITIES_URL.to_string(),
    };

    let client = reqwest::Client::new();
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access_token}")).unwrap(),
    );
    let response = client
        .get(&url)
        .headers(headers)
        .send()
        .await
        .map_err(|err| FetchError::Other(format!("Failed to get activities: {err:?}")))?;
    if !response.status().is_success() {
        let fault = response
            .json::<Fault>()
            .await
            .map_err(|err| FetchError::Other(format!("Failed to parse fault: {err:?}")))?;
        tracing::error!("Request (url={url}) failed with fault: {fault}");
        return Err(FetchError::Backoff);
    }
    response
        .json::<Vec<SummaryActivity>>()
        .await
        .map_err(|err| FetchError::Other(format!("Failed to parse activities: {err:?}")))
}
