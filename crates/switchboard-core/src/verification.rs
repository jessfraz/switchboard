use serde::{Deserialize, Serialize};

use crate::ToolRef;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Verified,
    Mismatch,
    Unavailable,
}

/// A readback observation, separate from successful command execution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct VerificationReceipt {
    pub status: VerificationStatus,
    pub checked_at: u64,
    pub summary: String,
    pub refs: Vec<ToolRef>,
    /// Provider-supplied recovery metadata, accepted only after a verified readback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_effect: Option<crate::OperationEffect>,
}

impl VerificationReceipt {
    pub fn new(status: VerificationStatus, summary: impl Into<String>, refs: Vec<ToolRef>) -> Self {
        Self {
            status,
            checked_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            summary: summary.into(),
            refs,
            recovered_effect: None,
        }
    }

    pub fn unavailable(summary: impl Into<String>) -> Self {
        Self::new(VerificationStatus::Unavailable, summary, Vec::new())
    }
}
