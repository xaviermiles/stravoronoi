mod login_button;
mod map;
mod session;
mod strava;
use gloo_history::BrowserHistory;
use gloo_history::History;
use login_button::LoginButton;
use serde::Deserialize;
use yew::prelude::*;

/// The base URL of the backend to use.
pub const BACKEND_BASE_URL: &str = if cfg!(debug_assertions) {
    "http://localhost:3000"
} else {
    "https://stravoronoi-production.up.railway.app"
};

#[derive(Deserialize)]
struct CallbackQuery {
    session_id: Option<String>,
}

#[derive(Properties, PartialEq)]
struct SessionIdProps {
    on_login: Callback<()>,
}

#[function_component]
fn SessionId(props: &SessionIdProps) -> Html {
    let history = BrowserHistory::new();
    match history.location().query::<CallbackQuery>() {
        Ok(CallbackQuery {
            session_id: Some(session_id),
        }) => {
            session::set_session_id(session_id);
            props.on_login.emit(());
            // Replace with the current path to drop the query string.
            history.replace(history.location().path());
        }
        Ok(CallbackQuery { session_id: None }) => {
            // No session id in the URL (normal page load) — nothing to do.
        }
        Err(err) => {
            log::warn!("Failed to parse location query: {err}");
        }
    };
    html! { <div /> }
}

#[function_component(App)]
fn app() -> Html {
    let auth = session::use_auth();
    let _map = map::use_map(auth.on_unauthorized.clone());

    html! {
      <div id="container">
        <div id="map" style="width: 100vw; height: 100vh;"></div>
        <LoginButton logged_in={auth.logged_in} profile={auth.profile.clone()} />
        <SessionId on_login={auth.on_login.clone()} />
      </div>
    }
}

fn main() {
    wasm_logger::init(wasm_logger::Config::new(log::Level::Info));
    yew::Renderer::<App>::new().render();
}
