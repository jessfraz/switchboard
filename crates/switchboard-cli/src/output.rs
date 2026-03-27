use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value as JsonValue;
use switchboard_core::{
    AggregateReadOutcome, ApprovalState, BackendKind, DispatchOutcome, NamespaceId, OperationEffect, OperationId,
    OperationOutcome, ProviderKind, RegisteredTool, ResolvedNamespace, StoredAuditEvent, StoredOperation,
    ToolArgument, ToolArgumentSpec, ToolArguments, ToolExecutionSupport, ToolKind, ToolName, ToolOutput, ToolRef,
    ToolSurface, ToolUndoSupport,
};

use crate::args::AuditSelector;

pub(crate) fn render_namespaces_human(namespaces: &[ResolvedNamespace]) -> String {
    let mut output = String::from("Namespaces\n");
    for namespace in namespaces {
        let state_dir = namespace
            .state_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".into());
        output.push_str(&format!(
            "- {} ({}, account={}, auth={}, default_read={}, state_dir={})\n",
            namespace.id,
            namespace.provider,
            namespace.account_label,
            namespace.auth_ref,
            namespace.default_read,
            state_dir
        ));
    }

    output
}

pub(crate) fn render_audit_events_human(events: &[StoredAuditEvent]) -> String {
    let mut output = String::from("Audit Events\n");
    for event in events {
        output.push_str(&format!(
            "- {} {} {} outcome={} recorded_at={}\n",
            event.id,
            event.tool,
            event.namespace,
            render_audit_outcome(&event.outcome),
            event.recorded_at
        ));
    }

    output
}

pub(crate) enum AuditSelection {
    Single(StoredAuditEvent),
    Operation(OperationId, Vec<StoredAuditEvent>),
}

pub(crate) fn render_audit_selection_human(selection: &AuditSelection) -> String {
    match selection {
        AuditSelection::Single(event) => render_audit_event_human(event),
        AuditSelection::Operation(operation_id, events) => {
            let mut output = format!("Audit for operation {operation_id}\n");
            if events.is_empty() {
                output.push_str("- no audit events recorded\n");
            } else {
                for event in events {
                    output.push_str(&format!(
                        "- {} outcome={} recorded_at={} summary={}\n",
                        event.id,
                        render_audit_outcome(&event.outcome),
                        event.recorded_at,
                        event.summary
                    ));
                }
            }
            output
        }
    }
}

pub(crate) fn render_audit_event_human(event: &StoredAuditEvent) -> String {
    let mut output = String::new();
    output.push_str(&format!("Audit Event: {}\n", event.id));
    output.push_str(&format!("Recorded at: {}\n", event.recorded_at));
    output.push_str(&format!("Outcome: {}\n", render_audit_outcome(&event.outcome)));
    output.push_str(&format!("Tool: {}\n", event.tool));
    output.push_str(&format!("Namespace: {}\n", event.namespace));
    output.push_str(&format!("Auth: {}\n", event.auth_ref));
    output.push_str(&format!("Backend: {}\n", event.backend));
    output.push_str(&format!("Summary: {}\n", event.summary));
    output.push_str(&format!("Approval required: {}\n", event.approval_required));
    if let Some(operation_id) = &event.operation_id {
        output.push_str(&format!("Operation ID: {operation_id}\n"));
    }
    if let Some(operation_id) = &event.compensates_operation_id {
        output.push_str(&format!("Compensates: {operation_id}\n"));
    }
    output
}

pub(crate) fn render_tools_human(tools: &[RegisteredTool]) -> String {
    let mut output = String::from("Tools\n");
    for tool in tools {
        let mut qualifiers = vec![
            tool.provider.to_string(),
            render_tool_kind(tool.kind).to_owned(),
            tool.backend.to_string(),
        ];
        qualifiers.push(render_tool_surface(tool.surface).to_owned());
        qualifiers.push(render_execution_support(tool.execution_support).to_owned());
        if tool.undo_support != ToolUndoSupport::None {
            qualifiers.push(render_undo_support(tool.undo_support).to_owned());
        }
        output.push_str(&format!(
            "- {} [{}] {}\n",
            tool.name,
            qualifiers.join(", "),
            tool.summary
        ));
    }

    output
}

pub(crate) fn render_tool_detail_human(detail: &ToolCatalogDetail) -> String {
    let mut output = String::new();
    output.push_str(&format!("Tool: {}\n", detail.name));
    output.push_str(&format!("Provider: {}\n", detail.provider));
    output.push_str(&format!("Kind: {}\n", render_tool_kind(detail.kind)));
    output.push_str(&format!("Backend: {}\n", detail.backend));
    output.push_str(&format!("Surface: {}\n", render_tool_surface(detail.surface)));
    output.push_str(&format!(
        "Execution: {}\n",
        render_execution_support(detail.execution_support)
    ));
    output.push_str(&format!("Undo: {}\n", render_undo_support(detail.undo_support)));
    output.push_str(&format!("Summary: {}\n", detail.summary));
    output.push_str(&format!(
        "Aggregate reads: {}\n",
        if detail.aggregate_read_supported {
            "supported"
        } else {
            "not supported"
        }
    ));
    output.push_str("Arguments:\n");
    if detail.arguments.is_empty() {
        output.push_str("- none\n");
    } else {
        for argument in &detail.arguments {
            output.push_str(&format!("- {}\n", render_tool_argument_spec_human(argument)));
        }
    }
    output.push_str("Namespaces:\n");
    if detail.available_namespaces.is_empty() {
        output.push_str("- none configured\n");
    } else {
        for namespace in &detail.available_namespaces {
            output.push_str(&format!("- {namespace}\n"));
        }
    }

    if !detail.notes.is_empty() {
        output.push_str("Notes:\n");
        for note in &detail.notes {
            output.push_str(&format!("- {note}\n"));
        }
    }

    if !detail.examples.is_empty() {
        output.push_str("Examples:\n");
        for example in &detail.examples {
            output.push_str(&format!("- {example}\n"));
        }
    }

    output
}

pub(crate) fn render_tool_argument_spec_human(argument: &ToolArgumentSpec) -> String {
    let mut qualifiers = vec![
        format!("transport={}", render_tool_argument_transport(argument)),
        format!("type={}", render_tool_argument_value_kind(argument)),
    ];
    if argument.required {
        qualifiers.push("required".to_owned());
    }
    if argument.repeated {
        qualifiers.push("repeated".to_owned());
    }
    if let Some(flag) = argument.forwarded_flag.as_ref() {
        let forwarded = match argument.forwarded_key.as_ref() {
            Some(key) => format!("{flag} {key}"),
            None => flag.clone(),
        };
        qualifiers.push(format!("forwarded={forwarded}"));
    }

    let mut rendered = format!("{} [{}]", argument.name, qualifiers.join(", "));
    if !argument.aliases.is_empty() {
        rendered.push_str(&format!(" aliases={}", argument.aliases.join("|")));
    }

    rendered
}

pub(crate) fn render_operations_human(operations: &[StoredOperation]) -> String {
    let mut output = String::from("Operations\n");
    if operations.is_empty() {
        output.push_str("- no operations\n");
        return output;
    }

    for operation in operations {
        output.push_str(&format!(
            "- {} {} {} approval={} status={}\n",
            operation.id,
            operation.tool,
            operation.namespace,
            render_approval_state(operation.approval.state),
            render_operation_status(operation.status)
        ));
    }

    output
}

pub(crate) fn render_stored_operation_human(operation: &StoredOperation) -> String {
    let mut output = String::new();
    output.push_str(&format!("Operation: {}\n", operation.id));
    output.push_str(&format!("Tool: {}\n", operation.tool));
    output.push_str(&format!("Namespace: {}\n", operation.namespace));
    output.push_str(&format!("Summary: {}\n", operation.summary));
    output.push_str(&format!("Backend: {}\n", operation.backend));
    output.push_str(&format!("Status: {}\n", render_operation_status(operation.status)));
    output.push_str(&format!(
        "Approval: {}\n",
        render_approval_state(operation.approval.state)
    ));
    if let Some(operation_id) = &operation.compensates_operation_id {
        output.push_str(&format!("Compensates: {operation_id}\n"));
    }
    if operation.args.iter().next().is_some() {
        output.push_str("Args:\n");
        output.push_str(&render_tool_arguments_human(&operation.args, "  "));
    }
    if let Some(reason) = &operation.approval_reason {
        output.push_str(&format!("Approval reason: {reason}\n"));
    }
    if let Some(actor) = &operation.approval.actor {
        output.push_str(&format!("Approval actor: {actor}\n"));
    }
    if let Some(note) = &operation.approval.note {
        output.push_str(&format!("Approval note: {note}\n"));
    }
    if let Some(reason) = &operation.failure_reason {
        output.push_str(&format!("Failure: {reason}\n"));
    }
    if let Some(effect) = &operation.effect {
        output.push_str(&render_effect_human(effect));
    }

    output
}

pub(crate) fn render_operation_human(outcome: &OperationOutcome) -> String {
    match outcome {
        OperationOutcome::Single(outcome) => render_dispatch_human(outcome),
        OperationOutcome::AggregateRead(outcome) => render_aggregate_read_human(outcome),
    }
}

pub(crate) fn render_dispatch_human(outcome: &DispatchOutcome) -> String {
    match outcome {
        DispatchOutcome::Planned(plan) => {
            let mut output = String::new();
            output.push_str(&format!("Planned: {}\n", plan.summary));
            output.push_str(&format!("Tool: {}\n", plan.tool));
            output.push_str(&format!("Namespace: {}\n", plan.namespace));
            output.push_str(&format!("Backend: {}\n", plan.backend));
            output.push_str(&format!("Approval required: {}\n", plan.approval_required));
            if let Some(operation_id) = &plan.operation_id {
                output.push_str(&format!("Operation ID: {operation_id}\n"));
            }
            if let Some(operation_id) = &plan.compensates_operation_id {
                output.push_str(&format!("Compensates: {operation_id}\n"));
            }
            if plan.args.iter().next().is_some() {
                output.push_str("Args:\n");
                output.push_str(&render_tool_arguments_human(&plan.args, "  "));
            }
            if let Some(reason) = &plan.approval_reason {
                output.push_str(&format!("Approval reason: {reason}\n"));
            }
            output
        }
        DispatchOutcome::Executed(output) => render_output_human(output),
    }
}

pub(crate) fn render_output_human(output: &ToolOutput) -> String {
    let mut rendered = String::new();
    rendered.push_str(&format!("Executed: {}\n", output.summary));
    rendered.push_str(&format!("Tool: {}\n", output.tool));
    rendered.push_str(&format!("Namespace: {}\n", output.namespace));
    if let Some(operation_id) = &output.operation_id {
        rendered.push_str(&format!("Operation ID: {operation_id}\n"));
    }
    if !output.fields.is_empty() {
        rendered.push_str("Fields:\n");
        for (key, value) in &output.fields {
            match value {
                JsonValue::String(value) => rendered.push_str(&format!("- {key}: {value}\n")),
                _ => {
                    rendered.push_str(&format!("- {key}:\n"));
                    let formatted =
                        serde_json::to_string_pretty(value).unwrap_or_else(|_| "<failed to render field>".to_owned());
                    for line in formatted.lines() {
                        rendered.push_str(&format!("  {line}\n"));
                    }
                }
            }
        }
    }
    if !output.refs.is_empty() {
        rendered.push_str("Refs:\n");
        for tool_ref in &output.refs {
            rendered.push_str(&format!("- {}\n", render_ref_human(tool_ref)));
        }
    }
    if let Some(effect) = &output.effect {
        rendered.push_str(&render_effect_human(effect));
    }

    rendered
}

pub(crate) fn render_ref_human(tool_ref: &ToolRef) -> String {
    let mut rendered = format!("{}:{} id={}", tool_ref.provider, tool_ref.kind, tool_ref.id);
    if let Some(parent_id) = &tool_ref.parent_id {
        rendered.push_str(&format!(" parent={parent_id}"));
    }
    if let Some(label) = &tool_ref.label {
        rendered.push_str(&format!(" label={label:?}"));
    }
    if let Some(web_url) = &tool_ref.web_url {
        rendered.push_str(&format!(" url={web_url}"));
    }

    rendered
}

pub(crate) fn render_effect_human(effect: &OperationEffect) -> String {
    let mut rendered = String::from("Effect:\n");
    rendered.push_str(&format!("- undoable: {}\n", effect.undoable));
    if let Some(undo_summary) = &effect.undo_summary {
        rendered.push_str(&format!("- undo_summary: {undo_summary}\n"));
    }
    if !effect.refs.is_empty() {
        rendered.push_str("- refs:\n");
        for tool_ref in &effect.refs {
            rendered.push_str(&format!("  - {}\n", render_ref_human(tool_ref)));
        }
    }

    rendered
}

pub(crate) fn render_tool_arguments_human(arguments: &ToolArguments, indent: &str) -> String {
    let mut rendered = String::new();
    for argument in arguments.iter() {
        match argument {
            ToolArgument::Flag { name } => {
                rendered.push_str(&format!("{indent}- --{name}\n"));
            }
            ToolArgument::Option { name, value } => {
                let value = value.replace('\r', "\\r").replace('\n', "\\n");
                rendered.push_str(&format!("{indent}- --{name}={value}\n"));
            }
        }
    }

    rendered
}

pub(crate) fn render_tool_kind(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Write => "write",
    }
}

pub(crate) fn render_tool_surface(surface: ToolSurface) -> &'static str {
    match surface {
        ToolSurface::Curated => "curated",
        ToolSurface::Raw => "raw",
    }
}

pub(crate) fn render_execution_support(execution_support: ToolExecutionSupport) -> &'static str {
    match execution_support {
        ToolExecutionSupport::PlanningOnly => "planning_only",
        ToolExecutionSupport::Executable => "executable",
    }
}

pub(crate) fn render_undo_support(undo_support: ToolUndoSupport) -> &'static str {
    match undo_support {
        ToolUndoSupport::None => "none",
        ToolUndoSupport::CompensatingAction => "compensating_action",
    }
}

pub(crate) fn render_tool_argument_transport(argument: &ToolArgumentSpec) -> &'static str {
    match argument.transport {
        switchboard_core::ToolArgumentTransport::Positional => "positional",
        switchboard_core::ToolArgumentTransport::Option => "option",
        switchboard_core::ToolArgumentTransport::KeyValueOption => "key_value_option",
        switchboard_core::ToolArgumentTransport::Flag => "flag",
        switchboard_core::ToolArgumentTransport::JsonField => "json_field",
        switchboard_core::ToolArgumentTransport::PassthroughArgv => "passthrough_argv",
    }
}

pub(crate) fn render_tool_argument_value_kind(argument: &ToolArgumentSpec) -> &'static str {
    match argument.value_kind {
        switchboard_core::ToolArgumentValueKind::String => "string",
        switchboard_core::ToolArgumentValueKind::Boolean => "boolean",
        switchboard_core::ToolArgumentValueKind::Json => "json",
    }
}

pub(crate) fn curated_tool_examples(tool: &RegisteredTool, namespace: &str) -> Vec<String> {
    let mode_flag = match tool.kind {
        ToolKind::Read => "--json",
        ToolKind::Write => "--draft",
    };
    vec![format!("switchboard {} --ns {namespace} {mode_flag} ...", tool.name)]
}

pub(crate) fn raw_tool_examples(tool: &RegisteredTool, namespace: &str) -> Vec<String> {
    if let Some(path) = inventory_raw_tool_path(&tool.name) {
        let command = path.join(" ");
        return match tool.kind {
            ToolKind::Read => vec![
                format!("switchboard {} --ns {namespace} --json -- --format json", tool.name),
                format!(
                    "switchboard {} --ns {namespace} --argv-json '[\"--format\",\"json\"]' --json",
                    tool.name
                ),
                format!("# fixed CLI path: {command}"),
            ],
            ToolKind::Write => vec![
                format!(
                    "switchboard {} --ns {namespace} --draft -- --format json ...",
                    tool.name
                ),
                format!(
                    "switchboard {} --ns {namespace} --argv-json '[\"--format\",\"json\",...]' --apply --json",
                    tool.name
                ),
                format!("# fixed CLI path: {command}"),
            ],
        };
    }

    match (tool.provider.clone(), tool.kind) {
        (ProviderKind::GoogleWorkspace, ToolKind::Read) => vec![
            format!(
                "switchboard {} --ns {namespace} --json -- calendar +agenda --format json --today",
                tool.name
            ),
            format!(
                "switchboard {} --ns {namespace} --argv-json '[\"gmail\",\"users\",\"messages\",\"list\",\"--query\",\"from:finance\",\"--format\",\"json\"]' --json",
                tool.name
            ),
        ],
        (ProviderKind::GoogleWorkspace, ToolKind::Write) => vec![
            format!(
                "switchboard {} --ns {namespace} --draft -- gmail users drafts create --params '{{\"userId\":\"me\"}}' --json '{{\"message\":{{\"raw\":\"SGVsbG8=\"}}}}' --format json",
                tool.name
            ),
            format!(
                "switchboard {} --ns {namespace} --argv-json '[\"calendar\",\"events\",\"insert\",\"--summary\",\"Vet visit\",\"--start\",\"2026-04-01T09:00:00-07:00\",\"--end\",\"2026-04-01T10:00:00-07:00\",\"--format\",\"json\"]' --apply --json",
                tool.name
            ),
        ],
        (ProviderKind::GitHub, ToolKind::Read) => vec![
            format!(
                "switchboard {} --ns {namespace} --json -- repo view owner/repo --json name,visibility,defaultBranchRef",
                tool.name
            ),
            format!(
                "switchboard {} --ns {namespace} --argv-json '[\"search\",\"prs\",\"--repo\",\"owner/repo\",\"--state\",\"open\",\"--json\",\"number,title\"]' --json",
                tool.name
            ),
        ],
        (ProviderKind::GitHub, ToolKind::Write) => vec![
            format!(
                "switchboard {} --ns {namespace} --draft -- pr comment 123 --body 'needs tests'",
                tool.name
            ),
            format!(
                "switchboard {} --ns {namespace} --argv-json '[\"issue\",\"edit\",\"77\",\"--add-label\",\"triage\"]' --apply --json",
                tool.name
            ),
        ],
        (_, _) => vec![format!("switchboard {} --ns {namespace} -- ...", tool.name)],
    }
}

pub(crate) fn inventory_raw_tool_path(tool: &ToolName) -> Option<Vec<String>> {
    let segments = tool.as_str().split('.').collect::<Vec<_>>();
    if segments.get(1).copied() != Some("cli") {
        return None;
    }
    if matches!(segments.get(2).copied(), Some("read" | "write")) && segments.len() == 3 {
        return None;
    }

    Some(segments.into_iter().skip(2).map(ToOwned::to_owned).collect())
}

pub(crate) fn render_approval_state(state: ApprovalState) -> &'static str {
    match state {
        ApprovalState::NotRequired => "not_required",
        ApprovalState::Pending => "pending",
        ApprovalState::Approved => "approved",
        ApprovalState::Rejected => "rejected",
    }
}

pub(crate) fn render_operation_status(status: switchboard_core::OperationStatus) -> &'static str {
    match status {
        switchboard_core::OperationStatus::Planned => "planned",
        switchboard_core::OperationStatus::Applied => "applied",
        switchboard_core::OperationStatus::Failed => "failed",
        switchboard_core::OperationStatus::Compensated => "compensated",
    }
}

pub(crate) fn operation_needs_attention(operation: &StoredOperation) -> bool {
    operation.status == switchboard_core::OperationStatus::Planned && operation.approval.state == ApprovalState::Pending
}

pub(crate) fn render_audit_outcome(outcome: &switchboard_core::AuditOutcome) -> &'static str {
    match outcome {
        switchboard_core::AuditOutcome::Planned => "planned",
        switchboard_core::AuditOutcome::Approved => "approved",
        switchboard_core::AuditOutcome::Rejected => "rejected",
        switchboard_core::AuditOutcome::Executed => "executed",
        switchboard_core::AuditOutcome::Failed => "failed",
        switchboard_core::AuditOutcome::Compensated => "compensated",
        switchboard_core::AuditOutcome::Blocked => "blocked",
    }
}

pub(crate) fn render_aggregate_read_human(outcome: &AggregateReadOutcome) -> String {
    let namespaces = outcome
        .namespaces
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let mut output = format!("Aggregate read: {}\nNamespaces: {namespaces}\n", outcome.tool);

    for result in &outcome.results {
        output.push('\n');
        output.push_str(&format!("[{}]\n", result.namespace));

        let rendered = render_dispatch_human(&result.outcome);
        for line in rendered.lines() {
            output.push_str(&format!("  {line}\n"));
        }
    }

    output
}

pub(crate) fn render_json_operation(outcome: &OperationOutcome) -> Result<String> {
    match outcome {
        OperationOutcome::Single(outcome) => render_json_dispatch(outcome),
        OperationOutcome::AggregateRead(outcome) => render_json(
            &AggregateReadResponse {
                status: "aggregate_read",
                tool: &outcome.tool,
                namespaces: &outcome.namespaces,
                results: outcome
                    .results
                    .iter()
                    .map(|result| AggregateReadResultResponse {
                        namespace: &result.namespace,
                        outcome: DispatchResponse::from(&result.outcome),
                    })
                    .collect(),
            },
            true,
        ),
    }
}

pub(crate) fn render_json_dispatch(outcome: &DispatchOutcome) -> Result<String> {
    match outcome {
        DispatchOutcome::Planned(plan) => render_json(&DispatchResponse::from_plan(plan), true),
        DispatchOutcome::Executed(output) => render_json(&DispatchResponse::from_output(output), true),
    }
}

pub(crate) fn render_json_error(message: &str) -> String {
    match serde_json::to_string_pretty(&ErrorResponse {
        status: "error",
        error: message,
    }) {
        Ok(json) => json,
        Err(_) => "{\"status\":\"error\",\"error\":\"failed to serialize error\"}".into(),
    }
}

pub(crate) fn render_json<T>(value: &T, _json: bool) -> Result<String>
where
    T: Serialize,
{
    serde_json::to_string_pretty(value).context("failed to serialize JSON output")
}

pub(crate) fn render_clap_error(error: clap::Error, json_requested: bool) -> std::process::ExitCode {
    match error.kind() {
        clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
            print!("{error}");
            std::process::ExitCode::SUCCESS
        }
        _ => {
            if json_requested {
                println!("{}", render_json_error(&error.to_string()));
            } else {
                eprint!("{error}");
            }
            std::process::ExitCode::FAILURE
        }
    }
}

#[derive(Serialize)]
pub(crate) struct NamespaceListResponse {
    pub(crate) namespaces: Vec<ResolvedNamespace>,
}

#[derive(Serialize)]
pub(crate) struct AuditListResponse<'a> {
    pub(crate) status: &'static str,
    pub(crate) events: &'a [StoredAuditEvent],
}

#[derive(Serialize)]
pub(crate) struct AuditEventResponse<'a> {
    pub(crate) status: &'static str,
    pub(crate) event: &'a StoredAuditEvent,
}

#[derive(Serialize)]
pub(crate) struct AuditOperationResponse<'a> {
    pub(crate) status: &'static str,
    pub(crate) operation_id: &'a OperationId,
    pub(crate) events: &'a [StoredAuditEvent],
}

#[derive(Serialize)]
pub(crate) struct ToolCatalogListResponse {
    pub(crate) status: &'static str,
    pub(crate) tools: Vec<ToolCatalogEntry>,
}

#[derive(Serialize)]
pub(crate) struct ToolCatalogDetailResponse {
    pub(crate) status: &'static str,
    pub(crate) tool: ToolCatalogDetail,
}

#[derive(Serialize)]
pub(crate) struct ToolCatalogEntry {
    name: ToolName,
    provider: ProviderKind,
    kind: ToolKind,
    backend: BackendKind,
    summary: String,
    surface: ToolSurface,
    aggregate_read_supported: bool,
    execution_support: ToolExecutionSupport,
    undo_support: ToolUndoSupport,
}

impl From<&RegisteredTool> for ToolCatalogEntry {
    fn from(tool: &RegisteredTool) -> Self {
        Self {
            name: tool.name.clone(),
            provider: tool.provider.clone(),
            kind: tool.kind,
            backend: tool.backend,
            summary: tool.summary.to_owned(),
            surface: tool.surface,
            aggregate_read_supported: tool.aggregate_read_supported,
            execution_support: tool.execution_support,
            undo_support: tool.undo_support,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct ToolCatalogDetail {
    pub(crate) name: ToolName,
    pub(crate) provider: ProviderKind,
    pub(crate) kind: ToolKind,
    pub(crate) backend: BackendKind,
    pub(crate) summary: String,
    pub(crate) surface: ToolSurface,
    pub(crate) aggregate_read_supported: bool,
    pub(crate) execution_support: ToolExecutionSupport,
    pub(crate) undo_support: ToolUndoSupport,
    pub(crate) arguments: Vec<ToolArgumentSpec>,
    pub(crate) available_namespaces: Vec<NamespaceId>,
    pub(crate) notes: Vec<String>,
    pub(crate) examples: Vec<String>,
}

impl ToolCatalogDetail {
    pub(crate) fn new(tool: &RegisteredTool, namespaces: &[ResolvedNamespace]) -> Self {
        let available_namespaces = namespaces
            .iter()
            .map(|namespace| namespace.id.clone())
            .collect::<Vec<_>>();
        let raw = tool.surface == ToolSurface::Raw;
        let example_namespace = namespaces
            .first()
            .map(|namespace| namespace.id.to_string())
            .unwrap_or_else(|| format!("{}.default", tool.provider));
        let mut notes = vec![
            "policy, auth isolation, and audit still apply".to_owned(),
            "repeat --ns for aggregate reads, writes stay single-namespace".to_owned(),
        ];
        if tool.execution_support == ToolExecutionSupport::PlanningOnly {
            notes.push("execution is not wired yet, this tool currently plans cleanly but will not apply".to_owned());
        }
        let examples = if raw {
            notes.push(
                "put switchboard flags before --, everything after -- is forwarded to the provider CLI unchanged"
                    .to_owned(),
            );
            notes.push("for scripted calls, --argv-json accepts one JSON array of argv tokens".to_owned());
            raw_tool_examples(tool, &example_namespace)
        } else {
            curated_tool_examples(tool, &example_namespace)
        };

        Self {
            name: tool.name.clone(),
            provider: tool.provider.clone(),
            kind: tool.kind,
            backend: tool.backend,
            summary: tool.summary.to_owned(),
            surface: tool.surface,
            aggregate_read_supported: tool.aggregate_read_supported,
            execution_support: tool.execution_support,
            undo_support: tool.undo_support,
            arguments: tool.arguments.clone(),
            available_namespaces,
            notes,
            examples,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct StoredOperationListResponse<'a> {
    pub(crate) status: &'static str,
    pub(crate) operations: &'a [StoredOperation],
}

#[derive(Serialize)]
pub(crate) struct StoredOperationResponse<'a> {
    pub(crate) status: &'static str,
    pub(crate) operation: &'a StoredOperation,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(crate) enum DispatchResponse<'a> {
    Planned {
        tool: &'a ToolName,
        namespace: &'a NamespaceId,
        summary: &'a str,
        backend: BackendKind,
        approval_required: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        operation_id: Option<&'a switchboard_core::OperationId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        compensates_operation_id: Option<&'a switchboard_core::OperationId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        approval_reason: Option<&'a str>,
    },
    Executed {
        tool: &'a ToolName,
        namespace: &'a NamespaceId,
        summary: &'a str,
        fields: &'a BTreeMap<String, JsonValue>,
        refs: &'a [ToolRef],
        #[serde(skip_serializing_if = "Option::is_none")]
        operation_id: Option<&'a switchboard_core::OperationId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        effect: Option<&'a OperationEffect>,
    },
}

impl<'a> DispatchResponse<'a> {
    pub(crate) fn from(outcome: &'a DispatchOutcome) -> Self {
        match outcome {
            DispatchOutcome::Planned(plan) => Self::from_plan(plan),
            DispatchOutcome::Executed(output) => Self::from_output(output),
        }
    }

    pub(crate) fn from_plan(plan: &'a switchboard_core::PlannedAction) -> Self {
        Self::Planned {
            tool: &plan.tool,
            namespace: &plan.namespace,
            summary: &plan.summary,
            backend: plan.backend,
            approval_required: plan.approval_required,
            operation_id: plan.operation_id.as_ref(),
            compensates_operation_id: plan.compensates_operation_id.as_ref(),
            approval_reason: plan.approval_reason.as_deref(),
        }
    }

    pub(crate) fn from_output(output: &'a ToolOutput) -> Self {
        Self::Executed {
            tool: &output.tool,
            namespace: &output.namespace,
            summary: &output.summary,
            fields: &output.fields,
            refs: &output.refs,
            operation_id: output.operation_id.as_ref(),
            effect: output.effect.as_ref(),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct AggregateReadResponse<'a> {
    status: &'static str,
    tool: &'a ToolName,
    namespaces: &'a [NamespaceId],
    results: Vec<AggregateReadResultResponse<'a>>,
}

#[derive(Serialize)]
pub(crate) struct AggregateReadResultResponse<'a> {
    namespace: &'a NamespaceId,
    outcome: DispatchResponse<'a>,
}

#[derive(Serialize)]
struct ErrorResponse<'a> {
    status: &'static str,
    error: &'a str,
}
