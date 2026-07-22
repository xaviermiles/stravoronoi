use sea_orm::EntityTrait;
use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "athlete")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub strava_id: i64,
    pub strava_username: Option<String>,
    /// URL to a 124x124 pixel profile picture.
    pub profile_url: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix timestamp (seconds) at which `access_token` expires.
    pub expires_at: i64,
}

impl ActiveModelBehavior for ActiveModel {}
