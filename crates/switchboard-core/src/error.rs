use std::error::Error as StdError;
use std::fmt::{self, Display};

use crate::types::ProviderKind;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Eq, PartialEq)]
pub enum Error {
    Audit(String),
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
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Audit(message) => write!(f, "audit failure: {message}"),
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
        }
    }
}

impl StdError for Error {}
