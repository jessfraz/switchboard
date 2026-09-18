use std::borrow::Cow;

use serde::Serialize;

/// Serialize one response without changing its JSON shape.
pub fn to_json<T: Serialize + ?Sized>(value: &T, compact: bool) -> serde_json::Result<String> {
    if compact {
        serde_json::to_string(value)
    } else {
        serde_json::to_string_pretty(value)
    }
}

/// Stream a response while preserving serialization and writer errors.
pub fn write_json<T: Serialize + ?Sized>(
    writer: impl std::io::Write,
    value: &T,
    compact: bool,
) -> serde_json::Result<()> {
    if compact {
        serde_json::to_writer(writer, value)
    } else {
        serde_json::to_writer_pretty(writer, value)
    }
}

/// Render a response or a valid JSON error if its serializer fails.
pub fn render_json<T: Serialize + ?Sized>(value: &T, compact: bool) -> String {
    to_json(value, compact).unwrap_or_else(|error| {
        serde_json::to_string(&MessageErrorResponse::new("serialization", error.to_string())).unwrap_or_else(|_| {
            "{\"status\":\"error\",\"kind\":\"serialization\",\"message\":\"failed to serialize error payload\"}"
                .to_owned()
        })
    })
}

#[derive(Serialize)]
pub struct MessageErrorResponse<'a> {
    status: &'static str,
    kind: &'static str,
    message: Cow<'a, str>,
}

impl<'a> MessageErrorResponse<'a> {
    pub fn new(kind: &'static str, message: impl Into<Cow<'a, str>>) -> Self {
        Self {
            status: "error",
            kind,
            message: message.into(),
        }
    }
}

#[derive(Serialize)]
pub struct ApiErrorResponse<B> {
    status: &'static str,
    kind: &'static str,
    status_code: u16,
    body: B,
}

impl<B> ApiErrorResponse<B> {
    pub fn new(status_code: u16, body: B) -> Self {
        Self {
            status: "error",
            kind: "api",
            status_code,
            body,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Message {
        status: String,
        kind: String,
        message: String,
    }

    #[test]
    fn error_messages_round_trip_in_both_formats() {
        let message = "a quoted \"message\"\nwith a backslash \\ and Unicode 🦀";
        for compact in [false, true] {
            let rendered = render_json(&MessageErrorResponse::new("arguments", message), compact);
            assert_eq!(
                serde_json::from_str::<Message>(&rendered).expect("valid JSON error"),
                Message {
                    status: "error".into(),
                    kind: "arguments".into(),
                    message: message.into()
                }
            );
            assert_eq!(rendered.contains('\n'), !compact);
            let mut streamed = Vec::new();
            write_json(&mut streamed, &MessageErrorResponse::new("arguments", message), compact)
                .expect("stream JSON error");
            assert_eq!(streamed, rendered.as_bytes());
        }
    }

    #[test]
    fn invalid_json_map_keys_produce_a_serialization_error() {
        let invalid = std::collections::BTreeMap::from([(vec![1, 2], "value")]);
        for compact in [false, true] {
            let rendered = render_json(&invalid, compact);
            let error: Message = serde_json::from_str(&rendered).expect("valid JSON error");
            assert_eq!(error.status, "error");
            assert_eq!(error.kind, "serialization");
            assert_eq!(error.message, "key must be a string");
        }
    }
}
