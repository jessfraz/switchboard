use switchboard_core::{Error, ExecutionTarget, PlannedAction, Result, ToolKind, ToolOutput};

use crate::google::materializer::{DefaultGoogleWorkspaceCliMaterializer, GoogleWorkspaceCliMaterializer};

pub(crate) trait GoogleWorkspaceBackend: Send + Sync {
    fn execute(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput>;
}

pub(crate) struct GoogleWorkspaceCliBackend {
    materializer: Box<dyn GoogleWorkspaceCliMaterializer>,
}

impl Default for GoogleWorkspaceCliBackend {
    fn default() -> Self {
        Self::new(Box::new(DefaultGoogleWorkspaceCliMaterializer))
    }
}

impl GoogleWorkspaceCliBackend {
    fn new(materializer: Box<dyn GoogleWorkspaceCliMaterializer>) -> Self {
        Self { materializer }
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
        .with_field(
            "note",
            "google workspace cli runtime is prepared, but command execution is not wired yet",
        )
    }
}

impl GoogleWorkspaceBackend for GoogleWorkspaceCliBackend {
    fn execute(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
        if matches!(action.kind, ToolKind::Write) {
            return Err(Error::NotImplemented(format!(
                "{} apply path is not wired to Google Workspace yet",
                action.tool
            )));
        }

        self.materializer.prepare(target)?;

        Ok(Self::stub_output(target, action))
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use switchboard_core::{
        AuthKind, AuthSecretRefs, BackendKind, ExecutionMode, ExecutionTarget, PlannedAction, PlanningTarget,
        ProviderKind, ResolvedAuth, ResolvedCredentials, ResolvedNamespace, SecretRef, ToolKind, ToolRequest,
    };

    use crate::google::backend::{GoogleWorkspaceBackend, GoogleWorkspaceCliBackend};

    const GOOGLE_PERSONAL_OAUTH_JSON: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/secrets/google-personal-oauth.json"
    ));

    #[test]
    fn read_execution_reports_prepared_runtime_fields() {
        let backend = GoogleWorkspaceCliBackend::default();
        let target = execution_target(
            ResolvedCredentials::GoogleOAuth {
                client_id: "client-id".to_owned().into(),
                client_secret: "client-secret".to_owned().into(),
                refresh_token: Some("refresh-token".to_owned().into()),
            },
            Some(PathBuf::from("/tmp/gws-work")),
        );
        let action = planned_read_action("google.mail.search");

        let output = backend
            .execute(&target, &action)
            .expect("google read execution should succeed");

        assert_eq!(
            output.fields.get("status").and_then(|value| value.as_str()),
            Some("stub")
        );
        assert_eq!(
            output.fields.get("backend").and_then(|value| value.as_str()),
            Some("cli")
        );
        assert_eq!(
            output.fields.get("auth").and_then(|value| value.as_str()),
            Some("google.work_auth")
        );
    }

    #[test]
    fn file_credentials_runtime_reports_credentials_file_mode() {
        let backend = GoogleWorkspaceCliBackend::default();
        let target = execution_target(
            ResolvedCredentials::GoogleOAuthFile {
                credentials: GOOGLE_PERSONAL_OAUTH_JSON.to_owned().into(),
            },
            Some(PathBuf::from("/tmp/gws-personal")),
        );
        let action = planned_read_action("google.calendar.list");

        let output = backend
            .execute(&target, &action)
            .expect("google read execution should succeed");

        assert_eq!(
            output.fields.get("status").and_then(|value| value.as_str()),
            Some("stub")
        );
        assert_eq!(
            output.fields.get("backend").and_then(|value| value.as_str()),
            Some("cli")
        );
        assert_eq!(
            output.fields.get("auth").and_then(|value| value.as_str()),
            Some("google.work_auth")
        );
    }

    fn planned_read_action(tool: &str) -> PlannedAction {
        let request = ToolRequest::new(
            tool,
            "google.work",
            ExecutionMode::Auto,
            BTreeMap::from([("query".to_owned(), "from:finance".to_owned())]),
        )
        .expect("tool request should build");
        let target = PlanningTarget {
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
                "jess@company.com",
                AuthSecretRefs::GoogleOAuth {
                    client_id: SecretRef::new("google.work_client_id").expect("secret ref should build"),
                    client_secret: SecretRef::new("google.work_client_secret").expect("secret ref should build"),
                    refresh_token: Some(SecretRef::new("google.work_refresh_token").expect("secret ref should build")),
                },
            )
            .expect("auth should build"),
        };

        PlannedAction::new(&request, &target, ToolKind::Read, "read gmail", BackendKind::Cli)
    }

    fn execution_target(credentials: ResolvedCredentials, state_dir: Option<PathBuf>) -> ExecutionTarget {
        let kind = match credentials {
            ResolvedCredentials::GoogleOAuth { .. } => AuthKind::GoogleOAuth,
            ResolvedCredentials::GoogleOAuthFile { .. } => AuthKind::GoogleOAuthFile,
            ResolvedCredentials::GitHubCli | ResolvedCredentials::GitHubToken { .. } => {
                panic!("google backend tests require google credentials")
            }
        };
        let secrets = match kind {
            AuthKind::GoogleOAuth => AuthSecretRefs::GoogleOAuth {
                client_id: SecretRef::new("google.work_client_id").expect("secret ref should build"),
                client_secret: SecretRef::new("google.work_client_secret").expect("secret ref should build"),
                refresh_token: Some(SecretRef::new("google.work_refresh_token").expect("secret ref should build")),
            },
            AuthKind::GoogleOAuthFile => AuthSecretRefs::GoogleOAuthFile {
                credentials: SecretRef::new("google.personal_credentials").expect("secret ref should build"),
            },
            AuthKind::GitHubCli | AuthKind::GitHubToken => {
                panic!("google backend tests require google credentials")
            }
        };

        ExecutionTarget {
            namespace: ResolvedNamespace::new(
                "google.work",
                ProviderKind::GoogleWorkspace,
                "Google Workspace",
                "google.work_auth",
                false,
                state_dir,
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "google.work_auth",
                ProviderKind::GoogleWorkspace,
                kind,
                "jess@example.com",
                secrets,
            )
            .expect("auth should build"),
            credentials,
        }
    }
}
