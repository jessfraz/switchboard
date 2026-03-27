use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::{
    commands::shared::{
        bundle_entries, concept_text, first_string, iso_on_or_after, normalize_match_text, open_patient_session,
        resolve_since_floor,
    },
    Result,
};

#[derive(Debug, Args)]
pub(crate) struct NotesCommand {
    #[command(subcommand)]
    pub(crate) command: NotesSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum NotesSubcommand {
    Search(NotesSearchArgs),
    Get(NotesGetArgs),
}

#[derive(Debug, Args)]
pub(crate) struct NotesSearchArgs {
    #[arg(long)]
    query: String,

    #[arg(long)]
    patient: Option<String>,

    #[arg(long)]
    since: Option<String>,

    #[arg(long, default_value_t = 20)]
    limit: usize,

    #[arg(long)]
    all_pages: bool,
}

#[derive(Debug, Args)]
pub(crate) struct NotesGetArgs {
    #[arg(value_name = "NOTE_ID")]
    note_id: String,

    #[arg(long)]
    patient: Option<String>,
}

pub(crate) fn run_notes(command: NotesSubcommand, context: &crate::state::ResolvedContext) -> Result<Value> {
    match command {
        NotesSubcommand::Search(args) => run_search(args, context),
        NotesSubcommand::Get(args) => run_get(args, context),
    }
}

fn run_search(args: NotesSearchArgs, context: &crate::state::ResolvedContext) -> Result<Value> {
    let session = open_patient_session(context, args.patient)?;
    let floor = resolve_since_floor(args.since.as_deref())?;
    let normalized_query = normalize_match_text(&args.query);
    let notes = session
        .search_resource(
            "DocumentReference",
            &[("_count".into(), args.limit.max(50).to_string())],
            args.all_pages,
        )?
        .map(|bundle| bundle_entries(&bundle))
        .unwrap_or_default();

    let mut matches = notes
        .into_iter()
        .filter_map(|resource| {
            let note = render_note_summary(&resource)?;
            let date = note.get("date").and_then(Value::as_str)?.to_owned();
            if floor.as_deref().is_some_and(|floor| !iso_on_or_after(&date, floor)) {
                return None;
            }
            if !normalize_match_text(&note_search_text(&note)).contains(&normalized_query) {
                return None;
            }

            Some(note)
        })
        .collect::<Vec<_>>();

    matches.sort_by(|left, right| {
        right
            .get("date")
            .and_then(Value::as_str)
            .cmp(&left.get("date").and_then(Value::as_str))
    });
    matches.truncate(args.limit);

    Ok(json!({
        "status": "ok",
        "patient_id": session.patient_id,
        "query": args.query,
        "notes": matches,
    }))
}

fn run_get(args: NotesGetArgs, context: &crate::state::ResolvedContext) -> Result<Value> {
    let session = open_patient_session(context, args.patient)?;
    let resource = session
        .read_resource("DocumentReference", &args.note_id)?
        .ok_or_else(|| {
            crate::Error::Arguments(format!(
                "note {:?} was not found or this endpoint does not expose DocumentReference read/search for it",
                args.note_id
            ))
        })?;
    let mut note = render_note_summary(&resource).ok_or_else(|| {
        crate::Error::Arguments(format!(
            "note {:?} did not contain enough metadata to render a useful summary",
            args.note_id
        ))
    })?;
    let hydrated_content = hydrate_note_content(&session, &resource);
    let aggregated_body = hydrated_content
        .iter()
        .filter_map(|entry| entry.get("body_text").and_then(Value::as_str))
        .filter(|body| !body.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    if let Some(note_object) = note.as_object_mut() {
        note_object.insert("content".into(), Value::Array(hydrated_content));
        note_object.insert(
            "body_text".into(),
            if aggregated_body.is_empty() {
                Value::Null
            } else {
                Value::String(aggregated_body)
            },
        );
    }

    Ok(json!({
        "status": "ok",
        "patient_id": session.patient_id,
        "note": note,
    }))
}

fn render_note_summary(resource: &Value) -> Option<Value> {
    let date = first_string(resource, &["/date", "/meta/lastUpdated"])?;
    Some(json!({
        "id": first_string(resource, &["/id"]),
        "date": date,
        "type": resource.pointer("/type").and_then(concept_text),
        "description": first_string(resource, &["/description"]),
        "author": note_author(resource),
        "content": note_content_metadata(resource),
    }))
}

fn note_author(resource: &Value) -> Option<String> {
    resource
        .get("author")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|author| author.get("display").and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn note_content_metadata(resource: &Value) -> Vec<Value> {
    resource
        .get("content")
        .and_then(Value::as_array)
        .map(|content| content.iter().map(render_content_metadata).collect::<Vec<_>>())
        .unwrap_or_default()
}

fn render_content_metadata(entry: &Value) -> Value {
    json!({
        "title": entry.pointer("/attachment/title").and_then(Value::as_str),
        "content_type": entry.pointer("/attachment/contentType").and_then(Value::as_str),
        "url": entry.pointer("/attachment/url").and_then(Value::as_str),
    })
}

fn note_search_text(note: &Value) -> String {
    let content_titles = note
        .get("content")
        .and_then(Value::as_array)
        .map(|content| {
            content
                .iter()
                .filter_map(|entry| entry.get("title").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();

    [
        note.get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        note.get("type").and_then(Value::as_str).unwrap_or_default().to_owned(),
        note.get("author")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        content_titles,
    ]
    .join(" ")
}

fn hydrate_note_content(session: &crate::commands::shared::PatientSession, resource: &Value) -> Vec<Value> {
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

fn hydrate_content_entry(session: &crate::commands::shared::PatientSession, entry: &Value) -> Value {
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
        Ok(bytes) => {
            AttachmentMaterialization::with_text(resolved_content_type, String::from_utf8_lossy(&bytes).into_owned())
        }
        Err(error) => AttachmentMaterialization::fetch_failed(resolved_content_type, error),
    }
}

fn materialize_remote_attachment(
    session: &crate::commands::shared::PatientSession,
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
    response: &crate::client::JsonResponse,
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
                    resolved_content_type,
                    String::from_utf8_lossy(&bytes).into_owned(),
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

    AttachmentMaterialization::with_text(resolved_content_type, body_text)
}

fn is_textual_content_type(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return true;
    };
    let normalized = content_type.to_ascii_lowercase();
    normalized.starts_with("text/")
        || normalized.contains("json")
        || normalized.contains("xml")
        || normalized.contains("html")
        || normalized.contains("rtf")
}

fn body_excerpt(body: &str) -> String {
    const MAX_CHARS: usize = 240;

    let mut excerpt = body
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_CHARS)
        .collect::<String>();
    if body.chars().count() > MAX_CHARS {
        excerpt.push_str("...");
    }
    excerpt
}

fn decode_base64(input: &str) -> std::result::Result<Vec<u8>, String> {
    let mut buffer = 0u32;
    let mut bits = 0u32;
    let mut output = Vec::new();

    for character in input.chars().filter(|character| !character.is_ascii_whitespace()) {
        if character == '=' {
            break;
        }

        let value = match character {
            'A'..='Z' => (character as u8) - b'A',
            'a'..='z' => (character as u8) - b'a' + 26,
            '0'..='9' => (character as u8) - b'0' + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            _ => {
                return Err(format!(
                    "attachment body included invalid base64 character {character:?}"
                ))
            }
        } as u32;

        buffer = (buffer << 6) | value;
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }

    Ok(output)
}
