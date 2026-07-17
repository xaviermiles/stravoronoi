use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "run")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub strava_activity_id: i64,
    /// Name of the activity.
    pub name: String,
    /// The summary map returned from Strava, as a Google Encoded Polyline.
    pub summary_map: Option<String>,
    /// Whether this activity is the final activity for this athlete.
    pub is_final_activity: bool,
}

impl ActiveModelBehavior for ActiveModel {}
