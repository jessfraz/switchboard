mod materializer;

use std::{fs, sync::OnceLock};

use switchboard_core::{
    Adapter, AuthScopeProfile, Error, ExecutionTarget, PlannedAction, PlanningTarget, ProviderKind, Result,
    ToolArgument, ToolDescriptor, ToolKind, ToolOutput, ToolRequest,
};

use crate::{
    cli::{passthrough, CliExecutableSpec, CliProviderBackend, CliProviderCatalog, CliStdioMode},
    google::materializer::DefaultGoogleWorkspaceCliMaterializer,
    inventory::embedded_inventory,
};
const MANIFEST_JSON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/manifests/google.json"));
const DEFAULT_AUTH_LOGIN_SCOPES: &str = concat!(
    "https://www.googleapis.com/auth/drive,",
    "https://www.googleapis.com/auth/spreadsheets,",
    "https://www.googleapis.com/auth/gmail.modify,",
    "https://www.googleapis.com/auth/calendar,",
    "https://www.googleapis.com/auth/documents,",
    "https://www.googleapis.com/auth/presentations,",
    "https://www.googleapis.com/auth/tasks,",
    "https://www.googleapis.com/auth/contacts,",
    "https://www.googleapis.com/auth/cloud-platform"
);
const WORKSPACE_ADMIN_AUTH_LOGIN_SCOPES: &str = concat!(
    "https://www.googleapis.com/auth/admin.directory.user,",
    "https://www.googleapis.com/auth/admin.directory.orgunit,",
    "https://www.googleapis.com/auth/admin.directory.group,",
    "https://www.googleapis.com/auth/apps.groups.settings"
);
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
            CliProviderCatalog::from_embedded(MANIFEST_JSON, &inventory)
                .expect("google provider manifest should be valid")
        })
    }

    fn find_command(tool: &str) -> Option<&'static crate::cli::CliCommandSpec> {
        Self::catalog().find_command(tool)
    }

    fn request_with_google_auth_defaults(target: &PlanningTarget, request: &ToolRequest) -> Result<ToolRequest> {
        if request.tool.as_str() != "google.cli.write" {
            return Ok(request.clone());
        }

        let argv = passthrough::parse_passthrough_argv(&request.args)?;
        if argv != ["auth", "login"] {
            return Ok(request.clone());
        }

        let scopes = if target.namespace.auth_scope_profile == AuthScopeProfile::WorkspaceAdmin {
            format!("{DEFAULT_AUTH_LOGIN_SCOPES},{WORKSPACE_ADMIN_AUTH_LOGIN_SCOPES}")
        } else {
            DEFAULT_AUTH_LOGIN_SCOPES.to_owned()
        };

        ToolRequest::new(
            request.tool.as_str(),
            request.namespace.as_str(),
            request.mode,
            vec![ToolArgument::option(
                "argv-json",
                serde_json::json!(["auth", "login", "--scopes", scopes]).to_string(),
            )?],
        )
    }

    fn is_google_auth_login(action: &PlannedAction) -> Result<bool> {
        if action.tool.as_str() != "google.cli.write" {
            return Ok(false);
        }

        let argv = passthrough::parse_passthrough_argv(&action.args)?;
        Ok(argv.len() >= 2 && argv[0] == "auth" && argv[1].starts_with("login"))
    }

    fn verify_google_auth_account(
        &self,
        target: &ExecutionTarget,
        executable: &CliExecutableSpec,
    ) -> Result<Option<String>> {
        let response = self.backend.execute_raw(
            target,
            executable,
            vec!["auth".into(), "status".into()],
            CliStdioMode::Capture,
        )?;
        let status: serde_json::Value = serde_json::from_str(&response.stdout)
            .map_err(|error| Error::Execution(format!("failed to parse gws auth status after auth login: {error}")))?;
        let actual = status
            .get("user")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Execution("gws auth status did not report an authenticated user".into()))?;
        let expected = target.namespace.account_label.trim();

        if expected.contains('@') && !actual.eq_ignore_ascii_case(expected) {
            return Err(Error::Execution(format!(
                "gws auth login authenticated {} as {actual}, but namespace {} expects {expected}",
                target.auth.id, target.namespace.id
            )));
        }

        Ok(Some(actual.to_owned()))
    }

    fn clear_google_auth_token_cache(target: &ExecutionTarget) -> Result<bool> {
        let Some(state_dir) = target.namespace.state_dir.as_ref() else {
            return Ok(false);
        };
        let token_cache = state_dir.join("token_cache.json");

        match fs::remove_file(&token_cache) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(Error::Execution(format!(
                "failed to clear gws token cache at {} after auth login: {error}",
                token_cache.display()
            ))),
        }
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
        let request = Self::request_with_google_auth_defaults(target, request)?;
        let summary = command.summarize.summarize(&target.namespace, &request)?;
        Ok(PlannedAction::new(
            &request,
            target,
            descriptor.kind,
            summary,
            descriptor.backend,
        ))
    }

    fn execute(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
        if let Some(command) = Self::find_command(action.tool.as_str()) {
            if let Some(executable) = command.executable.as_ref() {
                let mut output = self.backend.execute(target, action, executable)?;
                if Self::is_google_auth_login(action)? {
                    if let Some(authenticated_user) = self.verify_google_auth_account(target, executable)? {
                        output = output.with_field("authenticated_user", authenticated_user);
                    }
                    if Self::clear_google_auth_token_cache(target)? {
                        output = output.with_value_field("token_cache_cleared", serde_json::Value::Bool(true));
                    }
                }
                return Ok(output);
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
    use std::{env, fs, path::PathBuf};

    use serde::Deserialize;
    use serde_json::{Map, Value};
    use switchboard_core::{
        Adapter, ApprovalState, AuthKind, AuthScopeProfile, AuthSecretRefs, ExecutionMode, ExecutionTarget,
        OperationApproval, PlanningTarget, ProviderKind, ResolvedAuth, ResolvedCredentials, ResolvedNamespace,
        SecretRef, ToolArgument, ToolExecutionSupport, ToolName, ToolRequest, ToolSurface, ToolUndoSupport,
    };

    use crate::{
        cli::passthrough,
        google::{GoogleWorkspaceAdapter, DEFAULT_AUTH_LOGIN_SCOPES, WORKSPACE_ADMIN_AUTH_LOGIN_SCOPES},
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
        let fields: CalendarListFields = parse_output_fields(&output);

        assert_eq!(output.summary, "Listed 2 calendar events for google.work");
        assert_eq!(fields.count, 2);
        assert_eq!(fields.events[0].title, "Standup");

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
        let fields: MailSearchFields = parse_output_fields(&output);

        assert_eq!(output.summary, "Found 2 Gmail messages for google.work");
        assert_eq!(fields.count, 2);
        assert_eq!(output.refs.len(), 2);
        assert_eq!(output.refs[0].id, "1960abc123work");
        assert_eq!(output.refs[0].kind, switchboard_core::ToolRefKind::Message);
        assert_eq!(fields.messages[0].gmail_message_id, "1960abc123work");

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
        let fields: MailReadFields = parse_output_fields(&output);

        assert_eq!(output.refs.len(), 2);
        assert_eq!(output.refs[0].id, "1960abc456work");
        assert_eq!(output.refs[1].id, "1960thread123work");
        assert_eq!(fields.message.gmail_message_id, "1960abc456work");
        assert_eq!(fields.message.rfc_message_id, "<booking-2026-03-26@doghotel.example>");

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
        let fields: MailDraftFields = parse_output_fields(&output);

        assert_eq!(
            output.summary,
            "Drafted Gmail message \"Boarding request\" for google.work"
        );
        assert_eq!(fields.draft.draft_id, "draft-1960work");
        assert_eq!(fields.draft.gmail_message_id, "1960draftmsgwork");
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
        let fields: CalendarCreateFields = parse_output_fields(&output);

        assert_eq!(
            output.summary,
            "Created calendar event \"Budget review\" for google.work"
        );
        assert_eq!(fields.event.event_id, "event-1960budgetwork");
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
        let fields: RawDraftResponseFields = parse_output_fields(&output);

        assert_eq!(fields.response.id, "draft-1960work");
        assert_eq!(fields.argv.len(), 10);

        let captured = script.capture_contents();
        assert!(captured.contains("ARGV=gmail users drafts create --params {\"userId\":\"me\"}"));
        assert!(captured.contains("--json {\"message\":{\"raw\":\"SGVsbG8sIHdvcmxkIQ==\"}} --format json"));
    }

    #[test]
    fn raw_cli_auth_login_uses_interactive_passthrough() {
        let _env_guard = lock_env();
        let script = google_test_script();
        env::set_var("SWITCHBOARD_GWS_BIN", script.path());

        let adapter = GoogleWorkspaceAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "google.cli.write",
            "google.work",
            ExecutionMode::Apply,
            vec![
                ToolArgument::option("argv-json", serde_json::json!(["auth", "login"]).to_string())
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

        assert_eq!(output.fields.get("status"), Some(&Value::String("ok".into())));
        assert!(!output.fields.contains_key("stdout_text"));
        assert!(!output.fields.contains_key("cli_stderr"));

        let captured = script.capture_contents();
        assert!(captured.contains(&format!(
            "ARGV=auth login --scopes {DEFAULT_AUTH_LOGIN_SCOPES},{WORKSPACE_ADMIN_AUTH_LOGIN_SCOPES}"
        )));
        assert!(captured.contains("https://www.googleapis.com/auth/contacts"));
        for scope in [
            "https://www.googleapis.com/auth/admin.directory.user",
            "https://www.googleapis.com/auth/admin.directory.orgunit",
            "https://www.googleapis.com/auth/admin.directory.group",
            "https://www.googleapis.com/auth/apps.groups.settings",
        ] {
            assert!(
                captured.contains(scope),
                "default Google auth login should request {scope}"
            );
        }
    }

    #[test]
    fn raw_cli_personal_auth_login_omits_workspace_admin_scopes() {
        let adapter = GoogleWorkspaceAdapter::default();
        let mut planning = planning_target();
        planning.namespace = ResolvedNamespace::new(
            "google.personal",
            ProviderKind::GoogleWorkspace,
            "me@example.com",
            "google.personal_auth",
            false,
            Some(PathBuf::from("/tmp/gws-personal")),
        )
        .expect("namespace should build");
        let request = ToolRequest::new(
            "google.cli.write",
            "google.personal",
            ExecutionMode::Apply,
            vec![
                ToolArgument::option("argv-json", serde_json::json!(["auth", "login"]).to_string())
                    .expect("option should build"),
            ],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let argv = passthrough::parse_passthrough_argv(&action.args).expect("auth login argv should parse");

        assert_eq!(argv, ["auth", "login", "--scopes", DEFAULT_AUTH_LOGIN_SCOPES]);
        assert!(!argv.iter().any(|arg| arg.contains(WORKSPACE_ADMIN_AUTH_LOGIN_SCOPES)));
    }

    #[test]
    fn raw_cli_auth_login_clears_cached_access_token() {
        let _env_guard = lock_env();
        let script = google_test_script();
        env::set_var("SWITCHBOARD_GWS_BIN", script.path());

        let state_dir = script
            .path()
            .parent()
            .expect("test script should live in a directory")
            .join("gws-state");
        fs::create_dir_all(&state_dir).expect("state directory should be created");
        let token_cache = state_dir.join("token_cache.json");
        fs::write(&token_cache, "stale access token").expect("token cache fixture should be written");

        let adapter = GoogleWorkspaceAdapter::default();
        let planning = planning_target_with_state_dir(state_dir.clone());
        let request = ToolRequest::new(
            "google.cli.write",
            "google.work",
            ExecutionMode::Apply,
            vec![
                ToolArgument::option("argv-json", serde_json::json!(["auth", "login"]).to_string())
                    .expect("option should build"),
            ],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let output = adapter
            .execute(&execution_target_with_state_dir(state_dir), &action)
            .expect("execution should succeed");

        assert!(!token_cache.exists());
        assert_eq!(output.fields.get("token_cache_cleared"), Some(&Value::Bool(true)));
    }

    #[test]
    fn raw_cli_auth_login_preserves_explicit_args() {
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
                serde_json::json!(["auth", "login", "--readonly"]).to_string(),
            )
            .expect("option should build")],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        adapter
            .execute(&execution_target(), &action)
            .expect("execution should succeed");

        let captured = script.capture_contents();
        assert!(captured.contains("ARGV=auth login --readonly"));
        assert!(!captured.contains(DEFAULT_AUTH_LOGIN_SCOPES));
    }

    #[test]
    fn raw_cli_auth_login_rejects_wrong_account() {
        let _env_guard = lock_env();
        let script = google_test_script_with_auth_user("wrong@example.com");
        env::set_var("SWITCHBOARD_GWS_BIN", script.path());

        let adapter = GoogleWorkspaceAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "google.cli.write",
            "google.work",
            ExecutionMode::Apply,
            vec![
                ToolArgument::option("argv-json", serde_json::json!(["auth", "login"]).to_string())
                    .expect("option should build"),
            ],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let error = adapter
            .execute(&execution_target(), &action)
            .expect_err("wrong google account should fail auth verification");

        assert!(error.to_string().contains("expects jess@example.com"));
        assert!(error.to_string().contains("wrong@example.com"));
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
        let fields: RawResponseCountFields = parse_output_fields(&output);

        assert_eq!(fields.response.count, 2);

        let captured = script.capture_contents();
        assert!(captured.contains("ARGV=calendar +agenda --format json --today"));
    }

    #[test]
    fn planning_only_drive_search_uses_manifest_summary_template() {
        let adapter = GoogleWorkspaceAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "google.drive.search",
            "google.work",
            ExecutionMode::Plan,
            vec![ToolArgument::option("query", "budget spreadsheet").expect("query should build")],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");

        assert_eq!(
            action.summary,
            "Search Google Drive in google.work for budget spreadsheet"
        );
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

    #[derive(Debug, Deserialize)]
    struct CalendarListFields {
        count: usize,
        events: Vec<CalendarListEvent>,
    }

    #[derive(Debug, Deserialize)]
    struct CalendarListEvent {
        title: String,
    }

    #[derive(Debug, Deserialize)]
    struct MailSearchFields {
        count: usize,
        messages: Vec<MailSearchMessage>,
    }

    #[derive(Debug, Deserialize)]
    struct MailSearchMessage {
        gmail_message_id: String,
    }

    #[derive(Debug, Deserialize)]
    struct MailReadFields {
        message: MailReadMessage,
    }

    #[derive(Debug, Deserialize)]
    struct MailReadMessage {
        gmail_message_id: String,
        rfc_message_id: String,
    }

    #[derive(Debug, Deserialize)]
    struct MailDraftFields {
        draft: MailDraftDetails,
    }

    #[derive(Debug, Deserialize)]
    struct MailDraftDetails {
        draft_id: String,
        gmail_message_id: String,
    }

    #[derive(Debug, Deserialize)]
    struct CalendarCreateFields {
        event: CalendarCreateEvent,
    }

    #[derive(Debug, Deserialize)]
    struct CalendarCreateEvent {
        event_id: String,
    }

    #[derive(Debug, Deserialize)]
    struct RawDraftResponseFields {
        response: RawDraftResponse,
        argv: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    struct RawDraftResponse {
        id: String,
    }

    #[derive(Debug, Deserialize)]
    struct RawResponseCountFields {
        response: CountResponse,
    }

    #[derive(Debug, Deserialize)]
    struct CountResponse {
        count: usize,
    }

    fn parse_output_fields<T: for<'de> Deserialize<'de>>(output: &switchboard_core::ToolOutput) -> T {
        serde_json::from_value(Value::Object(
            output
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Map<String, Value>>(),
        ))
        .expect("output fields should deserialize")
    }

    fn google_test_script() -> TempScript {
        google_test_script_with_auth_user("jess@example.com")
    }

    fn google_test_script_with_auth_user(auth_user: &str) -> TempScript {
        TempScript::new("gws-test", &render_google_script(auth_user))
    }

    fn render_google_script(auth_user: &str) -> String {
        GOOGLE_SCRIPT_TEMPLATE
            .replace("__AGENDA_FIXTURE__", AGENDA_FIXTURE)
            .replace("__GMAIL_TRIAGE_FIXTURE__", GMAIL_TRIAGE_FIXTURE)
            .replace("__GMAIL_READ_FIXTURE__", GMAIL_READ_FIXTURE)
            .replace("__GMAIL_DRAFT_CREATE_FIXTURE__", GMAIL_DRAFT_CREATE_FIXTURE)
            .replace("__CALENDAR_DELETE_FIXTURE__", CALENDAR_DELETE_FIXTURE)
            .replace("__CALENDAR_CREATE_FIXTURE__", CALENDAR_CREATE_FIXTURE)
            .replace("__AUTH_STATUS_USER__", auth_user)
    }

    fn planning_target() -> PlanningTarget {
        planning_target_with_state_dir(PathBuf::from("/tmp/gws-work"))
    }

    fn planning_target_with_state_dir(state_dir: PathBuf) -> PlanningTarget {
        PlanningTarget {
            namespace: ResolvedNamespace::new(
                "google.work",
                ProviderKind::GoogleWorkspace,
                "jess@example.com",
                "google.work_auth",
                false,
                Some(state_dir),
            )
            .expect("namespace should build")
            .with_auth_scope_profile(AuthScopeProfile::WorkspaceAdmin)
            .expect("workspace admin scope profile should build"),
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
        execution_target_with_state_dir(PathBuf::from("/tmp/gws-work"))
    }

    fn execution_target_with_state_dir(state_dir: PathBuf) -> ExecutionTarget {
        ExecutionTarget {
            namespace: planning_target_with_state_dir(state_dir).namespace,
            auth: planning_target().auth,
            credentials: ResolvedCredentials::GoogleOAuth {
                client_id: "client-id".to_owned().into(),
                client_secret: "client-secret".to_owned().into(),
                refresh_token: Some("refresh-token".to_owned().into()),
            },
        }
    }
}
