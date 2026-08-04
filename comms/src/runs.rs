use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct RunResponse {
    pub strava_activity_id: i64,
    /// Name of the activity.
    pub name: String,
    /// The activity's distance, in metres.
    pub distance: i64,
    /// The activity's moving time, in seconds.
    pub moving_time: i64,
    /// The time at which the activity was started.
    pub start_date: DateTime<Utc>,
    /// The summary map returned from Strava, as a Google Encoded Polyline.
    pub summary_map: String,
}
