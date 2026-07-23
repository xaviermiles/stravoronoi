use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct AthleteResponse {
    /// Athlete username.
    pub username: Option<String>,
    /// URL to the athlete's profile picture.
    pub profile_url: String,
}
