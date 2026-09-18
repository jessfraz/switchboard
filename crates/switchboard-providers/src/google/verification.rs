use serde::{Deserialize, Serialize};
use switchboard_core::{
    Error, ExecutionTarget, FileChecksum, PlannedAction, Result, StoredOperation, ToolOutput, ToolRef, ToolRefKind,
    VerificationReceipt, VerificationStatus,
};

use crate::{cli::passthrough, google::GoogleWorkspaceAdapter};

#[derive(Deserialize)]
struct DraftOutput {
    draft_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileIdentity {
    id: String,
    sha256_checksum: Option<String>,
    sha1_checksum: Option<String>,
    md5_checksum: Option<String>,
    #[serde(default)]
    trashed: bool,
}

impl FileIdentity {
    fn checksum(&self) -> Option<FileChecksum> {
        self.sha256_checksum
            .clone()
            .map(FileChecksum::Sha256)
            .or_else(|| self.sha1_checksum.clone().map(FileChecksum::Sha1))
            .or_else(|| self.md5_checksum.clone().map(FileChecksum::Md5))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageParams<'a> {
    user_id: &'static str,
    id: &'a str,
    format: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventParams<'a> {
    calendar_id: &'a str,
    event_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileParams<'a> {
    file_id: &'a str,
    fields: &'static str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Message {
    id: String,
    #[serde(default)]
    label_ids: Vec<String>,
}

#[derive(Deserialize)]
struct Draft {
    id: String,
    message: Message,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    id: String,
    status: String,
    summary: Option<String>,
    start: Option<EventTime>,
    end: Option<EventTime>,
    description: Option<String>,
    location: Option<String>,
    #[serde(default)]
    attendees: Vec<Attendee>,
    hangout_link: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventTime {
    date_time: Option<String>,
    date: Option<String>,
}

impl EventTime {
    fn matches(&self, expected: &str) -> bool {
        // Conservative comparison: a differently formatted timestamp is not proof.
        self.date_time.as_deref().or(self.date.as_deref()) == Some(expected)
    }
}

#[derive(Deserialize)]
struct Attendee {
    email: String,
}

#[derive(Deserialize)]
struct ModifyParams {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LabelMutation {
    #[serde(default)]
    add_label_ids: Vec<String>,
    #[serde(default)]
    remove_label_ids: Vec<String>,
}

impl LabelMutation {
    fn matches(&self, message: &Message) -> bool {
        self.add_label_ids.iter().all(|id| message.label_ids.contains(id))
            && self.remove_label_ids.iter().all(|id| !message.label_ids.contains(id))
    }
}

impl GoogleWorkspaceAdapter {
    pub(super) fn prepare_verifiable_upload(
        request: &switchboard_core::ToolRequest,
    ) -> Result<switchboard_core::ToolRequest> {
        if request.tool.as_str() != "google.cli.write" {
            return Ok(request.clone());
        }
        let mut argv = passthrough::parse_passthrough_argv(&request.args)?;
        if !(argv.starts_with(&["drive".into(), "files".into(), "create".into()])
            || argv.starts_with(&["drive".into(), "files".into(), "update".into()]))
            || !argv
                .iter()
                .any(|value| value == "--upload" || value.starts_with("--upload="))
        {
            return Ok(request.clone());
        }
        let positions = argv
            .iter()
            .enumerate()
            .filter(|(_, value)| value.as_str() == "--params" || value.starts_with("--params="))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if positions.len() > 1 {
            return Err(Error::InvalidArguments(
                "upload verification requires a single --params argument".into(),
            ));
        }
        let mut params = if positions.is_empty() {
            serde_json::Map::new()
        } else {
            parse_option::<serde_json::Map<String, serde_json::Value>>(&argv, "--params")?
        };
        let fields = params
            .get("fields")
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| Error::InvalidArguments("Drive fields must be a string".into()))
            })
            .transpose()?
            .unwrap_or("id,name,mimeType");
        let fields = if fields.split(',').any(|part| part.trim() == "*") {
            fields.to_owned()
        } else {
            format!("{fields},id,trashed,sha256Checksum,sha1Checksum,md5Checksum")
        };
        params.insert("fields".into(), serde_json::Value::String(fields));
        let encoded = serde_json::to_string(&params).map_err(|error| Error::InvalidArguments(error.to_string()))?;
        if let Some(index) = positions.first().copied() {
            if argv[index] == "--params" {
                argv[index + 1] = encoded;
            } else {
                argv[index] = format!("--params={encoded}");
            }
        } else {
            argv.extend(["--params".into(), encoded]);
        }
        switchboard_core::ToolRequest::new(
            request.tool.as_str(),
            request.namespace.as_str(),
            request.mode,
            vec![switchboard_core::ToolArgument::option(
                "argv-json",
                serde_json::to_string(&argv).map_err(|error| Error::InvalidArguments(error.to_string()))?,
            )?],
        )
    }

    pub(super) fn capture_verification_refs(
        &self,
        target: &ExecutionTarget,
        action: &PlannedAction,
        output: &mut ToolOutput,
    ) -> Result<()> {
        if action.tool.as_str() == "google.mail.draft" {
            let fields = output
                .fields
                .get("draft")
                .ok_or_else(|| Error::Execution("draft response omitted draft identity".into()))?;
            let draft: DraftOutput = serde_json::from_value(fields.clone())
                .map_err(|error| Error::Execution(format!("invalid draft identity: {error}")))?;
            output.refs.push(ToolRef::new(
                target.namespace.provider.clone(),
                action.namespace.clone(),
                ToolRefKind::Draft,
                draft.draft_id,
            )?);
        } else if action.tool.as_str() == "google.cli.write" {
            let argv = passthrough::parse_passthrough_argv(&action.args)?;
            if argv.starts_with(&["drive".into(), "files".into(), "create".into()])
                || argv.starts_with(&["drive".into(), "files".into(), "update".into()])
            {
                if let Some(value) = output.fields.get("response") {
                    let file: FileIdentity = serde_json::from_value(value.clone())
                        .map_err(|error| Error::Execution(format!("invalid uploaded file identity: {error}")))?;
                    let mut reference = ToolRef::new(
                        target.namespace.provider.clone(),
                        action.namespace.clone(),
                        ToolRefKind::File,
                        &file.id,
                    )?;
                    reference.checksum = file.checksum();
                    output.refs.push(reference);
                }
            } else if argv.starts_with(&["gmail".into(), "users".into(), "drafts".into(), "create".into()]) {
                if let Some(value) = output.fields.get("response") {
                    let draft: Draft = serde_json::from_value(value.clone())
                        .map_err(|error| Error::Execution(format!("invalid draft identity: {error}")))?;
                    output.refs.push(ToolRef::new(
                        target.namespace.provider.clone(),
                        action.namespace.clone(),
                        ToolRefKind::Draft,
                        draft.id,
                    )?);
                    output.refs.push(ToolRef::new(
                        target.namespace.provider.clone(),
                        action.namespace.clone(),
                        ToolRefKind::Message,
                        draft.message.id,
                    )?);
                }
            }
        }
        Ok(())
    }

    pub(super) fn verify_write(
        &self,
        target: &ExecutionTarget,
        operation: &StoredOperation,
    ) -> Result<VerificationReceipt> {
        let refs = operation
            .effect
            .as_ref()
            .map(|effect| effect.refs.as_slice())
            .unwrap_or_default();
        if let Some(reference) = refs.iter().find(|reference| reference.kind == ToolRefKind::Draft) {
            let draft: Draft = self.read_json(
                target,
                &["gmail", "users", "drafts", "get"],
                &MessageParams {
                    user_id: "me",
                    id: &reference.id,
                    format: "minimal",
                },
            )?;
            let expected_message = refs.iter().find(|reference| reference.kind == ToolRefKind::Message);
            let matches = draft.id == reference.id
                && expected_message.is_some_and(|reference| reference.id == draft.message.id)
                && draft.message.label_ids.iter().any(|label| label == "DRAFT")
                && !draft.message.label_ids.iter().any(|label| label == "SENT");
            return Ok(receipt(matches, "saved draft identity and unsent labels", refs));
        }
        if operation.tool.as_str() == "google.calendar.create" {
            let id = refs
                .iter()
                .find(|reference| reference.kind == ToolRefKind::Event)
                .map(|reference| reference.id.clone())
                .unwrap_or_else(|| crate::google::calendar::event_id(&operation.id));
            let event: Event = self.read_json(
                target,
                &["calendar", "events", "get"],
                &EventParams {
                    calendar_id: operation.args.value("calendar").unwrap_or("primary"),
                    event_id: &id,
                },
            )?;
            let matches = event.id == id
                && event.status != "cancelled"
                && event.summary.as_deref() == operation.args.value("title").or(operation.args.value("summary"))
                && operation
                    .args
                    .value("start")
                    .is_some_and(|value| event.start.as_ref().is_some_and(|time| time.matches(value)))
                && operation
                    .args
                    .value("end")
                    .is_some_and(|value| event.end.as_ref().is_some_and(|time| time.matches(value)))
                && operation
                    .args
                    .value("description")
                    .map_or(true, |value| event.description.as_deref() == Some(value))
                && operation
                    .args
                    .value("location")
                    .map_or(true, |value| event.location.as_deref() == Some(value))
                && operation.args.values("attendee").all(|email| {
                    event
                        .attendees
                        .iter()
                        .any(|attendee| attendee.email.eq_ignore_ascii_case(email))
                })
                && (!operation.args.has_flag("meet") || event.hangout_link.is_some());
            let event_ref = ToolRef::new(
                target.namespace.provider.clone(),
                operation.namespace.clone(),
                ToolRefKind::Event,
                id,
            )?
            .with_parent_id(operation.args.value("calendar").unwrap_or("primary"))?;
            let mut receipt = receipt(
                matches,
                "calendar event identity and requested fields",
                &[event_ref.clone()],
            );
            if matches {
                receipt.recovered_effect = Some(
                    switchboard_core::OperationEffect::new(true)
                        .with_ref(event_ref)
                        .with_undo_summary("Delete the verified calendar event")?,
                );
            }
            return Ok(receipt);
        }
        if let Some(reference) = refs.iter().find(|reference| reference.kind == ToolRefKind::File) {
            let file: FileIdentity = self.read_json(
                target,
                &["drive", "files", "get"],
                &FileParams {
                    file_id: &reference.id,
                    fields: "id,trashed,sha256Checksum,sha1Checksum,md5Checksum",
                },
            )?;
            let Some(checksum) = &reference.checksum else {
                return Ok(VerificationReceipt::unavailable("file identity recorded, but upload response omitted a checksum; request checksum fields to enable content readback"));
            };
            let matches = file.id == reference.id
                && !file.trashed
                && match checksum {
                    FileChecksum::Sha256(value) => file.sha256_checksum.as_ref() == Some(value),
                    FileChecksum::Sha1(value) => file.sha1_checksum.as_ref() == Some(value),
                    FileChecksum::Md5(value) => file.md5_checksum.as_ref() == Some(value),
                };
            return Ok(receipt(
                matches,
                "uploaded file identity and provider-returned content checksum",
                refs,
            ));
        }
        if operation.tool.as_str() == "google.cli.write" {
            let argv = passthrough::parse_passthrough_argv(&operation.args)?;
            if argv.starts_with(&["gmail".into(), "users".into(), "messages".into(), "modify".into()]) {
                let params: ModifyParams = parse_option(&argv, "--params")?;
                let mutation: LabelMutation = parse_option(&argv, "--json")?;
                if mutation.add_label_ids.is_empty() && mutation.remove_label_ids.is_empty() {
                    return Ok(VerificationReceipt::unavailable(
                        "label mutation contained no expected labels",
                    ));
                }
                let message: Message = self.read_json(
                    target,
                    &["gmail", "users", "messages", "get"],
                    &MessageParams {
                        user_id: "me",
                        id: &params.id,
                        format: "minimal",
                    },
                )?;
                let reference = ToolRef::new(
                    target.namespace.provider.clone(),
                    operation.namespace.clone(),
                    ToolRefKind::Message,
                    &params.id,
                )?;
                return Ok(receipt(
                    message.id == params.id && mutation.matches(&message),
                    "exact requested Gmail label additions and removals",
                    &[reference],
                ));
            }
        }
        Ok(VerificationReceipt::unavailable(
            "this operation has no supported readback contract; inspect provider state before claiming verification",
        ))
    }
}

fn parse_option<T: serde::de::DeserializeOwned>(argv: &[String], flag: &str) -> Result<T> {
    let value = argv
        .iter()
        .enumerate()
        .find_map(|(index, value)| {
            if value == flag {
                argv.get(index + 1).map(String::as_str)
            } else {
                value.strip_prefix(&format!("{flag}="))
            }
        })
        .ok_or_else(|| Error::InvalidArguments(format!("verification requires {flag}")))?;
    serde_json::from_str(value).map_err(|error| Error::InvalidArguments(format!("invalid {flag}: {error}")))
}

fn receipt(matches: bool, subject: &str, refs: &[ToolRef]) -> VerificationReceipt {
    VerificationReceipt::new(
        if matches {
            VerificationStatus::Verified
        } else {
            VerificationStatus::Mismatch
        },
        format!("{}: {subject}", if matches { "Verified" } else { "Readback mismatch" }),
        refs.to_vec(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_readback_requires_every_addition_and_removal() {
        let mutation = LabelMutation {
            add_label_ids: vec!["SPAM".into()],
            remove_label_ids: vec!["INBOX".into()],
        };
        assert!(!mutation.matches(&Message {
            id: "m1".into(),
            label_ids: vec!["SPAM".into(), "INBOX".into()]
        }));
        assert!(!mutation.matches(&Message {
            id: "m1".into(),
            label_ids: vec![]
        }));
        assert!(mutation.matches(&Message {
            id: "m1".into(),
            label_ids: vec!["SPAM".into(), "UNREAD".into()]
        }));
    }
}
