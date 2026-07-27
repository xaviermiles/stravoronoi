use crate::strava::{self, LoadState};
use boostvoronoi::prelude::*;
use chrono::{DateTime, Utc};
use geojson::{Feature, FeatureCollection, GeoJson};
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

/// Strava's brand orange, used for all run lines.
const RUN_LINE_COLOR: &str = "#fc4c02";

// If the API returns nothing, avoid spamming the backend while it populates.
const SLOW_CONTINUE_TIME: Duration = Duration::from_secs(1);
const FAST_CONTINUE_TIME: Duration = Duration::from_millis(10);

// TODO: will be moved to backend eventually
static ALL_GRID_FILE: &str = include_str!("../../christchurch_ways.geojson");
static INTERSECTION_FILE: &str = include_str!("../../christchurch_intersections.geojson");

struct Listener {
    on_unauthorized: Callback<()>,
}

// #[derive(Deserialize, Debug)]
// struct OverpassResponse {
//     elements: Vec<OsmElement>,
// }

impl MapEventListener for Listener {
    fn on_load(&mut self, map: Rc<Map>, _e: event::MapBaseEvent) {
        // Draw the grid lines & intersections for debugging purposes.
        add_grid_layers(&map);
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

fn add_grid_layers(map: &Map) {
    let all_highways: FeatureCollection = ALL_GRID_FILE.parse().unwrap();
    let feature1 = &all_highways.features[0].geometry.clone().unwrap().value;
    log::warn!("{feature1:?}");
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
    log::error!("{polygons:?}");
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
    let mut lines = LineLayer::new("polygons", "polygons");
    lines.layout.line_join = Some(LineJoin::Round.into());
    lines.layout.line_cap = Some(LineCap::Round.into());
    lines.paint.line_color = Some(RUN_LINE_COLOR.into());
    lines.paint.line_width = Some(3.0.into());
    map.add_layer(lines, None).unwrap();

    map.add_geojson_source("all-highways", GeoJson::FeatureCollection(all_highways))
        .unwrap();
    let lines = LineLayer::new("all-highways", "all-highways");
    map.add_layer(lines, None).unwrap();

    let intersections: FeatureCollection = INTERSECTION_FILE.parse().unwrap();
    map.add_geojson_source("intersections", GeoJson::FeatureCollection(intersections))
        .unwrap();
    let circles = CircleLayer::new("intersections", "intersections");
    map.add_layer(circles, None).unwrap();
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
