use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "snapped_run")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub strava_activity_id: i64,
    /// The snapped run. This will be None if the run can't be snapped.
    pub polyline: Option<String>,
    /// When the run was snapped.
    pub processed_time: ChronoUnixTimestamp,
}

impl ActiveModelBehavior for ActiveModel {}
