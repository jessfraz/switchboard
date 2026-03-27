use std::{
    error::Error as StdError,
    fmt::{self, Display},
};

use crate::types::ProviderKind;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Eq, PartialEq)]
pub enum Error {
    Audit(String),
    Config(String),
    Execution(String),
    Operation(String),
    MissingAuth(String),
    MissingSecret(String),
    AuthProviderMismatch {
        auth_ref: String,
        auth_provider: ProviderKind,
        namespace_provider: ProviderKind,
    },
    InvalidArguments(String),
    InvalidToolName(String),
    MissingAdapter(ProviderKind),
    PolicyDenied(String),
    ProviderMismatch {
        namespace: String,
        namespace_provider: ProviderKind,
        requested_provider: ProviderKind,
    },
    UnknownNamespace(String),
    UnsupportedOperation(String),
    UnsupportedTool(String),
    NotImplemented(String),
    SecretResolution {
        secret_ref: String,
        reason: String,
    },
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Audit(message) => write!(f, "audit failure: {message}"),
            Self::Config(message) => write!(f, "config error: {message}"),
            Self::Execution(message) => write!(f, "execution failure: {message}"),
            Self::Operation(message) => write!(f, "operation failure: {message}"),
            Self::MissingAuth(auth_ref) => write!(f, "missing auth configuration: {auth_ref}"),
            Self::MissingSecret(secret_ref) => write!(f, "missing secret configuration: {secret_ref}"),
            Self::AuthProviderMismatch {
                auth_ref,
                auth_provider,
                namespace_provider,
            } => write!(
                f,
                "auth ref {auth_ref} belongs to provider {auth_provider}, but the namespace expects {namespace_provider}"
            ),
            Self::InvalidArguments(message) => write!(f, "invalid arguments: {message}"),
            Self::InvalidToolName(tool) => write!(f, "invalid tool name: {tool}"),
            Self::MissingAdapter(provider) => write!(f, "missing adapter for provider: {provider}"),
            Self::PolicyDenied(reason) => write!(f, "policy denied request: {reason}"),
            Self::ProviderMismatch {
                namespace,
                namespace_provider,
                requested_provider,
            } => write!(
                f,
                "namespace {namespace} belongs to provider {namespace_provider}, requested tool targets {requested_provider}"
            ),
            Self::UnknownNamespace(namespace) => write!(f, "unknown namespace: {namespace}"),
            Self::UnsupportedOperation(message) => write!(f, "unsupported operation: {message}"),
            Self::UnsupportedTool(tool) => write!(f, "unsupported tool: {tool}"),
            Self::NotImplemented(message) => write!(f, "not implemented: {message}"),
            Self::SecretResolution { secret_ref, reason } => {
                write!(f, "failed to resolve secret {secret_ref}: {reason}")
            }
        }
    }
}

impl StdError for Error {}
