mod map;
mod strava;
use yew::prelude::*;

#[function_component(App)]
fn app() -> Html {
    let _map = map::use_map();
    let onclick = Callback::from(|_e: MouseEvent| {
        web_sys::window()
            .unwrap()
            .location()
            .set_href(strava::BACKEND_LOGIN_URL)
            .unwrap();
    });

    html! {
      <div id="container">
        <div id="map" style="width: 100vw; height: 100vh;"></div>
        <button
            data-key="log-in"
            style="position: absolute; top: 10px; min-height: 40px; padding: 6px 16px; right: 10px; z-index: 1; border: 1px solid white; font-weight: 600; background-color: white; font-size: 14px; border-radius: 4px; font-family: \"Boathouse,Segoe UI,Helvetica Neue,-apple-system,system-ui,BlinkMacSystemFont,Roboto,Arial,sans-serif,Apple Color Emoji,Segoe UI Emoji,Segoe UI Symbol;\""
            onclick={onclick}>
            { "Log In" }
        </button>
      </div>
    }
}

fn main() {
    wasm_logger::init(wasm_logger::Config::default());
    yew::Renderer::<App>::new().render();
}
