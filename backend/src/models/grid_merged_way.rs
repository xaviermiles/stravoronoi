/// A contiguous way without intersections on the road grid.
use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "grid_merged_way")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub way_id: i64,
    /// The geometry of the merged way as a geojson string.
    pub geojson: String,
}

impl ActiveModelBehavior for ActiveModel {}
