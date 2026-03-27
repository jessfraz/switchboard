mod commands;
mod materializer;

use std::sync::OnceLock;

use switchboard_core::{
    Adapter, Error, ExecutionTarget, PlannedAction, PlanningTarget, ProviderKind, Result, ToolArgument, ToolDescriptor,
    ToolKind, ToolOutput, ToolRequest,
};

use crate::{
    cli::{CliProviderBackend, CliProviderCatalog},
    google::{commands::HANDLERS, materializer::DefaultGoogleWorkspaceCliMaterializer},
    inventory::embedded_inventory,
};
const MANIFEST_JSON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/manifests/google.json"));
static CATALOG: OnceLock<CliProviderCatalog> = OnceLock::new();

pub struct GoogleWorkspaceAdapter {
    backend: CliProviderBackend,
}

impl Default for GoogleWorkspaceAdapter {
    fn default() -> Self {
        Self {
            backend: CliProviderBackend::new(Box::new(DefaultGoogleWorkspaceCliMaterializer)),
        }
    }
}

impl GoogleWorkspaceAdapter {
    fn catalog() -> &'static CliProviderCatalog {
        CATALOG.get_or_init(|| {
            let inventory =
                embedded_inventory(ProviderKind::GoogleWorkspace).expect("google inventory should be valid");
            CliProviderCatalog::from_embedded(MANIFEST_JSON, &inventory, HANDLERS)
                .expect("google provider manifest should be valid")
        })
    }

    fn find_command(tool: &str) -> Option<&'static crate::cli::CliCommandSpec> {
        Self::catalog().find_command(tool)
    }

    fn stub_output(target: &ExecutionTarget, action: &PlannedAction) -> ToolOutput {
        ToolOutput::new(
            action.tool.clone(),
            action.namespace.clone(),
            format!("{} via {} (stub)", action.summary, action.backend),
        )
        .with_field("status", "stub")
        .with_field("backend", action.backend.to_string())
        .with_field("auth", target.auth.id.to_string())
        .with_field("note", "google workspace command execution is not wired yet")
    }
}

impl Adapter for GoogleWorkspaceAdapter {
    fn provider(&self) -> ProviderKind {
        ProviderKind::GoogleWorkspace
    }

    fn tools(&self) -> &'static [ToolDescriptor] {
        Self::catalog().tools()
    }

    fn plan(
        &self,
        target: &PlanningTarget,
        request: &ToolRequest,
        descriptor: &'static ToolDescriptor,
    ) -> Result<PlannedAction> {
        let command = Self::find_command(request.tool.as_str())
            .ok_or_else(|| Error::UnsupportedTool(request.tool.to_string()))?;
        let summary = command.summarize.summarize(&target.namespace, request)?;
        Ok(PlannedAction::new(
            request,
            target,
            descriptor.kind,
            summary,
            descriptor.backend,
        ))
    }

    fn execute(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
        if let Some(command) = Self::find_command(action.tool.as_str()) {
            if let Some(executable) = command.executable.as_ref() {
                return self.backend.execute(target, action, executable);
            }

            if matches!(action.kind, ToolKind::Write) {
                return Err(Error::NotImplemented(format!(
                    "{} apply path is not wired to Google Workspace yet",
                    action.tool
                )));
            }

            return Ok(Self::stub_output(target, action));
        }

        Err(Error::UnsupportedTool(action.tool.to_string()))
    }

    fn compensation_request(
        &self,
        operation: &switchboard_core::StoredOperation,
        mode: switchboard_core::ExecutionMode,
    ) -> Result<Option<ToolRequest>> {
        if operation.tool.as_str() != "google.calendar.create" {
            return Ok(None);
        }

        let effect = operation
            .effect
            .as_ref()
            .ok_or_else(|| Error::OperationNotUndoable(operation.id.clone()))?;
        let event_ref = effect
            .refs
            .iter()
            .find(|tool_ref| tool_ref.kind == switchboard_core::ToolRefKind::Event)
            .ok_or_else(|| Error::OperationNotUndoable(operation.id.clone()))?;
        let calendar = event_ref.parent_id.clone().unwrap_or_else(|| "primary".to_owned());

        Ok(Some(ToolRequest::new(
            "google.calendar.delete",
            operation.namespace.to_string(),
            mode,
            vec![
                ToolArgument::option("event-id", event_ref.id.clone())?,
                ToolArgument::option("calendar", calendar)?,
                ToolArgument::option("send-updates", "none")?,
            ],
        )?))
    }
}

#[cfg(test)]
mod tests {
    use std::{env, path::PathBuf};

    use serde_json::Value;
    use switchboard_core::{
        Adapter, ApprovalState, AuthKind, AuthSecretRefs, ExecutionMode, ExecutionTarget, OperationApproval,
        PlanningTarget, ProviderKind, ResolvedAuth, ResolvedCredentials, ResolvedNamespace, SecretRef, ToolArgument,
        ToolExecutionSupport, ToolName, ToolRequest, ToolSurface, ToolUndoSupport,
    };

    use crate::{
        google::GoogleWorkspaceAdapter,
        test_support::{lock_env, TempScript},
    };

    const AGENDA_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/google-calendar-agenda.json"
    ));
    const GMAIL_TRIAGE_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/google-gmail-triage.json"
    ));
    const GMAIL_READ_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/google-gmail-read.json"
    ));
    const GMAIL_DRAFT_CREATE_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/google-gmail-draft-create.json"
    ));
    const CALENDAR_CREATE_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/google-calendar-create.json"
    ));
    const CALENDAR_DELETE_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/google-calendar-delete.json"
    ));
    const GOOGLE_SCRIPT_TEMPLATE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/scripts/gws-test.sh"
    ));

    #[test]
    fn calendar_list_executes_through_generic_cli_runtime() {
        let _env_guard = lock_env();
        let script = google_test_script();
        env::set_var("SWITCHBOARD_GWS_BIN", script.path());

        let adapter = GoogleWorkspaceAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "google.calendar.list",
            "google.work",
            ExecutionMode::Auto,
            vec![
                ToolArgument::flag("today").expect("flag should build"),
                ToolArgument::option("timezone", "America/Los_Angeles").expect("option should build"),
            ],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let output = adapter
            .execute(&execution_target(), &action)
            .expect("execution should succeed");

        assert_eq!(output.summary, "Listed 2 calendar events for google.work");
        assert_eq!(output.fields.get("count"), Some(&serde_json::json!(2)));
        assert_eq!(
            output
                .fields
                .get("events")
                .and_then(Value::as_array)
                .and_then(|events| events.first())
                .and_then(|event| event.get("title"))
                .and_then(Value::as_str),
            Some("Standup")
        );

        let captured = script.capture_contents();
        assert!(captured.contains("CONFIG_DIR=/tmp/gws-work"));
        assert!(captured.contains("CLIENT_ID=client-id"));
        assert!(captured.contains("CLIENT_SECRET=client-secret"));
        assert!(captured.contains("CREDENTIALS_FILE="));
        assert!(captured.contains("TOKEN="));
        assert!(captured.contains("ARGV=calendar +agenda --format json --today --timezone America/Los_Angeles"));
    }

    #[test]
    fn mail_search_executes_through_generic_cli_runtime() {
        let _env_guard = lock_env();
        let script = google_test_script();
        env::set_var("SWITCHBOARD_GWS_BIN", script.path());

        let adapter = GoogleWorkspaceAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "google.mail.search",
            "google.work",
            ExecutionMode::Auto,
            vec![
                ToolArgument::option("query", "from:carwash newer_than:30d").expect("option should build"),
                ToolArgument::option("max", "5").expect("option should build"),
                ToolArgument::flag("labels").expect("flag should build"),
            ],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let output = adapter
            .execute(&execution_target(), &action)
            .expect("execution should succeed");

        assert_eq!(output.summary, "Found 2 Gmail messages for google.work");
        assert_eq!(output.fields.get("count"), Some(&serde_json::json!(2)));
        assert_eq!(output.refs.len(), 2);
        assert_eq!(output.refs[0].id, "1960abc123work");
        assert_eq!(output.refs[0].kind, switchboard_core::ToolRefKind::Message);
        assert_eq!(
            output
                .fields
                .get("messages")
                .and_then(Value::as_array)
                .and_then(|messages| messages.first())
                .and_then(|message| message.get("gmail_message_id"))
                .and_then(Value::as_str),
            Some("1960abc123work")
        );

        let captured = script.capture_contents();
        assert!(
            captured.contains("ARGV=gmail +triage --format json --query from:carwash newer_than:30d --max 5 --labels")
        );
    }

    #[test]
    fn mail_read_executes_through_generic_cli_runtime() {
        let _env_guard = lock_env();
        let script = google_test_script();
        env::set_var("SWITCHBOARD_GWS_BIN", script.path());

        let adapter = GoogleWorkspaceAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "google.mail.read",
            "google.work",
            ExecutionMode::Auto,
            vec![ToolArgument::option("message-id", "1960abc456work").expect("option should build")],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let output = adapter
            .execute(&execution_target(), &action)
            .expect("execution should succeed");

        assert_eq!(output.refs.len(), 2);
        assert_eq!(output.refs[0].id, "1960abc456work");
        assert_eq!(output.refs[1].id, "1960thread123work");
        assert_eq!(
            output
                .fields
                .get("message")
                .and_then(|message| message.get("gmail_message_id"))
                .and_then(Value::as_str),
            Some("1960abc456work")
        );
        assert_eq!(
            output
                .fields
                .get("message")
                .and_then(|message| message.get("rfc_message_id"))
                .and_then(Value::as_str),
            Some("<booking-2026-03-26@doghotel.example>")
        );

        let captured = script.capture_contents();
        assert!(captured.contains("ARGV=gmail +read --id 1960abc456work --format json"));
    }

    #[test]
    fn mail_draft_executes_through_generic_cli_runtime() {
        let _env_guard = lock_env();
        let script = google_test_script();
        env::set_var("SWITCHBOARD_GWS_BIN", script.path());

        let adapter = GoogleWorkspaceAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "google.mail.draft",
            "google.work",
            ExecutionMode::Apply,
            vec![
                ToolArgument::option("to", "dogs@example.com").expect("option should build"),
                ToolArgument::option("cc", "frontdesk@example.com").expect("option should build"),
                ToolArgument::option("subject", "Boarding request").expect("option should build"),
                ToolArgument::option("body-text", "Hi there,\nCan you board the dogs next week?")
                    .expect("option should build"),
            ],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let output = adapter
            .execute(&execution_target(), &action)
            .expect("execution should succeed");

        assert_eq!(
            output.summary,
            "Drafted Gmail message \"Boarding request\" for google.work"
        );
        assert_eq!(
            output
                .fields
                .get("draft")
                .and_then(|draft| draft.get("draft_id"))
                .and_then(Value::as_str),
            Some("draft-1960work")
        );
        assert_eq!(
            output
                .fields
                .get("draft")
                .and_then(|draft| draft.get("gmail_message_id"))
                .and_then(Value::as_str),
            Some("1960draftmsgwork")
        );
        assert_eq!(output.refs.len(), 2);
        assert_eq!(output.refs[0].kind, switchboard_core::ToolRefKind::Message);
        assert_eq!(output.refs[1].kind, switchboard_core::ToolRefKind::Thread);
        assert_eq!(output.effect.as_ref().map(|effect| effect.undoable), Some(false));

        let captured = script.capture_contents();
        assert!(captured.contains("ARGV=gmail users drafts create --params {\"userId\":\"me\"}"));
        assert!(captured.contains("--format json"));
        assert!(captured.contains("\"raw\":"));
    }

    #[test]
    fn calendar_create_executes_through_generic_cli_runtime_and_captures_effects() {
        let _env_guard = lock_env();
        let script = google_test_script();
        env::set_var("SWITCHBOARD_GWS_BIN", script.path());

        let adapter = GoogleWorkspaceAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "google.calendar.create",
            "google.work",
            ExecutionMode::Apply,
            vec![
                ToolArgument::option("title", "Budget review").expect("option should build"),
                ToolArgument::option("start", "2026-03-30T15:00:00-07:00").expect("option should build"),
                ToolArgument::option("end", "2026-03-30T15:30:00-07:00").expect("option should build"),
                ToolArgument::option("calendar", "primary").expect("option should build"),
                ToolArgument::option("location", "Conference Room B").expect("option should build"),
                ToolArgument::option("description", "Review the Q2 budget plan").expect("option should build"),
                ToolArgument::option("attendee", "alice@example.com").expect("option should build"),
                ToolArgument::option("attendee", "bob@example.com").expect("option should build"),
                ToolArgument::flag("meet").expect("flag should build"),
            ],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let output = adapter
            .execute(&execution_target(), &action)
            .expect("execution should succeed");

        assert_eq!(
            output.summary,
            "Created calendar event \"Budget review\" for google.work"
        );
        assert_eq!(
            output
                .fields
                .get("event")
                .and_then(|event| event.get("event_id"))
                .and_then(Value::as_str),
            Some("event-1960budgetwork")
        );
        assert_eq!(output.refs.len(), 1);
        assert_eq!(output.refs[0].kind, switchboard_core::ToolRefKind::Event);
        assert_eq!(output.refs[0].id, "event-1960budgetwork");
        assert_eq!(output.effect.as_ref().map(|effect| effect.undoable), Some(true));
        assert_eq!(
            output.effect.as_ref().and_then(|effect| effect.undo_summary.as_deref()),
            Some("Delete calendar event \"Budget review\" from google.work")
        );

        let captured = script.capture_contents();
        assert!(captured.contains("ARGV=calendar +insert --format json --summary Budget review"));
        assert!(captured.contains("--calendar primary"));
        assert!(captured.contains("--attendee alice@example.com --attendee bob@example.com --meet"));
    }

    #[test]
    fn raw_cli_write_executes_arbitrary_gws_command() {
        let _env_guard = lock_env();
        let script = google_test_script();
        env::set_var("SWITCHBOARD_GWS_BIN", script.path());

        let adapter = GoogleWorkspaceAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "google.cli.write",
            "google.work",
            ExecutionMode::Apply,
            vec![ToolArgument::option(
                "argv-json",
                serde_json::json!([
                    "gmail",
                    "users",
                    "drafts",
                    "create",
                    "--params",
                    "{\"userId\":\"me\"}",
                    "--json",
                    "{\"message\":{\"raw\":\"SGVsbG8sIHdvcmxkIQ==\"}}",
                    "--format",
                    "json"
                ])
                .to_string(),
            )
            .expect("option should build")],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let output = adapter
            .execute(&execution_target(), &action)
            .expect("execution should succeed");

        assert_eq!(
            output
                .fields
                .get("response")
                .and_then(|response| response.get("id"))
                .and_then(Value::as_str),
            Some("draft-1960work")
        );
        assert_eq!(
            output
                .fields
                .get("argv")
                .and_then(Value::as_array)
                .map(|argv| argv.len()),
            Some(10)
        );

        let captured = script.capture_contents();
        assert!(captured.contains("ARGV=gmail users drafts create --params {\"userId\":\"me\"}"));
        assert!(captured.contains("--json {\"message\":{\"raw\":\"SGVsbG8sIHdvcmxkIQ==\"}} --format json"));
    }

    #[test]
    fn generated_raw_calendar_tool_executes_with_fixed_cli_prefix() {
        let _env_guard = lock_env();
        let script = google_test_script();
        env::set_var("SWITCHBOARD_GWS_BIN", script.path());

        let adapter = GoogleWorkspaceAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "google.cli.calendar.+agenda",
            "google.work",
            ExecutionMode::Auto,
            vec![ToolArgument::option(
                "argv-json",
                serde_json::json!(["--format", "json", "--today"]).to_string(),
            )
            .expect("option should build")],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let output = adapter
            .execute(&execution_target(), &action)
            .expect("execution should succeed");

        assert_eq!(
            output
                .fields
                .get("response")
                .and_then(|response| response.get("count"))
                .and_then(Value::as_u64),
            Some(2)
        );

        let captured = script.capture_contents();
        assert!(captured.contains("ARGV=calendar +agenda --format json --today"));
    }

    #[test]
    fn compensation_request_for_calendar_create_targets_calendar_delete() {
        let adapter = GoogleWorkspaceAdapter::default();
        let request = adapter
            .compensation_request(
                &switchboard_core::StoredOperation {
                    id: switchboard_core::OperationId::new("op_undo_test").expect("operation id should build"),
                    tool: switchboard_core::ToolName::new("google.calendar.create").expect("tool should build"),
                    namespace: switchboard_core::NamespaceId::new("google.work").expect("namespace should build"),
                    auth_ref: switchboard_core::AuthRef::new("google.work").expect("auth ref should build"),
                    kind: switchboard_core::ToolKind::Write,
                    summary: "Create calendar event".into(),
                    backend: switchboard_core::BackendKind::Cli,
                    approval_required: true,
                    approval_reason: Some("approval".into()),
                    compensates_operation_id: None,
                    approval: OperationApproval {
                        state: ApprovalState::Approved,
                        actor: Some("tester".into()),
                        note: None,
                    },
                    status: switchboard_core::OperationStatus::Applied,
                    args: switchboard_core::ToolArguments::empty(),
                    effect: Some(
                        switchboard_core::OperationEffect::new(true).with_ref(
                            switchboard_core::ToolRef::new(
                                switchboard_core::ProviderKind::GoogleWorkspace,
                                switchboard_core::NamespaceId::new("google.work").expect("namespace should build"),
                                switchboard_core::ToolRefKind::Event,
                                "event-1960budgetwork",
                            )
                            .expect("tool ref should build")
                            .with_parent_id("primary")
                            .expect("parent id should build"),
                        ),
                    ),
                    failure_reason: None,
                },
                ExecutionMode::Apply,
            )
            .expect("compensation request should build")
            .expect("compensation request should exist");

        assert_eq!(request.tool.to_string(), "google.calendar.delete");
        assert_eq!(request.namespace.to_string(), "google.work");
        assert_eq!(request.args.value("event-id"), Some("event-1960budgetwork"));
        assert_eq!(request.args.value("calendar"), Some("primary"));
    }

    #[test]
    fn manifest_catalog_marks_raw_planning_and_undo_metadata() {
        let adapter = GoogleWorkspaceAdapter::default();
        let calendar_create = adapter
            .find_tool(&ToolName::new("google.calendar.create").expect("tool should build"))
            .expect("tool should exist");
        assert_eq!(calendar_create.surface, ToolSurface::Curated);
        assert_eq!(calendar_create.execution_support, ToolExecutionSupport::Executable);
        assert_eq!(calendar_create.undo_support, ToolUndoSupport::CompensatingAction);

        let mail_send = adapter
            .find_tool(&ToolName::new("google.mail.send").expect("tool should build"))
            .expect("tool should exist");
        assert_eq!(mail_send.execution_support, ToolExecutionSupport::PlanningOnly);

        let raw_write = adapter
            .find_tool(&ToolName::new("google.cli.write").expect("tool should build"))
            .expect("tool should exist");
        assert_eq!(raw_write.surface, ToolSurface::Raw);

        let inventory_raw = adapter
            .find_tool(&ToolName::new("google.cli.calendar.events.delete").expect("tool should build"))
            .expect("tool should exist");
        assert_eq!(inventory_raw.surface, ToolSurface::Raw);
        assert_eq!(inventory_raw.execution_support, ToolExecutionSupport::Executable);
    }

    fn google_test_script() -> TempScript {
        TempScript::new("gws-test", &render_google_script())
    }

    fn render_google_script() -> String {
        GOOGLE_SCRIPT_TEMPLATE
            .replace("__AGENDA_FIXTURE__", AGENDA_FIXTURE)
            .replace("__GMAIL_TRIAGE_FIXTURE__", GMAIL_TRIAGE_FIXTURE)
            .replace("__GMAIL_READ_FIXTURE__", GMAIL_READ_FIXTURE)
            .replace("__GMAIL_DRAFT_CREATE_FIXTURE__", GMAIL_DRAFT_CREATE_FIXTURE)
            .replace("__CALENDAR_DELETE_FIXTURE__", CALENDAR_DELETE_FIXTURE)
            .replace("__CALENDAR_CREATE_FIXTURE__", CALENDAR_CREATE_FIXTURE)
    }

    fn planning_target() -> PlanningTarget {
        PlanningTarget {
            namespace: ResolvedNamespace::new(
                "google.work",
                ProviderKind::GoogleWorkspace,
                "Google Workspace (work)",
                "google.work_auth",
                false,
                Some(PathBuf::from("/tmp/gws-work")),
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "google.work_auth",
                ProviderKind::GoogleWorkspace,
                AuthKind::GoogleOAuth,
                "jess@example.com",
                AuthSecretRefs::GoogleOAuth {
                    client_id: SecretRef::new("google.work_client_id").expect("secret ref should build"),
                    client_secret: SecretRef::new("google.work_client_secret").expect("secret ref should build"),
                    refresh_token: Some(SecretRef::new("google.work_refresh_token").expect("secret ref should build")),
                },
            )
            .expect("auth should build"),
        }
    }

    fn execution_target() -> ExecutionTarget {
        ExecutionTarget {
            namespace: planning_target().namespace,
            auth: planning_target().auth,
            credentials: ResolvedCredentials::GoogleOAuth {
                client_id: "client-id".to_owned().into(),
                client_secret: "client-secret".to_owned().into(),
                refresh_token: Some("refresh-token".to_owned().into()),
            },
        }
    }
}
