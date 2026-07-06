use geo_types::geometry::Coord;
use geojson::Value;
use serde::{self, Deserialize};

const MAPBOX_TOKEN: &str = env!("MAPBOX_TOKEN");
const MATCHING_URL: &str = "https://api.mapbox.com/matching/v5/mapbox/walking";
/// Mapbox Map Matching accepts at most 100 coordinates per request.
const MAX_MATCH_COORDS: usize = 100;

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

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("Map matching failed: {e}"))?;
    let matched: MatchResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse matching response: {e}"))?;
    if matched.code != "Ok" {
        return Err(format!("Matching code: {}", matched.code));
    }
    Ok(matched
        .matchings
        .into_iter()
        .next()
        .map(|m| as_line(&m.geometry))
        .unwrap_or_default())
}

/// Map-match a full run, splitting into overlapping 100-point chunks.
pub async fn map_match(coords: &[Vec<f64>]) -> Vec<Coord<f64>> {
    // if coords.len() < 2 {
    //     return coords.to_vec();
    // }
    let mut out: Vec<Coord<f64>> = Vec::new();
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
        out.extend(seg.into_iter().map(|p| Coord { x: p[0], y: p[1] }));

        if end == coords.len() {
            break;
        }
        start = end - 1; // overlap for continuity
    }
    out
}
