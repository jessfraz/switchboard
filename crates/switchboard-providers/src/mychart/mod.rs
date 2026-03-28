mod materializer;

use std::sync::OnceLock;

use switchboard_core::{
    Adapter, Error, ExecutionTarget, PlannedAction, PlanningTarget, ProviderKind, Result, ToolDescriptor, ToolKind,
    ToolOutput, ToolRequest,
};

use crate::{
    cli::{CliProviderBackend, CliProviderCatalog},
    inventory::embedded_inventory,
    mychart::materializer::DefaultMyChartCliMaterializer,
};

const MANIFEST_JSON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/manifests/mychart.json"));
static CATALOG: OnceLock<CliProviderCatalog> = OnceLock::new();

pub struct MyChartAdapter {
    backend: CliProviderBackend,
}

impl Default for MyChartAdapter {
    fn default() -> Self {
        Self {
            backend: CliProviderBackend::new(Box::new(DefaultMyChartCliMaterializer)),
        }
    }
}

impl MyChartAdapter {
    fn catalog() -> &'static CliProviderCatalog {
        CATALOG.get_or_init(|| {
            let inventory = embedded_inventory(ProviderKind::MyChart).expect("mychart inventory should be valid");
            CliProviderCatalog::from_embedded(MANIFEST_JSON, &inventory)
                .expect("mychart provider manifest should be valid")
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
        .with_field("note", "mychart command execution is not wired yet")
    }
}

impl Adapter for MyChartAdapter {
    fn provider(&self) -> ProviderKind {
        ProviderKind::MyChart
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
                    "{} apply path is not wired to MyChart yet",
                    action.tool
                )));
            }

            return Ok(Self::stub_output(target, action));
        }

        Err(Error::UnsupportedTool(action.tool.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::{env, path::PathBuf};

    use serde::Deserialize;
    use switchboard_core::{
        Adapter, AuthKind, AuthSecretRefs, ExecutionMode, ExecutionTarget, PlanningTarget, ProviderKind, ResolvedAuth,
        ResolvedCredentials, ResolvedNamespace, ToolArgument, ToolExecutionSupport, ToolName, ToolRequest, ToolSurface,
    };

    use crate::{
        mychart::MyChartAdapter,
        test_support::{lock_env, TempScript},
    };

    const APPOINTMENTS_UPCOMING_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/mychart-appointments-upcoming.json"
    ));
    const NOTES_SEARCH_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/mychart-notes-search.json"
    ));
    const MYCHART_SCRIPT_TEMPLATE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/scripts/mychart-test.sh"
    ));

    #[test]
    fn raw_read_passthrough_executes_through_generic_cli_runtime() {
        let _env_guard = lock_env();
        let script = TempScript::new("mychart-test", &render_mychart_script());
        env::set_var("SWITCHBOARD_MYCHART_BIN", script.path());

        let adapter = MyChartAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "mychart.cli.read",
            "mychart.ucla",
            ExecutionMode::Auto,
            vec![
                ToolArgument::option("argv", "notes").expect("argv should build"),
                ToolArgument::option("argv", "search").expect("argv should build"),
                ToolArgument::option("argv", "--query").expect("argv should build"),
                ToolArgument::option("argv", "migraine").expect("argv should build"),
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
        let fields: RawFields<NotesSearchPayload> = parse_output_fields(&output);

        assert_eq!(
            output.summary,
            "Ran raw mychart command for mychart.ucla: notes search --query migraine"
        );
        assert_eq!(fields.response.query, "migraine");
        assert_eq!(fields.response.notes.len(), 1);
        assert_eq!(fields.response.notes[0].id.as_deref(), Some("note-123"));
        assert_eq!(fields.argv, vec!["notes", "search", "--query", "migraine"]);

        let captured = script.capture_contents();
        assert!(captured.contains("CONFIG=/tmp/mychart-ucla/config.json"));
        assert!(captured.contains("ACCOUNT=ucla"));
        assert!(captured.contains("BASE_URL=https://fhir.example.org/api/FHIR/R4"));
        assert!(captured.contains("ACCESS_TOKEN="));
        assert!(captured.contains("ARGV=notes search --query migraine"));
    }

    #[test]
    fn inventory_backed_leaf_command_executes_through_generic_cli_runtime() {
        let _env_guard = lock_env();
        let script = TempScript::new("mychart-test", &render_mychart_script());
        env::set_var("SWITCHBOARD_MYCHART_BIN", script.path());

        let adapter = MyChartAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "mychart.cli.appointments.upcoming",
            "mychart.ucla",
            ExecutionMode::Auto,
            vec![
                ToolArgument::option("argv", "--limit").expect("argv should build"),
                ToolArgument::option("argv", "2").expect("argv should build"),
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
        let fields: RawFields<AppointmentsUpcomingPayload> = parse_output_fields(&output);

        assert_eq!(
            output.summary,
            "Ran raw mychart command for mychart.ucla: appointments upcoming --limit 2"
        );
        assert_eq!(fields.response.appointments.len(), 1);
        assert_eq!(
            fields.response.appointments[0].description.as_deref(),
            Some("Dermatology follow-up")
        );

        let captured = script.capture_contents();
        assert!(captured.contains("ARGV=appointments upcoming --limit 2"));
    }

    #[test]
    fn tool_catalog_includes_inventory_backed_leaf_tools() {
        let adapter = MyChartAdapter::default();
        let appointment_tool = adapter
            .find_tool(&ToolName::new("mychart.cli.appointments.upcoming").expect("tool name should parse"))
            .expect("inventory-backed tool should exist");
        let root_tool = adapter
            .find_tool(&ToolName::new("mychart.cli.read").expect("tool name should parse"))
            .expect("root raw tool should exist");

        assert_eq!(appointment_tool.surface, ToolSurface::Raw);
        assert_eq!(appointment_tool.execution_support, ToolExecutionSupport::Executable);
        assert_eq!(root_tool.surface, ToolSurface::Raw);
        assert_eq!(root_tool.execution_support, ToolExecutionSupport::Executable);
    }

    #[derive(Debug, Deserialize)]
    struct RawFields<TResponse> {
        argv: Vec<String>,
        response: TResponse,
    }

    #[derive(Debug, Deserialize)]
    struct NotesSearchPayload {
        query: String,
        notes: Vec<NoteSummary>,
    }

    #[derive(Debug, Deserialize)]
    struct NoteSummary {
        id: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct AppointmentsUpcomingPayload {
        appointments: Vec<AppointmentRecord>,
    }

    #[derive(Debug, Deserialize)]
    struct AppointmentRecord {
        description: Option<String>,
    }

    fn parse_output_fields<T>(output: &switchboard_core::ToolOutput) -> T
    where
        T: for<'de> Deserialize<'de>,
    {
        serde_json::from_value(serde_json::to_value(&output.fields).expect("fields should serialize"))
            .expect("fields should deserialize")
    }

    fn render_mychart_script() -> String {
        MYCHART_SCRIPT_TEMPLATE
            .replace("__APPOINTMENTS_UPCOMING_FIXTURE__", APPOINTMENTS_UPCOMING_FIXTURE)
            .replace("__NOTES_SEARCH_FIXTURE__", NOTES_SEARCH_FIXTURE)
    }

    fn planning_target() -> PlanningTarget {
        PlanningTarget {
            namespace: ResolvedNamespace::new(
                "mychart.ucla",
                ProviderKind::MyChart,
                "UCLA Health",
                "mychart_ucla",
                false,
                Some(PathBuf::from("/tmp/mychart-ucla")),
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "mychart_ucla",
                ProviderKind::MyChart,
                AuthKind::MyChartCli,
                "ucla",
                AuthSecretRefs::MyChartCli {
                    base_url: None,
                    portal_base_url: None,
                    client_id: None,
                    client_secret: None,
                    redirect_uri: None,
                    access_token: None,
                    refresh_token: None,
                    username: None,
                },
            )
            .expect("auth should build"),
        }
    }

    fn execution_target() -> ExecutionTarget {
        ExecutionTarget {
            namespace: planning_target().namespace,
            auth: planning_target().auth,
            credentials: ResolvedCredentials::MyChartCli {
                base_url: Some("https://fhir.example.org/api/FHIR/R4".to_owned().into()),
                portal_base_url: None,
                client_id: Some("client-id".to_owned().into()),
                client_secret: Some("client-secret".to_owned().into()),
                redirect_uri: Some("http://127.0.0.1:8910/callback".to_owned().into()),
                access_token: None,
                refresh_token: Some("refresh-token".to_owned().into()),
                username: Some("jess@example.com".to_owned().into()),
            },
        }
    }
}
