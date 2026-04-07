mod materializer;

use std::sync::OnceLock;

use switchboard_core::{
    Adapter, Error, ExecutionTarget, PlannedAction, PlanningTarget, ProviderKind, Result, ToolDescriptor, ToolKind,
    ToolOutput, ToolRequest,
};

use crate::{
    cli::{CliProviderBackend, CliProviderCatalog},
    inventory::embedded_inventory,
    schwab::materializer::DefaultSchwabCliMaterializer,
};

const MANIFEST_JSON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/manifests/schwab.json"));
static CATALOG: OnceLock<CliProviderCatalog> = OnceLock::new();

pub struct SchwabAdapter {
    backend: CliProviderBackend,
}

impl Default for SchwabAdapter {
    fn default() -> Self {
        Self {
            backend: CliProviderBackend::new(Box::new(DefaultSchwabCliMaterializer)),
        }
    }
}

impl SchwabAdapter {
    fn catalog() -> &'static CliProviderCatalog {
        CATALOG.get_or_init(|| {
            let inventory = embedded_inventory(ProviderKind::Schwab).expect("schwab inventory should be valid");
            CliProviderCatalog::from_embedded(MANIFEST_JSON, &inventory)
                .expect("schwab provider manifest should be valid")
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
        .with_field("note", "schwab command execution is not wired yet")
    }
}

impl Adapter for SchwabAdapter {
    fn provider(&self) -> ProviderKind {
        ProviderKind::Schwab
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
                    "{} apply path is not wired to Schwab yet",
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
        ResolvedCredentials, ResolvedNamespace, SecretRef, ToolArgument, ToolExecutionSupport, ToolName, ToolRequest,
        ToolSurface,
    };

    use crate::{
        schwab::SchwabAdapter,
        test_support::{lock_env, TempScript},
    };

    const SCHWAB_SCRIPT_TEMPLATE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/scripts/schwab-test.sh"
    ));

    #[test]
    fn raw_read_passthrough_executes_through_generic_cli_runtime() {
        let _env_guard = lock_env();
        let script = TempScript::new("schwab-test", SCHWAB_SCRIPT_TEMPLATE);
        env::set_var("SWITCHBOARD_SCHWAB_BIN", script.path());

        let adapter = SchwabAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "schwab.cli.read",
            "schwab.personal",
            ExecutionMode::Auto,
            vec![
                ToolArgument::option("argv", "auth").expect("argv should build"),
                ToolArgument::option("argv", "status").expect("argv should build"),
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
        let fields: RawFields<AuthStatusPayload> = parse_output_fields(&output);

        assert_eq!(
            output.summary,
            "Ran raw schwab command for schwab.personal: auth status"
        );
        assert_eq!(fields.argv, vec!["auth", "status"]);
        assert!(fields.response.authenticated);

        let captured = script.capture_contents();
        assert!(captured.contains("CONFIG=/tmp/schwab-personal/config.json"));
        assert!(captured.contains("CLIENT_ID=client-id"));
        assert!(captured.contains("CLIENT_SECRET=client-secret"));
        assert!(captured.contains("REDIRECT_URI=https://jessfraz.github.io/switchboard/schwab-callback/"));
        assert!(captured.contains("ARGV=auth status"));
    }

    #[test]
    fn tool_catalog_includes_inventory_backed_leaf_tools() {
        let adapter = SchwabAdapter::default();
        let auth_status_tool = adapter
            .find_tool(&ToolName::new("schwab.cli.auth.status").expect("tool name should parse"))
            .expect("inventory-backed tool should exist");
        let root_tool = adapter
            .find_tool(&ToolName::new("schwab.cli.read").expect("tool name should parse"))
            .expect("root raw tool should exist");

        assert_eq!(auth_status_tool.surface, ToolSurface::Raw);
        assert_eq!(auth_status_tool.execution_support, ToolExecutionSupport::Executable);
        assert_eq!(root_tool.surface, ToolSurface::Raw);
        assert_eq!(root_tool.execution_support, ToolExecutionSupport::Executable);
    }

    #[derive(Debug, Deserialize)]
    struct RawFields<TResponse> {
        argv: Vec<String>,
        response: TResponse,
    }

    #[derive(Debug, Deserialize)]
    struct AuthStatusPayload {
        authenticated: bool,
    }

    fn parse_output_fields<T>(output: &switchboard_core::ToolOutput) -> T
    where
        T: for<'de> Deserialize<'de>,
    {
        serde_json::from_value(serde_json::to_value(&output.fields).expect("fields should serialize"))
            .expect("fields should deserialize")
    }

    fn planning_target() -> PlanningTarget {
        PlanningTarget {
            namespace: ResolvedNamespace::new(
                "schwab.personal",
                ProviderKind::Schwab,
                "jessfraz",
                "schwab_personal",
                false,
                Some(PathBuf::from("/tmp/schwab-personal")),
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "schwab_personal",
                ProviderKind::Schwab,
                AuthKind::SchwabCli,
                "jessfraz",
                AuthSecretRefs::SchwabCli {
                    base_url: None,
                    market_data_base_url: None,
                    authorize_url: None,
                    token_url: None,
                    client_id: Some(SecretRef::new("schwab.personal_client_id").expect("secret ref should build")),
                    client_secret: Some(
                        SecretRef::new("schwab.personal_client_secret").expect("secret ref should build"),
                    ),
                    third_party_id: None,
                    client_channel: None,
                    client_app_id: None,
                    client_function_id: None,
                    resource_version: None,
                    rrbus_pilot_rollout: None,
                    redirect_uri: Some(
                        SecretRef::new("schwab.personal_redirect_uri").expect("secret ref should build"),
                    ),
                    access_token: None,
                    refresh_token: None,
                },
            )
            .expect("auth should build"),
        }
    }

    fn execution_target() -> ExecutionTarget {
        ExecutionTarget {
            namespace: ResolvedNamespace::new(
                "schwab.personal",
                ProviderKind::Schwab,
                "jessfraz",
                "schwab_personal",
                false,
                Some(PathBuf::from("/tmp/schwab-personal")),
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "schwab_personal",
                ProviderKind::Schwab,
                AuthKind::SchwabCli,
                "jessfraz",
                AuthSecretRefs::SchwabCli {
                    base_url: None,
                    market_data_base_url: None,
                    authorize_url: None,
                    token_url: None,
                    client_id: Some(SecretRef::new("schwab.personal_client_id").expect("secret ref should build")),
                    client_secret: Some(
                        SecretRef::new("schwab.personal_client_secret").expect("secret ref should build"),
                    ),
                    third_party_id: None,
                    client_channel: None,
                    client_app_id: None,
                    client_function_id: None,
                    resource_version: None,
                    rrbus_pilot_rollout: None,
                    redirect_uri: Some(
                        SecretRef::new("schwab.personal_redirect_uri").expect("secret ref should build"),
                    ),
                    access_token: None,
                    refresh_token: None,
                },
            )
            .expect("auth should build"),
            credentials: ResolvedCredentials::SchwabCli {
                base_url: None,
                market_data_base_url: None,
                authorize_url: None,
                token_url: None,
                client_id: Some("client-id".to_owned().into()),
                client_secret: Some("client-secret".to_owned().into()),
                third_party_id: None,
                client_channel: None,
                client_app_id: None,
                client_function_id: None,
                resource_version: None,
                rrbus_pilot_rollout: None,
                redirect_uri: Some(
                    "https://jessfraz.github.io/switchboard/schwab-callback/"
                        .to_owned()
                        .into(),
                ),
                access_token: None,
                refresh_token: None,
            },
        }
    }
}
