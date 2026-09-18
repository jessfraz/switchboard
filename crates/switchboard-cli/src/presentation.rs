use std::{collections::BTreeMap, ffi::OsString};

use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use serde::Serialize;
use serde_json::Value;
use switchboard_core::{DispatchOutcome, OperationOutcome, ToolOutput};

use crate::output;

/// Presentation changes the returned view, never the durable operation receipt.
#[derive(Clone, Debug, Default, Args)]
pub(crate) struct Presentation {
    /// Show full diagnostic output and pretty-printed JSON.
    #[arg(long, global = true)]
    pub(crate) full: bool,
    /// Select comma-separated paths within result fields; arrays apply each path to every row.
    #[arg(long, global = true, value_delimiter = ',')]
    fields: Vec<String>,
}

impl Presentation {
    // Clap leaves external-command arguments untouched. Consume only our flags,
    // preserving opaque raw argv values and everything after the native delimiter.
    pub(crate) fn extract(&mut self, tokens: &mut Vec<OsString>) -> Result<()> {
        let mut remaining = Vec::with_capacity(tokens.len());
        let mut input = std::mem::take(tokens).into_iter();
        while let Some(token) = input.next() {
            match token.to_str() {
                Some("--") => {
                    remaining.push(token);
                    remaining.extend(input);
                    break;
                }
                Some("--argv" | "--argv-json") => {
                    remaining.push(token);
                    if let Some(value) = input.next() {
                        remaining.push(value);
                    }
                }
                Some("--full") => self.full = true,
                Some("--fields") => {
                    let value = input.next().ok_or_else(|| anyhow!("missing value for --fields"))?;
                    self.add_fields(value.to_str().ok_or_else(|| anyhow!("--fields must be UTF-8"))?);
                }
                Some(value) if value.starts_with("--fields=") => self.add_fields(&value[9..]),
                _ => remaining.push(token),
            }
        }
        *tokens = remaining;
        Ok(())
    }

    fn add_fields(&mut self, value: &str) {
        self.fields.extend(value.split(',').map(str::to_owned));
    }

    pub(crate) fn validate(&self) -> Result<()> {
        for path in &self.fields {
            if path.split('.').any(|part| {
                part.is_empty()
                    || !part
                        .chars()
                        .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-'))
            }) {
                bail!("invalid --fields path {path:?}; use dotted field names separated by commas");
            }
        }
        Ok(())
    }

    pub(crate) fn validate_command(&self, command: &crate::args::CommandKind) -> Result<()> {
        if !self.fields.is_empty()
            && !matches!(
                command,
                crate::args::CommandKind::Operation(_)
                    | crate::args::CommandKind::ApproveAndApply(_)
                    | crate::args::CommandKind::ReadBatch(_)
            )
        {
            bail!("--fields applies to tool execution and read-batch results");
        }
        Ok(())
    }

    pub(crate) fn append_argv(&self, argv: &mut Vec<String>) {
        if self.full {
            argv.push("--full".into());
        }
        if !self.fields.is_empty() {
            argv.extend(["--fields".into(), self.fields.join(",")]);
        }
    }

    pub(crate) fn operation(&self, outcome: &OperationOutcome, json: bool) -> Result<String> {
        let aggregate = match outcome {
            OperationOutcome::Single(outcome) => return self.dispatch(outcome, json),
            OperationOutcome::AggregateRead(aggregate) if json => {
                return self.json(&output::AggregateReadResponse::from(aggregate))
            }
            OperationOutcome::AggregateRead(aggregate) => aggregate,
        };
        let mut text = String::new();
        for result in &aggregate.results {
            match &result.outcome {
                Ok(outcome) => text.push_str(&self.dispatch(outcome, false)?),
                Err(failure) => text.push_str(&format!("{}: {}\n", result.namespace, failure.message)),
            }
        }
        Ok(text)
    }

    pub(crate) fn dispatch(&self, outcome: &DispatchOutcome, json: bool) -> Result<String> {
        if json {
            return self.json(&output::DispatchResponse::from(outcome));
        }
        match outcome {
            DispatchOutcome::Executed(value) => Ok(self.human(value)),
            _ => Ok(output::render_dispatch_human(outcome)),
        }
    }

    pub(crate) fn json(&self, result: &impl Serialize) -> Result<String> {
        if self.fields.is_empty() {
            return output::render_json(result, self.full);
        }
        // Only projections need a dynamic tree. Ordinary output serializes its
        // typed envelope directly, without an intermediate JSON string or parse.
        let mut value = serde_json::to_value(output::versioned(result)).context("failed to serialize result view")?;
        Self::project_envelope(&mut value, &Projection::new(&self.fields));
        switchboard_cli_support::output::to_json(&value, !self.full).context("failed to serialize result view")
    }

    fn project_envelope(value: &mut Value, projection: &Projection) {
        if let Some(fields) = value.get_mut("fields") {
            *fields = projection.select(fields);
        }
        if let Some(items) = value.get_mut("items").and_then(Value::as_object_mut) {
            for item in items.values_mut() {
                if let Some(pages) = item.get_mut("pages").and_then(Value::as_array_mut) {
                    for page in pages {
                        if let Some(output) = page.get_mut("output") {
                            Self::project_envelope(output, projection);
                        }
                    }
                }
            }
        }
        if let Some(results) = value.get_mut("results").and_then(Value::as_array_mut) {
            for result in results {
                if let Some(outcome) = result.get_mut("outcome") {
                    Self::project_envelope(outcome, projection);
                }
            }
        }
    }

    fn human(&self, value: &ToolOutput) -> String {
        let projected;
        let fields = if self.fields.is_empty() {
            &value.fields
        } else {
            let projection = Projection::new(&self.fields);
            projected = value
                .fields
                .iter()
                .filter_map(|(key, child)| projection.select_member(key, child).map(|value| (key.clone(), value)))
                .collect();
            &projected
        };
        let format = if self.full {
            output::HumanOutputFormat::Full
        } else {
            output::HumanOutputFormat::Compact
        };
        output::render_output_view(value, fields, format)
    }
}

fn retained_metadata(key: &str) -> bool {
    matches!(
        key,
        "status"
            | "id"
            | "gmail_message_id"
            | "gmail_thread_id"
            | "number"
            | "url"
            | "commit"
            | "head_sha"
            | "failure"
            | "failures"
            | "coverage"
            | "next_cursor"
            | "cursor"
            | "truncated_sources"
            | "attachment_coverage"
            | "warnings"
            | "operation_id"
            | "verification"
            | "refs"
            | "truncated"
            | "has_more"
            | "messages_omitted"
            | "body_truncated"
            | "attachments_omitted"
    )
}

/// A selection tree is prepared once and reused for every array row and page.
#[derive(Default)]
struct Projection {
    whole: bool,
    children: BTreeMap<String, Projection>,
}

impl Projection {
    fn new(paths: &[String]) -> Self {
        let mut selection = Self::default();
        for path in paths {
            let mut node = &mut selection;
            for component in path.split('.') {
                node = node.children.entry(component.to_owned()).or_default();
            }
            node.whole = true;
        }
        selection
    }

    fn select(&self, value: &Value) -> Value {
        if self.whole {
            return value.clone();
        }
        match value {
            Value::Array(rows) => Value::Array(rows.iter().map(|row| self.select(row)).collect()),
            Value::Object(object) => Value::Object(
                object
                    .iter()
                    .filter_map(|(key, child)| self.select_member(key, child).map(|value| (key.clone(), value)))
                    .collect(),
            ),
            _ => Value::Null,
        }
    }

    fn select_member(&self, key: &str, child: &Value) -> Option<Value> {
        if retained_metadata(key) {
            Some(child.clone())
        } else {
            self.children.get(key).map(|selection| selection.select(child))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Serialize};
    use switchboard_core::{CoverageStatus, DispatchOutcome, NamespaceId, ReadCoverage, ToolName, ToolOutput};

    use super::Presentation;

    #[derive(Clone, Serialize)]
    struct Message {
        subject: String,
        body_text: String,
        body_html: String,
        body_truncated: bool,
    }

    #[derive(Serialize)]
    struct Fields {
        messages: Vec<Message>,
        failures: Vec<String>,
    }

    #[derive(Deserialize)]
    struct Selected {
        schema_version: u32,
        status: String,
        coverage: ReadCoverage,
        fields: SelectedFields,
    }

    #[derive(Deserialize)]
    struct SelectedFields {
        messages: Vec<SelectedMessage>,
        failures: Vec<String>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct SelectedMessage {
        subject: String,
        body_truncated: bool,
    }

    fn output() -> ToolOutput {
        let mut output = ToolOutput::new(
            ToolName::new("google.mail.search").expect("valid tool"),
            NamespaceId::new("google.test").expect("valid namespace"),
            "Two selected messages",
        );
        let message = Message {
            subject: "Relevant message".into(),
            body_text: "é".repeat(2500),
            body_html: "<p>duplicate HTML body</p>".into(),
            body_truncated: false,
        };
        output.fields = serde_json::from_value(
            serde_json::to_value(Fields {
                messages: vec![message.clone(), message],
                failures: vec!["Another message was unavailable".into()],
            })
            .expect("serialize fixture fields"),
        )
        .expect("deserialize provider field map");
        output.coverage = Some(ReadCoverage {
            status: CoverageStatus::Unknown,
            next_cursor: Some("next-page".into()),
        });
        output
    }

    #[test]
    fn projection_keeps_each_row_and_failure_coverage_without_mutating_receipt() {
        let original = output();
        let before = original.fields.clone();
        let options = Presentation {
            full: false,
            fields: vec!["messages.subject".into()],
        };
        let text = options
            .dispatch(&DispatchOutcome::Executed(original.clone()), true)
            .expect("render selected fields");
        let selected: Selected = serde_json::from_str(&text).expect("decode selected response");
        assert_eq!(selected.schema_version, 1);
        assert_eq!(selected.status, "partial");
        assert_eq!(selected.coverage.status, CoverageStatus::Unknown);
        assert_eq!(selected.coverage.next_cursor.as_deref(), Some("next-page"));
        assert_eq!(selected.fields.messages.len(), 2);
        assert!(selected
            .fields
            .messages
            .iter()
            .all(|message| message.subject == "Relevant message" && !message.body_truncated));
        assert_eq!(selected.fields.failures, vec!["Another message was unavailable"]);
        assert_eq!(original.fields, before);
    }

    #[test]
    fn human_view_shortens_display_explicitly_and_full_json_preserves_content() {
        let outcome = DispatchOutcome::Executed(output());
        let human = Presentation::default()
            .dispatch(&outcome, false)
            .expect("render human response");
        assert!(human.contains("display shortened"));
        assert!(!human.contains("duplicate HTML body"));
        assert!(human.contains("Another message was unavailable"));
        assert!(human.contains("Coverage: Unknown"));
        let compact = Presentation::default()
            .dispatch(&outcome, true)
            .expect("render compact response");
        let full = Presentation {
            full: true,
            fields: Vec::new(),
        }
        .dispatch(&outcome, true)
        .expect("render full response");
        let compact: BTreeMap<String, serde_json::Value> =
            serde_json::from_str(&compact).expect("decode compact response");
        let full: BTreeMap<String, serde_json::Value> = serde_json::from_str(&full).expect("decode full response");
        assert_eq!(compact, full);
    }

    #[test]
    fn human_views_preserve_effects_receipts_and_coverage_with_selected_fields() {
        let mut value = output();
        value.effect = Some(
            switchboard_core::OperationEffect::new(true)
                .with_undo_summary("Restore the previous labels")
                .expect("valid undo summary"),
        );
        value.verification = Some(Box::new(switchboard_core::VerificationReceipt::new(
            switchboard_core::VerificationStatus::Verified,
            "Labels confirmed by readback",
            Vec::new(),
        )));
        let outcome = DispatchOutcome::Executed(value);
        for full in [false, true] {
            for fields in [Vec::new(), vec!["messages.subject".into()]] {
                let rendered = Presentation { full, fields }
                    .dispatch(&outcome, false)
                    .expect("render human response");
                assert!(rendered.contains("undoable: true"));
                assert!(rendered.contains("Restore the previous labels"));
                assert!(rendered.contains("Verification: Verified: Labels confirmed by readback"));
                assert!(rendered.contains("Coverage: Unknown"));
                assert!(rendered.contains("Next cursor: next-page"));
                assert!(rendered.contains("Another message was unavailable"));
            }
        }
    }

    #[test]
    fn output_flags_preserve_native_argv_and_delimiter() {
        let mut options = Presentation::default();
        let mut tokens = [
            "github.cli.read",
            "--argv",
            "--fields",
            "--full",
            "--fields=message.subject",
            "--",
            "--full",
            "--fields",
            "native",
        ]
        .map(Into::into)
        .to_vec();
        options.extract(&mut tokens).expect("extract output options");
        options.validate().expect("validate field selection");
        assert!(options.full);
        assert_eq!(options.fields, vec!["message.subject"]);
        assert_eq!(
            tokens,
            [
                "github.cli.read",
                "--argv",
                "--fields",
                "--",
                "--full",
                "--fields",
                "native"
            ]
            .map(std::ffi::OsString::from)
        );
        let invalid = Presentation {
            full: false,
            fields: vec!["messages..subject".into()],
        };
        assert!(invalid.validate().is_err());
    }
}
