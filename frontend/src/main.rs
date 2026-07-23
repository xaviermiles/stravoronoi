mod login_button;
mod map;
mod session;
mod strava;
use login_button::LoginButton;
use yew::prelude::*;

/// The base URL of the backend to use.
pub const BACKEND_BASE_URL: &str = if cfg!(debug_assertions) {
    "http://localhost:3000"
} else {
    "https://stravoronoi-production.up.railway.app"
};

#[function_component(App)]
fn app() -> Html {
    let auth = session::use_auth();
    let _map = map::use_map(auth.on_unauthorized.clone());

    html! {
      <div id="container">
        <div id="map" style="width: 100vw; height: 100vh;"></div>
        <LoginButton logged_in={auth.logged_in} profile={auth.profile} />
      </div>
    }
}

fn main() {
    wasm_logger::init(wasm_logger::Config::new(log::Level::Info));
    yew::Renderer::<App>::new().render();
}
