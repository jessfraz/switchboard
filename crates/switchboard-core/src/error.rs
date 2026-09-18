use crate::types::{AuditEventId, OperationId, ProviderKind, ToolName};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum Error {
    #[error("audit failure: {0}")]
    Audit(String),
    #[error("config error: {0}")]
    Config(String),
    #[error("execution failure: {0}")]
    Execution(String),
    #[error("provider CLI could not start: {0}")]
    Launch(String),
    #[error("provider asked to retry after {retry_after_seconds} seconds: {reason}")]
    RateLimited { retry_after_seconds: u64, reason: String },
    #[error("provider rejected authentication: {reason}")]
    AuthenticationRejected { reason: String },
    #[error("authenticated account {actual:?} does not match configured account {expected:?}")]
    AccountMismatch { expected: String, actual: String },
    #[error(
        "authentication timed out after at most {seconds} seconds; inspect the authentication blocker before retrying"
    )]
    AuthenticationTimeout { seconds: u64 },
    #[error("authentication requires browser consent: {reason}")]
    BrowserConsentRequired { reason: String },
    #[error("provider {program} failed (exit {exit_code:?}): {reason}")]
    ProviderFailed {
        program: String,
        exit_code: Option<i32>,
        reason: String,
    },
    #[error("{program} timed out after {seconds} seconds; the remote outcome may be unknown")]
    TimedOut { program: String, seconds: u64 },
    #[error("operation {operation_id} has an unknown remote outcome: {reason}; run op verify before retrying")]
    OutcomeUnknown { operation_id: OperationId, reason: String },
    #[error("authentication recovery exhausted: {0}")]
    RecoveryExhausted(String),
    #[error("operation failure: {0}")]
    Operation(String),
    #[error("missing auth configuration: {0}")]
    MissingAuth(String),
    #[error("missing secret configuration: {0}")]
    MissingSecret(String),
    #[error("auth ref {auth_ref} belongs to provider {auth_provider}, but the namespace expects {namespace_provider}")]
    AuthProviderMismatch {
        auth_ref: String,
        auth_provider: ProviderKind,
        namespace_provider: ProviderKind,
    },
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("invalid tool name: {0}")]
    InvalidToolName(String),
    #[error("missing adapter for provider: {0}")]
    MissingAdapter(ProviderKind),
    #[error("policy denied request: {0}")]
    PolicyDenied(String),
    #[error(
        "namespace {namespace} belongs to provider {namespace_provider}, requested tool targets {requested_provider}"
    )]
    ProviderMismatch {
        namespace: String,
        namespace_provider: ProviderKind,
        requested_provider: ProviderKind,
    },
    #[error("unknown namespace: {0}")]
    UnknownNamespace(String),
    #[error("unknown audit event: {0}")]
    UnknownAuditEvent(AuditEventId),
    #[error("operation {0} is not undoable")]
    OperationNotUndoable(OperationId),
    #[error("aggregate reads require a read tool, got {0}")]
    AggregateReadRequiresReadTool(ToolName),
    #[error("undo is not supported for tool: {0}")]
    UndoUnsupported(ToolName),
    #[error("unsupported operation: {0}")]
    UnsupportedOperation(String),
    #[error("unsupported tool: {0}")]
    UnsupportedTool(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error("failed to resolve secret {secret_ref}: {reason}")]
    SecretResolution { secret_ref: String, reason: String },
}
