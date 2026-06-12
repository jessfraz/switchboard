use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::MutexGuard,
};

use clap::{CommandFactory, Parser};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::json;
use switchboard_core::{
    AggregateReadRequest, ApprovalState, DispatchOutcome, ExecutionMode, NamespaceId, OperationEffect,
    OperationOutcome, OperationRequest, StoredAuditEvent, ToolExecutionSupport, ToolName, ToolOutput, ToolRequest,
    ToolSurface, ToolUndoSupport,
};

use crate::{
    run, select_config_path,
    test_support::{lock_env, TempScript},
    Cli, ConfigPathCandidates,
};

const BASIC_CONFIG_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/config/basic.toml"
));
const ALLOW_WRITES_CONFIG_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/config/allow-writes.toml"
));
const GOOGLE_CALENDAR_AGENDA_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/google-calendar-agenda.json"
));
const GOOGLE_GMAIL_TRIAGE_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/google-gmail-triage.json"
));
const GOOGLE_GMAIL_READ_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/google-gmail-read.json"
));
const GOOGLE_GMAIL_DRAFT_CREATE_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/google-gmail-draft-create.json"
));
const GOOGLE_CALENDAR_CREATE_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/google-calendar-create.json"
));
const GOOGLE_CALENDAR_DELETE_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/google-calendar-delete.json"
));
const GITHUB_NOTIFICATIONS_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/github-notifications.json"
));
const GITHUB_PR_SEARCH_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/github-pull-request-search.json"
));
const GITHUB_PR_READ_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/github-pull-request-read.json"
));
const GITHUB_ISSUE_READ_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/github-issue-read.json"
));
const GITHUB_REPOSITORY_SEARCH_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/github-repository-search.json"
));
const GITHUB_REPO_VIEW_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/github-repo-view.json"
));
const MYCHART_APPOINTMENTS_UPCOMING_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/mychart-appointments-upcoming.json"
));
const MYCHART_NOTES_SEARCH_FIXTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/mychart-notes-search.json"
));
const GOOGLE_SCRIPT_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/scripts/gws-test.sh"
));
const GITHUB_SCRIPT_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/scripts/gh-test.sh"
));
const MYCHART_SCRIPT_TEMPLATE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/scripts/mychart-test.sh"
));
const GOOGLE_PERSONAL_OAUTH_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/secrets/google-personal-oauth.json"
));

#[derive(Debug, Deserialize)]
struct JsonPlannedResponse {
    status: String,
    tool: ToolName,
    namespace: NamespaceId,
    summary: String,
    backend: switchboard_core::BackendKind,
    approval_required: bool,
    operation_id: Option<switchboard_core::OperationId>,
    compensates_operation_id: Option<switchboard_core::OperationId>,
    approval_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JsonExecutedResponse<TFields> {
    status: String,
    tool: ToolName,
    namespace: NamespaceId,
    summary: String,
    fields: TFields,
    #[serde(default)]
    refs: Vec<switchboard_core::ToolRef>,
    operation_id: Option<switchboard_core::OperationId>,
    effect: Option<OperationEffect>,
}

#[derive(Debug, Deserialize)]
struct JsonAggregateReadResponse<TFields> {
    status: String,
    tool: ToolName,
    namespaces: Vec<NamespaceId>,
    results: Vec<JsonAggregateReadResult<TFields>>,
}

#[derive(Debug, Deserialize)]
struct JsonAggregateReadResult<TFields> {
    namespace: NamespaceId,
    outcome: JsonExecutedResponse<TFields>,
}

#[derive(Debug, Deserialize)]
struct JsonStoredOperationEnvelope {
    status: String,
    operation: JsonStoredOperation,
}

#[derive(Debug, Deserialize)]
struct JsonStoredOperationListEnvelope {
    status: String,
    operations: Vec<JsonStoredOperation>,
}

#[derive(Debug, Deserialize)]
struct JsonStoredOperation {
    id: switchboard_core::OperationId,
    status: switchboard_core::OperationStatus,
    compensates_operation_id: Option<switchboard_core::OperationId>,
    approval: JsonOperationApproval,
    effect: Option<OperationEffect>,
}

#[derive(Debug, Deserialize)]
struct JsonOperationApproval {
    state: ApprovalState,
}

#[derive(Debug, Deserialize)]
struct JsonToolCatalogList {
    status: String,
    tools: Vec<JsonToolCatalogEntry>,
}

#[derive(Debug, Deserialize)]
struct JsonToolCatalogEntry {
    name: ToolName,
    surface: ToolSurface,
    execution_support: ToolExecutionSupport,
    undo_support: ToolUndoSupport,
}

#[derive(Debug, Deserialize)]
struct JsonToolCatalogDetailEnvelope {
    status: String,
    tool: JsonToolCatalogDetail,
}

#[derive(Debug, Deserialize)]
struct JsonToolCatalogDetail {
    name: ToolName,
    execution_support: ToolExecutionSupport,
    undo_support: ToolUndoSupport,
    arguments: Vec<JsonToolArgumentSpec>,
    notes: Vec<String>,
    examples: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct JsonToolArgumentSpec {
    name: String,
    #[serde(default)]
    aliases: Vec<String>,
    transport: switchboard_core::ToolArgumentTransport,
    value_kind: switchboard_core::ToolArgumentValueKind,
    required: bool,
    repeated: bool,
    #[allow(dead_code)]
    forwarded_flag: Option<String>,
    #[allow(dead_code)]
    forwarded_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JsonAuditListResponse {
    status: String,
    events: Vec<StoredAuditEvent>,
}

#[derive(Debug, Deserialize)]
struct JsonAuditEventEnvelope {
    status: String,
    event: StoredAuditEvent,
}

#[derive(Debug, Deserialize)]
struct JsonAuditOperationEnvelope {
    status: String,
    operation_id: switchboard_core::OperationId,
    events: Vec<StoredAuditEvent>,
}

#[derive(Debug, Deserialize)]
struct RawGoogleReadFields {
    response: GoogleAgendaPayload,
}

#[derive(Debug, Deserialize)]
struct GoogleAgendaPayload {
    count: usize,
    events: Vec<GoogleAgendaEvent>,
}

#[derive(Debug, Deserialize)]
struct GoogleAgendaEvent {
    summary: String,
}

#[derive(Debug, Deserialize)]
struct RawGoogleWriteFields {
    response: GoogleDraftPayload,
}

#[derive(Debug, Deserialize)]
struct GoogleDraftPayload {
    id: String,
}

#[derive(Debug, Deserialize)]
struct RawMyChartReadFields<TResponse> {
    response: TResponse,
}

#[derive(Debug, Deserialize)]
struct MyChartNotesSearchPayload {
    query: String,
    notes: Vec<MyChartNoteSummary>,
}

#[derive(Debug, Eq, PartialEq)]
struct MyChartScriptCapture {
    config: Option<String>,
    account: Option<String>,
    base_url: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    redirect_uri: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    username: Option<String>,
    argv: String,
}

#[derive(Debug, Deserialize)]
struct MyChartNoteSummary {
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleCalendarDeleteFields {
    event: GoogleCalendarDeleteEvent,
}

#[derive(Debug, Deserialize)]
struct GoogleCalendarCreateFields {
    event: GoogleCalendarCreateEvent,
}

#[derive(Debug, Deserialize)]
struct GoogleCalendarCreateEvent {
    event_id: String,
    calendar: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct GoogleCalendarDeleteEvent {
    event_id: String,
    calendar: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct CountFields {
    count: usize,
}

#[derive(Debug, Deserialize)]
struct GoogleMailSearchFields {
    count: usize,
}

#[derive(Debug, Deserialize)]
struct GoogleMailReadFields {
    message: GoogleMailReadMessage,
}

#[derive(Debug, Deserialize)]
struct GoogleMailReadMessage {
    gmail_message_id: String,
    gmail_thread_id: String,
}

#[derive(Debug, Deserialize)]
struct GoogleMailDraftFields {
    draft: GoogleMailDraftPayload,
}

#[derive(Debug, Deserialize)]
struct GoogleMailDraftPayload {
    draft_id: String,
    gmail_message_id: String,
    gmail_thread_id: Option<String>,
    to: Vec<String>,
    subject: Option<String>,
    has_body_text: bool,
    has_body_html: bool,
}

#[derive(Debug, Deserialize)]
struct GitHubPullRequestSearchFields {
    count: usize,
}

#[derive(Debug, Deserialize)]
struct GitHubPullRequestReadFields {
    pull_request: GitHubPullRequestPayload,
}

#[derive(Debug, Deserialize)]
struct GitHubIssueReadFields {
    issue: GitHubIssuePayload,
}

#[derive(Debug, Deserialize)]
struct GitHubRepositorySearchFields {
    count: usize,
    repositories: Vec<GitHubRepositoryPayload>,
}

#[derive(Debug, Deserialize)]
struct GitHubRepositoryPayload {
    full_name: String,
    owner: String,
}

#[derive(Debug, Deserialize)]
struct GitHubIssuePayload {
    number: usize,
    labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubPullRequestPayload {
    number: usize,
    assignees: Vec<String>,
    labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct StubStatusFields {
    status: String,
}

fn parse_json<T: DeserializeOwned>(output: &str) -> T {
    serde_json::from_str(output).expect("output should be valid json")
}

fn parse_mychart_capture(output: &str) -> MyChartScriptCapture {
    let block = output
        .rsplit("\n---")
        .find(|block| !block.trim().is_empty())
        .map(str::trim)
        .expect("capture should contain at least one block");
    let mut fields = BTreeMap::new();

    for line in block.lines() {
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .expect("capture lines should be formatted as KEY=value");
        fields.insert(key.to_owned(), value.to_owned());
    }

    MyChartScriptCapture {
        config: non_empty_capture_field(&fields, "CONFIG"),
        account: non_empty_capture_field(&fields, "ACCOUNT"),
        base_url: non_empty_capture_field(&fields, "BASE_URL"),
        client_id: non_empty_capture_field(&fields, "CLIENT_ID"),
        client_secret: non_empty_capture_field(&fields, "CLIENT_SECRET"),
        redirect_uri: non_empty_capture_field(&fields, "REDIRECT_URI"),
        access_token: non_empty_capture_field(&fields, "ACCESS_TOKEN"),
        refresh_token: non_empty_capture_field(&fields, "REFRESH_TOKEN"),
        username: non_empty_capture_field(&fields, "USERNAME"),
        argv: fields.remove("ARGV").expect("capture should include ARGV"),
    }
}

fn non_empty_capture_field(fields: &BTreeMap<String, String>, key: &str) -> Option<String> {
    fields
        .get(key)
        .and_then(|value| if value.is_empty() { None } else { Some(value.clone()) })
}

fn parse_output_fields<T: DeserializeOwned>(output: &switchboard_core::ToolOutput) -> T {
    serde_json::from_value(serde_json::Value::Object(
        output
            .fields
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    ))
    .expect("tool output fields should deserialize")
}

#[test]
fn configured_namespaces_match_current_examples() {
    let environment = TestEnvironment::new();
    let switchboard = super::load_switchboard(Some(environment.path())).expect("switchboard should build");
    let namespaces = switchboard.list_namespaces();
    let ids = namespaces
        .into_iter()
        .map(|namespace| namespace.id.to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        ids,
        vec![
            "github.personal",
            "github.personal_token",
            "google.personal",
            "google.work",
            "mychart.ucla",
            "schwab.personal"
        ]
    );
}

#[test]
fn write_requests_default_to_planning_until_approval_exists() {
    let environment = TestEnvironment::new();
    let switchboard = super::load_switchboard(Some(environment.path())).expect("switchboard should build");
    let request = ToolRequest::new(
        "github.pull_request.comment",
        "github.personal",
        ExecutionMode::Auto,
        BTreeMap::from([
            ("repo".into(), "owner/repo".into()),
            ("number".into(), "42".into()),
            ("body".into(), "Needs a regression test".into()),
        ]),
    )
    .expect("request should parse");

    let outcome = switchboard.dispatch(request).expect("dispatch should succeed");
    match outcome {
        DispatchOutcome::Planned(plan) => {
            assert!(plan.approval_required);
            assert_eq!(plan.backend.to_string(), "cli");
            assert!(plan.operation_id.is_some());
        }
        DispatchOutcome::Executed(_) => {
            panic!("write requests should not execute yet");
        }
    }
}

#[test]
fn operation_approval_flow_can_approve_and_apply_planned_writes() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();

    let draft = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.calendar.create",
        "--ns",
        "google.work",
        "--title",
        "Budget review",
        "--start",
        "2026-03-30T15:00:00-07:00",
        "--end",
        "2026-03-30T15:30:00-07:00",
        "--draft",
        "--json",
    ])
    .expect("cli should parse");

    let draft_output = run(draft).expect("draft should succeed");
    let draft_value: JsonPlannedResponse = parse_json(&draft_output);
    assert_eq!(draft_value.status, "planned");
    assert_eq!(
        draft_value.tool,
        ToolName::new("google.calendar.create").expect("tool should build")
    );
    assert_eq!(
        draft_value.namespace,
        NamespaceId::new("google.work").expect("namespace should build")
    );
    assert_eq!(draft_value.backend, switchboard_core::BackendKind::Cli);
    assert!(draft_value.summary.contains("calendar"));
    assert!(draft_value.approval_required);
    assert!(draft_value.approval_reason.is_some());
    let operation_id = draft_value.operation_id.expect("draft operation id should exist");
    assert!(draft_value.compensates_operation_id.is_none());
    let operation_id_string = operation_id.to_string();

    let approve = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "op",
        "approve",
        &operation_id_string,
        "--actor",
        "codex",
        "--note",
        "looks good",
        "--json",
    ])
    .expect("approve cli should parse");

    let approve_output = run(approve).expect("approve should succeed");
    let approve_value: JsonStoredOperationEnvelope = parse_json(&approve_output);
    assert_eq!(approve_value.status, "approved");
    assert_eq!(approve_value.operation.id, operation_id);
    assert_eq!(
        approve_value.operation.status,
        switchboard_core::OperationStatus::Planned
    );
    assert_eq!(approve_value.operation.approval.state, ApprovalState::Approved);

    let apply = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "op",
        "apply",
        &operation_id_string,
        "--json",
    ])
    .expect("apply cli should parse");

    let apply_output = run(apply).expect("apply should succeed");
    let apply_value: JsonExecutedResponse<GoogleCalendarCreateFields> = parse_json(&apply_output);
    assert_eq!(apply_value.status, "executed");
    assert_eq!(
        apply_value.tool,
        ToolName::new("google.calendar.create").expect("tool should build")
    );
    assert_eq!(
        apply_value.namespace,
        NamespaceId::new("google.work").expect("namespace should build")
    );
    assert!(apply_value.summary.contains("calendar"));
    assert_eq!(apply_value.operation_id.as_ref(), Some(&operation_id));
    assert_eq!(apply_value.effect.as_ref().map(|effect| effect.undoable), Some(true));
    assert_eq!(apply_value.fields.event.event_id, "event-1960budgetwork");
    assert_eq!(apply_value.fields.event.calendar, "primary");
    assert_eq!(apply_value.fields.event.status, "confirmed");
    assert_eq!(apply_value.refs[0].kind, switchboard_core::ToolRefKind::Event);
    assert!(
        environment
            .gws_capture_contents()
            .contains("ARGV=calendar +insert --format json --summary Budget review"),
        "expected calendar insert command to run after approval"
    );
}

#[test]
fn allow_policy_executes_writes_without_manual_approval() {
    let environment = TestEnvironment::with_config_template(ALLOW_WRITES_CONFIG_TEMPLATE);
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.calendar.create",
        "--ns",
        "google.work",
        "--title",
        "Budget review",
        "--start",
        "2026-03-30T15:00:00-07:00",
        "--end",
        "2026-03-30T15:30:00-07:00",
        "--apply",
        "--json",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("write should execute");
    let value: JsonExecutedResponse<GoogleCalendarCreateFields> = parse_json(&output);

    assert_eq!(value.status, "executed");
    assert_eq!(
        value.tool,
        ToolName::new("google.calendar.create").expect("tool should build")
    );
    assert_eq!(
        value.namespace,
        NamespaceId::new("google.work").expect("namespace should build")
    );
    assert!(value.summary.contains("calendar"));
    assert_eq!(value.effect.as_ref().map(|effect| effect.undoable), Some(true));
    assert_eq!(value.fields.event.event_id, "event-1960budgetwork");
    assert_eq!(value.fields.event.calendar, "primary");
    assert_eq!(value.fields.event.status, "confirmed");
    assert_eq!(value.refs[0].kind, switchboard_core::ToolRefKind::Event);
}

#[test]
fn operation_list_pending_filters_for_attention_only() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();

    let pending_write = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.mail.draft",
        "--ns",
        "google.work",
        "--to",
        "dogs@example.com",
        "--subject",
        "Boarding request",
        "--body-text",
        "Can you board the dogs next week?",
        "--draft",
        "--json",
    ])
    .expect("draft cli should parse");
    let pending_output = run(pending_write).expect("draft should succeed");
    let pending_value: JsonPlannedResponse = parse_json(&pending_output);
    let pending_operation_id = pending_value.operation_id.expect("pending operation should exist");

    let approved_write = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.calendar.create",
        "--ns",
        "google.work",
        "--title",
        "Budget review",
        "--start",
        "2026-03-30T15:00:00-07:00",
        "--end",
        "2026-03-30T15:30:00-07:00",
        "--draft",
        "--json",
    ])
    .expect("calendar draft cli should parse");
    let approved_output = run(approved_write).expect("calendar draft should succeed");
    let approved_value: JsonPlannedResponse = parse_json(&approved_output);
    let approved_operation_id = approved_value.operation_id.expect("approved operation should exist");

    let approve = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "op",
        "approve",
        &approved_operation_id.to_string(),
        "--actor",
        "codex",
        "--json",
    ])
    .expect("approve cli should parse");
    run(approve).expect("approve should succeed");

    let list_pending = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "op",
        "list",
        "--pending",
        "--json",
    ])
    .expect("op list cli should parse");
    let list_output = run(list_pending).expect("op list should succeed");
    let list_value: JsonStoredOperationListEnvelope = parse_json(&list_output);

    assert_eq!(list_value.status, "ok");
    assert_eq!(list_value.operations.len(), 1);
    assert_eq!(list_value.operations[0].id, pending_operation_id);
    assert_eq!(list_value.operations[0].approval.state, ApprovalState::Pending);
    assert_eq!(
        list_value.operations[0].status,
        switchboard_core::OperationStatus::Planned
    );
    assert_ne!(list_value.operations[0].id, approved_operation_id);
}

#[test]
fn approve_with_apply_executes_gmail_draft_in_one_step() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();

    let draft = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.mail.draft",
        "--ns",
        "google.work",
        "--to",
        "dogs@example.com",
        "--cc",
        "frontdesk@example.com",
        "--subject",
        "Boarding request",
        "--body-text",
        "Hi there,\nCan you board the dogs next week?",
        "--draft",
        "--json",
    ])
    .expect("draft cli should parse");

    let draft_output = run(draft).expect("draft should succeed");
    let draft_value: JsonPlannedResponse = parse_json(&draft_output);
    let operation_id = draft_value.operation_id.expect("draft operation id should exist");
    let operation_id_string = operation_id.to_string();

    let approve_apply = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "op",
        "approve",
        &operation_id_string,
        "--actor",
        "codex",
        "--note",
        "looks right",
        "--apply",
        "--json",
    ])
    .expect("approve apply cli should parse");

    let output = run(approve_apply).expect("approve apply should succeed");
    let value: JsonExecutedResponse<GoogleMailDraftFields> = parse_json(&output);

    assert_eq!(value.status, "executed");
    assert_eq!(
        value.tool,
        ToolName::new("google.mail.draft").expect("tool should build")
    );
    assert_eq!(
        value.namespace,
        NamespaceId::new("google.work").expect("namespace should build")
    );
    assert_eq!(value.operation_id.as_ref(), Some(&operation_id));
    assert_eq!(value.fields.draft.draft_id, "draft-1960work");
    assert_eq!(value.fields.draft.gmail_message_id, "1960draftmsgwork");
    assert_eq!(
        value.fields.draft.gmail_thread_id.as_deref(),
        Some("1960draftthreadwork")
    );
    assert_eq!(value.fields.draft.to, vec!["dogs@example.com"]);
    assert_eq!(value.fields.draft.subject.as_deref(), Some("Boarding request"));
    assert!(value.fields.draft.has_body_text);
    assert!(!value.fields.draft.has_body_html);
    assert_eq!(value.refs[0].kind, switchboard_core::ToolRefKind::Message);
    assert_eq!(value.refs[1].kind, switchboard_core::ToolRefKind::Thread);
    assert_eq!(value.effect.as_ref().map(|effect| effect.undoable), Some(false));
    assert!(
        environment
            .gws_capture_contents()
            .contains("ARGV=gmail users drafts create --params {\"userId\":\"me\"}"),
        "expected curated gmail draft command to run through gws"
    );
}

#[test]
fn operation_show_renders_argument_preview_for_humans() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();

    let draft = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.mail.draft",
        "--ns",
        "google.work",
        "--to",
        "dogs@example.com",
        "--subject",
        "Boarding request",
        "--body-text",
        "Hi there,\nCan you board the dogs next week?",
        "--draft",
        "--json",
    ])
    .expect("draft cli should parse");
    let draft_output = run(draft).expect("draft should succeed");
    let draft_value: JsonPlannedResponse = parse_json(&draft_output);
    let operation_id = draft_value.operation_id.expect("draft operation id should exist");

    let show = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "op",
        "show",
        &operation_id.to_string(),
    ])
    .expect("show cli should parse");
    let output = run(show).expect("show should succeed");

    assert!(output.contains("Args:"));
    assert!(output.contains("--to=dogs@example.com"));
    assert!(output.contains("--subject=Boarding request"));
    assert!(output.contains("--body-text=Hi there,\\nCan you board the dogs next week?"));
}

#[test]
fn audit_commands_show_persisted_operation_history() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();

    let draft = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.calendar.create",
        "--ns",
        "google.work",
        "--title",
        "Vet visit",
        "--start",
        "2026-04-02T09:00:00-07:00",
        "--end",
        "2026-04-02T10:00:00-07:00",
        "--draft",
        "--json",
    ])
    .expect("draft cli should parse");

    let draft_output = run(draft).expect("draft should succeed");
    let draft_value: JsonPlannedResponse = parse_json(&draft_output);
    let operation_id = draft_value.operation_id.expect("operation id should exist");
    let operation_id_string = operation_id.to_string();

    let approve = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "op",
        "approve",
        &operation_id_string,
        "--actor",
        "codex",
        "--note",
        "approved for testing",
        "--json",
    ])
    .expect("approve cli should parse");
    run(approve).expect("approve should succeed");

    let apply = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "op",
        "apply",
        &operation_id_string,
        "--json",
    ])
    .expect("apply cli should parse");
    run(apply).expect("apply should succeed");

    let audit_list = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "audit",
        "list",
        "--operation-id",
        &operation_id_string,
        "--json",
    ])
    .expect("audit list cli should parse");
    let audit_list_output = run(audit_list).expect("audit list should succeed");
    let audit_list_value: JsonAuditListResponse = parse_json(&audit_list_output);

    assert_eq!(audit_list_value.status, "ok");
    assert_eq!(audit_list_value.events.len(), 3);
    assert!(audit_list_value
        .events
        .iter()
        .all(|event| event.operation_id.as_ref() == Some(&operation_id)));
    let outcomes = audit_list_value
        .events
        .iter()
        .map(|event| event.outcome.clone())
        .collect::<Vec<_>>();
    assert!(outcomes.contains(&switchboard_core::AuditOutcome::Planned));
    assert!(outcomes.contains(&switchboard_core::AuditOutcome::Approved));
    assert!(outcomes.contains(&switchboard_core::AuditOutcome::Executed));

    let executed_event = audit_list_value
        .events
        .iter()
        .find(|event| event.outcome == switchboard_core::AuditOutcome::Executed)
        .expect("executed audit event should exist");
    let event_id_string = executed_event.id.to_string();

    let audit_show_event = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "audit",
        "show",
        &event_id_string,
        "--json",
    ])
    .expect("audit show event cli should parse");
    let audit_show_event_output = run(audit_show_event).expect("audit show event should succeed");
    let audit_show_event_value: JsonAuditEventEnvelope = parse_json(&audit_show_event_output);

    assert_eq!(audit_show_event_value.status, "ok");
    assert_eq!(audit_show_event_value.event.id, executed_event.id);
    assert_eq!(audit_show_event_value.event.operation_id.as_ref(), Some(&operation_id));
    assert_eq!(
        audit_show_event_value.event.outcome,
        switchboard_core::AuditOutcome::Executed
    );

    let audit_show_operation = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "audit",
        "show",
        &operation_id_string,
        "--json",
    ])
    .expect("audit show operation cli should parse");
    let audit_show_operation_output = run(audit_show_operation).expect("audit show operation should succeed");
    let audit_show_operation_value: JsonAuditOperationEnvelope = parse_json(&audit_show_operation_output);

    assert_eq!(audit_show_operation_value.status, "ok");
    assert_eq!(audit_show_operation_value.operation_id, operation_id);
    assert_eq!(audit_show_operation_value.events.len(), 3);
}

#[test]
fn undo_creates_compensating_delete_and_marks_original_operation_compensated() {
    let environment = TestEnvironment::with_config_template(ALLOW_WRITES_CONFIG_TEMPLATE);
    let config_path = environment.path_string();

    let create = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.calendar.create",
        "--ns",
        "google.work",
        "--title",
        "Budget review",
        "--start",
        "2026-03-30T15:00:00-07:00",
        "--end",
        "2026-03-30T15:30:00-07:00",
        "--apply",
        "--json",
    ])
    .expect("create cli should parse");

    let create_output = run(create).expect("create should execute");
    let create_value: JsonExecutedResponse<GoogleCalendarCreateFields> = parse_json(&create_output);
    let original_operation_id = create_value
        .operation_id
        .clone()
        .expect("created event should have an operation id");
    assert_eq!(create_value.fields.event.event_id, "event-1960budgetwork");
    let original_operation_id_string = original_operation_id.to_string();

    let undo = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "op",
        "undo",
        &original_operation_id_string,
        "--apply",
        "--json",
    ])
    .expect("undo cli should parse");

    let undo_output = run(undo).expect("undo should execute");
    let undo_value: JsonExecutedResponse<GoogleCalendarDeleteFields> = parse_json(&undo_output);
    let compensating_operation_id = undo_value
        .operation_id
        .clone()
        .expect("compensating delete should have an operation id");
    let compensating_operation_id_string = compensating_operation_id.to_string();

    assert_eq!(undo_value.status, "executed");
    assert_eq!(
        undo_value.tool,
        ToolName::new("google.calendar.delete").expect("tool should build")
    );
    assert_eq!(
        undo_value.namespace,
        NamespaceId::new("google.work").expect("namespace should build")
    );
    assert_ne!(compensating_operation_id, original_operation_id);
    assert_eq!(undo_value.fields.event.calendar, "primary");
    assert_eq!(undo_value.fields.event.event_id, "event-1960budgetwork");
    assert_eq!(undo_value.fields.event.status, "deleted");

    let original_show = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "op",
        "show",
        &original_operation_id_string,
        "--json",
    ])
    .expect("op show original cli should parse");
    let original_show_output = run(original_show).expect("original op show should succeed");
    let original_show_value: JsonStoredOperationEnvelope = parse_json(&original_show_output);

    assert_eq!(original_show_value.status, "ok");
    assert_eq!(original_show_value.operation.id, original_operation_id);
    assert_eq!(
        original_show_value.operation.status,
        switchboard_core::OperationStatus::Compensated
    );
    assert_eq!(
        original_show_value
            .operation
            .effect
            .as_ref()
            .map(|effect| effect.undoable),
        Some(true)
    );

    let compensating_show = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "op",
        "show",
        &compensating_operation_id_string,
        "--json",
    ])
    .expect("op show compensation cli should parse");
    let compensating_show_output = run(compensating_show).expect("compensating op show should succeed");
    let compensating_show_value: JsonStoredOperationEnvelope = parse_json(&compensating_show_output);

    assert_eq!(compensating_show_value.status, "ok");
    assert_eq!(compensating_show_value.operation.id, compensating_operation_id);
    assert_eq!(
        compensating_show_value.operation.status,
        switchboard_core::OperationStatus::Applied
    );
    assert_eq!(
        compensating_show_value.operation.compensates_operation_id.as_ref(),
        Some(&original_operation_id)
    );

    let audit_list = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "audit",
        "list",
        "--operation-id",
        &original_operation_id_string,
        "--json",
    ])
    .expect("audit list cli should parse");
    let audit_list_output = run(audit_list).expect("audit list should succeed");
    let audit_list_value: JsonAuditListResponse = parse_json(&audit_list_output);
    let outcomes = audit_list_value
        .events
        .iter()
        .map(|event| event.outcome.clone())
        .collect::<Vec<_>>();

    assert!(outcomes.contains(&switchboard_core::AuditOutcome::Executed));
    assert!(outcomes.contains(&switchboard_core::AuditOutcome::Compensated));
    assert!(
            environment
                .gws_capture_contents()
                .contains("ARGV=calendar events delete --format json --params {\"calendarId\":\"primary\",\"eventId\":\"event-1960budgetwork\"} --send-updates none"),
            "expected calendar delete command to run during undo"
        );
}

#[test]
fn tools_list_includes_curated_and_raw_tools() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from(["switchboard", "--config", &config_path, "tools", "list", "--json"])
        .expect("cli should parse");

    let output = run(cli).expect("tools list should succeed");
    let value: JsonToolCatalogList = parse_json(&output);

    assert_eq!(value.status, "ok");
    assert!(
        value.tools.iter().any(|tool| {
            tool.name == ToolName::new("google.mail.search").expect("tool should build")
                && tool.surface == ToolSurface::Curated
                && tool.execution_support == ToolExecutionSupport::Executable
        }),
        "expected curated google tool in catalog"
    );
    assert!(
        value.tools.iter().any(|tool| {
            tool.name == ToolName::new("google.cli.write").expect("tool should build")
                && tool.surface == ToolSurface::Raw
        }),
        "expected raw google write tool in catalog"
    );
    assert!(
        value.tools.iter().any(|tool| {
            tool.name == ToolName::new("github.cli.read").expect("tool should build")
                && tool.surface == ToolSurface::Raw
        }),
        "expected raw github read tool in catalog"
    );
    assert!(
        value.tools.iter().any(|tool| {
            tool.name == ToolName::new("google.cli.calendar.+agenda").expect("tool should build")
                && tool.surface == ToolSurface::Raw
        }),
        "expected generated raw google agenda tool in catalog"
    );
    assert!(
        value.tools.iter().any(|tool| {
            tool.name == ToolName::new("google.calendar.create").expect("tool should build")
                && tool.undo_support == ToolUndoSupport::CompensatingAction
        }),
        "expected undoable calendar create tool in catalog"
    );
}

#[test]
fn tools_describe_raw_google_tool_explains_passthrough_usage() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "tools",
        "describe",
        "google.cli.write",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("tools describe should succeed");

    assert!(output.contains("Tool: google.cli.write"));
    assert!(output.contains("Arguments:\n- argv [transport=passthrough_argv, type=string, repeated]"));
    assert!(output.contains("policy, auth isolation, and audit still apply"));
    assert!(output.contains("put switchboard flags before --"));
    assert!(output.contains("switchboard google.cli.write --ns google."));
}

#[test]
fn tools_describe_mychart_write_prefers_ucla_preset_login() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "tools",
        "describe",
        "mychart.cli.write",
        "--json",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("tools describe should succeed");
    let value: JsonToolCatalogDetailEnvelope = parse_json(&output);

    assert_eq!(
        value.tool.name,
        ToolName::new("mychart.cli.write").expect("tool should build")
    );
    assert!(
        value.tool.notes.iter().any(|note| note.contains("mychart login ucla")),
        "expected MyChart write notes to point at the UCLA preset login"
    );
    assert!(
        value
            .tool
            .examples
            .iter()
            .any(|example| example == "switchboard mychart.cli.write --ns mychart.ucla --draft -- login ucla"),
        "expected MyChart write examples to prefer the UCLA preset login"
    );
    assert!(
        value
            .tool
            .examples
            .iter()
            .all(|example| !example.contains("auth login --dynamic-client")),
        "expected MyChart write examples not to advertise low-level dynamic auth"
    );
}

#[test]
fn tools_describe_mychart_auth_login_redirects_to_ucla_preset_login() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "tools",
        "describe",
        "mychart.cli.auth.login",
        "--json",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("tools describe should succeed");
    let value: JsonToolCatalogDetailEnvelope = parse_json(&output);

    assert_eq!(
        value.tool.name,
        ToolName::new("mychart.cli.auth.login").expect("tool should build")
    );
    assert!(
        value
            .tool
            .notes
            .iter()
            .any(|note| note.contains("prefer the preset login flow")),
        "expected raw auth login notes to steer toward the preset"
    );
    assert!(
        value
            .tool
            .examples
            .iter()
            .any(|example| example == "switchboard mychart.cli.login --ns mychart.ucla --draft -- ucla"),
        "expected raw auth login examples to show mychart login ucla"
    );
    assert!(
        value
            .tool
            .examples
            .iter()
            .all(|example| !example.contains("--base-url")),
        "expected raw auth login examples not to teach manual FHIR endpoint auth"
    );
}

#[test]
fn top_level_help_makes_raw_cli_coverage_explicit() {
    let mut command = Cli::command();
    let mut help = Vec::new();
    command.write_help(&mut help).expect("help output should render");
    let help = String::from_utf8(help).expect("help output should be utf-8");

    assert!(help.contains("Raw CLI Coverage:"));
    assert!(help.contains("any discovered provider CLI command"));
    assert!(help.contains("<provider>.cli.read"));
    assert!(help.contains("<provider>.cli.write"));
    assert!(help.contains("Put switchboard flags before --"));
    assert!(help.contains("switchboard github.cli.write --ns github.personal -- --repo owner/repo"));
}

#[test]
fn tools_describe_curated_tool_includes_typed_arguments() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "tools",
        "describe",
        "google.mail.draft",
        "--json",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("tools describe should succeed");
    let value: JsonToolCatalogDetailEnvelope = parse_json(&output);

    assert_eq!(value.status, "ok");
    assert_eq!(
        value.tool.name,
        ToolName::new("google.mail.draft").expect("tool should build")
    );
    assert_eq!(value.tool.execution_support, ToolExecutionSupport::Executable);
    assert_eq!(value.tool.undo_support, ToolUndoSupport::None);

    let to = value
        .tool
        .arguments
        .iter()
        .find(|argument| argument.name == "to")
        .expect("typed to argument should exist");
    assert!(to.required);
    assert!(to.repeated);
    assert_eq!(to.transport, switchboard_core::ToolArgumentTransport::JsonField);
    assert_eq!(to.value_kind, switchboard_core::ToolArgumentValueKind::String);

    let thread_id = value
        .tool
        .arguments
        .iter()
        .find(|argument| argument.name == "thread-id")
        .expect("thread-id argument should exist");
    assert_eq!(thread_id.transport, switchboard_core::ToolArgumentTransport::JsonField);
    assert_eq!(thread_id.aliases, Vec::<String>::new());
    assert!(!thread_id.required);
}

#[test]
fn raw_google_cli_write_accepts_argv_json_and_executes() {
    let environment = TestEnvironment::with_config_template(ALLOW_WRITES_CONFIG_TEMPLATE);
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
            "switchboard",
            "--config",
            &config_path,
            "google.cli.write",
            "--ns",
            "google.work",
            "--argv-json",
            r#"["gmail","users","drafts","create","--params","{\"userId\":\"me\"}","--json","{\"message\":{\"raw\":\"SGVsbG8sIHdvcmxkIQ==\"}}","--format","json"]"#,
            "--apply",
            "--json",
        ])
        .expect("cli should parse");

    let output = run(cli).expect("raw cli write should execute");
    let value: JsonExecutedResponse<RawGoogleWriteFields> = parse_json(&output);

    assert_eq!(value.status, "executed");
    assert_eq!(
        value.tool,
        ToolName::new("google.cli.write").expect("tool should build")
    );
    assert_eq!(
        value.namespace,
        NamespaceId::new("google.work").expect("namespace should build")
    );
    assert!(value.summary.contains("gws"));
    assert_eq!(value.fields.response.id, "draft-1960work");
    assert!(
        environment
            .gws_capture_contents()
            .contains("ARGV=gmail users drafts create --params {\"userId\":\"me\"}"),
        "expected raw gws argv to reach the provider backend"
    );
}

#[test]
fn raw_google_cli_read_supports_double_dash_passthrough() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.cli.read",
        "--ns",
        "google.work",
        "--json",
        "--",
        "calendar",
        "+agenda",
        "--format",
        "json",
        "--today",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("raw cli read should execute");
    let value: JsonExecutedResponse<RawGoogleReadFields> = parse_json(&output);

    assert_eq!(value.status, "executed");
    assert_eq!(value.tool, ToolName::new("google.cli.read").expect("tool should build"));
    assert_eq!(
        value.namespace,
        NamespaceId::new("google.work").expect("namespace should build")
    );
    assert!(value.summary.contains("gws"));
    assert_eq!(value.fields.response.count, 2);
    assert_eq!(value.fields.response.events[0].summary, "Standup");
    assert!(
        environment
            .gws_capture_contents()
            .contains("ARGV=calendar +agenda --format json --today"),
        "expected raw passthrough argv to reach gws unchanged"
    );
}

#[test]
fn generated_raw_google_calendar_tool_supports_double_dash_passthrough() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.cli.calendar.+agenda",
        "--ns",
        "google.work",
        "--json",
        "--",
        "--format",
        "json",
        "--today",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("raw cli read should execute");
    let value: JsonExecutedResponse<RawGoogleReadFields> = parse_json(&output);

    assert_eq!(
        value.tool,
        ToolName::new("google.cli.calendar.+agenda").expect("tool should build")
    );
    assert_eq!(value.fields.response.count, 2);
    assert!(
        environment
            .gws_capture_contents()
            .contains("ARGV=calendar +agenda --format json --today"),
        "expected generated raw tool to prepend the fixed agenda command"
    );
}

#[test]
fn raw_mychart_cli_read_supports_double_dash_passthrough() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "mychart.cli.read",
        "--ns",
        "mychart.ucla",
        "--json",
        "--",
        "notes",
        "search",
        "--query",
        "migraine",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("raw mychart cli read should execute");
    let value: JsonExecutedResponse<RawMyChartReadFields<MyChartNotesSearchPayload>> = parse_json(&output);

    assert_eq!(value.status, "executed");
    assert_eq!(
        value.tool,
        ToolName::new("mychart.cli.read").expect("tool should build")
    );
    assert_eq!(
        value.namespace,
        NamespaceId::new("mychart.ucla").expect("namespace should build")
    );
    assert_eq!(value.fields.response.query, "migraine");
    assert_eq!(value.fields.response.notes[0].id.as_deref(), Some("note-123"));
    let capture = parse_mychart_capture(&environment.mychart_capture_contents());
    assert_eq!(
        capture,
        MyChartScriptCapture {
            config: Some("/tmp/switchboard-mychart-ucla/config.json".to_owned()),
            account: Some("ucla".to_owned()),
            base_url: None,
            client_id: None,
            client_secret: None,
            redirect_uri: None,
            access_token: None,
            refresh_token: None,
            username: None,
            argv: "notes search --query migraine".to_owned(),
        }
    );
}

#[test]
fn generated_raw_mychart_notes_search_tool_supports_double_dash_passthrough() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "mychart.cli.notes.search",
        "--ns",
        "mychart.ucla",
        "--json",
        "--",
        "--query",
        "migraine",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("generated raw mychart cli read should execute");
    let value: JsonExecutedResponse<RawMyChartReadFields<MyChartNotesSearchPayload>> = parse_json(&output);

    assert_eq!(
        value.tool,
        ToolName::new("mychart.cli.notes.search").expect("tool should build")
    );
    assert_eq!(value.fields.response.notes.len(), 1);
    assert!(
        environment
            .mychart_capture_contents()
            .contains("ARGV=notes search --query migraine"),
        "expected generated raw mychart tool to prepend the fixed notes search command"
    );
}

#[test]
fn repeated_argv_accepts_dash_prefixed_passthrough_tokens() {
    let request = crate::args::parse_external_tool_invocation(
        [
            "google.cli.read",
            "--ns",
            "google.work",
            "--argv",
            "calendar",
            "--argv",
            "+agenda",
            "--argv",
            "--format",
            "--argv",
            "json",
            "--argv",
            "--today",
        ]
        .into_iter()
        .map(OsString::from)
        .collect(),
    )
    .expect("external tool invocation should parse");

    match request {
        OperationRequest::Single(request) => {
            let argv = request.args.values("argv").collect::<Vec<_>>();
            assert_eq!(argv, vec!["calendar", "+agenda", "--format", "json", "--today"]);
        }
        OperationRequest::AggregateRead(_) => panic!("expected single operation request"),
    }
}

#[test]
fn unwired_read_requests_execute_into_stub_results() {
    let environment = TestEnvironment::new();
    let switchboard = super::load_switchboard(Some(environment.path())).expect("switchboard should build");
    let request = ToolRequest::new(
        "google.drive.search",
        "google.work",
        ExecutionMode::Auto,
        BTreeMap::from([("query".into(), "from:finance".into())]),
    )
    .expect("request should parse");

    let outcome = switchboard.dispatch(request).expect("dispatch should succeed");
    match outcome {
        DispatchOutcome::Executed(output) => {
            let fields: StubStatusFields = parse_output_fields(&output);
            assert_eq!(fields.status, "stub");
        }
        DispatchOutcome::Planned(_) => {
            panic!("read requests should execute by default");
        }
    }
}

#[test]
fn flat_tool_invocation_still_parses_with_clap() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.mail.search",
        "--ns",
        "google.work",
        "--query",
        "from:finance newer_than:7d",
        "--json",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("command should run");
    let value: JsonExecutedResponse<GoogleMailSearchFields> = parse_json(&output);

    assert_eq!(value.status, "executed");
    assert_eq!(
        value.tool,
        ToolName::new("google.mail.search").expect("tool should build")
    );
    assert_eq!(
        value.namespace,
        NamespaceId::new("google.work").expect("namespace should build")
    );
    assert_eq!(value.fields.count, 2);
    assert_eq!(value.refs[0].kind, switchboard_core::ToolRefKind::Message);
}

#[test]
fn gmail_read_returns_stable_message_and_thread_refs() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.mail.read",
        "--ns",
        "google.work",
        "--message-id",
        "1960abc456work",
        "--json",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("command should run");
    let value: JsonExecutedResponse<GoogleMailReadFields> = parse_json(&output);

    assert_eq!(value.status, "executed");
    assert_eq!(
        value.tool,
        ToolName::new("google.mail.read").expect("tool should build")
    );
    assert_eq!(
        value.namespace,
        NamespaceId::new("google.work").expect("namespace should build")
    );
    assert_eq!(value.fields.message.gmail_message_id, "1960abc456work");
    assert_eq!(value.fields.message.gmail_thread_id, "1960thread123work");
    assert_eq!(value.refs[0].kind, switchboard_core::ToolRefKind::Message);
    assert_eq!(value.refs[1].kind, switchboard_core::ToolRefKind::Thread);
}

#[test]
fn github_pull_request_search_returns_stable_refs() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "github.pull_request.search",
        "--ns",
        "github.personal",
        "--query",
        "is:open review-requested:@me",
        "--repo",
        "openai/codex",
        "--limit",
        "10",
        "--json",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("command should run");
    let value: JsonExecutedResponse<GitHubPullRequestSearchFields> = parse_json(&output);

    assert_eq!(value.status, "executed");
    assert_eq!(
        value.tool,
        ToolName::new("github.pull_request.search").expect("tool should build")
    );
    assert_eq!(
        value.namespace,
        NamespaceId::new("github.personal").expect("namespace should build")
    );
    assert!(value.summary.contains("GitHub"));
    assert_eq!(value.fields.count, 2);
    assert_eq!(value.refs[0].kind, switchboard_core::ToolRefKind::PullRequest);
    assert_eq!(value.refs[0].parent_id.as_deref(), Some("openai/codex"));
}

#[test]
fn github_pull_request_read_returns_typed_payload() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "github.pull_request.read",
        "--ns",
        "github.personal",
        "--repo",
        "openai/codex",
        "--number",
        "1382",
        "--json",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("command should run");
    let value: JsonExecutedResponse<GitHubPullRequestReadFields> = parse_json(&output);

    assert_eq!(value.status, "executed");
    assert_eq!(
        value.tool,
        ToolName::new("github.pull_request.read").expect("tool should build")
    );
    assert_eq!(
        value.namespace,
        NamespaceId::new("github.personal").expect("namespace should build")
    );
    assert_eq!(value.fields.pull_request.number, 1382);
    assert_eq!(value.fields.pull_request.assignees, vec!["jessfraz"]);
    assert_eq!(value.fields.pull_request.labels, vec!["infra", "tooling"]);
    assert_eq!(value.refs[0].kind, switchboard_core::ToolRefKind::PullRequest);
    assert_eq!(value.refs[0].parent_id.as_deref(), Some("openai/codex"));
}

#[test]
fn github_issue_read_returns_stable_issue_refs() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "github.issue.read",
        "--ns",
        "github.personal",
        "--repo",
        "openai/codex",
        "--number",
        "77",
        "--json",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("command should run");
    let value: JsonExecutedResponse<GitHubIssueReadFields> = parse_json(&output);

    assert_eq!(value.status, "executed");
    assert_eq!(
        value.tool,
        ToolName::new("github.issue.read").expect("tool should build")
    );
    assert_eq!(
        value.namespace,
        NamespaceId::new("github.personal").expect("namespace should build")
    );
    assert!(value.summary.contains("GitHub"));
    assert_eq!(value.fields.issue.number, 77);
    assert_eq!(value.fields.issue.labels, vec!["enhancement"]);
    assert_eq!(value.refs[0].kind, switchboard_core::ToolRefKind::Issue);
    assert_eq!(value.refs[0].parent_id.as_deref(), Some("openai/codex"));
}

#[test]
fn github_repository_search_returns_typed_repository_results() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "github.repository.search",
        "--ns",
        "github.personal",
        "--query",
        "switchboard",
        "--limit",
        "5",
        "--owner",
        "jessfraz",
        "--topic",
        "rust",
        "--topic",
        "cli",
        "--json",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("command should run");
    let value: JsonExecutedResponse<GitHubRepositorySearchFields> = parse_json(&output);

    assert_eq!(value.status, "executed");
    assert_eq!(
        value.tool,
        ToolName::new("github.repository.search").expect("tool should build")
    );
    assert_eq!(
        value.namespace,
        NamespaceId::new("github.personal").expect("namespace should build")
    );
    assert_eq!(value.fields.count, 2);
    assert_eq!(value.fields.repositories[0].full_name, "jessfraz/switchboard");
    assert_eq!(value.fields.repositories[0].owner, "jessfraz");
    assert_eq!(value.refs[0].kind, switchboard_core::ToolRefKind::Repository);
    assert_eq!(value.refs[0].id, "jessfraz/switchboard");
}

#[test]
fn repeated_namespace_flags_become_aggregate_reads() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.calendar.list",
        "--ns",
        "google.work",
        "--ns",
        "google.personal",
        "--json",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("command should run");
    let value: JsonAggregateReadResponse<CountFields> = parse_json(&output);

    assert_eq!(value.status, "aggregate_read");
    assert_eq!(
        value.tool,
        ToolName::new("google.calendar.list").expect("tool should build")
    );
    assert_eq!(
        value.namespaces,
        vec![
            NamespaceId::new("google.work").expect("namespace should build"),
            NamespaceId::new("google.personal").expect("namespace should build"),
        ]
    );
    assert_eq!(
        value.results[0].namespace,
        NamespaceId::new("google.work").expect("namespace should build")
    );
    assert_eq!(
        value.results[1].namespace,
        NamespaceId::new("google.personal").expect("namespace should build")
    );
    assert_eq!(value.results[0].outcome.status, "executed");
    assert_eq!(value.results[1].outcome.status, "executed");
    assert_eq!(value.results[0].outcome.fields.count, 2);
    assert_eq!(value.results[1].outcome.fields.count, 2);
}

#[test]
fn repeated_namespace_flags_reject_write_tools() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.calendar.create",
        "--ns",
        "google.work",
        "--ns",
        "google.personal",
        "--json",
    ])
    .expect("cli should parse");

    let error = run(cli).expect_err("aggregate write should fail");
    let error = error
        .downcast::<switchboard_core::Error>()
        .expect("expected typed switchboard error");
    match error {
        switchboard_core::Error::AggregateReadRequiresReadTool(tool) => {
            assert_eq!(tool.to_string(), "google.calendar.create");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn valueless_flags_flow_through_to_real_cli_backends() {
    let environment = TestEnvironment::new();
    let config_path = environment.path_string();
    let cli = Cli::try_parse_from([
        "switchboard",
        "--config",
        &config_path,
        "google.calendar.list",
        "--ns",
        "google.work",
        "--today",
        "--json",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("command should run");
    let value: JsonExecutedResponse<CountFields> = parse_json(&output);

    assert_eq!(value.status, "executed");
    assert_eq!(
        value.tool,
        ToolName::new("google.calendar.list").expect("tool should build")
    );
    assert_eq!(
        value.namespace,
        NamespaceId::new("google.work").expect("namespace should build")
    );
    assert!(value.summary.contains("calendar"));
    assert_eq!(value.fields.count, 2);
    assert!(
        environment
            .gws_capture_contents()
            .contains("ARGV=calendar +agenda --format json --today"),
        "expected --today to reach gws"
    );
}

#[test]
fn aggregate_read_operations_can_fan_out_across_calendar_namespaces() {
    let environment = TestEnvironment::new();
    let switchboard = super::load_switchboard(Some(environment.path())).expect("switchboard should build");
    let request = AggregateReadRequest::new(
        "google.calendar.list",
        ["google.work", "google.personal"],
        ExecutionMode::Auto,
        BTreeMap::new(),
    )
    .expect("aggregate request should parse");

    let outcome = switchboard
        .execute_operation(OperationRequest::aggregate_read(request))
        .expect("aggregate read should succeed");

    match outcome {
        OperationOutcome::AggregateRead(outcome) => {
            assert_eq!(outcome.namespaces.len(), 2);
            assert_eq!(outcome.results.len(), 2);
            assert_eq!(outcome.results[0].namespace.to_string(), "google.work");
            assert_eq!(outcome.results[1].namespace.to_string(), "google.personal");
        }
        OperationOutcome::Single(_) => {
            panic!("aggregate read should not collapse into a single operation");
        }
    }
}

#[test]
fn aggregate_reads_reject_write_tools() {
    let environment = TestEnvironment::new();
    let switchboard = super::load_switchboard(Some(environment.path())).expect("switchboard should build");
    let request = AggregateReadRequest::new(
        "google.calendar.create",
        ["google.work", "google.personal"],
        ExecutionMode::Auto,
        BTreeMap::new(),
    )
    .expect("aggregate request should parse");

    let error = switchboard
        .execute_operation(OperationRequest::aggregate_read(request))
        .expect_err("aggregate write should fail");

    assert_eq!(
        error,
        switchboard_core::Error::AggregateReadRequiresReadTool(
            ToolName::new("google.calendar.create").expect("tool should build")
        )
    );
}

#[test]
fn human_output_renders_structured_fields_without_flattening_them_into_nonsense() {
    let output = ToolOutput::new(
        ToolName::new("google.calendar.list").expect("tool should build"),
        NamespaceId::new("google.work").expect("namespace should build"),
        "agenda summary",
    )
    .with_field("status", "ok")
    .with_value_field(
        "events",
        json!([
            {
                "id": "event-123",
                "title": "Vet visit",
            }
        ]),
    );

    let rendered = super::render_output_human(&output);

    assert!(rendered.contains("- status: ok"));
    assert!(rendered.contains("- events:"));
    assert!(rendered.contains("\"title\": \"Vet visit\""));
}

#[test]
fn human_output_renders_refs_without_hiding_them_in_json_soup() {
    let output = ToolOutput::new(
        ToolName::new("google.mail.read").expect("tool should build"),
        NamespaceId::new("google.work").expect("namespace should build"),
        "read gmail message",
    )
    .with_ref(
        switchboard_core::ToolRef::new(
            switchboard_core::ProviderKind::GoogleWorkspace,
            NamespaceId::new("google.work").expect("namespace should build"),
            switchboard_core::ToolRefKind::Message,
            "1960abc456work",
        )
        .expect("tool ref should build")
        .with_label("Booking details for June stay")
        .expect("tool ref label should build"),
    );

    let rendered = super::render_output_human(&output);

    assert!(rendered.contains("Refs:"));
    assert!(rendered.contains("google:message id=1960abc456work"));
}

#[test]
fn config_path_selection_prefers_explicit_paths_first() {
    let selected = select_config_path(ConfigPathCandidates {
        explicit: Some(PathBuf::from("/explicit.toml")),
        cwd: Some(PathBuf::from("/cwd.toml")),
        ..ConfigPathCandidates::default()
    })
    .expect("an explicit path should win");

    assert_eq!(selected, PathBuf::from("/explicit.toml"));
}

#[test]
fn config_path_selection_falls_back_in_documented_order() {
    let selected = select_config_path(ConfigPathCandidates {
        cwd: Some(PathBuf::from("/cwd.toml")),
        home: Some(PathBuf::from("/home.toml")),
        ..ConfigPathCandidates::default()
    })
    .expect("a discovered config should be selected");

    assert_eq!(selected, PathBuf::from("/cwd.toml"));
}

#[test]
fn local_build_discovers_xdg_global_config_and_resolves_relative_paths() {
    let environment = TestEnvironment::new();
    let xdg_root = environment.directory.join("xdg");
    let config_dir = xdg_root.join("switchboard");
    fs::create_dir_all(config_dir.join("secrets")).expect("xdg config dir should exist");
    let oauth_path = config_dir.join("secrets").join("google-personal-oauth.json");
    fs::write(&oauth_path, GOOGLE_PERSONAL_OAUTH_JSON).expect("oauth file should write");
    let config_path = config_dir.join("config.toml");
    fs::write(
        &config_path,
        render_relative_global_config("secrets/google-personal-oauth.json"),
    )
    .expect("global config should write");
    env::set_var("XDG_CONFIG_HOME", &xdg_root);

    let cli = Cli::try_parse_from([
        "switchboard",
        "google.calendar.list",
        "--ns",
        "google.personal",
        "--today",
        "--json",
    ])
    .expect("cli should parse");

    let output = run(cli).expect("command should run from global config");
    let value: JsonExecutedResponse<CountFields> = parse_json(&output);
    assert_eq!(value.status, "executed");
    assert_eq!(
        value.namespace,
        NamespaceId::new("google.personal").expect("namespace should build")
    );
    assert_eq!(value.fields.count, 2);

    let capture = environment.gws_capture_contents();
    assert!(
        capture.contains(&format!(
            "CONFIG_DIR={}",
            config_dir.join("state").join("google-personal").display()
        )),
        "expected global config relative state_dir to resolve against the config directory"
    );
    assert!(
        capture.contains("CREDENTIALS_FILE="),
        "expected global config oauth-file auth to materialize credentials for the backend"
    );

    env::remove_var("XDG_CONFIG_HOME");
}

struct TestEnvironment {
    _env_guard: MutexGuard<'static, ()>,
    directory: PathBuf,
    path: PathBuf,
    _gws_script: TempScript,
    _gh_script: TempScript,
    _mychart_script: TempScript,
}

impl TestEnvironment {
    fn new() -> Self {
        Self::with_config_template(BASIC_CONFIG_TEMPLATE)
    }

    fn with_config_template(config_template: &str) -> Self {
        let env_guard = lock_env();
        let directory = test_fixture_directory();
        fs::create_dir_all(&directory).expect("temp dir should be created");
        env::set_var("GOOGLE_WORKSPACE_CLI_CLIENT_ID", "google-work-client-id");
        env::set_var("GOOGLE_WORKSPACE_CLI_CLIENT_SECRET", "google-work-client-secret");
        let gws_script = TempScript::new("gws-test", &render_google_script_template());
        let gh_script = TempScript::new("gh-test", &render_github_script_template());
        let mychart_script = TempScript::new("mychart-test", &render_mychart_script_template());
        env::set_var("SWITCHBOARD_GWS_BIN", gws_script.path());
        env::set_var("SWITCHBOARD_GH_BIN", gh_script.path());
        env::set_var("SWITCHBOARD_MYCHART_BIN", mychart_script.path());
        env::set_var("SWITCHBOARD_STATE_DIR", directory.join("state"));
        let oauth_path = directory.join("google-personal-oauth.json");
        fs::write(&oauth_path, GOOGLE_PERSONAL_OAUTH_JSON).expect("oauth fixture should be written");
        let path = directory.join("switchboard.toml");
        let contents = config_template.replace(
            "__GOOGLE_PERSONAL_OAUTH_PATH__",
            oauth_path.to_str().expect("oauth fixture path should be valid utf-8"),
        );
        fs::write(&path, contents).expect("config should be written");

        Self {
            _env_guard: env_guard,
            directory,
            path,
            _gws_script: gws_script,
            _gh_script: gh_script,
            _mychart_script: mychart_script,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn path_string(&self) -> String {
        self.path.to_str().expect("temp path should be valid utf-8").to_owned()
    }

    fn gws_capture_contents(&self) -> String {
        self._gws_script.capture_contents()
    }

    fn mychart_capture_contents(&self) -> String {
        self._mychart_script.capture_contents()
    }
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        env::remove_var("GOOGLE_WORKSPACE_CLI_CLIENT_ID");
        env::remove_var("GOOGLE_WORKSPACE_CLI_CLIENT_SECRET");
        env::remove_var("SWITCHBOARD_GWS_BIN");
        env::remove_var("SWITCHBOARD_GH_BIN");
        env::remove_var("SWITCHBOARD_MYCHART_BIN");
        env::remove_var("SWITCHBOARD_STATE_DIR");
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn test_fixture_directory() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    env::temp_dir().join(format!("switchboard-test-{}-{stamp}", std::process::id()))
}

fn render_google_script_template() -> String {
    GOOGLE_SCRIPT_TEMPLATE
        .replace("__AGENDA_FIXTURE__", GOOGLE_CALENDAR_AGENDA_FIXTURE)
        .replace("__GMAIL_TRIAGE_FIXTURE__", GOOGLE_GMAIL_TRIAGE_FIXTURE)
        .replace("__GMAIL_READ_FIXTURE__", GOOGLE_GMAIL_READ_FIXTURE)
        .replace("__GMAIL_DRAFT_CREATE_FIXTURE__", GOOGLE_GMAIL_DRAFT_CREATE_FIXTURE)
        .replace("__CALENDAR_CREATE_FIXTURE__", GOOGLE_CALENDAR_CREATE_FIXTURE)
        .replace("__CALENDAR_DELETE_FIXTURE__", GOOGLE_CALENDAR_DELETE_FIXTURE)
        .replace("__AUTH_STATUS_USER__", "jess@example.com")
}

fn render_github_script_template() -> String {
    GITHUB_SCRIPT_TEMPLATE
        .replace("__NOTIFICATIONS_FIXTURE__", GITHUB_NOTIFICATIONS_FIXTURE)
        .replace("__PR_SEARCH_FIXTURE__", GITHUB_PR_SEARCH_FIXTURE)
        .replace("__PR_READ_FIXTURE__", GITHUB_PR_READ_FIXTURE)
        .replace("__ISSUE_READ_FIXTURE__", GITHUB_ISSUE_READ_FIXTURE)
        .replace("__REPOSITORY_SEARCH_FIXTURE__", GITHUB_REPOSITORY_SEARCH_FIXTURE)
        .replace("__REPO_VIEW_FIXTURE__", GITHUB_REPO_VIEW_FIXTURE)
}

fn render_relative_global_config(google_personal_oauth_path: &str) -> String {
    BASIC_CONFIG_TEMPLATE
        .replace("__GOOGLE_PERSONAL_OAUTH_PATH__", google_personal_oauth_path)
        .replace("/tmp/switchboard-google-work", "state/google-work")
        .replace("/tmp/switchboard-google-personal", "state/google-personal")
        .replace("/tmp/switchboard-mychart-ucla", "state/mychart-ucla")
}

fn render_mychart_script_template() -> String {
    MYCHART_SCRIPT_TEMPLATE
        .replace(
            "__APPOINTMENTS_UPCOMING_FIXTURE__",
            MYCHART_APPOINTMENTS_UPCOMING_FIXTURE,
        )
        .replace("__NOTES_SEARCH_FIXTURE__", MYCHART_NOTES_SEARCH_FIXTURE)
}
