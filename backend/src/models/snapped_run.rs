use sea_orm::EntityTrait;
use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "snapped_run")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub strava_activity_id: i64,
    /// The snapped run. This will be None if the run can't be snapped.
    pub polyline: Option<String>,
    /// When the run was snapped.
    pub processed_time: ChronoUnixTimestamp,
    #[sea_orm(belongs_to, from = "strava_activity_id", to = "strava_activity_id")]
    pub run: HasOne<super::run::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
