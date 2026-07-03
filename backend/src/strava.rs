//! Strava OAuth + API client (server-side, per user).
//!
//! The client secret lives here and never leaves the backend. Each user
//! authorizes the app, and we store *their* tokens keyed by athlete id.
//!
//! Note: Strava does not support PKCE, and it expects the client credentials in
//! the request body (not as an HTTP Basic auth header).

use oauth2::basic::{BasicClient, BasicTokenResponse};
use oauth2::reqwest;
use oauth2::url::Url;
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet,
    EndpointSet, RedirectUrl, RefreshToken, Scope, TokenResponse as _, TokenUrl,
};

const AUTHORIZE_URL: &str = "https://www.strava.com/oauth/authorize";
const TOKEN_URL: &str = "https://www.strava.com/oauth/token";
const DEFAULT_REDIRECT_URI: &str = "http://localhost:3000/auth/callback";

/// A Strava OAuth client with the authorize + token endpoints configured.
type StravaClient =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

/// The subset of Strava's token response that we care about.
pub struct StravaTokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix timestamp (seconds) at which `access_token` expires.
    pub expires_at: i64,
}

/// Build a Strava OAuth client from environment configuration.
fn oauth_client() -> StravaClient {
    let client_id = std::env::var("STRAVA_CLIENT_ID").unwrap();
    let client_secret = std::env::var("STRAVA_CLIENT_SECRET").unwrap();
    let redirect = std::env::var("STRAVA_REDIRECT_URI").unwrap_or_else(|_| DEFAULT_REDIRECT_URI.to_string());

    BasicClient::new(ClientId::new(client_id))
        .set_client_secret(ClientSecret::new(client_secret))
        .set_auth_uri(AuthUrl::new(AUTHORIZE_URL.to_string()).expect("valid authorize url"))
        .set_token_uri(TokenUrl::new(TOKEN_URL.to_string()).expect("valid token url"))
        .set_redirect_uri(RedirectUrl::new(redirect).expect("valid redirect url"))
        // Strava wants the client credentials in the request body.
        .set_auth_type(AuthType::RequestBody)
}

/// An HTTP client for talking to Strava. Redirects are disabled to avoid SSRF.
fn http_client() -> reqwest::Client {
    reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build HTTP client")
}

/// Build the Strava authorize URL to redirect the user to, plus the CSRF token
/// that must be echoed back on the callback.
pub fn authorize_url() -> (Url, CsrfToken) {
    oauth_client()
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("activity:read".to_string()))
        .url()
}

/// Exchange an authorization `code` (from the OAuth callback) for a new user's
/// tokens. The only place the client secret is used for a brand-new user.
pub async fn exchange_code(code: &str) -> Result<StravaTokens, String> {
    let token = oauth_client()
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .request_async(&http_client())
        .await
        .map_err(|e| format!("code exchange failed: {e}"))?;
    Ok(into_tokens(&token))
}

/// Refresh one user's expired access token using their stored refresh token.
pub async fn refresh_access_token(refresh_token: &str) -> Result<StravaTokens, String> {
    let token = oauth_client()
        .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
        .request_async(&http_client())
        .await
        .map_err(|e| format!("token refresh failed: {e}"))?;
    Ok(into_tokens(&token))
}

/// Extract the fields we care about from an OAuth token response.
fn into_tokens(token: &BasicTokenResponse) -> StravaTokens {
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
    }
}
