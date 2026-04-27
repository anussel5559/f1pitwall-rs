pub mod practice;
pub mod qualifying;
pub mod race;

/// Polling endpoint identifiers shared across session type modules.
#[derive(Debug, Clone, Copy)]
pub enum Endpoint {
    #[allow(dead_code)] // Used directly in bootstrap, not in polling cycles
    Drivers,
    Laps,
    Position,
    Intervals,
    Stints,
    PitStops,
    RaceControl,
    Weather,
}

impl Endpoint {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Drivers => "drivers",
            Self::Laps => "laps",
            Self::Position => "position",
            Self::Intervals => "intervals",
            Self::Stints => "stints",
            Self::PitStops => "pit_stops",
            Self::RaceControl => "race_control",
            Self::Weather => "weather",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub enum SessionType {
    Race,
    Sprint,
    Qualifying,
    SprintQualifying,
    Practice,
}

impl SessionType {
    /// Parse from OpenF1 API session_type string.
    pub fn from_api_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "race" => Some(Self::Race),
            "sprint" => Some(Self::Sprint),
            "qualifying" => Some(Self::Qualifying),
            "sprint_qualifying" | "sprint_shootout" | "sprint qualifying" | "sprint shootout" => {
                Some(Self::SprintQualifying)
            }
            "practice" => Some(Self::Practice),
            _ => None,
        }
    }

    /// Whether this session type is currently supported for viewing.
    pub fn is_supported(&self) -> bool {
        matches!(
            self,
            Self::Race | Self::Sprint | Self::Qualifying | Self::SprintQualifying | Self::Practice
        )
    }
}
