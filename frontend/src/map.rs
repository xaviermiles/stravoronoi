use crate::strava;
use geojson::Value;
use gloo_net::http::Request;
use mapboxgl::layer::{LineCap, LineJoin, LineLayer};
use mapboxgl::{LngLat, Map, MapEventListener, MapOptions, event};
use serde::{self, Deserialize};
use std::{cell::RefCell, rc::Rc};
use yew::prelude::*;
use yew::{use_effect_with_deps, use_mut_ref};

const MAPBOX_TOKEN: &str = env!("MAPBOX_TOKEN");
const MATCHING_URL: &str = "https://api.mapbox.com/matching/v5/mapbox/walking";
/// Mapbox Map Matching accepts at most 100 coordinates per request.
const MAX_MATCH_COORDS: usize = 100;

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
    log::info!("{MAPBOX_TOKEN}");
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

#[derive(Deserialize)]
struct MatchResponse {
    #[serde(default)]
    code: String,
    #[serde(default)]
    matchings: Vec<Matching>,
}

#[derive(Deserialize)]
struct Matching {
    geometry: geojson::Geometry, // geometries=geojson => a LineString
}

fn as_line(geom: &geojson::Geometry) -> Vec<Vec<f64>> {
    match &geom.value {
        Value::LineString(coords) => coords.clone(),
        _ => Vec::new(),
    }
}

/// Snap up to 100 points to the road/path network.
async fn match_chunk(coords: &[Vec<f64>]) -> Result<Vec<Vec<f64>>, String> {
    let path = coords
        .iter()
        .map(|c| format!("{},{}", c[0], c[1])) // lng,lat
        .collect::<Vec<_>>()
        .join(";");
    // Per-point search radius (m). One value per coordinate is required.
    let radiuses = vec!["25"; coords.len()].join(";");

    let url = format!(
        "{MATCHING_URL}/{path}?geometries=geojson&overview=full&tidy=true\
         &radiuses={radiuses}&access_token={MAPBOX_TOKEN}"
    );

    let resp = Request::get(&url)
        .send()
        .await
        .map_err(|e| format!("map matching failed: {e}"))?;
    if !resp.ok() {
        return Err(format!("map matching returned HTTP {}", resp.status()));
    }
    let matched: MatchResponse = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse matching response: {e}"))?;
    if matched.code != "Ok" {
        return Err(format!("matching code: {}", matched.code));
    }
    Ok(matched
        .matchings
        .into_iter()
        .next()
        .map(|m| as_line(&m.geometry))
        .unwrap_or_default())
}

/// Map-match a full run, splitting into overlapping 100-point chunks.
pub async fn map_match(coords: &[Vec<f64>]) -> Vec<Vec<f64>> {
    if coords.len() < 2 {
        return coords.to_vec();
    }
    let mut out: Vec<Vec<f64>> = Vec::new();
    let mut start = 0;
    while start < coords.len() - 1 {
        let end = (start + MAX_MATCH_COORDS).min(coords.len());
        let chunk = &coords[start..end];

        let mut seg = match match_chunk(chunk).await {
            Ok(s) => s,
            Err(e) => {
                log::warn!("chunk match failed ({e}); using raw points");
                chunk.to_vec()
            }
        };
        // Chunks overlap by one input point; drop the seam vertex to reduce duplication.
        if !out.is_empty() && !seg.is_empty() {
            seg.remove(0);
        }
        out.append(&mut seg);

        if end == coords.len() {
            break;
        }
        start = end - 1; // overlap for continuity
    }
    out
}
