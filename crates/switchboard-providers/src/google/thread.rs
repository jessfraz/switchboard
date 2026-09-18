use serde::{Deserialize, Serialize};
use switchboard_core::{Error, ExecutionTarget, PlannedAction, Result, ToolOutput, ToolRef, ToolRefKind};

use crate::google::{
    context::{bounded_integer, encode, ContextLimits, ContextSource},
    GoogleWorkspaceAdapter,
};

const ATTACHMENT_DEPTH: usize = 8;

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Attachment {
    pub(super) part_id: String,
    pub(super) filename: String,
    pub(super) mime_type: String,
    pub(super) size: u64,
    pub(super) attachment_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Part {
    #[serde(default)]
    part_id: String,
    #[serde(default)]
    filename: String,
    mime_type: Option<String>,
    body: Option<PartBody>,
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartBody {
    #[serde(default)]
    size: u64,
    attachment_id: Option<String>,
}

impl Part {
    pub(super) fn attachments(&self, items: &mut Vec<Attachment>, omitted: &mut usize, unknown: &mut bool, max: usize) {
        let Some(mime_type) = &self.mime_type else {
            // The deepest projected children carry only their ID. Their presence
            // means metadata was omitted, rather than proving no attachments exist.
            *unknown = true;
            return;
        };
        let attachment_id = self.body.as_ref().and_then(|body| body.attachment_id.clone());
        if !self.filename.is_empty() || attachment_id.is_some() {
            if items.len() < max {
                items.push(Attachment {
                    part_id: self.part_id.clone(),
                    filename: self.filename.clone(),
                    mime_type: mime_type.clone(),
                    size: self.body.as_ref().map_or(0, |body| body.size),
                    attachment_id,
                });
            } else {
                *omitted += 1;
            }
        }
        for part in &self.parts {
            part.attachments(items, omitted, unknown, max);
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadParams<'a> {
    pub(super) user_id: &'static str,
    pub(super) id: &'a str,
    pub(super) format: &'static str,
    pub(super) fields: String,
}

#[derive(Deserialize)]
struct ThreadInventory {
    id: String,
    messages: Vec<ThreadMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadMessage {
    id: String,
    thread_id: String,
    payload: Part,
}

pub(super) fn thread_fields() -> String {
    // Thread GET cannot page its messages. Project metadata, never body data or
    // attachment bytes, before selecting the bounded set of bodies to read.
    let mut part = "partId".to_owned();
    for _ in 0..ATTACHMENT_DEPTH {
        part = format!("partId,filename,mimeType,body(size,attachmentId),parts({part})");
    }
    format!("id,messages(id,threadId,payload({part}))")
}

impl GoogleWorkspaceAdapter {
    pub(super) fn thread(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
        let id = action
            .args
            .value("thread-id")
            .ok_or_else(|| Error::InvalidArguments("missing --thread-id".into()))?;
        let max = bounded_integer(&action.args, "max-messages", 20, 100)?;
        let limits = ContextLimits::from_args(&action.args)?;
        let inventory: ThreadInventory = self.read_json(
            target,
            &["gmail", "users", "threads", "get"],
            &ThreadParams {
                user_id: "me",
                id,
                format: "full",
                fields: thread_fields(),
            },
        )?;
        let mut seen = std::collections::BTreeSet::new();
        if inventory.id != id
            || inventory
                .messages
                .iter()
                .any(|message| message.thread_id != id || message.id.trim().is_empty() || !seen.insert(&message.id))
        {
            return Err(Error::Execution("Gmail returned a different thread inventory".into()));
        }
        let total = inventory.messages.len();
        let sources = inventory
            .messages
            .into_iter()
            .skip(total.saturating_sub(max))
            .map(|message| ContextSource {
                id: message.id,
                thread_id: Some(message.thread_id),
                payload: Some(message.payload),
            })
            .collect::<Vec<_>>();
        let mut result = self.read_context(target, action, &sources, limits)?;
        let omitted = total - sources.len();
        result.truncated |= omitted > 0;
        let mut output = ToolOutput::new(
            action.tool.clone(),
            action.namespace.clone(),
            format!("Read {} of {total} Gmail thread messages", result.read_count()),
        );
        output.coverage = Some(result.coverage(None));
        output.refs.push(ToolRef::new(
            target.namespace.provider.clone(),
            action.namespace.clone(),
            ToolRefKind::Thread,
            id,
        )?);
        result.append_refs(target, &mut output)?;
        Ok(output
            .with_field("thread_id", id)
            .with_value_field("count", encode(&result.messages.len())?)
            .with_value_field("read_count", encode(&result.read_count())?)
            .with_value_field("total_messages", encode(&total)?)
            .with_value_field("has_more", encode(&(omitted > 0))?)
            .with_value_field("messages_omitted", encode(&omitted)?)
            .with_value_field("messages", encode(&result.messages)?)
            .with_value_field("failures", encode(&result.failures)?))
    }
}
