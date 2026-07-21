use crate::strava::{self, LoadState};
use chrono::{DateTime, Utc};
use geojson::{Feature, GeoJson};
use mapboxgl::Source;
use mapboxgl::layer::{IntoLayer, Layer, RasterLayer};
use mapboxgl::layer::{LineCap, LineJoin, LineLayer};
use mapboxgl::style::Sources;
use mapboxgl::{LngLat, Map, MapEventListener, MapOptions, Style, event};
use std::time::Duration;
use std::{cell::RefCell, rc::Rc};
use yew::platform::time;
use yew::prelude::*;
use yew::{use_effect_with_deps, use_mut_ref};

const MAPBOX_TOKEN: &str = env!("MAPBOX_TOKEN");

/// Strava's brand orange, used for all run lines.
const RUN_LINE_COLOR: &str = "#fc4c02";

// If the API returns nothing, avoid spamming the backend while it populates.
const SLOW_CONTINUE_TIME: Duration = Duration::from_secs(1);
const FAST_CONTINUE_TIME: Duration = Duration::from_millis(10);

struct Listener {
    on_unauthorized: Callback<()>,
}

impl MapEventListener for Listener {
    fn on_load(&mut self, map: Rc<Map>, _e: event::MapBaseEvent) {
        // Once the base map style has loaded, fetch the runs and overlay them.
        let on_unauthorized = self.on_unauthorized.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let mut before: Option<DateTime<Utc>> = None;
            loop {
                let loaded_runs = match strava::load_run_lines(before).await {
                    Ok(loaded_runs) => loaded_runs,
                    Err(strava::LoadError::Unauthorized) => {
                        log::info!("Session rejected: logging out.");
                        on_unauthorized.emit(());
                        break;
                    }
                    Err(strava::LoadError::Other(e)) => {
                        log::error!("Failed to load Strava runs: {e}");
                        break;
                    }
                };
                add_run_layers(&map, loaded_runs.features);
                let next_before = match loaded_runs.load_state {
                    LoadState::Continue(next_before) => next_before,
                    LoadState::Finished => break,
                };
                let wait_time = if next_before == before {
                    SLOW_CONTINUE_TIME
                } else {
                    FAST_CONTINUE_TIME
                };
                time::sleep(wait_time).await;
                before = next_before;
            }
        });
    }
}

/// Add the decoded Strava runs to the map as single-color line layers.
fn add_run_layers(map: &Map, run_lines: Vec<(i64, Feature)>) {
    for (run_id, run_line) in run_lines {
        let layer_id = &run_id.to_string();
        if let Err(e) = map.add_geojson_source(layer_id, GeoJson::Feature(run_line)) {
            log::error!("failed to add Strava source: {e:?}");
            continue;
        }
        log::info!("Adding Strava run layer");

        let mut layer = LineLayer::new(layer_id, layer_id);
        layer.layout.line_join = Some(LineJoin::Round.into());
        layer.layout.line_cap = Some(LineCap::Round.into());
        layer.paint.line_color = Some(RUN_LINE_COLOR.into());
        layer.paint.line_width = Some(3.0.into());

        if let Err(e) = map.add_layer(layer, None) {
            log::error!("failed to add Strava layer: {e:?}");
        }
    }
}

fn create_map() -> Rc<Map> {
    let mut sources = Sources::new();
    sources.insert(
        "carto-light".into(),
        Source {
            r#type: "raster".into(),
            // The @2x is to avoid upscaling blue to make the rendering sharper on HiDPI screens.
            tiles: Some(vec![
                "https://a.basemaps.cartocdn.com/light_all/{z}/{x}/{y}@2x.png".into(),
                "https://b.basemaps.cartocdn.com/light_all/{z}/{x}/{y}@2x.png".into(),
                "https://c.basemaps.cartocdn.com/light_all/{z}/{x}/{y}@2x.png".into(),
                "https://d.basemaps.cartocdn.com/light_all/{z}/{x}/{y}@2x.png".into(),
            ]),
            ..Default::default()
        },
    );
    let layers: Vec<Layer> = vec![
        RasterLayer {
            id: "carto-light-layer".into(),
            source: "carto-light".into(),
            minzoom: Some(0.0),
            maxzoom: Some(21.0),
            ..Default::default()
        }
        .into_layer(),
    ];

    // The default coordinates are Christchurch.
    let opts = MapOptions::new(MAPBOX_TOKEN.into(), "map".into())
        .style(Style {
            version: 8,
            sources,
            layers,
            ..Default::default()
        })
        .center(LngLat::new(172.637491, -43.530950))
        .zoom(13.0);

    Map::new(opts).unwrap()
}

#[hook]
pub fn use_map(on_unauthorized: Callback<()>) -> Rc<RefCell<Option<Rc<Map>>>> {
    let map = use_mut_ref(|| Option::<Rc<Map>>::None);

    {
        let map = map.clone();
        use_effect_with_deps(
            move |_| {
                let m = create_map();
                if let Err(e) = m.on(Listener { on_unauthorized }) {
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
