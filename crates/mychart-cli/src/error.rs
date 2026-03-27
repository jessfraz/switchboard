use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("MyChart API request failed with status {status_code}")]
    Api { status_code: u16, body: Value },
    #[error("{message}")]
    Auth { message: String, details: Value },
    #[error("{0}")]
    Arguments(String),
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    Http(String),
    #[error("{0}")]
    Io(String),
}

impl Error {
    pub(crate) fn render(&self, compact: bool) -> String {
        let value = match self {
            Self::Api { status_code, body } => json!({
                "status": "error",
                "kind": "api",
                "status_code": status_code,
                "body": body,
            }),
            Self::Auth { message, details } => json!({
                "status": "error",
                "kind": "auth",
                "message": message,
                "details": details,
            }),
            Self::Arguments(message) => json!({
                "status": "error",
                "kind": "arguments",
                "message": message,
            }),
            Self::Config(message) => json!({
                "status": "error",
                "kind": "config",
                "message": message,
            }),
            Self::Http(message) => json!({
                "status": "error",
                "kind": "http",
                "message": message,
            }),
            Self::Io(message) => json!({
                "status": "error",
                "kind": "io",
                "message": message,
            }),
        };

        crate::render_json(&value, compact)
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;
