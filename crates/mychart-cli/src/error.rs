use serde_json::Value;

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
        crate::output::render_domain_error(self, compact)
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;
