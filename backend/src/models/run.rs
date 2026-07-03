use sea_orm::entity::prelude::*;

// TODO: move fetching of runs into backend (using this) so that map matching can be cached.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "run")]
#[allow(dead_code)]
pub struct Model {
    #[sea_orm(primary_key)]
    pub strava_activity_id: i32,
    name: String,
    summary_map: Option<String>,
}

impl ActiveModelBehavior for ActiveModel {}
