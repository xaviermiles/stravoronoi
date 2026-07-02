//! Loads variables from a local `.env` file at build time and re-exports them
//! as compile-time environment variables via `cargo:rustc-env`, so they can be
//! read from WASM with the `env!()` macro (runtime `std::env::var` does not work
//! in the browser).

fn main() {
    println!("cargo:rerun-if-changed=.env");

    // Best-effort: if there's no .env, we still emit empty values so `env!` compiles.
    let _ = dotenvy::dotenv();

    for key in [
        "MAPBOX_TOKEN",
        "STRAVA_CLIENT_ID",
        "STRAVA_CLIENT_SECRET",
        "STRAVA_REFRESH_TOKEN",
    ] {
        let value = std::env::var(key).unwrap_or_default();
        println!("cargo:rustc-env={key}={value}");
    }
}
