/// Create and manage grid layers.
use geojson::GeoJson;
use mapboxgl::FillLayer;
use mapboxgl::Map;
use mapboxgl::layer::CircleLayer;
use mapboxgl::layer::{LineCap, LineJoin, LineLayer};

/// Ids of the layers that make up the debug road-grid overlay.
const GRID_LAYER_IDS: [&str; 4] = ["ways", "intersections", "cells-fill", "cells-outline"];

/// Add the styled grid overlay layers, assuming their sources already exist.
/// Each layer is skipped if it is already present, so this doubles as the
/// "switch the overlay back on" path after it has been hidden.
fn add_grid_layer_styles(map: &Map) {
    if map.get_geojson_source("cells").is_some() {
        if map.get_layer("cells-outline").is_err() {
            let mut fill = FillLayer::new("cells-fill", "cells");
            // This colour is borrowed to be visually distinct to the unselected run lines, so the
            // polygons are somewhat viewable at the same time as runs.
            fill.paint.fill_color = Some("rgba(0,96,208,0.5)".into());
            if let Err(err) = map.add_layer(fill, None) {
                log::error!("Failed to add cells layer: {err:?}");
            }
        }
        if map.get_layer("cells-outline").is_err() {
            let mut lines = LineLayer::new("cells-outline", "cells");
            lines.layout.line_join = Some(LineJoin::Round.into());
            lines.layout.line_cap = Some(LineCap::Round.into());
            lines.paint.line_color = Some("rgba(0,0,0,0.5)".into());
            // lines.paint.line_color = Some(CELL_OUTLINE_COLOUR.into());
            // lines.paint.line_opacity = Some(0.0.into());
            lines.paint.line_width = Some(3.0.into());
            if let Err(err) = map.add_layer(lines, None) {
                log::error!("Failed to add cells layer: {err:?}");
            }
        }
    }
    if map.get_geojson_source("ways").is_some() && map.get_layer("ways").is_err() {
        let lines = LineLayer::new("ways", "ways");
        if let Err(err) = map.add_layer(lines, None) {
            log::error!("Failed to add ways layer: {err:?}");
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

/// Add the grid layers to the map.
pub async fn add_grid_layers(map: &Map, is_grid_visible: bool) {
    let all_ways = crate::road_grid::get_all_ways().await;
    map.add_geojson_source("ways", GeoJson::FeatureCollection(all_ways))
        .unwrap();
    let intersections = crate::road_grid::get_intersections().await;
    map.add_geojson_source("intersections", GeoJson::FeatureCollection(intersections))
        .unwrap();
    let cells = crate::road_grid::get_voronoi_cells().await;
    map.add_geojson_source("cells", GeoJson::FeatureCollection(cells))
        .unwrap();

    set_grid_visible(&map, is_grid_visible);
}
