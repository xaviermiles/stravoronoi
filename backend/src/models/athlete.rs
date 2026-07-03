use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "athlete")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub strava_athlete_id: i32,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix timestamp (seconds) at which `access_token` expires.
    pub expires_at: i64,
}

impl ActiveModelBehavior for ActiveModel {}
