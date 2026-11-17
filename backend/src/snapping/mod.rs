/// Handles snapping polylines to the road grid.
mod segment_index;
mod snapper;
mod worker;

pub use segment_index::RoadSegmentIndex;
pub use worker::start;
