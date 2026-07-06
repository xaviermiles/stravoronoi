mod map;
mod strava;
use gloo_history::BrowserHistory;
use yew::prelude::*;
use serde::Deserialize;
use gloo_history::History;
use gloo_storage::{LocalStorage, Storage};

/// The base URL of the backend to use.
pub const BACKEND_BASE_URL: &str = if cfg!(debug_assertions) {
    "http://localhost:3000"
} else {
    "https://stravoronoi-production.up.railway.app"
};

#[function_component]
fn LoginButton() -> Html {
    let onclick = Callback::from(|_e: MouseEvent| {
        web_sys::window()
            .unwrap()
            .location()
            .set_href(&format!("{BACKEND_BASE_URL}/auth/login"))
            .unwrap();
    });
    html! {
        <button
            data-key="log-in"
            style="position: absolute; top: 10px; min-height: 40px; padding: 6px 16px; right: 10px; z-index: 1; border: 1px solid white; font-weight: 600; background-color: white; font-size: 14px; border-radius: 4px; font-family: \"Boathouse,Segoe UI,Helvetica Neue,-apple-system,system-ui,BlinkMacSystemFont,Roboto,Arial,sans-serif,Apple Color Emoji,Segoe UI Emoji,Segoe UI Symbol;\""
            onclick={onclick}>
            { "Log In" }
        </button>
    }
}

#[derive(Deserialize)]
struct CallbackQuery { session_id: String }

#[function_component]
fn SessionId() -> Html {
    let history = BrowserHistory::new();
    match history.location().query::<CallbackQuery>() {
        Ok(callback_query) => {
            if let Err(err) = LocalStorage::set("session_id", &callback_query.session_id) {
                log::info!("{}", err.to_string());
            };

        }
        Err(err) => {
            log::info!("{}", err.to_string());
        }
    };
    history.replace("/");
    html! { <div /> }
}

#[function_component(App)]
fn app() -> Html {
    let _map = map::use_map();

    html! {
      <div id="container">
        <div id="map" style="width: 100vw; height: 100vh;"></div>
        <LoginButton />
        <SessionId />
      </div>
    }
}

fn main() {
    wasm_logger::init(wasm_logger::Config::default());
    yew::Renderer::<App>::new().render();
}
