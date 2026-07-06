use sea_orm::entity::prelude::*;

/// An opaque, server-side session. The `session_id` is a random bearer token
/// handed to the frontend; it maps back to the authenticated athlete.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "session")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub session_id: String,
    pub athlete_id: i64,
    /// Unix timestamp (seconds) at which this session expires.
    pub expires_at: i64,
}

impl ActiveModelBehavior for ActiveModel {}
