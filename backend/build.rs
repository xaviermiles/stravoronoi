//! Loads variables from a local `.env` file at build time and re-exports them
//! as compile-time environment variables via `cargo:rustc-env`, so they can be
//! read from WASM with the `env!()` macro.

fn main() {
    println!("cargo:rerun-if-changed=.env");

    // This .env file doesn't exist when running in railway.
    if std::env::var("RAILWAY_ENVIRONMENT").is_err() {
        dotenvy::dotenv().unwrap();
    }

    for key in ["STRAVA_CLIENT_ID", "STRAVA_CLIENT_SECRET"] {
        let value = std::env::var(key).unwrap();
        println!("cargo:rustc-env={key}={value}");
    }
}
