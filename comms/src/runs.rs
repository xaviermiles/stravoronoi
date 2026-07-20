use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct RunResponse {
    pub strava_activity_id: i64,
    /// Name of the activity.
    pub name: String,
    /// The time at which the activity was started.
    pub start_date: DateTime<Utc>,
    /// The summary map returned from Strava, as a Google Encoded Polyline.
    pub summary_map: String,
}
