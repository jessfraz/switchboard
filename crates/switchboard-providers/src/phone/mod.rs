mod request;
pub(crate) mod runtime;
#[cfg(test)]
mod tests;

use std::time::Duration;

use serde::{de::DeserializeOwned, Serialize};
use switchboard_core::{
    Adapter, BackendKind, Error, ExecutionTarget, OperationEffect, PlannedAction, PlanningTarget, ProviderKind, Result,
    ToolArgumentSpec, ToolArgumentTransport, ToolArgumentValueKind, ToolDescriptor, ToolKind, ToolOutput, ToolRequest,
};

use crate::phone::request::{CallResult, DoctorResult, RunRequest, TranscriptList};

pub struct PhoneAdapter {
    tools: Vec<ToolDescriptor>,
}

impl PhoneAdapter {
    pub fn new() -> Result<Self> {
        let arguments = ["destination", "task", "caller-name", "max-duration-seconds"]
            .into_iter()
            .map(|name| {
                ToolArgumentSpec::new(name, ToolArgumentTransport::Option, ToolArgumentValueKind::String)
                    .map(|spec| spec.with_required(name != "max-duration-seconds"))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            tools: vec![
                ToolDescriptor::new(
                    "phone.call.run",
                    ToolKind::Write,
                    "Make one approved business call and save its encrypted transcript",
                    BackendKind::Cli,
                )?
                .with_arguments(arguments),
                ToolDescriptor::new(
                    "phone.doctor",
                    ToolKind::Read,
                    "Check local phone configuration without contacting a provider",
                    BackendKind::Cli,
                )?,
                ToolDescriptor::new(
                    "phone.transcripts.list",
                    ToolKind::Read,
                    "List local encrypted call records without decrypting them",
                    BackendKind::Cli,
                )?,
            ],
        })
    }
}

impl Adapter for PhoneAdapter {
    fn provider(&self) -> ProviderKind {
        ProviderKind::Phone
    }
    fn tools(&self) -> &[ToolDescriptor] {
        &self.tools
    }

    fn plan(
        &self,
        target: &PlanningTarget,
        request: &ToolRequest,
        descriptor: &ToolDescriptor,
    ) -> Result<PlannedAction> {
        runtime::state_dir(target.namespace.state_dir.as_deref())?;
        let summary = match request.tool.as_str() {
            "phone.call.run" => {
                let input = RunRequest::parse(&request.args)?;
                format!(
                    "Call {} on behalf of {} for at most {} seconds: {}",
                    input.destination, input.caller_name, input.max_duration_seconds, input.task
                )
            }
            "phone.doctor" | "phone.transcripts.list" => {
                if request.args.iter().next().is_some() {
                    return Err(Error::InvalidArguments(
                        "this phone read tool accepts no arguments".into(),
                    ));
                }
                descriptor.summary.clone()
            }
            _ => return Err(Error::UnsupportedTool(request.tool.to_string())),
        };
        let mut action = PlannedAction::new(request, target, descriptor.kind, summary, BackendKind::Cli);
        if request.tool.as_str() == "phone.call.run" {
            action.approval_required = true;
            action.approval_reason =
                Some("approve this exact destination, brief, and duration before placing the call".into());
        }
        Ok(action)
    }

    fn execute(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
        let mut command = runtime::command(target)?;
        match action.tool.as_str() {
            "phone.call.run" => {
                let id = action
                    .operation_id
                    .as_ref()
                    .ok_or_else(|| Error::Operation("phone call requires a persisted operation ID".into()))?
                    .to_string();
                let mut input = RunRequest::parse(&action.args)?;
                input.call_id = Some(&id);
                let timeout = Duration::from_secs(input.max_duration_seconds + 120);
                let bytes = serde_json::to_vec(&input)
                    .map_err(|_| Error::Execution("could not encode phone request".into()))?;
                command.args(["run", "--request-stdin", "--approve"]);
                let output = runtime::capture(command, bytes, timeout)?;
                // Terminal failures still describe a call that happened. Preserve
                // that outcome and never turn a retry into a second dial.
                let result: CallResult = parse(&output.stdout)?;
                let state = runtime::state_dir(target.namespace.state_dir.as_deref())?;
                if result.call_id != id || result.transcript_path != state.join("calls").join(&id) {
                    return Err(Error::Execution("phone returned a mismatched call record".into()));
                }
                response(
                    action,
                    "Phone call ended; inspect result status and hangup confirmation",
                    &result,
                )
                .map(|output| output.with_effect(OperationEffect::new(false)))
            }
            "phone.doctor" => {
                command.arg("doctor");
                let output = runtime::capture(command, Vec::new(), Duration::from_secs(15))?;
                let result: DoctorResult = parse(&output.stdout)?;
                response(action, "Checked local phone configuration", &result)
            }
            "phone.transcripts.list" => {
                command.args(["transcripts", "list"]);
                let output = runtime::capture(command, Vec::new(), Duration::from_secs(15))?;
                let result: TranscriptList = parse(&output.stdout)?;
                response(action, "Listed encrypted call records", &result)
            }
            _ => Err(Error::UnsupportedTool(action.tool.to_string())),
        }
    }
}

fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|_| Error::Execution("phone did not return a valid result; check configuration with phone doctor and inspect the encrypted call record before retrying".into()))
}

fn response<T: Serialize>(action: &PlannedAction, summary: &str, value: &T) -> Result<ToolOutput> {
    let value = serde_json::to_value(value).map_err(|_| Error::Execution("could not encode phone result".into()))?;
    Ok(ToolOutput::new(action.tool.clone(), action.namespace.clone(), summary).with_value_field("response", value))
}
