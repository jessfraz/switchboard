//! The calling contract. No carrier, voice SDK, or subprocess types cross it.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::CallError;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct CallId(String);

impl CallId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for CallId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CallId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for CallId {
    type Err = CallError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value.to_owned())
    }
}

impl TryFrom<String> for CallId {
    type Error = CallError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > 80
            || !value.bytes().next().is_some_and(|byte| byte.is_ascii_alphanumeric())
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(CallError::InvalidRequest("call_id must start with a letter or digit and contain at most 80 ASCII letters, digits, underscores, or hyphens".into()));
        }
        Ok(Self(value))
    }
}

impl From<CallId> for String {
    fn from(value: CallId) -> Self {
        value.0
    }
}

/// Untrusted input is validated and explicitly authorized before backend use.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CallRequest {
    #[serde(default)]
    pub call_id: CallId,
    pub destination: String,
    pub task: String,
    pub caller_name: String,
    #[serde(default = "default_duration")]
    pub max_duration_seconds: u64,
}

fn default_duration() -> u64 {
    600
}

pub struct AuthorizedCall(CallRequest);

impl CallRequest {
    pub fn authorize(self, approved: bool) -> Result<AuthorizedCall, CallError> {
        if !approved {
            return Err(CallError::ApprovalRequired);
        }
        let digits = self
            .destination
            .strip_prefix('+')
            .ok_or_else(|| CallError::InvalidRequest("destination must use E.164 format".into()))?;
        if !(8..=15).contains(&digits.len())
            || !digits.bytes().all(|byte| byte.is_ascii_digit())
            || digits.starts_with('0')
        {
            return Err(CallError::InvalidRequest("destination must use E.164 format".into()));
        }
        if self.task.trim().is_empty() || self.task.len() > 8_000 || self.task.contains('\0') {
            return Err(CallError::InvalidRequest("task must contain 1 to 8000 bytes".into()));
        }
        if self.caller_name.trim().is_empty()
            || self.caller_name.len() > 80
            || self.caller_name.chars().any(|character| character < ' ')
        {
            return Err(CallError::InvalidRequest(
                "caller_name must contain 1 to 80 bytes".into(),
            ));
        }
        if !(30..=3600).contains(&self.max_duration_seconds) {
            return Err(CallError::InvalidRequest(
                "max_duration_seconds must be between 30 and 3600".into(),
            ));
        }
        Ok(AuthorizedCall(self))
    }
}

impl AuthorizedCall {
    pub fn request(&self) -> &CallRequest {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    Agent,
    Recipient,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TranscriptSegment {
    pub speaker: Speaker,
    pub text: String,
    pub timestamp_ms: u64,
    pub interrupted: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    Completed,
    Cancelled,
    Timeout,
    ApprovalRequired,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CallOutcome {
    pub reason: TerminationReason,
    pub remote_hangup_confirmed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CallEvent {
    Ready,
    Dialing,
    Connected,
    Transcript(TranscriptSegment),
    ApprovalRequired { reason: String },
    Completed(CallOutcome),
    Error { code: String, message: String },
}

/// A started call remains owned until it ends or the handle is dropped.
/// Dropping a handle must stop local processing; it never proves remote hangup.
pub trait ActiveCall {
    fn next_event(&mut self, timeout: Duration) -> Result<Option<CallEvent>, CallError>;
    fn cancel(&mut self) -> Result<(), CallError>;
}

pub trait CallBackend {
    fn start(&self, call: &AuthorizedCall) -> Result<Box<dyn ActiveCall>, CallError>;
}

#[cfg(test)]
mod tests {
    use crate::domain::{CallId, CallRequest};
    use crate::error::CallError;

    fn request() -> CallRequest {
        CallRequest {
            call_id: CallId::new(),
            destination: "+12125550100".into(),
            task: "Ask for opening hours".into(),
            caller_name: "Test".into(),
            max_duration_seconds: 30,
        }
    }

    #[test]
    fn approval_and_validated_destination_are_required_before_backend_use() {
        assert!(matches!(request().authorize(false), Err(CallError::ApprovalRequired)));
        for destination in ["123", "+01234567890", "+1234567", "+1;touch /tmp/x"] {
            let mut value = request();
            value.destination = destination.into();
            assert!(value.authorize(true).is_err());
        }
        assert!(request().authorize(true).is_ok());
    }
}
