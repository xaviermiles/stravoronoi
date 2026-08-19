use crate::components::grid_toggle::get_show_grid_storage_value;
/// Create and draw on the mapboxgl map.
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
use web_sys::wasm_bindgen::JsCast;
use yew::platform::time;
use yew::prelude::*;
use yew::{use_effect_with_deps, use_mut_ref};

mod grid;
pub use grid::set_grid_visible;
mod hit_testing;
use hit_testing::distance_to_run_squared;

const MAPBOX_TOKEN: &str = env!("MAPBOX_TOKEN");

/// Shared handle to the map, populated once the map has been created.
pub type MapRef = Rc<RefCell<Option<Rc<Map>>>>;

/// Ids of the per-run hit-area layers, shared with the map listeners so they
/// only query interactive run geometry. Grows as runs are paged in.
type RunHitLayerIds = Rc<RefCell<Vec<String>>>;

/// Id of the currently selected (clicked) run layer, if any. Shared with the
/// click listener so it can restore the previous selection's colour when a
/// different run, or empty space, is clicked.
type SelectedRun = Rc<RefCell<Option<String>>>;

/// Strava's brand orange, used for all run lines.
const RUN_LINE_COLOUR: &str = "#fc4c02";
/// Use another colour if a run is clicked.
const SELECTED_RUN_LINE_COLOUR: &str = "#0060d0";
/// Width of the transparent companion layer used for touch hit testing.
const RUN_HIT_WIDTH: f64 = 20.0;

// If the API returns nothing, avoid spamming the backend while it populates.
const SLOW_CONTINUE_TIME: Duration = Duration::from_millis(100);
const FAST_CONTINUE_TIME: Duration = Duration::from_millis(10);

struct Listener {
    on_unauthorized: Callback<()>,
}

impl MapEventListener for Listener {
    fn on_load(&mut self, map: Rc<Map>, _e: event::MapBaseEvent) {
        // Draw the grid lines & intersections for debugging purposes.
        let grid_map = map.clone();
        wasm_bindgen_futures::spawn_local(async move {
            grid::add_grid_layers(&grid_map, get_show_grid_storage_value()).await;
        });
        // Once the base map style has loaded, fetch the runs and overlay them.
        let on_unauthorized = self.on_unauthorized.clone();
        // A single map-level click listener, shared with the run layers it filters on.
        // Registered once here rather than per-run to avoid re-adding listeners in the
        // paging loop below.
        let run_hit_layers: RunHitLayerIds = Rc::new(RefCell::new(Vec::new()));
        let selected_run: SelectedRun = Rc::new(RefCell::new(None));
        if let Err(err) = map.on(RunClickListener {
            run_hit_layers: run_hit_layers.clone(),
            selected_run,
        }) {
            log::error!("Failed to register run click listener: {err:?}");
        }
        // Show a pointer cursor while hovering a run line to signal it's clickable.
        if let Err(err) = map.on(RunHoverListener {
            run_hit_layers: run_hit_layers.clone(),
        }) {
            log::error!("Failed to register run hover listener: {err:?}");
        }
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
                add_run_layers(&map, loaded_runs.features, &run_hit_layers);
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

/// Add the decoded Strava runs to the map as single-colour line layers.
///
/// Each run gets its own source and layer, keyed by the run id. The layer ids
/// are recorded in `run_layers` so the shared click listener can restrict its
/// feature queries to run layers only.
fn add_run_layers(map: &Map, run_lines: Vec<(i64, Feature)>, run_hit_layers: &RunHitLayerIds) {
    for (run_id, mut run_line) in run_lines {
        let layer_id = run_id.to_string();
        let hit_layer_id = format!("{layer_id}-hit-area");
        // Tag the feature with the run id so the click listener can label popups.
        run_line.id = Some(geojson::feature::Id::Number(run_id.into()));
        if let Err(err) = map.add_geojson_source(&layer_id, GeoJson::Feature(run_line)) {
            log::error!("Failed to add Strava source: {err:?}");
            continue;
        }

        if let Err(err) = map.add_layer(run_line_layer(&layer_id, RUN_LINE_COLOUR), None) {
            log::error!("Failed to add Strava layer: {err:?}");
            continue;
        }
        if let Err(err) = map.add_layer(run_hit_layer(&hit_layer_id, &layer_id), None) {
            log::error!("Failed to add Strava hit-area layer: {err:?}");
            continue;
        }
        run_hit_layers.borrow_mut().push(hit_layer_id);
    }
}

/// Build the styled line layer for a single run, coloured `colour`.
fn run_line_layer(layer_id: &str, colour: &str) -> LineLayer {
    let mut layer = LineLayer::new(layer_id, layer_id);
    layer.layout.line_join = Some(LineJoin::Round.into());
    layer.layout.line_cap = Some(LineCap::Round.into());
    layer.paint.line_color = Some(colour.into());
    layer.paint.line_width = Some(3.5.into());
    layer
}

/// Build the invisible layer that acts as a proxy for hit tests for a given run line.
fn run_hit_layer(layer_id: &str, source_id: &str) -> LineLayer {
    let mut layer = LineLayer::new(layer_id, source_id);
    layer.layout.line_join = Some(LineJoin::Round.into());
    layer.layout.line_cap = Some(LineCap::Round.into());
    layer.paint.line_color = Some("rgba(0, 0, 0, 0)".into());
    layer.paint.line_width = Some(RUN_HIT_WIDTH.into());
    layer
}

/// Recolour an existing run layer by removing and re-adding it. The mapboxgl
/// wrapper doesn't expose `setPaintProperty`, but the underlying GeoJSON source
/// persists, so only the (cheap) styling layer is rebuilt. Re-adding also lifts
/// the layer above the others, keeping the selected run on top.
fn recolour_run(map: &Map, layer_id: &str, colour: &str) {
    if map.get_layer(layer_id).is_err() {
        return;
    }
    if let Err(err) = map.remove_layer(layer_id) {
        log::error!("Failed to remove run layer {layer_id}: {err:?}");
        return;
    }
    if let Err(err) = map.add_layer(run_line_layer(layer_id, colour), None) {
        log::error!("Failed to re-add run layer {layer_id}: {err:?}");
    }
}

/// Single, map-level click listener that shows a popup only when a run line is clicked.
struct RunClickListener {
    run_hit_layers: RunHitLayerIds,
    selected_run: SelectedRun,
}

impl MapEventListener for RunClickListener {
    fn on_click(&mut self, map: Rc<Map>, e: event::MapMouseEvent) {
        let layers = self.run_hit_layers.borrow().clone();

        let hits = match map.query_rendered_features(
            Some(e.point.clone()),
            mapboxgl::QueryFeatureOptions {
                layers,
                ..Default::default()
            },
        ) {
            Ok(hits) => hits,
            Err(err) => {
                log::error!("Failed to query run features: {err:?}");
                return;
            }
        };

        let tap_lng_lat = [e.lng_lat.lng, e.lng_lat.lat];
        let Some(feature) = hits.into_iter().min_by(|left, right| {
            let left_distance = distance_to_run_squared(left, &tap_lng_lat).unwrap_or(f64::MAX);
            let right_distance = distance_to_run_squared(right, &tap_lng_lat).unwrap_or(f64::MAX);
            left_distance.total_cmp(&right_distance)
        }) else {
            // No run line under the cursor: deselect the previously selected run, if any.
            if let Some(prev) = self.selected_run.borrow_mut().take() {
                recolour_run(&map, &prev, RUN_LINE_COLOUR);
            }
            return;
        };

        // The feature id was tagged with the run id, which is also its layer id.
        let clicked_id = match &feature.id {
            Some(geojson::feature::Id::Number(n)) => n.to_string(),
            Some(geojson::feature::Id::String(s)) => s.clone(),
            None => return,
        };

        // Highlight the clicked run, restoring the previous selection to orange.
        {
            let mut selected = self.selected_run.borrow_mut();
            if selected.as_deref() != Some(clicked_id.as_str()) {
                if let Some(prev) = selected.take() {
                    recolour_run(&map, &prev, RUN_LINE_COLOUR);
                }
                recolour_run(&map, &clicked_id, SELECTED_RUN_LINE_COLOUR);
                *selected = Some(clicked_id);
            }
        }

        let Some(properties) = feature.properties else {
            return;
        };
        let Some(serde_json::Value::String(popup_text)) = properties.get("popup_text") else {
            // This shouldn't happen as strava::get_properties() always inserts this property.
            return;
        };

        let popup = mapboxgl::Popup::new(
            LngLat::new(e.lng_lat.lng, e.lng_lat.lat),
            mapboxgl::PopupOptions::new(),
        );
        popup.set_html(popup_text);
        popup.add_to(&map);
    }
}

/// Single, map-level listener that shows a pointer cursor while hovering a run line,
/// signalling that it can be clicked. Mirrors `RunClickListener` by filtering the
/// feature query to the run layers only.
struct RunHoverListener {
    run_hit_layers: RunHitLayerIds,
}

impl MapEventListener for RunHoverListener {
    fn on_mousemove(&mut self, map: Rc<Map>, e: event::MapMouseEvent) {
        let layers = self.run_hit_layers.borrow().clone();
        let over_run = map
            .query_rendered_features(
                Some(e.point.clone()),
                mapboxgl::QueryFeatureOptions {
                    layers,
                    ..Default::default()
                },
            )
            .map(|hits| !hits.is_empty())
            .unwrap_or(false);
        set_cursor(&map, if over_run { "pointer" } else { "" });
    }
}

/// Set the CSS cursor on the map canvas. An empty string restores the default
/// (grab/drag) cursor that Mapbox manages.
fn set_cursor(map: &Map, cursor: &str) {
    if let Ok(Some(canvas)) = map.get_container().query_selector(".mapboxgl-canvas")
        && let Ok(canvas) = canvas.dyn_into::<web_sys::HtmlElement>()
    {
        let _ = canvas.style().set_property("cursor", cursor);
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
