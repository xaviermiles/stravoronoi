/// A way on the road grid.
use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "grid_cell")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub cell_id: u32,
    /// The geometry of the voronoi cell as a geojson string.
    pub geojson: String,
}

impl ActiveModelBehavior for ActiveModel {}
