use serde::{Deserialize, Serialize};
use switchboard_core::{Error, Result, ToolArgument, ToolArguments};

/// The standalone CLI's input contract. Personal details travel over stdin.
#[derive(Serialize)]
pub(crate) struct RunRequest<'a> {
    pub call_id: Option<&'a str>,
    pub destination: &'a str,
    pub task: &'a str,
    pub caller_name: &'a str,
    pub max_duration_seconds: u64,
}

impl<'a> RunRequest<'a> {
    pub fn parse(args: &'a ToolArguments) -> Result<Self> {
        let mut seen = std::collections::BTreeSet::new();
        for argument in args.iter() {
            if !matches!(argument, ToolArgument::Option { .. })
                || !matches!(
                    argument.name(),
                    "destination" | "task" | "caller-name" | "max-duration-seconds"
                )
                || !seen.insert(argument.name())
            {
                return Err(Error::InvalidArguments(
                    "phone call accepts each declared option exactly once".into(),
                ));
            }
        }
        let required = |name| {
            args.value(name)
                .ok_or_else(|| Error::InvalidArguments(format!("missing --{name}")))
        };
        let destination = required("destination")?;
        let digits = destination.strip_prefix('+').unwrap_or_default();
        if !(8..=15).contains(&digits.len()) || digits.starts_with('0') || !digits.bytes().all(|c| c.is_ascii_digit()) {
            return Err(Error::InvalidArguments("destination must use E.164 format".into()));
        }
        let task = required("task")?;
        let caller_name = required("caller-name")?;
        if task.trim().is_empty()
            || task.len() > 8000
            || task.contains('\0')
            || caller_name.trim().is_empty()
            || caller_name.len() > 80
            || caller_name.chars().any(|character| character < ' ')
        {
            return Err(Error::InvalidArguments(
                "task must contain 1 to 8000 bytes and caller-name 1 to 80 bytes".into(),
            ));
        }
        let max_duration_seconds = args
            .value("max-duration-seconds")
            .unwrap_or("600")
            .parse::<u64>()
            .map_err(|_| Error::InvalidArguments("max-duration-seconds must be an integer".into()))?;
        if !(30..=3600).contains(&max_duration_seconds) {
            return Err(Error::InvalidArguments(
                "max-duration-seconds must be between 30 and 3600".into(),
            ));
        }
        Ok(Self {
            call_id: None,
            destination,
            task,
            caller_name,
            max_duration_seconds,
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CallResult {
    pub call_id: String,
    pub status: CallStatus,
    pub transcript_path: std::path::PathBuf,
    pub remote_hangup_confirmed: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CallStatus {
    Completed,
    Cancelled,
    Timeout,
    ApprovalRequired,
    Failed,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DoctorResult {
    pub configuration_valid: bool,
    pub encryption_ready: bool,
    pub worker_project_exists: bool,
    pub credentials_present: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TranscriptList {
    pub calls: Vec<String>,
}
