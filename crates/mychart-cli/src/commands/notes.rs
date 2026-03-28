mod body;

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use self::body::{aggregate_note_body_text, body_excerpt_for_query, hydrate_note_content};
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
                note_object.insert(
                    "body_excerpt".into(),
                    Value::String(body_excerpt_for_query(&aggregated_body, &args.query)),
                );
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
    let aggregated_body = aggregate_note_body_text(&hydrated_content);

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
