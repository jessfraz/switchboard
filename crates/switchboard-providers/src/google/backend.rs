use switchboard_core::{Error, ExecutionTarget, PlannedAction, Result, ToolKind, ToolOutput};

use crate::{
    google::materializer::{
        DefaultGoogleWorkspaceCliMaterializer, GoogleWorkspaceCliMaterializer, CLIENT_ID_ENV, CLIENT_SECRET_ENV,
        CONFIG_DIR_ENV, CREDENTIALS_FILE_ENV, TOKEN_ENV,
    },
    process_runtime::ProcessContext,
};

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

    fn stub_output(target: &ExecutionTarget, action: &PlannedAction, process: &ProcessContext) -> ToolOutput {
        let mut output = ToolOutput::new(
            action.tool.clone(),
            action.namespace.clone(),
            format!("{} via {} (stub)", action.summary, action.backend),
        )
        .with_field("status", "stub")
        .with_field("backend", action.backend.to_string())
        .with_field("auth", target.auth.id.to_string())
        .with_field("credential_mode", credential_mode(target))
        .with_field("note", "google workspace cli runtime is prepared, but command execution is not wired yet");

        if let Some(config_dir) = target.namespace.state_dir.as_ref() {
            output = output.with_field("config_dir", config_dir.display().to_string());
        }

        let prepared_env = process.env().keys().cloned().collect::<Vec<_>>().join(",");
        if !prepared_env.is_empty() {
            output = output.with_field("prepared_env", prepared_env);
        }

        let cleared_env = process.cleared_env().iter().cloned().collect::<Vec<_>>().join(",");
        if !cleared_env.is_empty() {
            output = output.with_field("cleared_env", cleared_env);
        }

        output
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

        let process = self.materializer.prepare(target)?;

        Ok(Self::stub_output(target, action, &process))
    }
}

fn credential_mode(target: &ExecutionTarget) -> &'static str {
    match target.credentials {
        switchboard_core::ResolvedCredentials::GoogleOAuth { .. } => "client_credentials",
        switchboard_core::ResolvedCredentials::GoogleOAuthFile { .. } => "credentials_file",
        switchboard_core::ResolvedCredentials::GitHubCli | switchboard_core::ResolvedCredentials::GitHubToken { .. } => {
            "unsupported"
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use switchboard_core::{
        AuthSecretRefs, ExecutionMode, ExecutionTarget, PlannedAction, PlanningTarget, ProviderKind, ResolvedAuth,
        ResolvedCredentials, ResolvedNamespace, SecretRef, ToolRequest, ToolStringErrorExt,
    };

    use crate::google::backend::{
        GoogleWorkspaceBackend, GoogleWorkspaceCliBackend, CLIENT_ID_ENV, CLIENT_SECRET_ENV, CONFIG_DIR_ENV,
        CREDENTIALS_FILE_ENV, TOKEN_ENV,
    };

    trait ToolStringErrorExt<T> {
        fn or_panic(self, message: &str) -> T;
    }

    impl<T> ToolStringErrorExt<T> for switchboard_core::Result<T> {
        fn or_panic(self, message: &str) -> T {
            match self {
                Ok(value) => value,
                Err(error) => panic!("{message}: {error}"),
            }
        }
    }

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
            .or_panic("google read execution should succeed");

        assert_eq!(output.fields.get("credential_mode").map(String::as_str), Some("client_credentials"));
        assert_eq!(
            output.fields.get("prepared_env").map(String::as_str),
            Some("GOOGLE_WORKSPACE_CLI_CLIENT_ID,GOOGLE_WORKSPACE_CLI_CLIENT_SECRET,GOOGLE_WORKSPACE_CLI_CONFIG_DIR")
        );
        assert_eq!(
            output.fields.get("cleared_env").map(String::as_str),
            Some("GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE,GOOGLE_WORKSPACE_CLI_TOKEN")
        );
        assert_eq!(
            output.fields.get("config_dir").map(String::as_str),
            Some("/tmp/gws-work")
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
            .or_panic("google read execution should succeed");

        assert_eq!(output.fields.get("credential_mode").map(String::as_str), Some("credentials_file"));
        assert_eq!(
            output.fields.get("prepared_env").map(String::as_str),
            Some("GOOGLE_WORKSPACE_CLI_CONFIG_DIR,GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE")
        );
        assert_eq!(
            output.fields.get("cleared_env").map(String::as_str),
            Some("GOOGLE_WORKSPACE_CLI_CLIENT_ID,GOOGLE_WORKSPACE_CLI_CLIENT_SECRET,GOOGLE_WORKSPACE_CLI_TOKEN")
        );

        let path = output
            .fields
            .get("prepared_env")
            .expect("prepared env should be included");
        assert!(path.contains(CONFIG_DIR_ENV));
        assert!(path.contains(CREDENTIALS_FILE_ENV));
    }

    fn planned_read_action(tool: &str) -> PlannedAction {
        let request = ToolRequest::new(
            tool,
            "google.work",
            ExecutionMode::Auto,
            BTreeMap::from([("query".to_owned(), "from:finance".to_owned())]),
        )
        .or_panic("tool request should build");
        let target = PlanningTarget {
            namespace: ResolvedNamespace::new(
                "google.work",
                ProviderKind::GoogleWorkspace,
                "Google Workspace (work)",
                "google.work_auth",
                false,
                Some(PathBuf::from("/tmp/gws-work")),
            )
            .or_panic("namespace should build"),
            auth: ResolvedAuth::new(
                "google.work_auth",
                ProviderKind::GoogleWorkspace,
                switchboard_core::AuthKind::GoogleOAuth,
                "jess@company.com",
                AuthSecretRefs::GoogleOAuth {
                    client_id: SecretRef::new("google.work_client_id").or_panic("secret ref should build"),
                    client_secret: SecretRef::new("google.work_client_secret")
                        .or_panic("secret ref should build"),
                    refresh_token: Some(
                        SecretRef::new("google.work_refresh_token").or_panic("secret ref should build"),
                    ),
                },
            )
            .or_panic("auth should build"),
        };

        PlannedAction::new(
            &request,
            &target,
            ToolKind::Read,
            "read gmail",
            switchboard_core::BackendKind::Cli,
        )
    }

    fn execution_target(credentials: ResolvedCredentials, state_dir: Option<PathBuf>) -> ExecutionTarget {
        let kind = match credentials {
            ResolvedCredentials::GoogleOAuth { .. } => switchboard_core::AuthKind::GoogleOAuth,
            ResolvedCredentials::GoogleOAuthFile { .. } => switchboard_core::AuthKind::GoogleOAuthFile,
            ResolvedCredentials::GitHubCli | ResolvedCredentials::GitHubToken { .. } => {
                panic!("google backend tests require google credentials")
            }
        };
        let secrets = match kind {
            switchboard_core::AuthKind::GoogleOAuth => AuthSecretRefs::GoogleOAuth {
                client_id: SecretRef::new("google.work_client_id").or_panic("secret ref should build"),
                client_secret: SecretRef::new("google.work_client_secret").or_panic("secret ref should build"),
                refresh_token: Some(SecretRef::new("google.work_refresh_token").or_panic("secret ref should build")),
            },
            switchboard_core::AuthKind::GoogleOAuthFile => AuthSecretRefs::GoogleOAuthFile {
                credentials: SecretRef::new("google.personal_credentials").or_panic("secret ref should build"),
            },
            switchboard_core::AuthKind::GitHubCli | switchboard_core::AuthKind::GitHubToken => {
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
            .or_panic("namespace should build"),
            auth: ResolvedAuth::new(
                "google.work_auth",
                ProviderKind::GoogleWorkspace,
                kind,
                "jess@example.com",
                secrets,
            )
            .or_panic("auth should build"),
            credentials,
        }
    }
}
