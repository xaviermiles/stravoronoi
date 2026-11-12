use geo_types::Coord;
/// Bindings to get nodes & ways from the Overpass API.
use serde::Deserialize;
use std::collections::HashMap;

const OVERPASS_URL: &str = "https://overpass-api.de/api/interpreter";

/// Openstreetmap element. This is specific to this query.
#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum OsmElement {
    #[serde(rename = "node")]
    Node { id: i64, lat: f64, lon: f64 },
    #[serde(rename = "way")]
    Way {
        nodes: Vec<i64>,
        tags: Option<HashMap<String, String>>,
    },
}

/// Response from Overpass query. This is specific to this query.
#[derive(Deserialize, Debug)]
pub struct OverpassResponse {
    pub elements: Vec<OsmElement>,
}

/// Get all Christchurch ways and nodes from Overpass API.
pub async fn get_overpass_data(
    min_coordinate: Coord,
    max_coordinate: Coord,
) -> Result<OverpassResponse, String> {
    let coordinates = format!(
        "{}, {}, {}, {}",
        min_coordinate.y, min_coordinate.x, max_coordinate.y, max_coordinate.x
    );
    tracing::error!("{coordinates:?}");
    // Using all "road" types, from https://wiki.openstreetmap.org/wiki/Key:highway
    // + living_street (special road type)
    // Needs a big timeout otherwise will get 504 error responses.
    let query = format!(
        r#"[out:json][timeout:500];
    way[highway~"^(motorway|trunk|primary|secondary|tertiary|residential|unclassified|living_street)$"]({coordinates}) -> .filtered_ways;
    (
      .filtered_ways;
      node(w.filtered_ways);
    );
    out body;"#
    );

    // Set up the HTTP client with a standard User-Agent header (required by Overpass)
    let client = reqwest::Client::builder()
        .user_agent("stravoronoi/1.0")
        .build()
        .map_err(|err| format!("Failed to build client: {err}"))?;

    let response = client
        .post(OVERPASS_URL)
        .form(&[("data", query)])
        .send()
        .await
        .map_err(|err| format!("Failed to send request: {err}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "Overpass API returned an error status code: {status}"
        ));
    }

    let raw_json_data = response
        .text()
        .await
        .map_err(|err| format!("Failed to read response: {}", err))?;
    serde_json::from_str(&raw_json_data).map_err(|err| format!("Failed to parse response {err}"))
}
