/// Create and draw on the mapboxgl map.
use crate::strava::{self, LoadState};
use boostvoronoi::prelude::*;
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

/// Ids of the per-run line layers, shared with the click listener so it can
/// restrict feature queries to run layers only. Grows as runs are paged in.
type RunLayerIds = Rc<RefCell<Vec<String>>>;

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
        wasm_bindgen_futures::spawn_local(async move { add_grid_layers(&grid_map).await });
        // Once the base map style has loaded, fetch the runs and overlay them.
        let on_unauthorized = self.on_unauthorized.clone();
        // A single map-level click listener, shared with the run layers it filters on.
        // Registered once here rather than per-run to avoid re-adding listeners in the
        // paging loop below.
        let run_layers: RunLayerIds = Rc::new(RefCell::new(Vec::new()));
        if let Err(err) = map.on(RunClickListener {
            run_layers: run_layers.clone(),
        }) {
            log::error!("Failed to register run click listener: {err:?}");
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
                add_run_layers(&map, loaded_runs.features, &run_layers);
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
///
/// Each run gets its own source and layer, keyed by the run id. The layer ids
/// are recorded in `run_layers` so the shared click listener can restrict its
/// feature queries to run layers only.
fn add_run_layers(map: &Map, run_lines: Vec<(i64, Feature)>, run_layers: &RunLayerIds) {
    for (run_id, mut run_line) in run_lines {
        let layer_id = run_id.to_string();
        // Tag the feature with the run id so the click listener can label popups.
        run_line.id = Some(geojson::feature::Id::Number(run_id.into()));
        if let Err(err) = map.add_geojson_source(&layer_id, GeoJson::Feature(run_line)) {
            log::error!("Failed to add Strava source: {err:?}");
            continue;
        }

        let mut layer = LineLayer::new(&layer_id, &layer_id);
        layer.layout.line_join = Some(LineJoin::Round.into());
        layer.layout.line_cap = Some(LineCap::Round.into());
        layer.paint.line_color = Some(RUN_LINE_COLOR.into());
        layer.paint.line_width = Some(3.0.into());

        match map.add_layer(layer, None) {
            Ok(()) => run_layers.borrow_mut().push(layer_id),
            Err(err) => log::error!("Failed to add Strava layer: {err:?}"),
        }
    }
}

/// Single, map-level click listener that shows a popup only when a run line is clicked.
struct RunClickListener {
    run_layers: RunLayerIds,
}

impl MapEventListener for RunClickListener {
    fn on_click(&mut self, map: Rc<Map>, e: event::MapMouseEvent) {
        let layers = self.run_layers.borrow().clone();

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

        // No run line under the cursor: the click wasn't on a run, so do nothing.
        let Some(feature) = hits.into_iter().next() else {
            return;
        };
        let Some(properties) = feature.properties else {
            return;
        };
        let (Some(serde_json::Value::String(name)), Some(serde_json::Value::String(start_date))) =
            (properties.get("name"), properties.get("start_date"))
        else {
            // This shouldn't happen as load_run_lines() always inserts these properties.
            return;
        };

        let popup = mapboxgl::Popup::new(
            LngLat::new(e.lng_lat.lng, e.lng_lat.lat),
            mapboxgl::PopupOptions::new(),
        );
        popup.set_html(format!("<h3>{start_date}</h3><h1>{name}"));
        popup.add_to(&map);
    }
}

const SCALING_FACTOR: f64 = 10_000_000.;

fn point_i64(point_f64: &[f64]) -> Point<i64> {
    // boostvoronoi only supports integer types so cast the f64 to i64 but keep a reasonable
    // amount of the precision by shifting left past the decimal place.
    Point::new(
        (point_f64[0] * SCALING_FACTOR) as i64,
        (point_f64[1] * SCALING_FACTOR) as i64,
    )
}

fn line_i64(start: &[f64], end: &[f64]) -> Line<i64> {
    Line::new(point_i64(start), point_i64(end))
}

fn get_segment_pairs(segment_coords: &[Vec<f64>]) -> Vec<Line<i64>> {
    segment_coords
        .iter()
        .zip(segment_coords.iter().skip(1))
        .map(|(start, end)| line_i64(start, end))
        .collect()
}

/// Add the grid layers to the map.
async fn add_grid_layers(map: &Map) {
    let all_highways = crate::road_grid::get_all_ways().await;
    let mut segment_pairs = Vec::new();
    for way in &all_highways {
        if let Some(name) = way.property("name")
            && name == "Oxford Terrace"
        {
            if let geojson::Value::LineString(segment) = way.geometry.clone().unwrap().value {
                segment_pairs.extend(get_segment_pairs(&segment));
            } else {
                log::error!("Not a line string");
            }
        }
    }

    let diagram = Builder::<i64>::default()
        .with_segments(segment_pairs)
        .unwrap()
        .build()
        .unwrap();
    let mut polygons = Vec::new();
    for cell in diagram.cells() {
        // combine continue/filter & map into a single iter operation?
        if diagram
            .cell_edge_iterator(cell.id())
            .any(|edge_index| diagram.edge(edge_index).unwrap().vertex0().is_none())
        {
            continue;
        }
        let cell_coords: Vec<_> = diagram
            .cell_edge_iterator(cell.id())
            .map(|edge_index| {
                let edge = diagram.edge(edge_index).unwrap();
                let vertex_index = edge.vertex0().expect("filtered out None above");
                let vertex = diagram.vertex(vertex_index).unwrap();
                vec![vertex.x() / SCALING_FACTOR, vertex.y() / SCALING_FACTOR]
            })
            .collect();
        polygons.push(cell_coords);
    }
    map.add_geojson_source(
        "polygons",
        GeoJson::Feature(Feature {
            geometry: Some(geojson::Geometry::new(geojson::Value::MultiLineString(
                polygons,
            ))),
            ..Default::default()
        }),
    )
    .unwrap();
    map.add_geojson_source("all-highways", GeoJson::FeatureCollection(all_highways))
        .unwrap();
    let intersections = crate::road_grid::get_intersections().await;
    map.add_geojson_source("intersections", GeoJson::FeatureCollection(intersections))
        .unwrap();
}

/// Ids of the layers that make up the debug road-grid overlay.
const GRID_LAYER_IDS: [&str; 3] = ["polygons", "all-highways", "intersections"];

/// Add the styled grid overlay layers, assuming their sources already exist.
/// Each layer is skipped if it is already present, so this doubles as the
/// "switch the overlay back on" path after it has been hidden.
fn add_grid_layer_styles(map: &Map) {
    if map.get_geojson_source("polygons").is_some() && map.get_layer("polygons").is_err() {
        let mut lines = LineLayer::new("polygons", "polygons");
        lines.layout.line_join = Some(LineJoin::Round.into());
        lines.layout.line_cap = Some(LineCap::Round.into());
        lines.paint.line_color = Some(RUN_LINE_COLOR.into());
        lines.paint.line_width = Some(3.0.into());
        if let Err(err) = map.add_layer(lines, None) {
            log::error!("Failed to add polygons layer: {err:?}");
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
            if map.get_layer(id).is_ok()
                && let Err(err) = map.remove_layer(id)
            {
                log::error!("Failed to remove grid layer {id}: {err:?}");
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
