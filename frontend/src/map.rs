use crate::components::grid_toggle::get_show_grid_storage_value;
/// Create and draw on the mapboxgl map.
use crate::strava::{self, LoadState};
use chrono::{DateTime, Utc};
use geojson::{Feature, GeoJson};
use mapboxgl::Source;
use mapboxgl::layer::{CircleLayer, IntoLayer, Layer, RasterLayer};
use mapboxgl::layer::{LineCap, LineJoin, LineLayer};
use mapboxgl::style::Sources;
use mapboxgl::{LngLat, Map, MapEventListener, MapOptions, Style, event};
use std::time::Duration;
use std::{cell::RefCell, rc::Rc};
use yew::platform::time;
use yew::prelude::*;
use yew::{use_effect_with_deps, use_mut_ref};

const MAPBOX_TOKEN: &str = env!("MAPBOX_TOKEN");

/// Shared handle to the map, populated once the map has been created.
pub type MapRef = Rc<RefCell<Option<Rc<Map>>>>;

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
        // Draw the grid lines & intersections for debugging purposes.
        let grid_map = map.clone();
        wasm_bindgen_futures::spawn_local(async move {
            add_grid_layers(&grid_map).await;
            // Once the sources exist, honour the overlay preference persisted in local storage.
            set_grid_visible(&grid_map, get_show_grid_storage_value());
        });
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
        if let Err(err) = map.add_geojson_source(layer_id, GeoJson::Feature(run_line)) {
            log::error!("Failed to add Strava source: {err:?}");
            continue;
        }

        let mut layer = LineLayer::new(layer_id, layer_id);
        layer.layout.line_join = Some(LineJoin::Round.into());
        layer.layout.line_cap = Some(LineCap::Round.into());
        layer.paint.line_color = Some(RUN_LINE_COLOR.into());
        layer.paint.line_width = Some(3.0.into());

        if let Err(err) = map.add_layer(layer, None) {
            log::error!("Failed to add Strava layer: {err:?}");
        }
    }
}

/// Add the grid layers to the map.
async fn add_grid_layers(map: &Map) {
    let all_highways = crate::road_grid::get_all_ways().await;
    map.add_geojson_source("all-highways", GeoJson::FeatureCollection(all_highways))
        .unwrap();
    let intersections = crate::road_grid::get_intersections().await;
    map.add_geojson_source("intersections", GeoJson::FeatureCollection(intersections))
        .unwrap();
    let cells = crate::road_grid::get_voronoi_cells().await;
    map.add_geojson_source("cells", GeoJson::FeatureCollection(cells))
        .unwrap();
}

/// Ids of the layers that make up the debug road-grid overlay.
const GRID_LAYER_IDS: [&str; 3] = ["all-highways", "intersections", "cells"];

/// Add the styled grid overlay layers, assuming their sources already exist.
/// Each layer is skipped if it is already present, so this doubles as the
/// "switch the overlay back on" path after it has been hidden.
fn add_grid_layer_styles(map: &Map) {
    if map.get_geojson_source("cells").is_some() && map.get_layer("cells").is_err() {
        let mut lines = LineLayer::new("cells", "cells");
        lines.layout.line_join = Some(LineJoin::Round.into());
        lines.layout.line_cap = Some(LineCap::Round.into());
        lines.paint.line_color = Some(RUN_LINE_COLOR.into());
        lines.paint.line_width = Some(3.0.into());
        if let Err(err) = map.add_layer(lines, None) {
            log::error!("Failed to add cells layer: {err:?}");
        }
    }
    if map.get_geojson_source("all-highways").is_some() && map.get_layer("all-highways").is_err() {
        let lines = LineLayer::new("all-highways", "all-highways");
        if let Err(err) = map.add_layer(lines, None) {
            log::error!("Failed to add all-highways layer: {err:?}");
        }
    }
    if map.get_geojson_source("intersections").is_some() && map.get_layer("intersections").is_err()
    {
        let circles = CircleLayer::new("intersections", "intersections");
        if let Err(err) = map.add_layer(circles, None) {
            log::error!("Failed to add intersections layer: {err:?}");
        }
    }
}

/// Show or hide the debug road-grid overlay. The GeoJSON sources persist when a
/// layer is removed, so toggling back on just re-adds the (cheap) layers.
pub fn set_grid_visible(map: &Map, visible: bool) {
    if visible {
        add_grid_layer_styles(map);
    } else {
        for id in GRID_LAYER_IDS {
            if map.get_layer(id).is_ok() {
                if let Err(err) = map.remove_layer(id) {
                    log::error!("Failed to remove grid layer {id}: {err:?}");
                }
            }
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
pub fn use_map(on_unauthorized: Callback<()>) -> MapRef {
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
