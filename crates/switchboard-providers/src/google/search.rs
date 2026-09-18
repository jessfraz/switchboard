use serde::{Deserialize, Serialize};
use switchboard_core::{
    CoverageStatus, Error, ExecutionTarget, PlannedAction, ReadCoverage, Result, ToolOutput, ToolRef, ToolRefKind,
};

use crate::{cli::CliStdioMode, google::GoogleWorkspaceAdapter};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchParams<'a> {
    user_id: &'static str,
    q: &'a str,
    max_results: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_token: Option<&'a str>,
    include_spam_trash: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MetadataParams<'a> {
    user_id: &'static str,
    id: &'a str,
    format: &'static str,
    metadata_headers: [&'static str; 3],
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchPage {
    #[serde(default)]
    messages: Vec<MessageId>,
    next_page_token: Option<String>,
    result_size_estimate: Option<u64>,
}

#[derive(Deserialize)]
struct MessageId {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageMetadata {
    id: String,
    #[serde(default)]
    label_ids: Vec<String>,
    payload: Option<MessagePayload>,
}

#[derive(Deserialize)]
struct MessagePayload {
    #[serde(default)]
    headers: Vec<Header>,
}

#[derive(Deserialize)]
struct Header {
    name: String,
    value: String,
}

#[derive(Serialize)]
struct SearchMessage {
    gmail_message_id: String,
    from: Option<String>,
    subject: Option<String>,
    date: Option<String>,
    labels: Option<Vec<String>>,
}

impl MessageMetadata {
    fn into_search_message(self, include_labels: bool) -> SearchMessage {
        let header = |name: &str| {
            self.payload
                .as_ref()
                .and_then(|payload| {
                    payload
                        .headers
                        .iter()
                        .find(|header| header.name.eq_ignore_ascii_case(name))
                })
                .map(|header| header.value.clone())
        };
        SearchMessage {
            from: header("From"),
            subject: header("Subject"),
            date: header("Date"),
            gmail_message_id: self.id,
            labels: include_labels.then_some(self.label_ids),
        }
    }
}

impl GoogleWorkspaceAdapter {
    pub(super) fn search(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
        let query = action
            .args
            .value("query")
            .ok_or_else(|| Error::InvalidArguments("missing --query".into()))?;
        let max_results = action
            .args
            .value("max")
            .unwrap_or("20")
            .parse::<u32>()
            .map_err(|_| Error::InvalidArguments("--max must be an integer between 1 and 500".into()))?;
        if !(1..=500).contains(&max_results) {
            return Err(Error::InvalidArguments("--max must be between 1 and 500".into()));
        }
        let params = SearchParams {
            user_id: "me",
            q: query,
            max_results,
            page_token: action.args.value("cursor"),
            include_spam_trash: query.contains("in:anywhere"),
        };
        let page: SearchPage = self.read_json(target, &["gmail", "users", "messages", "list"], &params)?;
        let mut output = ToolOutput::new(
            action.tool.clone(),
            action.namespace.clone(),
            format!("Found {} Gmail messages for {}", page.messages.len(), action.namespace),
        );
        let mut messages = Vec::with_capacity(page.messages.len());
        let mut failures = Vec::new();
        let mut authentication_blocked = false;
        // The first list call establishes auth before bounded metadata fan-out.
        for chunk in page.messages.chunks(4) {
            let results = if authentication_blocked {
                chunk.iter().map(|_| Ok(None)).collect::<Vec<_>>()
            } else {
                std::thread::scope(|scope| {
                    let handles = chunk
                        .iter()
                        .map(|message| {
                            scope.spawn(move || {
                                self.read_json::<_, MessageMetadata>(
                                    target,
                                    &["gmail", "users", "messages", "get"],
                                    &MetadataParams {
                                        user_id: "me",
                                        id: &message.id,
                                        format: "metadata",
                                        metadata_headers: ["From", "Subject", "Date"],
                                    },
                                )
                                .map(Some)
                            })
                        })
                        .collect::<Vec<_>>();
                    handles
                        .into_iter()
                        .map(|handle| {
                            handle
                                .join()
                                .unwrap_or_else(|_| Err(Error::Execution("message metadata worker failed".into())))
                        })
                        .collect::<Vec<_>>()
                })
            };
            for (message, result) in chunk.iter().zip(results) {
                let row = match result {
                    Ok(Some(metadata)) if metadata.id == message.id => {
                        metadata.into_search_message(action.args.has_flag("labels"))
                    }
                    Ok(Some(_)) => {
                        return Err(Error::Execution(
                            "Gmail returned metadata for a different message".into(),
                        ))
                    }
                    Err(error) => {
                        let failure =
                            switchboard_core::Failure::from_error(&error).with_namespace(action.namespace.clone());
                        authentication_blocked |= failure.phase == switchboard_core::FailurePhase::Authentication;
                        failures.push(failure);
                        SearchMessage {
                            gmail_message_id: message.id.clone(),
                            from: None,
                            subject: None,
                            date: None,
                            labels: None,
                        }
                    }
                    Ok(None) => SearchMessage {
                        gmail_message_id: message.id.clone(),
                        from: None,
                        subject: None,
                        date: None,
                        labels: None,
                    },
                };
                let mut reference = ToolRef::new(
                    target.namespace.provider.clone(),
                    action.namespace.clone(),
                    ToolRefKind::Message,
                    &row.gmail_message_id,
                )?;
                if let Some(subject) = row.subject.as_ref().filter(|subject| !subject.trim().is_empty()) {
                    reference = reference.with_label(subject)?;
                }
                output.refs.push(reference);
                messages.push(row);
            }
        }
        let mut coverage = ReadCoverage::page(page.next_page_token);
        if !failures.is_empty() {
            coverage.status = CoverageStatus::Unknown;
        }
        output.coverage = Some(coverage);
        output = output
            .with_field("status", if failures.is_empty() { "ok" } else { "partial" })
            .with_field("query", query)
            .with_value_field("count", encode(&messages.len())?)
            .with_value_field("messages", encode(&messages)?)
            .with_value_field("result_size_estimate", encode(&page.result_size_estimate)?)
            .with_value_field("failures", encode(&failures)?);
        Ok(output)
    }

    pub(super) fn read_json<P: Serialize, T: serde::de::DeserializeOwned>(
        &self,
        target: &ExecutionTarget,
        path: &[&str],
        params: &P,
    ) -> Result<T> {
        let spec = self
            .catalog
            .find_command("google.cli.read")
            .and_then(|command| command.executable.as_ref())
            .ok_or_else(|| Error::NotImplemented("Google raw reads are unavailable".into()))?;
        let mut argv = path.iter().map(|part| (*part).to_owned()).collect::<Vec<_>>();
        argv.extend([
            "--params".into(),
            serde_json::to_string(params).map_err(|error| Error::InvalidArguments(error.to_string()))?,
            "--format".into(),
            "json".into(),
        ]);
        let response = self.backend.execute_raw(target, spec, argv, CliStdioMode::Capture)?;
        serde_json::from_str(&response.stdout)
            .map_err(|error| Error::Execution(format!("invalid Google response: {error}")))
    }
}

fn encode(value: &impl Serialize) -> Result<serde_json::Value> {
    serde_json::to_value(value).map_err(|error| Error::Execution(format!("failed to encode search result: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_preserves_message_identity_and_case_insensitive_headers() {
        let metadata: MessageMetadata = serde_json::from_str(r#"{"id":"m1","labelIds":["INBOX"],"payload":{"headers":[{"name":"sUbJeCt","value":"Example"},{"name":"From","value":"sender@example.com"}]}}"#).expect("test setup should succeed");
        let result = metadata.into_search_message(true);
        assert_eq!(result.gmail_message_id, "m1");
        assert_eq!(result.subject.as_deref(), Some("Example"));
        assert_eq!(result.labels, Some(vec!["INBOX".to_owned()]));
        assert_eq!(result.date, None);
    }

    #[test]
    fn an_empty_page_can_still_have_a_continuation() {
        let page: SearchPage = serde_json::from_str(r#"{"nextPageToken":"next","resultSizeEstimate":2}"#)
            .expect("test setup should succeed");
        assert!(page.messages.is_empty());
        let coverage = ReadCoverage::page(page.next_page_token);
        assert_eq!(coverage.status, CoverageStatus::Truncated);
        assert_eq!(coverage.next_cursor.as_deref(), Some("next"));
        assert!(serde_json::from_str::<SearchPage>("").is_err());
    }
}
