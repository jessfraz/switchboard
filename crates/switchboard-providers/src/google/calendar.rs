use serde::Serialize;
use switchboard_core::{Error, ExecutionTarget, OperationId, PlannedAction, Result, ToolOutput};

use crate::{cli::CliStdioMode, google::GoogleWorkspaceAdapter};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InsertParams<'a> {
    calendar_id: &'a str,
    conference_data_version: u8,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Event<'a> {
    id: String,
    summary: &'a str,
    start: EventTime<'a>,
    end: EventTime<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<&'a str>,
    attendees: Vec<Attendee<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conference_data: Option<ConferenceData>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventTime<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    date_time: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<&'a str>,
}

impl<'a> EventTime<'a> {
    fn new(value: &'a str) -> Self {
        if value.len() == 10 {
            Self {
                date: Some(value),
                date_time: None,
            }
        } else {
            Self {
                date_time: Some(value),
                date: None,
            }
        }
    }
}

#[derive(Serialize)]
struct Attendee<'a> {
    email: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConferenceData {
    create_request: ConferenceRequest,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConferenceRequest {
    request_id: String,
}

// Hex-encoding the durable operation ID satisfies Calendar's base32hex ID alphabet.
// The same stored operation always addresses the same event, including after a crash.
pub(super) fn event_id(id: &OperationId) -> String {
    id.as_str().bytes().map(|byte| format!("{byte:02x}")).collect()
}

impl GoogleWorkspaceAdapter {
    pub(super) fn create_calendar_event(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
        let required = |name: &str| {
            action
                .args
                .value(name)
                .ok_or_else(|| Error::InvalidArguments(format!("missing --{name}")))
        };
        let id = action
            .operation_id
            .as_ref()
            .ok_or_else(|| Error::InvalidArguments("calendar creation requires a durable operation ID".into()))?;
        let event = Event {
            id: event_id(id),
            summary: action
                .args
                .value("title")
                .or(action.args.value("summary"))
                .ok_or_else(|| Error::InvalidArguments("missing --title".into()))?,
            start: EventTime::new(required("start")?),
            end: EventTime::new(required("end")?),
            description: action.args.value("description"),
            location: action.args.value("location"),
            attendees: action.args.values("attendee").map(|email| Attendee { email }).collect(),
            conference_data: action.args.has_flag("meet").then(|| ConferenceData {
                create_request: ConferenceRequest {
                    request_id: event_id(id),
                },
            }),
        };
        let params = InsertParams {
            calendar_id: action.args.value("calendar").unwrap_or("primary"),
            conference_data_version: 1,
        };
        let spec = self
            .catalog
            .find_command("google.cli.write")
            .and_then(|command| command.executable.as_ref())
            .ok_or_else(|| Error::UnsupportedTool("google.cli.write".into()))?;
        let argv = vec![
            "calendar".into(),
            "events".into(),
            "insert".into(),
            "--params".into(),
            encode(&params)?,
            "--json".into(),
            encode(&event)?,
            "--format".into(),
            "json".into(),
        ];
        let response = self.backend.execute_raw(target, spec, argv, CliStdioMode::Capture)?;
        let curated = self
            .catalog
            .find_command("google.calendar.create")
            .and_then(|command| command.executable.as_ref())
            .ok_or_else(|| Error::UnsupportedTool("google.calendar.create".into()))?;
        curated.decode.decode(target, action, response)
    }
}

fn encode(value: &impl Serialize) -> Result<String> {
    serde_json::to_string(value).map_err(|error| Error::InvalidArguments(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_ids_preserve_operation_identity_with_provider_safe_characters() {
        let first = event_id(&OperationId::new("op_example-123").expect("test setup should succeed"));
        assert_eq!(
            first,
            event_id(&OperationId::new("op_example-123").expect("test setup should succeed"))
        );
        assert_ne!(
            first,
            event_id(&OperationId::new("op_example-124").expect("test setup should succeed"))
        );
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
