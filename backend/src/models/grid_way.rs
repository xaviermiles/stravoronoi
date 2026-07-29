/// A way on the road grid.
use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "grid_way")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub way_id: i64,
    /// Name of the way.
    ///
    /// This is not (necessarily) unique since a single street/road may be split into multiple
    /// ways.
    pub name: Option<String>,
    /// Ordering along the polyline.
    #[sea_orm(primary_key)]
    pub sequence: i32,
    pub node_id: i64,
    #[sea_orm(belongs_to, from = "node_id", to = "id")]
    pub node: HasOne<super::grid_node::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
