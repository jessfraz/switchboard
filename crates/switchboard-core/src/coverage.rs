use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    Complete,
    Truncated,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReadCoverage {
    pub status: CoverageStatus,
    pub next_cursor: Option<String>,
}

impl ReadCoverage {
    pub fn page(next_cursor: Option<String>) -> Self {
        let next_cursor = next_cursor.filter(|cursor| !cursor.is_empty());
        Self {
            status: if next_cursor.is_some() {
                CoverageStatus::Truncated
            } else {
                CoverageStatus::Complete
            },
            next_cursor,
        }
    }
}

impl Default for ReadCoverage {
    fn default() -> Self {
        Self {
            status: CoverageStatus::Unknown,
            next_cursor: None,
        }
    }
}
