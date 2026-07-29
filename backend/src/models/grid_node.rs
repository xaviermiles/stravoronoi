/// A node on the road grid.
use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "grid_node")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    /// Latitude (scaled e7).
    pub latitude: i32,
    /// Longitude (scaled e7).
    pub longitude: i32,
    /// Whether this node is where three or more road segments meet.
    pub is_intersection: bool,
}

impl ActiveModelBehavior for ActiveModel {}
