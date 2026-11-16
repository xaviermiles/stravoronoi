pub mod error;
pub mod segment_index;
pub mod snapper;
/// Handles snapping polylines to the road grid.
mod worker;

pub use segment_index::RoadSegmentIndex;
pub use worker::start;
