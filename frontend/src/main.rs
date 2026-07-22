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
            history.replace("/");
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
    let logged_in = use_state(session::is_logged_in);

    let on_unauthorized = {
        let logged_in = logged_in.clone();
        Callback::from(move |_| logged_in.set(false))
    };
    let on_login = {
        let logged_in = logged_in.clone();
        Callback::from(move |_| logged_in.set(true))
    };

    let _map = map::use_map(on_unauthorized);

    html! {
      <div id="container">
        <div id="map" style="width: 100vw; height: 100vh;"></div>
        <LoginButton logged_in={*logged_in} />
        <SessionId {on_login} />
      </div>
    }
}

fn main() {
    wasm_logger::init(wasm_logger::Config::default());
    yew::Renderer::<App>::new().render();
}
