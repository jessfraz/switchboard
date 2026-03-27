use anyhow::Error as AnyhowError;
use serde::Serialize;
use serde_json::Value;

use crate::Error;

#[derive(Serialize)]
struct MessageErrorResponse<'a> {
    status: &'a str,
    kind: &'a str,
    message: &'a str,
}

#[derive(Serialize)]
struct ApiErrorResponse<'a> {
    status: &'a str,
    kind: &'a str,
    status_code: u16,
    body: &'a Value,
}

#[derive(Serialize)]
struct OwnedMessageErrorResponse<'a> {
    status: &'a str,
    kind: &'a str,
    message: String,
}

pub(crate) fn render_json<T>(value: &T, compact: bool) -> String
where
    T: Serialize,
{
    let rendered = if compact {
        serde_json::to_string(value)
    } else {
        serde_json::to_string_pretty(value)
    };

    match rendered {
        Ok(text) => text,
        Err(error) => {
            panic!("failed to serialize output as JSON: {error}");
        }
    }
}

pub(crate) fn render_cli_error(error: &AnyhowError, compact: bool) -> String {
    if let Some(error) = error.chain().find_map(|cause| cause.downcast_ref::<Error>()) {
        return error.render(compact);
    }

    render_json(
        &OwnedMessageErrorResponse {
            status: "error",
            kind: "internal",
            message: error.to_string(),
        },
        compact,
    )
}

pub(crate) fn render_domain_error(error: &Error, compact: bool) -> String {
    match error {
        Error::Api { status_code, body } => render_json(
            &ApiErrorResponse {
                status: "error",
                kind: "api",
                status_code: *status_code,
                body,
            },
            compact,
        ),
        Error::Auth { message, details } => render_json(
            &serde_json::json!({
                "status": "error",
                "kind": "auth",
                "message": message,
                "details": details,
            }),
            compact,
        ),
        Error::Arguments(message) => render_json(
            &MessageErrorResponse {
                status: "error",
                kind: "arguments",
                message,
            },
            compact,
        ),
        Error::Config(message) => render_json(
            &MessageErrorResponse {
                status: "error",
                kind: "config",
                message,
            },
            compact,
        ),
        Error::Http(message) => render_json(
            &MessageErrorResponse {
                status: "error",
                kind: "http",
                message,
            },
            compact,
        ),
        Error::Io(message) => render_json(
            &MessageErrorResponse {
                status: "error",
                kind: "io",
                message,
            },
            compact,
        ),
    }
}
