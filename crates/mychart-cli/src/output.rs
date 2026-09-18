use anyhow::Error as AnyhowError;
use serde::Serialize;
use serde_json::Value;

use switchboard_cli_support::output::{render_json, ApiErrorResponse, MessageErrorResponse};

use crate::Error;

#[derive(Serialize)]
struct AuthErrorResponse<'a> {
    details: &'a Value,
    kind: &'static str,
    message: &'a str,
    status: &'static str,
}

pub(crate) fn render_cli_error(error: &AnyhowError, compact: bool) -> String {
    if let Some(error) = error.chain().find_map(|cause| cause.downcast_ref::<Error>()) {
        return error.render(compact);
    }

    render_json(&MessageErrorResponse::new("internal", error.to_string()), compact)
}

pub(crate) fn render_domain_error(error: &Error, compact: bool) -> String {
    match error {
        Error::Api { status_code, body } => render_json(&ApiErrorResponse::new(*status_code, body), compact),
        Error::Auth { message, details } => render_json(
            &AuthErrorResponse {
                details,
                kind: "auth",
                message,
                status: "error",
            },
            compact,
        ),
        Error::Arguments(message) => render_json(&MessageErrorResponse::new("arguments", message), compact),
        Error::Config(message) => render_json(&MessageErrorResponse::new("config", message), compact),
        Error::Http(message) => render_json(&MessageErrorResponse::new("http", message), compact),
        Error::Io(message) => render_json(&MessageErrorResponse::new("io", message), compact),
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[test]
    fn serialization_failure_returns_a_json_error() {
        #[derive(Deserialize)]
        struct SerializationError {
            status: String,
            kind: String,
            message: String,
        }
        let invalid = std::collections::BTreeMap::from([(vec![1, 2], "value")]);
        let output = render_json(&invalid, true);
        let error: SerializationError = serde_json::from_str(&output).expect("valid JSON error");
        assert_eq!(error.status, "error");
        assert_eq!(error.kind, "serialization");
        assert_eq!(error.message, "key must be a string");
    }
}
