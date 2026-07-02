mod map;
mod strava;
use yew::prelude::*;

#[function_component(App)]
fn app() -> Html {
    let _map = map::use_map();

    html! {
      <div id="map" style="width: 100vw; height: 100vh;"></div>
    }
}

fn main() {
    wasm_logger::init(wasm_logger::Config::default());
    yew::Renderer::<App>::new().render();
}
