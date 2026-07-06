use crate::strava;
use mapboxgl::layer::{LineCap, LineJoin, LineLayer};
use mapboxgl::{LngLat, Map, MapEventListener, MapOptions, event};
use std::{cell::RefCell, rc::Rc};
use yew::prelude::*;
use yew::{use_effect_with_deps, use_mut_ref};

const MAPBOX_TOKEN: &str = env!("MAPBOX_TOKEN");

/// Strava's brand orange, used for all run lines.
const RUN_LINE_COLOR: &str = "#fc4c02";

struct Listener;

impl MapEventListener for Listener {
    fn on_load(&mut self, map: Rc<Map>, _e: event::MapBaseEvent) {
        // Once the base map style has loaded, fetch the runs and overlay them.
        wasm_bindgen_futures::spawn_local(async move {
            match strava::load_run_lines().await {
                Ok(geojson) => add_run_layer(&map, geojson),
                Err(e) => log::error!("failed to load Strava runs: {e}"),
            }
        });
    }
}

/// Add the decoded Strava runs to the map as a single-color line layer.
fn add_run_layer(map: &Map, geojson: geojson::GeoJson) {
    if let Err(e) = map.add_geojson_source("strava-runs", geojson) {
        log::error!("failed to add Strava source: {e:?}");
        return;
    }
    log::info!("Adding Strava run layer");

    let mut layer = LineLayer::new("strava-runs", "strava-runs");
    layer.layout.line_join = Some(LineJoin::Round.into());
    layer.layout.line_cap = Some(LineCap::Round.into());
    layer.paint.line_color = Some(RUN_LINE_COLOR.into());
    layer.paint.line_width = Some(3.0.into());

    if let Err(e) = map.add_layer(layer, None) {
        log::error!("failed to add Strava layer: {e:?}");
    }
}

fn create_map() -> Rc<Map> {
    // The default coordinates are Christchurch.
    let opts = MapOptions::new(MAPBOX_TOKEN.into(), "map".into())
        .center(LngLat::new(172.637491, -43.530950))
        .zoom(13.0);

    Map::new(opts).unwrap()
}

#[hook]
pub fn use_map() -> Rc<RefCell<Option<Rc<Map>>>> {
    let map = use_mut_ref(|| Option::<Rc<Map>>::None);

    {
        let map = map.clone();
        use_effect_with_deps(
            move |_| {
                let m = create_map();
                if let Err(e) = m.on(Listener) {
                    log::error!("failed to register map listener: {e:?}");
                }
                log::info!("Map created, waiting for load event");

                if let Ok(mut map) = map.try_borrow_mut() {
                    map.replace(m);
                } else {
                    log::error!("Failed to store Map");
                }
                || {}
            },
            (),
        );
    }

    map
}
