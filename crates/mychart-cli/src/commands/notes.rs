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

pub(crate) fn run_notes(command: NotesSubcommand, context: &crate::state::ResolvedContext) -> Result<Value> {
    match command {
        NotesSubcommand::Search(args) => run_search(args, context),
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
            let date = first_string(&resource, &["/date", "/meta/lastUpdated"])?;
            if floor.as_deref().is_some_and(|floor| !iso_on_or_after(&date, floor)) {
                return None;
            }

            let description = first_string(&resource, &["/description"]);
            let note_type = resource.pointer("/type").and_then(concept_text);
            let author = resource
                .get("author")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find_map(|author| author.get("display").and_then(Value::as_str))
                .map(ToOwned::to_owned);
            let haystack = [
                description.clone().unwrap_or_default(),
                note_type.clone().unwrap_or_default(),
                author.clone().unwrap_or_default(),
            ]
            .join(" ");
            if !normalize_match_text(&haystack).contains(&normalized_query) {
                return None;
            }

            Some(json!({
                "id": first_string(&resource, &["/id"]),
                "date": date,
                "type": note_type,
                "description": description,
                "author": author,
                "content": resource
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|content| {
                        content
                            .iter()
                            .map(|entry| {
                                json!({
                                    "title": entry.pointer("/attachment/title").and_then(Value::as_str),
                                    "content_type": entry.pointer("/attachment/contentType").and_then(Value::as_str),
                                    "url": entry.pointer("/attachment/url").and_then(Value::as_str),
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            }))
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
