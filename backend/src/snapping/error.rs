/// Error for snapping operations.

#[derive(Debug, Clone)]
pub enum SnapError {
    InsufficientPoints,
    NoRoadSegmentsLoaded,
    AllPointsOutOfRange,
}

impl std::fmt::Display for SnapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapError::InsufficientPoints => write!(f, "Less than 2 points means it isn't a line."),
            SnapError::NoRoadSegmentsLoaded => write!(f, "Road segment index is empty"),
            SnapError::AllPointsOutOfRange => {
                write!(f, "All points are too far from any road segment")
            }
        }
    }
}

impl std::error::Error for SnapError {}
