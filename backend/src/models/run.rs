use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "run")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub strava_activity_id: i64,
    /// Strava athelete ID.
    pub athlete_id: i64,
    /// Name of the activity.
    pub name: String,
    /// The activity's distance, in metres.
    pub distance: i64,
    /// The activity's moving time, in seconds.
    pub moving_time: i64,
    /// The datetime at which the activity was started.
    pub start_datetime: ChronoUnixTimestamp,
    /// The summary map returned from Strava, as a Google Encoded Polyline.
    pub summary_map: Option<String>,
    /// Whether this activity is the first run for this athlete.
    pub is_first_run: bool,
    #[sea_orm(has_one)]
    pub snapped_run: HasOne<super::snapped_run::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
