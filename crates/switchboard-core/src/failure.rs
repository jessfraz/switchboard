use serde::{Deserialize, Serialize};

use crate::{Error, NamespaceId, OperationId, ProviderKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    InvalidArguments,
    Configuration,
    Authentication,
    WrongAccount,
    AuthenticationTimeout,
    BrowserConsentRequired,
    RecoveryExhausted,
    PolicyDenied,
    Unsupported,
    ProviderFailed,
    Timeout,
    Storage,
    OutcomeUnknown,
    Internal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailurePhase {
    Validation,
    Authentication,
    Execution,
    Persistence,
    Verification,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecoveryAction {
    InspectConfiguration,
    CheckAuthentication,
    InspectProvider,
    DescribeTool,
    VerifyOperation { operation_id: OperationId },
}

/// Stable failure details. Messages are for people; callers branch on `code`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub code: FailureCode,
    pub phase: FailurePhase,
    pub message: String,
    pub provider: Option<ProviderKind>,
    pub namespace: Option<NamespaceId>,
    pub retryable: bool,
    pub next_action: Option<RecoveryAction>,
    pub exit_code: Option<i32>,
    pub retry_after_seconds: Option<u64>,
}

impl Failure {
    pub fn from_error(error: &Error) -> Self {
        let (code, phase, next_action) = match error {
            Error::InvalidArguments(_) | Error::InvalidToolName(_) | Error::AggregateReadRequiresReadTool(_) => {
                (FailureCode::InvalidArguments, FailurePhase::Validation, None)
            }
            Error::SecretResolution { .. }
            | Error::MissingAuth(_)
            | Error::MissingSecret(_)
            | Error::AuthenticationRejected { .. } => (
                FailureCode::Authentication,
                FailurePhase::Authentication,
                Some(RecoveryAction::CheckAuthentication),
            ),
            Error::AccountMismatch { .. } => (
                FailureCode::WrongAccount,
                FailurePhase::Authentication,
                Some(RecoveryAction::CheckAuthentication),
            ),
            Error::AuthenticationTimeout { .. } => (
                FailureCode::AuthenticationTimeout,
                FailurePhase::Authentication,
                Some(RecoveryAction::CheckAuthentication),
            ),
            Error::BrowserConsentRequired { .. } => (
                FailureCode::BrowserConsentRequired,
                FailurePhase::Authentication,
                Some(RecoveryAction::CheckAuthentication),
            ),
            Error::RecoveryExhausted(_) => (
                FailureCode::RecoveryExhausted,
                FailurePhase::Authentication,
                Some(RecoveryAction::CheckAuthentication),
            ),
            Error::PolicyDenied(_) => (FailureCode::PolicyDenied, FailurePhase::Validation, None),
            Error::UnsupportedTool(_)
            | Error::NotImplemented(_)
            | Error::UnsupportedOperation(_)
            | Error::MissingAdapter(_)
            | Error::UndoUnsupported(_)
            | Error::OperationNotUndoable(_) => (
                FailureCode::Unsupported,
                FailurePhase::Validation,
                Some(RecoveryAction::DescribeTool),
            ),
            Error::TimedOut { .. } => (
                FailureCode::Timeout,
                FailurePhase::Execution,
                Some(RecoveryAction::InspectProvider),
            ),
            Error::OutcomeUnknown { operation_id, .. } => (
                FailureCode::OutcomeUnknown,
                FailurePhase::Verification,
                Some(RecoveryAction::VerifyOperation {
                    operation_id: operation_id.clone(),
                }),
            ),
            Error::Execution(_)
            | Error::ProviderFailed { .. }
            | Error::ProviderRejected { .. }
            | Error::RateLimited { .. } => (
                FailureCode::ProviderFailed,
                FailurePhase::Execution,
                Some(RecoveryAction::InspectProvider),
            ),
            Error::Audit(_) | Error::Operation(_) | Error::UnknownAuditEvent(_) => {
                (FailureCode::Storage, FailurePhase::Persistence, None)
            }
            Error::Config(_)
            | Error::Launch(_)
            | Error::UnknownNamespace(_)
            | Error::AuthProviderMismatch { .. }
            | Error::ProviderMismatch { .. } => (
                FailureCode::Configuration,
                FailurePhase::Validation,
                Some(RecoveryAction::InspectConfiguration),
            ),
        };
        Self {
            code,
            phase,
            message: error.to_string(),
            provider: None,
            namespace: None,
            // Execution failure does not prove that a write did not happen.
            retryable: matches!(error, Error::RateLimited { .. }),
            next_action,
            exit_code: match error {
                Error::ProviderFailed { exit_code, .. } => *exit_code,
                _ => None,
            },
            retry_after_seconds: match error {
                Error::RateLimited {
                    retry_after_seconds, ..
                } => Some(*retry_after_seconds),
                _ => None,
            },
        }
    }

    pub fn with_namespace(mut self, namespace: NamespaceId) -> Self {
        self.provider = ProviderKind::from_tool_name(namespace.as_str());
        self.namespace = Some(namespace);
        self
    }
}
