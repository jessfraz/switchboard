use clap::{Args, Subcommand};
use quick_xml::{escape::unescape, events::Event, name::QName, Reader};
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
            let mut note = render_note_summary(&resource)?;
            let date = note.get("date").and_then(Value::as_str)?.to_owned();
            if floor.as_deref().is_some_and(|floor| !iso_on_or_after(&date, floor)) {
                return None;
            }

            let metadata_match = normalize_match_text(&note_search_text(&note)).contains(&normalized_query);
            if metadata_match {
                if let Some(note_object) = note.as_object_mut() {
                    note_object.insert("match_source".into(), Value::String("metadata".into()));
                }
                return Some(note);
            }

            let hydrated_content = hydrate_note_content(&session, &resource);
            let aggregated_body = aggregate_note_body_text(&hydrated_content);
            if !normalize_match_text(&aggregated_body).contains(&normalized_query) {
                return None;
            }

            if let Some(note_object) = note.as_object_mut() {
                note_object.insert("match_source".into(), Value::String("body".into()));
                note_object.insert("body_excerpt".into(), Value::String(body_excerpt(&aggregated_body)));
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

fn aggregate_note_body_text(content: &[Value]) -> String {
    content
        .iter()
        .filter_map(|entry| entry.get("body_text").and_then(Value::as_str))
        .filter(|body| !body.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
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

fn normalize_attachment_body_text(content_type: Option<&str>, body_text: String) -> String {
    let trimmed = body_text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if looks_like_cda_document(trimmed) {
        if let Some(extracted) = extract_cda_section_text(trimmed) {
            if !extracted.is_empty() {
                return extracted;
            }
        }
    }

    if is_rtf_content_type(content_type) || looks_like_rtf(trimmed) {
        let stripped = strip_rtf_to_text(trimmed);
        if !stripped.is_empty() {
            return stripped;
        }
    }

    if is_markup_content_type(content_type) || looks_like_markup(trimmed) {
        let stripped = strip_markup_to_text(trimmed);
        if !stripped.is_empty() {
            return stripped;
        }
    }

    collapse_plain_text(&replace_embedded_base64_rtf_payloads(trimmed))
}

fn is_markup_content_type(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return false;
    };
    let normalized = content_type.to_ascii_lowercase();
    normalized.contains("xml") || normalized.contains("html")
}

fn is_rtf_content_type(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return false;
    };
    content_type.to_ascii_lowercase().contains("rtf")
}

fn looks_like_markup(body_text: &str) -> bool {
    let trimmed = body_text.trim_start();
    trimmed.starts_with('<') && trimmed.contains('>')
}

fn looks_like_rtf(body_text: &str) -> bool {
    body_text.trim_start().starts_with("{\\rtf")
}

fn looks_like_cda_document(body_text: &str) -> bool {
    let trimmed = body_text.trim_start();
    trimmed.starts_with("<ClinicalDocument") || trimmed.contains("urn:hl7-org:v3")
}

fn strip_markup_to_text(input: &str) -> String {
    let mut output = String::new();
    let mut entity = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    let mut in_entity = false;

    for character in input.chars() {
        if in_tag {
            if character == '>' {
                push_block_break_for_tag(&mut output, &tag);
                tag.clear();
                in_tag = false;
            } else {
                tag.push(character);
            }
            continue;
        }

        if in_entity {
            if character == ';' {
                output.push_str(&decode_markup_entity(&entity));
                entity.clear();
                in_entity = false;
            } else if entity.len() < 16 {
                entity.push(character);
            } else {
                output.push('&');
                output.push_str(&entity);
                output.push(character);
                entity.clear();
                in_entity = false;
            }
            continue;
        }

        match character {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '&' => {
                in_entity = true;
                entity.clear();
            }
            _ => output.push(character),
        }
    }

    if in_entity {
        output.push('&');
        output.push_str(&entity);
    }

    collapse_plain_text(&replace_embedded_base64_rtf_payloads(&output))
}

#[derive(Debug, Default)]
struct CdaSectionText {
    title: String,
    text: String,
}

fn extract_cda_section_text(input: &str) -> Option<String> {
    let mut reader = Reader::from_str(input);
    reader.config_mut().trim_text(false);

    let mut buffer = Vec::new();
    let mut tag_stack = Vec::<String>::new();
    let mut document_title = String::new();
    let mut sections = Vec::<CdaSectionText>::new();
    let mut inside_section_text = 0usize;
    let mut capture_document_title = false;
    let mut capture_section_title = false;

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                let name = xml_name_string(event.name());
                let parent = tag_stack.last().cloned();

                if name == "section" {
                    sections.push(CdaSectionText::default());
                } else if name == "title" && parent.as_deref() == Some("ClinicalDocument") {
                    capture_document_title = true;
                } else if name == "title" && parent.as_deref() == Some("section") {
                    capture_section_title = true;
                } else if name == "text" && has_open_section(&tag_stack) {
                    inside_section_text += 1;
                } else if inside_section_text > 0 {
                    push_cda_tag_start(&mut sections, &name);
                }

                tag_stack.push(name);
            }
            Ok(Event::Empty(event)) => {
                let name = xml_name_string(event.name());
                if inside_section_text > 0 {
                    push_cda_tag_start(&mut sections, &name);
                    push_cda_tag_end(&mut sections, &name);
                }
            }
            Ok(Event::End(event)) => {
                let name = xml_name_string(event.name());

                if name == "title" {
                    capture_document_title = false;
                    capture_section_title = false;
                } else if name == "text" && inside_section_text > 0 {
                    inside_section_text -= 1;
                    push_cda_text_separator(&mut sections, '\n');
                } else if inside_section_text > 0 {
                    push_cda_tag_end(&mut sections, &name);
                }

                tag_stack.pop();
            }
            Ok(Event::Text(event)) => {
                if let Some(text) = decode_xml_text(event.as_ref()) {
                    push_cda_text_fragment(
                        &mut sections,
                        &mut document_title,
                        capture_document_title,
                        capture_section_title,
                        inside_section_text > 0,
                        &text,
                    );
                }
            }
            Ok(Event::CData(event)) => {
                if let Some(text) = decode_cdata_text(event.as_ref()) {
                    push_cda_text_fragment(
                        &mut sections,
                        &mut document_title,
                        capture_document_title,
                        capture_section_title,
                        inside_section_text > 0,
                        &text,
                    );
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }

        buffer.clear();
    }

    let document_title = collapse_plain_text(&document_title);
    let rendered_sections = sections
        .into_iter()
        .filter_map(|section| {
            let title = collapse_plain_text(&section.title);
            let text = collapse_plain_text(&replace_embedded_base64_rtf_payloads(&section.text));
            if title.is_empty() && text.is_empty() {
                return None;
            }
            if title.is_empty() {
                return Some(text);
            }
            if text.is_empty() {
                return Some(title);
            }
            Some(format!("{title}\n{text}"))
        })
        .collect::<Vec<_>>();

    if rendered_sections.is_empty() {
        return None;
    }

    let mut output = String::new();
    if !document_title.is_empty() {
        output.push_str(&document_title);
        output.push_str("\n\n");
    }
    output.push_str(&rendered_sections.join("\n\n"));
    Some(output.trim().to_owned())
}

fn has_open_section(tag_stack: &[String]) -> bool {
    tag_stack.iter().rev().any(|name| name == "section")
}

fn xml_name_string(name: QName<'_>) -> String {
    let local = name
        .as_ref()
        .rsplit(|byte| *byte == b':')
        .next()
        .unwrap_or(name.as_ref());
    String::from_utf8_lossy(local).into_owned()
}

fn decode_xml_text(bytes: &[u8]) -> Option<String> {
    let decoded = std::str::from_utf8(bytes).ok()?;
    let unescaped = unescape(decoded).ok()?;
    Some(unescaped.into_owned())
}

fn decode_cdata_text(bytes: &[u8]) -> Option<String> {
    Some(String::from_utf8_lossy(bytes).into_owned())
}

fn push_cda_text_fragment(
    sections: &mut [CdaSectionText],
    document_title: &mut String,
    capture_document_title: bool,
    capture_section_title: bool,
    inside_section_text: bool,
    text: &str,
) {
    if text.trim().is_empty() {
        return;
    }

    if capture_document_title {
        append_text_fragment(document_title, text);
        return;
    }

    if capture_section_title {
        if let Some(section) = sections.last_mut() {
            append_text_fragment(&mut section.title, text);
        }
        return;
    }

    if inside_section_text {
        if let Some(section) = sections.last_mut() {
            section.text.push_str(text);
        }
    }
}

fn append_text_fragment(target: &mut String, text: &str) {
    if target.is_empty() {
        target.push_str(text);
        return;
    }

    let needs_space = !target.ends_with([' ', '\n', '\t'])
        && !text.starts_with([' ', '\n', '\t'])
        && target
            .chars()
            .last()
            .is_some_and(|character| character.is_ascii_alphanumeric())
        && text
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric());
    if needs_space {
        target.push(' ');
    }
    target.push_str(text);
}

fn push_cda_text_separator(sections: &mut [CdaSectionText], separator: char) {
    if let Some(section) = sections.last_mut() {
        push_text_separator(&mut section.text, separator);
    }
}

fn push_cda_tag_start(sections: &mut [CdaSectionText], name: &str) {
    match name {
        "paragraph" | "list" | "table" | "tbody" | "thead" | "tfoot" | "tr" | "caption" | "content" | "br" => {
            push_cda_text_separator(sections, '\n')
        }
        "item" | "td" | "th" => push_cda_text_separator(sections, ' '),
        _ => {}
    }
}

fn push_cda_tag_end(sections: &mut [CdaSectionText], name: &str) {
    match name {
        "paragraph" | "item" | "tr" | "table" | "list" | "caption" | "content" => {
            push_cda_text_separator(sections, '\n')
        }
        "td" | "th" => push_cda_text_separator(sections, ' '),
        _ => {}
    }
}

fn replace_embedded_base64_rtf_payloads(input: &str) -> String {
    let characters = input.chars().collect::<Vec<_>>();
    let mut output = String::new();
    let mut index = 0;

    while index < characters.len() {
        if let Some((next_index, replacement)) = decode_embedded_base64_rtf_payload(&characters, index) {
            let replacement = replacement.trim();
            if !replacement.is_empty() {
                if !output.is_empty() && !output.ends_with([' ', '\n', '\t']) {
                    output.push(' ');
                }
                output.push_str(replacement);
                if next_index < characters.len()
                    && !output.ends_with([' ', '\n', '\t'])
                    && !characters[next_index].is_ascii_whitespace()
                {
                    output.push(' ');
                }
            }
            index = next_index;
            continue;
        }

        output.push(characters[index]);
        index += 1;
    }

    output
}

fn decode_embedded_base64_rtf_payload(characters: &[char], start_index: usize) -> Option<(usize, String)> {
    if !characters
        .get(start_index)
        .is_some_and(|character| matches!(character, 'e' | 'E'))
    {
        return None;
    }

    let mut collapsed = String::new();
    let mut end_index = start_index;
    let mut saw_padding = false;

    while end_index < characters.len() {
        let character = characters[end_index];
        if character.is_ascii_whitespace() {
            end_index += 1;
            continue;
        }

        if saw_padding {
            if character == '=' {
                collapsed.push(character);
                end_index += 1;
                continue;
            }
            break;
        }

        if is_base64_character(character) {
            collapsed.push(character);
            if character == '=' {
                saw_padding = true;
            }
            end_index += 1;
            continue;
        }

        break;
    }

    if collapsed.len() < 128 || !collapsed.starts_with("e1xydGY") {
        return None;
    }

    let decoded = decode_base64(&collapsed).ok()?;
    let decoded_text = String::from_utf8_lossy(&decoded);
    if !decoded_text.starts_with("{\\rtf") {
        return None;
    }

    let stripped = strip_rtf_to_text(&decoded_text);
    if stripped.is_empty() {
        return None;
    }

    Some((end_index, stripped))
}

fn is_base64_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '-' | '_' | '=')
}

#[derive(Clone, Copy)]
struct RtfGroupState {
    ignorable: bool,
    uc_skip: usize,
}

fn strip_rtf_to_text(input: &str) -> String {
    let mut states = vec![RtfGroupState {
        ignorable: false,
        uc_skip: 1,
    }];
    let mut pending_ignorable_group = false;
    let mut fallback_skip = 0usize;
    let mut output = String::new();
    let characters = input.chars().collect::<Vec<_>>();
    let mut index = 0usize;

    while index < characters.len() {
        match characters[index] {
            '{' => {
                let mut next_state = *states.last().unwrap_or(&RtfGroupState {
                    ignorable: false,
                    uc_skip: 1,
                });
                if pending_ignorable_group {
                    next_state.ignorable = true;
                    pending_ignorable_group = false;
                }
                states.push(next_state);
                index += 1;
            }
            '}' => {
                if states.len() > 1 {
                    states.pop();
                }
                pending_ignorable_group = false;
                fallback_skip = 0;
                index += 1;
            }
            '\\' => {
                let current = *states.last().unwrap_or(&RtfGroupState {
                    ignorable: false,
                    uc_skip: 1,
                });
                index += 1;
                if index >= characters.len() {
                    break;
                }

                match characters[index] {
                    '\\' | '{' | '}' if !current.ignorable => {
                        output.push(characters[index]);
                        index += 1;
                    }
                    '~' if !current.ignorable => {
                        output.push(' ');
                        index += 1;
                    }
                    '_' if !current.ignorable => {
                        output.push('-');
                        index += 1;
                    }
                    '-' if !current.ignorable => {
                        output.push('-');
                        index += 1;
                    }
                    '*' => {
                        pending_ignorable_group = true;
                        index += 1;
                    }
                    '\'' => {
                        if index + 2 < characters.len() {
                            let hex = [characters[index + 1], characters[index + 2]]
                                .iter()
                                .collect::<String>();
                            if !current.ignorable {
                                if let Ok(value) = u8::from_str_radix(&hex, 16) {
                                    output.push(value as char);
                                }
                            }
                            index += 3;
                        } else {
                            break;
                        }
                    }
                    character if character.is_ascii_alphabetic() => {
                        let word_start = index;
                        index += 1;
                        while index < characters.len() && characters[index].is_ascii_alphabetic() {
                            index += 1;
                        }
                        let word = characters[word_start..index].iter().collect::<String>();

                        let mut sign = 1i32;
                        if index < characters.len() && characters[index] == '-' {
                            sign = -1;
                            index += 1;
                        }

                        let number_start = index;
                        while index < characters.len() && characters[index].is_ascii_digit() {
                            index += 1;
                        }
                        let argument = if number_start < index {
                            characters[number_start..index]
                                .iter()
                                .collect::<String>()
                                .parse::<i32>()
                                .ok()
                                .map(|value| value * sign)
                        } else {
                            None
                        };

                        if index < characters.len() && characters[index] == ' ' {
                            index += 1;
                        }

                        handle_rtf_control_word(
                            &word,
                            argument,
                            current,
                            &mut states,
                            &mut pending_ignorable_group,
                            &mut fallback_skip,
                            &mut output,
                        );
                    }
                    _ => {
                        index += 1;
                    }
                }
            }
            character => {
                let current = *states.last().unwrap_or(&RtfGroupState {
                    ignorable: false,
                    uc_skip: 1,
                });
                if fallback_skip > 0 {
                    fallback_skip -= 1;
                } else if !current.ignorable {
                    output.push(character);
                }
                index += 1;
            }
        }
    }

    collapse_plain_text(&output)
}

fn handle_rtf_control_word(
    word: &str,
    argument: Option<i32>,
    current: RtfGroupState,
    states: &mut [RtfGroupState],
    pending_ignorable_group: &mut bool,
    fallback_skip: &mut usize,
    output: &mut String,
) {
    if *pending_ignorable_group {
        if let Some(state) = states.last_mut() {
            state.ignorable = true;
        }
        *pending_ignorable_group = false;
    }

    let current = states.last().copied().unwrap_or(current);
    match word {
        "par" | "line" => {
            if !current.ignorable {
                push_text_separator(output, '\n');
            }
        }
        "tab" => {
            if !current.ignorable {
                push_text_separator(output, ' ');
            }
        }
        "emdash" | "endash" => {
            if !current.ignorable {
                output.push('-');
            }
        }
        "uc" => {
            if let Some(value) = argument {
                if let Some(state) = states.last_mut() {
                    state.uc_skip = value.max(0) as usize;
                }
            }
        }
        "u" => {
            if !current.ignorable {
                if let Some(value) = argument {
                    let codepoint = if value < 0 {
                        (value + 65_536) as u32
                    } else {
                        value as u32
                    };
                    if let Some(character) = char::from_u32(codepoint) {
                        output.push(character);
                    }
                }
            }
            *fallback_skip = current.uc_skip;
        }
        "fonttbl" | "colortbl" | "stylesheet" | "info" | "pict" | "object" | "fldinst" | "xmlopen" | "xmlattrname"
        | "xmlattrvalue" | "datastore" => {
            if let Some(state) = states.last_mut() {
                state.ignorable = true;
            }
        }
        _ => {}
    }
}

fn push_block_break_for_tag(output: &mut String, raw_tag: &str) {
    let trimmed = raw_tag.trim();
    let is_closing = trimmed.starts_with('/');
    let is_self_closing = trimmed.ends_with('/');
    let normalized = trimmed
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_ascii_lowercase();

    if matches!(
        normalized.as_str(),
        "br" | "div"
            | "p"
            | "li"
            | "tr"
            | "td"
            | "th"
            | "section"
            | "title"
            | "text"
            | "paragraph"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
    ) && (is_closing || is_self_closing || normalized == "br")
    {
        push_text_separator(output, '\n');
        return;
    }

    if matches!(
        normalized.as_str(),
        "given"
            | "family"
            | "prefix"
            | "suffix"
            | "streetaddressline"
            | "city"
            | "district"
            | "county"
            | "state"
            | "postalcode"
            | "country"
    ) && (is_closing || is_self_closing)
    {
        push_text_separator(output, ' ');
    }
}

fn push_text_separator(output: &mut String, separator: char) {
    if output.is_empty() {
        return;
    }

    if output.ends_with([' ', '\n', '\t']) {
        if separator == '\n' && !output.ends_with('\n') {
            output.push('\n');
        }
        return;
    }

    output.push(separator);
}

fn decode_markup_entity(entity: &str) -> String {
    match entity {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" => "'".into(),
        "nbsp" => " ".into(),
        _ if entity.starts_with("#x") => u32::from_str_radix(&entity[2..], 16)
            .ok()
            .and_then(char::from_u32)
            .map(|character| character.to_string())
            .unwrap_or_else(|| format!("&{entity};")),
        _ if entity.starts_with('#') => entity[1..]
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(|character| character.to_string())
            .unwrap_or_else(|| format!("&{entity};")),
        _ => format!("&{entity};"),
    }
}

fn collapse_plain_text(input: &str) -> String {
    let collapsed = input
        .lines()
        .map(|line| {
            line.split_whitespace()
                .filter(|token| !token.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    insert_soft_boundaries_in_text(&collapsed)
}

fn insert_soft_boundaries_in_text(text: &str) -> String {
    let characters = text.chars().collect::<Vec<_>>();
    let mut normalized = String::new();

    for (index, character) in characters.iter().copied().enumerate() {
        if should_insert_soft_boundary(&characters, index)
            && !normalized.ends_with([' ', '\n', '\t', '/', '-', '(', '['])
        {
            normalized.push(' ');
        }
        normalized.push(character);
    }

    normalized
}

fn should_insert_soft_boundary(characters: &[char], index: usize) -> bool {
    if index == 0 {
        return false;
    }

    let previous = characters[index - 1];
    let current = characters[index];
    let next = characters.get(index + 1).copied();

    if previous.is_ascii_whitespace() || current.is_ascii_whitespace() {
        return false;
    }

    if matches!(previous, '.' | '!' | '?') && current.is_ascii_uppercase() {
        return true;
    }

    if is_soft_break_punctuation(previous) && current.is_ascii_alphanumeric() {
        return true;
    }

    if previous.is_ascii_lowercase()
        && current.is_ascii_uppercase()
        && contiguous_run_len(characters, index - 1, |character| character.is_ascii_alphabetic()) >= 3
    {
        return true;
    }

    if previous.is_ascii_uppercase()
        && current.is_ascii_uppercase()
        && next.is_some_and(|next| next.is_ascii_lowercase())
        && contiguous_run_len(characters, index - 1, |character| character.is_ascii_uppercase()) >= 2
    {
        return true;
    }

    if previous.is_ascii_alphabetic() && current.is_ascii_digit() {
        return true;
    }

    if previous.is_ascii_digit() && current.is_ascii_alphabetic() {
        return true;
    }

    false
}

fn contiguous_run_len<F>(characters: &[char], end_index: usize, predicate: F) -> usize
where
    F: Fn(char) -> bool,
{
    let mut index = end_index;
    let mut count = 0;

    loop {
        if !predicate(characters[index]) {
            break;
        }
        count += 1;
        if index == 0 {
            break;
        }
        index -= 1;
    }

    count
}

fn is_soft_break_punctuation(character: char) -> bool {
    matches!(character, ')' | ']' | '}' | ':' | ';')
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
