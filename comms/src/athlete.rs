use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct AthleteResponse {
    /// URL to the athlete's profile picture.
    pub profile_url: String,
}
