use serde_json::{json, Value};

use super::{
    decode_base64,
    excerpt::body_excerpt,
    normalize::{is_textual_content_type, normalize_attachment_body_text},
};
use crate::{
    client::JsonResponse,
    commands::shared::{first_string, PatientSession},
};

pub(super) fn hydrate_note_content(session: &PatientSession, resource: &Value) -> Vec<Value> {
    resource
        .get("content")
        .and_then(Value::as_array)
        .map(|content| {
            content
                .iter()
                .map(|entry| hydrate_content_entry(session, entry))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn hydrate_content_entry(session: &PatientSession, entry: &Value) -> Value {
    let title = entry
        .pointer("/attachment/title")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let declared_content_type = entry
        .pointer("/attachment/contentType")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let url = entry
        .pointer("/attachment/url")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    let hydrated = if let Some(data) = entry.pointer("/attachment/data").and_then(Value::as_str) {
        materialize_inline_attachment(data, declared_content_type.as_deref())
    } else if let Some(url) = url.as_deref() {
        materialize_remote_attachment(session, url, declared_content_type.as_deref())
    } else {
        AttachmentMaterialization::unavailable(
            declared_content_type.clone(),
            "attachment did not include inline data or a retrievable url".into(),
        )
    };

    json!({
        "title": title,
        "content_type": hydrated
            .resolved_content_type
            .or(declared_content_type)
            .map(Value::String)
            .unwrap_or(Value::Null),
        "url": url,
        "body_text": hydrated.body_text.clone().map(Value::String).unwrap_or(Value::Null),
        "body_excerpt": hydrated
            .body_text
            .as_deref()
            .map(body_excerpt)
            .map(Value::String)
            .unwrap_or(Value::Null),
        "fetch_error": hydrated.fetch_error.map(Value::String).unwrap_or(Value::Null),
        "body_unavailable_reason": hydrated
            .body_unavailable_reason
            .map(Value::String)
            .unwrap_or(Value::Null),
    })
}

#[derive(Debug, Default)]
struct AttachmentMaterialization {
    resolved_content_type: Option<String>,
    body_text: Option<String>,
    fetch_error: Option<String>,
    body_unavailable_reason: Option<String>,
}

impl AttachmentMaterialization {
    fn with_text(content_type: Option<String>, body_text: String) -> Self {
        Self {
            resolved_content_type: content_type,
            body_text: Some(body_text),
            fetch_error: None,
            body_unavailable_reason: None,
        }
    }

    fn unavailable(content_type: Option<String>, reason: String) -> Self {
        Self {
            resolved_content_type: content_type,
            body_text: None,
            fetch_error: None,
            body_unavailable_reason: Some(reason),
        }
    }

    fn fetch_failed(content_type: Option<String>, reason: String) -> Self {
        Self {
            resolved_content_type: content_type,
            body_text: None,
            fetch_error: Some(reason),
            body_unavailable_reason: None,
        }
    }
}

fn materialize_inline_attachment(data: &str, declared_content_type: Option<&str>) -> AttachmentMaterialization {
    let resolved_content_type = declared_content_type.map(ToOwned::to_owned);
    if !is_textual_content_type(declared_content_type) {
        return AttachmentMaterialization::unavailable(
            resolved_content_type,
            format!(
                "attachment content type {} is not text-like enough to render cleanly",
                declared_content_type.unwrap_or("unknown")
            ),
        );
    }

    match decode_base64(data) {
        Ok(bytes) => AttachmentMaterialization::with_text(
            resolved_content_type.clone(),
            normalize_attachment_body_text(
                resolved_content_type.as_deref(),
                String::from_utf8_lossy(&bytes).into_owned(),
            ),
        ),
        Err(error) => AttachmentMaterialization::fetch_failed(resolved_content_type, error),
    }
}

fn materialize_remote_attachment(
    session: &PatientSession,
    url: &str,
    declared_content_type: Option<&str>,
) -> AttachmentMaterialization {
    if !is_textual_content_type(declared_content_type) {
        return AttachmentMaterialization::unavailable(
            declared_content_type.map(ToOwned::to_owned),
            format!(
                "attachment content type {} is not text-like enough to render cleanly",
                declared_content_type.unwrap_or("unknown")
            ),
        );
    }

    match session.fetch_url(url) {
        Ok(response) => {
            if response.status_code >= 400 {
                return AttachmentMaterialization::fetch_failed(
                    response
                        .content_type
                        .clone()
                        .or_else(|| declared_content_type.map(ToOwned::to_owned)),
                    format!("attachment fetch failed with HTTP {}", response.status_code),
                );
            }
            materialize_response_body(&response, declared_content_type)
        }
        Err(error) => {
            AttachmentMaterialization::fetch_failed(declared_content_type.map(ToOwned::to_owned), error.to_string())
        }
    }
}

fn materialize_response_body(
    response: &JsonResponse,
    declared_content_type: Option<&str>,
) -> AttachmentMaterialization {
    if response.body.get("resourceType").and_then(Value::as_str) == Some("Binary") {
        let binary_content_type = first_string(&response.body, &["/contentType"]);
        let resolved_content_type = binary_content_type
            .clone()
            .or_else(|| response.content_type.clone())
            .or_else(|| declared_content_type.map(ToOwned::to_owned));

        if !is_textual_content_type(resolved_content_type.as_deref()) {
            return AttachmentMaterialization::unavailable(
                resolved_content_type,
                "Binary attachment is not text-like enough to render cleanly".into(),
            );
        }

        if let Some(data) = response.body.get("data").and_then(Value::as_str) {
            return match decode_base64(data) {
                Ok(bytes) => AttachmentMaterialization::with_text(
                    resolved_content_type.clone(),
                    normalize_attachment_body_text(
                        resolved_content_type.as_deref(),
                        String::from_utf8_lossy(&bytes).into_owned(),
                    ),
                ),
                Err(error) => AttachmentMaterialization::fetch_failed(resolved_content_type, error),
            };
        }
    }

    let resolved_content_type = response
        .content_type
        .clone()
        .or_else(|| declared_content_type.map(ToOwned::to_owned));
    if !is_textual_content_type(resolved_content_type.as_deref()) {
        return AttachmentMaterialization::unavailable(
            resolved_content_type,
            "attachment response is not text-like enough to render cleanly".into(),
        );
    }

    let body_text = match &response.body {
        Value::String(text) => text.clone(),
        Value::Null => response.body_text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| response.body_text.clone()),
    };

    AttachmentMaterialization::with_text(
        resolved_content_type.clone(),
        normalize_attachment_body_text(resolved_content_type.as_deref(), body_text),
    )
}
