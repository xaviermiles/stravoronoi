/// Imports and processes the road grid.
use crate::models;
use crate::services::overpass::{self, OsmElement};
use sea_orm::ActiveValue::Set;
use sea_orm::prelude::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use std::collections::{HashMap, HashSet};

/// OSM coordinates are persisted as fixed-point integers scaled by 1e7 (E7),
/// the same convention OpenStreetMap uses. This keeps full precision without
/// storing floats.
pub const COORD_SCALE: f64 = 1e7;

/// Maximum rows per bulk insert, kept well under SQLite's bound-parameter limit.
const INSERT_CHUNK: usize = 300;

/// Fetch the Christchurch road network from Overpass and persist it as grid
/// nodes ([`models::grid_node`]) and ordered way-node segments
/// ([`models::grid_way`]).
async fn load(database: &DatabaseConnection) -> Result<(), String> {
    let response = overpass::get_overpass_data().await?;

    let mut nodes = Vec::new();
    let mut ways = Vec::new();
    let mut node_to_nodes: HashMap<i64, HashSet<i64>> = HashMap::new();
    for element in response.elements {
        match element {
            OsmElement::Node { id, lat, lon } => {
                nodes.push(models::grid_node::ActiveModel {
                    id: Set(id),
                    latitude: Set((lat * COORD_SCALE) as i32),
                    longitude: Set((lon * COORD_SCALE) as i32),
                    is_intersection: Set(false),
                });
            }
            OsmElement::Way {
                id,
                nodes: node_ids,
                tags,
            } => {
                // A way is an ordered list of node references: each consecutive
                // pair forms a segment of the polyline, captured here as one
                // (way_id, sequence, node_id) row.
                let tags = tags.unwrap_or_default();
                for (sequence, node_id) in node_ids.iter().enumerate() {
                    ways.push(models::grid_way::ActiveModel {
                        way_id: Set(id),
                        name: Set(tags.get("name").cloned()),
                        sequence: Set(sequence as i32),
                        node_id: Set(*node_id),
                    });
                }
                for (node_id1, node_id2) in node_ids.iter().zip(node_ids.iter().skip(1)) {
                    node_to_nodes
                        .entry(*node_id1)
                        .or_default()
                        .insert(*node_id2);
                    node_to_nodes
                        .entry(*node_id2)
                        .or_default()
                        .insert(*node_id1);
                }
            }
        }
    }

    tracing::info!(
        "Persisting {} grid nodes and {} way-node segments",
        nodes.len(),
        ways.len()
    );

    // Insert nodes first so the way-node references always resolve.
    for nodes_chunk in nodes.chunks(INSERT_CHUNK) {
        models::grid_node::Entity::insert_many(nodes_chunk.to_vec())
            .exec(database)
            .await
            .map_err(|err| format!("Failed to insert grid nodes: {err}"))?;
    }
    for ways_chunk in ways.chunks(INSERT_CHUNK) {
        models::grid_way::Entity::insert_many(ways_chunk.to_vec())
            .exec(database)
            .await
            .map_err(|err| format!("Failed to insert way nodes: {err}"))?;
    }

    let intersection_ids: Vec<_> = node_to_nodes
        .into_iter()
        .filter(|(_node, other_nodes)| other_nodes.len() > 2)
        .map(|(node, _other_nodes)| node)
        .collect();

    tracing::info!("Found {} intersection nodes", intersection_ids.len());

    for intersection_ids_chunk in intersection_ids.chunks(INSERT_CHUNK) {
        models::grid_node::Entity::update_many()
            .col_expr(models::grid_node::Column::IsIntersection, Expr::value(true))
            .filter(models::grid_node::Column::Id.is_in(intersection_ids_chunk.iter().copied()))
            .exec(database)
            .await
            .map_err(|err| format!("Failed to flag intersection nodes: {err}"))?;
    }

    Ok(())
}

/// Populate the road grid only when it is empty, so a cold database gets seeded
/// while repeat startups are cheap no-ops.
pub async fn seed(database: &DatabaseConnection) -> Result<(), String> {
    let existing_count = models::grid_node::Entity::find()
        .count(database)
        .await
        .map_err(|err| format!("Failed to count grid nodes: {err}"))?;
    if existing_count > 0 {
        tracing::info!("Skipping populating road grid as already filled.");
        return Ok(());
    }
    load(database).await
}
