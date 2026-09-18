use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use switchboard_core::{
    CoverageStatus, Error, ExecutionTarget, Failure, FailurePhase, PlannedAction, ReadCoverage, Result, ToolArgument,
    ToolArguments, ToolName, ToolOutput, ToolRef, ToolRefKind,
};

use crate::google::{
    thread::{Attachment, Part},
    GoogleWorkspaceAdapter,
};

pub(super) fn validate_args(tool: &str, args: &ToolArguments) -> Result<()> {
    match tool {
        "google.mail.search" if args.has_flag("hydrate") => {
            if args.has_flag("labels") {
                return Err(Error::InvalidArguments(
                    "--hydrate cannot be combined with --labels; decoded reads do not include labels".into(),
                ));
            }
            bounded_integer(args, "max", 20, 50)?;
            ContextLimits::from_args(args)?;
        }
        "google.mail.search" if args.value("body-limit").is_some() => {
            return Err(Error::InvalidArguments("--body-limit requires --hydrate".into()));
        }
        "google.mail.thread" => {
            bounded_integer(args, "max-messages", 20, 100)?;
            ContextLimits::from_args(args)?;
        }
        _ => {}
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) struct ContextLimits {
    body: usize,
    attachments: usize,
    started: Instant,
}

impl ContextLimits {
    pub(super) fn from_args(args: &ToolArguments) -> Result<Self> {
        Ok(Self {
            body: bounded_integer(args, "body-limit", 4_000, 20_000)?,
            attachments: bounded_integer(args, "max-attachments", 20, 100)?,
            started: Instant::now(),
        })
    }
}

pub(super) fn bounded_integer(args: &ToolArguments, name: &str, default: usize, max: usize) -> Result<usize> {
    let parsed = args.value(name).map(str::parse::<usize>).transpose();
    match parsed {
        Ok(value) if (1..=max).contains(&value.unwrap_or(default)) => Ok(value.unwrap_or(default)),
        _ => Err(Error::InvalidArguments(format!(
            "--{name} must be an integer between 1 and {max}"
        ))),
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Mailbox {
    name: Option<String>,
    email: String,
}

/// The existing `google.mail.read` projection, retaining its structured addresses.
/// Context reads select decoded text; HTML stays available through the full reader.
#[derive(Debug, Deserialize, Serialize)]
struct MessageContent {
    rfc_message_id: Option<String>,
    references: Vec<String>,
    from: Mailbox,
    reply_to: Option<Vec<Mailbox>>,
    to: Vec<Mailbox>,
    cc: Option<Vec<Mailbox>>,
    subject: String,
    date: Option<String>,
    body_text: String,
}

#[derive(Deserialize)]
struct ReadMessage {
    gmail_message_id: String,
    gmail_thread_id: Option<String>,
    #[serde(flatten)]
    content: MessageContent,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct ContextMessage {
    gmail_message_id: String,
    gmail_thread_id: Option<String>,
    coverage: CoverageStatus,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    content: Option<MessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body_chars: Option<usize>,
    body_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachments: Option<Vec<Attachment>>,
    attachments_omitted: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachment_coverage: Option<CoverageStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<Failure>,
}

pub(super) struct ContextSource {
    pub(super) id: String,
    pub(super) thread_id: Option<String>,
    pub(super) payload: Option<Part>,
}

impl ContextSource {
    pub(super) fn message(id: String, thread_id: Option<String>) -> Self {
        Self {
            id,
            thread_id,
            payload: None,
        }
    }
}

pub(super) struct ContextCollection {
    pub(super) messages: Vec<ContextMessage>,
    pub(super) failures: Vec<Failure>,
    pub(super) truncated: bool,
    pub(super) unknown: bool,
}

impl ContextCollection {
    pub(super) fn read_count(&self) -> usize {
        self.messages.iter().filter(|message| message.content.is_some()).count()
    }

    pub(super) fn coverage(&self, next_cursor: Option<String>) -> ReadCoverage {
        let mut coverage = ReadCoverage::page(next_cursor);
        if self.unknown || !self.failures.is_empty() {
            coverage.status = CoverageStatus::Unknown;
        } else if self.truncated {
            coverage.status = CoverageStatus::Truncated;
        }
        coverage
    }

    pub(super) fn append_refs(&self, target: &ExecutionTarget, output: &mut ToolOutput) -> Result<()> {
        let mut threads = output
            .refs
            .iter()
            .filter(|reference| reference.kind == ToolRefKind::Thread)
            .map(|reference| reference.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        for message in &self.messages {
            let mut reference = ToolRef::new(
                target.namespace.provider.clone(),
                output.namespace.clone(),
                ToolRefKind::Message,
                &message.gmail_message_id,
            )?;
            if let Some(thread) = &message.gmail_thread_id {
                reference = reference.with_parent_id(thread)?;
                if threads.insert(thread.clone()) {
                    output.refs.push(ToolRef::new(
                        target.namespace.provider.clone(),
                        output.namespace.clone(),
                        ToolRefKind::Thread,
                        thread,
                    )?);
                }
            }
            output.refs.push(reference);
        }
        Ok(())
    }
}

impl GoogleWorkspaceAdapter {
    pub(super) fn read_context(
        &self,
        target: &ExecutionTarget,
        action: &PlannedAction,
        sources: &[ContextSource],
        limits: ContextLimits,
    ) -> Result<ContextCollection> {
        let mut collection = ContextCollection {
            messages: Vec::with_capacity(sources.len()),
            failures: Vec::new(),
            truncated: false,
            unknown: false,
        };
        let mut blocked = false;
        for chunk in sources.chunks(4) {
            // Preserve the caller's deadline. Direct context reads also stop
            // scheduling after two minutes; the current wave retains its normal
            // bounded subprocess timeout without mutating process-global state.
            if !blocked
                && (limits.started.elapsed() >= Duration::from_secs(120)
                    || switchboard_core::process::remaining_timeout(Duration::from_secs(120)).is_err())
            {
                collection.failures.push(
                    Failure::from_error(&Error::TimedOut {
                        program: "Gmail context read".into(),
                        seconds: limits.started.elapsed().as_secs(),
                    })
                    .with_namespace(action.namespace.clone()),
                );
                blocked = true;
            }
            let results = if blocked {
                chunk.iter().map(|_| Ok(None)).collect::<Vec<_>>()
            } else {
                std::thread::scope(|scope| {
                    let handles = chunk
                        .iter()
                        .map(|source| scope.spawn(move || self.read_context_message(target, action, source).map(Some)))
                        .collect::<Vec<_>>();
                    handles
                        .into_iter()
                        .map(|handle| {
                            handle
                                .join()
                                .unwrap_or_else(|_| Err(Error::Execution("message context worker failed".into())))
                        })
                        .collect::<Vec<_>>()
                })
            };
            for (source, result) in chunk.iter().zip(results) {
                let mut row = ContextMessage {
                    gmail_message_id: source.id.clone(),
                    gmail_thread_id: source.thread_id.clone(),
                    coverage: CoverageStatus::Unknown,
                    content: None,
                    body_chars: None,
                    body_truncated: false,
                    attachments: None,
                    attachments_omitted: 0,
                    attachment_coverage: None,
                    failure: None,
                };
                if let Some(payload) = &source.payload {
                    let mut items = Vec::new();
                    let mut unknown = false;
                    payload.attachments(
                        &mut items,
                        &mut row.attachments_omitted,
                        &mut unknown,
                        limits.attachments,
                    );
                    row.attachments = Some(items);
                    row.attachment_coverage = Some(if unknown {
                        CoverageStatus::Unknown
                    } else if row.attachments_omitted > 0 {
                        CoverageStatus::Truncated
                    } else {
                        CoverageStatus::Complete
                    });
                    collection.unknown |= unknown;
                    collection.truncated |= row.attachments_omitted > 0;
                }
                match result {
                    Ok(Some(mut message)) => {
                        row.gmail_thread_id = message.gmail_thread_id;
                        let chars = message.content.body_text.chars().count();
                        row.body_chars = Some(chars);
                        row.body_truncated = chars > limits.body;
                        if row.body_truncated {
                            let end = message
                                .content
                                .body_text
                                .char_indices()
                                .nth(limits.body)
                                .map_or(message.content.body_text.len(), |(at, _)| at);
                            message.content.body_text.truncate(end);
                        }
                        collection.truncated |= row.body_truncated;
                        row.content = Some(message.content);
                        row.coverage = if row.attachment_coverage == Some(CoverageStatus::Unknown) {
                            CoverageStatus::Unknown
                        } else if row.body_truncated || row.attachments_omitted > 0 {
                            CoverageStatus::Truncated
                        } else {
                            CoverageStatus::Complete
                        };
                    }
                    Err(error) => {
                        let failure = Failure::from_error(&error).with_namespace(action.namespace.clone());
                        blocked |= failure.phase == FailurePhase::Authentication;
                        row.failure = Some(failure.clone());
                        collection.failures.push(failure);
                    }
                    Ok(None) => collection.unknown = true,
                }
                collection.messages.push(row);
            }
        }
        Ok(collection)
    }

    fn read_context_message(
        &self,
        target: &ExecutionTarget,
        action: &PlannedAction,
        source: &ContextSource,
    ) -> Result<ReadMessage> {
        let spec = self
            .catalog
            .find_command("google.mail.read")
            .and_then(|command| command.executable.as_ref())
            .ok_or_else(|| Error::NotImplemented("decoded Gmail reads are unavailable".into()))?;
        let mut read = action.clone();
        read.tool = ToolName::new("google.mail.read")?;
        read.args = vec![ToolArgument::option("id", &source.id)?].into();
        let output = self.backend.execute(target, &read, spec)?;
        let message: ReadMessage = serde_json::from_value(
            output
                .fields
                .get("message")
                .cloned()
                .ok_or_else(|| Error::Execution("decoded Gmail read did not return a message".into()))?,
        )
        .map_err(|error| Error::Execution(format!("invalid decoded Gmail message: {error}")))?;
        if message.gmail_message_id != source.id
            || source
                .thread_id
                .as_ref()
                .is_some_and(|expected| message.gmail_thread_id.as_ref() != Some(expected))
        {
            return Err(Error::Execution(
                "Gmail returned a message from a different source thread".into(),
            ));
        }
        Ok(message)
    }
}

pub(super) fn encode(value: &impl Serialize) -> Result<serde_json::Value> {
    serde_json::to_value(value).map_err(|error| Error::Execution(format!("failed to encode message context: {error}")))
}

#[cfg(test)]
mod tests;
