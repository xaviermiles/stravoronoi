# stravoronoi

The aim is to generate a voronoi map based on the polylines of my previous Strava run routes.

## Plan

### Storage (new)
Token store: athlete_id → { access_token, refresh_token, expires_at }. Any small DB (SQLite/Postgres/KV). Encrypt tokens at rest — a refresh token is a long-lived key to someone's data.
Session store: session_id (cookie) → athlete_id. Can be a DB row or a signed cookie.
Optional: cache each user's matched-route GeoJSON keyed by athlete — this is where your PER_PAGE/map-matching performance concern goes away.

### Frontend (WASM) changes — small
Add a "Connect with Strava" button that navigates to /auth/login.
Track logged-in vs logged-out state; only fetch /api/runs when logged in.
Send requests with cookies included (credentials: "include" equivalent on your Request).
Handle a 401 from /api/runs by showing the connect button again.
The WASM never sees any token — only the session cookie (which the browser manages automatically).

### Security must-haves
state parameter on every authorize request, validated on callback (CSRF protection).
Cookies: HttpOnly, Secure, SameSite=Lax.
Request minimal scopes (activity:read is enough for routes; avoid activity:read_all unless you need private activities).
Never return tokens to the browser; the backend is the only holder.
Per-user rate limiting / caching so one user can't exhaust your Strava API quota.
Provide a disconnect endpoint that deletes stored tokens (and ideally calls Strava's deauthorize endpoint) — good practice and often required for compliance.

### Strava app config
In your Strava API application settings, set the Authorization Callback Domain to your deployed domain (e.g. yourapp.com). Strava validates redirects against this.
Your single client_id + client_secret serve all users — you don't need per-user credentials.

### Net effect on your current code
build.rs / env!() secrets → gone from the deployed build; secrets live only in the backend's secret store.
refresh_access_token() logic → moves server-side and becomes per-user (refresh when expires_at has passed).
WASM load_run_lines() → just GET /api/runs with the session cookie.
