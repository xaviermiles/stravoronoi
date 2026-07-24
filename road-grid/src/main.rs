use geojson::feature::Id;
use geojson::{Feature, FeatureCollection, Geometry, PointType, Value};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::hash::Hash;
use std::io::Write;

const CACHE_FILE: &str = "road-grid/christchurch_highways.json";
const OVERPASS_URL: &str = "https://overpass-api.de/api/interpreter";

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum OsmElement {
    #[serde(rename = "node")]
    Node { id: u64, lat: f64, lon: f64 },
    #[serde(rename = "way")]
    Way {
        id: u64,
        nodes: Vec<u64>,
        tags: Option<HashMap<String, String>>,
    },
}

#[derive(Deserialize, Debug)]
struct OverpassResponse {
    elements: Vec<OsmElement>,
}

struct RoadMeta {
    node_ids: Vec<u64>,
    name: Option<String>,
    ref_code: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let raw_json_data = get_overpass_data()?;

    let data: OverpassResponse = serde_json::from_str(&raw_json_data)?;

    let mut node_to_ways: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut way_metadata: HashMap<u64, RoadMeta> = HashMap::new();
    let mut nodes_geo: HashMap<u64, (f64, f64)> = HashMap::new();

    // Parse elements into fast, in-memory topological structures
    println!("Indexing OSM elements into topology hashes...");
    for element in data.elements {
        match element {
            OsmElement::Node { id, lat, lon } => {
                nodes_geo.insert(id, (lat, lon));
            }
            OsmElement::Way { id, nodes, tags } => {
                let tags = tags.unwrap_or_default();
                let name = tags.get("name").cloned();
                let ref_code = tags.get("ref").cloned();

                // TODO: lazy clone?
                for node_id in nodes.clone() {
                    node_to_ways.entry(node_id).or_default().push(id);
                }

                way_metadata.insert(
                    id,
                    RoadMeta {
                        node_ids: nodes,
                        name,
                        ref_code,
                    },
                );
            }
        }
    }

    // Expand ways into their coordinates and export as GeoJSON LineStrings.
    let ways_features: Vec<Feature> = way_metadata
        .iter()
        .map(|(id, meta)| {
            let coordinates: Vec<[f64; 2]> = meta
                .node_ids
                .iter()
                .filter_map(|node_id| nodes_geo.get(node_id))
                // GeoJSON uses [longitude, latitude] order
                .map(|&(lat, lon)| [lon, lat])
                .collect();
            Feature {
                id: Some((*id).into()),
                geometry: LineString { coordinates },
                properties: GeoJsonLineProperties {
                    way_id: *id,
                    name: meta.name.clone(),
                    ref_code: meta.ref_code.clone(),
                },
            }
        })
        .collect();
    let ways_collection = FeatureCollection {
        bbox: None,
        features: ways_features,
        foreign_members: None,
    };
    let ways_file = File::create("christchurch_ways.geojson")?;
    serde_json::to_writer_pretty(ways_file, &ways_collection)?;

    // Process every node to filter out straight continuations of the same road
    println!("Evaluating intersections...");
    let mut features = Vec::new();

    for (node_id, way_ids) in node_to_ways {
        // A node must join at least two ways to be considered
        if way_ids.len() < 2 {
            continue;
        }

        let mut unique_names = HashSet::new();
        let mut unique_refs = HashSet::new();
        let mut unnamed_count = 0;

        for way_id in &way_ids {
            if let Some(meta) = way_metadata.get(way_id) {
                match &meta.name {
                    Some(name) => {
                        unique_names.insert(name.clone());
                    }
                    None => unnamed_count += 1,
                }
                if let Some(ref_code) = &meta.ref_code {
                    unique_refs.insert(ref_code.clone());
                }
            }
        }

        // Logic check:
        // - More than 1 distinct name or reference number
        // - Or a mixture of named segments and completely unnamed roads
        // - Or multiple distinct unnamed segments intersecting
        let is_true_intersection = unique_names.len() > 1
            || unique_refs.len() > 1
            || (unnamed_count > 0 && !unique_names.is_empty())
            || (unnamed_count > 1 && way_ids.len() > unnamed_count);

        if is_true_intersection {
            if let Some(&(lat, lon)) = nodes_geo.get(&node_id) {
                // let mut properties = HashMap::new();
                // properties.insert("node_id", node_id);
                // properties.insert("connected_ways", way_ids.len());

                let mut properties = serde_json::Map::new();
                properties.insert(
                    "node_id"
                    serde_json::Value::String(node_id),
                );
                properties.insert("connected_ways", way_ids.len());
                features.push(Feature {
                    geometry: Some(Geometry::new(Value::Point(vec![lon, lat]))),
                    properties: Some(properties),
                });
            }
        }
    }

    // Construct and save the final GeoJSON output
    let output_collection = FeatureCollection {
        bbox: None,
        features,
        foreign_members: None,
    };

    println!(
        "Writing {} filtered intersections to output file...",
        output_collection.features.len()
    );
    let intersections_file = File::create("christchurch_intersections.geojson")?;
    serde_json::to_writer_pretty(intersections_file, &output_collection)?;

    println!("Success! File written to 'christchurch_intersections.geojson'.");
    Ok(())
}

fn get_overpass_data() -> Result<String, Box<dyn std::error::Error>> {
    if let Ok(contents) = std::fs::read_to_string(CACHE_FILE) {
        println!("Found local cache file '{}'. Loading data...", CACHE_FILE);
        return Ok(contents);
    }

    println!("Cache file not found. Fetching raw data from Overpass API...");
    // Using all "road" types, from https://wiki.openstreetmap.org/wiki/Key:highway
    // + living_street (special road type)
    // Using raw coordinates for Christchurch city for simplicity for now.
    // Needs a big timeout otherwise will get 504 error responses.
    let query = r#"[out:json][timeout:500];
    way[highway~"^(motorway|trunk|primary|secondary|tertiary|residential|unclassified|living_street)$"](-43.60, 172.50, -43.45, 172.75) -> .filtered_ways;
    (
      .filtered_ways;
      node(w.filtered_ways);
    );
    out body;"#;

    // Set up the HTTP client with a standard User-Agent header (required by Overpass)
    let client = reqwest::blocking::Client::builder()
        .user_agent("RustOverpassDownloader/1.0 (mapping project)")
        .build()?;

    let response = client.post(OVERPASS_URL).form(&[("data", query)]).send()?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("Overpass API returned an error status code: {}", status).into());
    }

    let body_text = response.text()?;

    // Cache the downloaded response to a local file for future runs
    println!("Caching download to local disk as '{}'...", CACHE_FILE);
    let mut file = File::create(CACHE_FILE)?;
    file.write_all(body_text.as_bytes())?;

    Ok(body_text)
}
