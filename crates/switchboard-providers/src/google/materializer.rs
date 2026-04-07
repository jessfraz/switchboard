use switchboard_core::{Error, ExecutionTarget, ResolvedCredentials, Result};

use crate::{cli::CliRuntimeMaterializer, process_runtime::ProcessContext};

pub(crate) const CLIENT_ID_ENV: &str = "GOOGLE_WORKSPACE_CLI_CLIENT_ID";
pub(crate) const CLIENT_SECRET_ENV: &str = "GOOGLE_WORKSPACE_CLI_CLIENT_SECRET";
pub(crate) const CONFIG_DIR_ENV: &str = "GOOGLE_WORKSPACE_CLI_CONFIG_DIR";
pub(crate) const CREDENTIALS_FILE_ENV: &str = "GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE";
pub(crate) const TOKEN_ENV: &str = "GOOGLE_WORKSPACE_CLI_TOKEN";

pub(crate) struct DefaultGoogleWorkspaceCliMaterializer;

pub(crate) enum GoogleWorkspaceCliCredentials<'a> {
    ClientSecrets { client_id: &'a str, client_secret: &'a str },
    CredentialsFile { credentials: &'a str },
}

impl<'a> GoogleWorkspaceCliCredentials<'a> {
    fn from_target(target: &'a ExecutionTarget) -> Result<Self> {
        match &target.credentials {
            ResolvedCredentials::GoogleOAuth {
                client_id,
                client_secret,
                ..
            } => Ok(Self::ClientSecrets {
                client_id: client_id.expose(),
                client_secret: client_secret.expose(),
            }),
            ResolvedCredentials::GoogleOAuthFile { credentials } => Ok(Self::CredentialsFile {
                credentials: credentials.expose(),
            }),
            ResolvedCredentials::GitHubCli
            | ResolvedCredentials::GitHubToken { .. }
            | ResolvedCredentials::MyChartCli { .. }
            | ResolvedCredentials::SchwabCli { .. } => Err(Error::UnsupportedOperation(format!(
                "google workspace cli materializer does not support {} credentials",
                target.auth.kind
            ))),
        }
    }
}

impl CliRuntimeMaterializer for DefaultGoogleWorkspaceCliMaterializer {
    fn prepare(&self, target: &ExecutionTarget) -> Result<ProcessContext> {
        let mut context = ProcessContext::new();

        if let Some(state_dir) = target.namespace.state_dir.as_ref() {
            context.set_env(CONFIG_DIR_ENV, state_dir.display().to_string());
        }

        match GoogleWorkspaceCliCredentials::from_target(target)? {
            GoogleWorkspaceCliCredentials::ClientSecrets {
                client_id,
                client_secret,
            } => {
                context.set_env(CLIENT_ID_ENV, client_id);
                context.set_env(CLIENT_SECRET_ENV, client_secret);
                context.clear_env(TOKEN_ENV);
                context.clear_env(CREDENTIALS_FILE_ENV);
                Ok(context)
            }
            GoogleWorkspaceCliCredentials::CredentialsFile { credentials } => {
                let path = context.write_temp_file("switchboard-google-oauth", "json", credentials)?;
                context.set_env(CREDENTIALS_FILE_ENV, path.display().to_string());
                context.clear_env(TOKEN_ENV);
                context.clear_env(CLIENT_ID_ENV);
                context.clear_env(CLIENT_SECRET_ENV);
                Ok(context)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use switchboard_core::{
        AuthKind, AuthSecretRefs, ExecutionTarget, ProviderKind, ResolvedAuth, ResolvedCredentials, ResolvedNamespace,
        SecretRef,
    };

    use crate::{
        cli::CliRuntimeMaterializer,
        google::materializer::{
            DefaultGoogleWorkspaceCliMaterializer, CLIENT_ID_ENV, CLIENT_SECRET_ENV, CONFIG_DIR_ENV,
            CREDENTIALS_FILE_ENV, TOKEN_ENV,
        },
    };

    const GOOGLE_PERSONAL_OAUTH_JSON: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/secrets/google-personal-oauth.json"
    ));

    #[test]
    fn oauth_client_credentials_set_env_and_clear_conflicts() {
        let materializer = DefaultGoogleWorkspaceCliMaterializer;
        let target = execution_target(
            ResolvedCredentials::GoogleOAuth {
                client_id: "client-id".to_owned().into(),
                client_secret: "client-secret".to_owned().into(),
                refresh_token: Some("refresh-token".to_owned().into()),
            },
            Some(PathBuf::from("/tmp/gws-work")),
        );

        let process = materializer.prepare(&target).expect("materialization should succeed");

        assert_eq!(process.env().get(CLIENT_ID_ENV).map(String::as_str), Some("client-id"));
        assert_eq!(
            process.env().get(CLIENT_SECRET_ENV).map(String::as_str),
            Some("client-secret")
        );
        assert_eq!(
            process.env().get(CONFIG_DIR_ENV).map(String::as_str),
            Some("/tmp/gws-work")
        );
        assert!(process.cleared_env().contains(TOKEN_ENV));
        assert!(process.cleared_env().contains(CREDENTIALS_FILE_ENV));
        assert!(!process.env().contains_key(CREDENTIALS_FILE_ENV));
    }

    #[test]
    fn oauth_file_credentials_write_temp_file_and_clear_other_modes() {
        let materializer = DefaultGoogleWorkspaceCliMaterializer;
        let target = execution_target(
            ResolvedCredentials::GoogleOAuthFile {
                credentials: GOOGLE_PERSONAL_OAUTH_JSON.to_owned().into(),
            },
            Some(PathBuf::from("/tmp/gws-personal")),
        );

        let process = materializer.prepare(&target).expect("materialization should succeed");

        let credentials_path = process
            .env()
            .get(CREDENTIALS_FILE_ENV)
            .map(PathBuf::from)
            .expect("credentials file should be prepared");
        assert!(credentials_path.exists());
        assert_eq!(
            fs::read_to_string(&credentials_path).expect("credentials file should be readable"),
            GOOGLE_PERSONAL_OAUTH_JSON
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = fs::metadata(&credentials_path)
                .expect("credentials file metadata should be readable")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert_eq!(
            process.env().get(CONFIG_DIR_ENV).map(String::as_str),
            Some("/tmp/gws-personal")
        );
        assert!(process.cleared_env().contains(TOKEN_ENV));
        assert!(process.cleared_env().contains(CLIENT_ID_ENV));
        assert!(process.cleared_env().contains(CLIENT_SECRET_ENV));
        drop(process);
        assert!(!credentials_path.exists());
    }

    #[test]
    fn materializer_rejects_non_google_credentials() {
        let materializer = DefaultGoogleWorkspaceCliMaterializer;
        let target = execution_target(
            ResolvedCredentials::GitHubToken {
                token: "definitely-not-google".to_owned().into(),
            },
            None,
        );

        let error = match materializer.prepare(&target) {
            Ok(_) => panic!("github credentials should be rejected"),
            Err(error) => error,
        };

        assert!(error
            .to_string()
            .contains("google workspace cli materializer does not support github_token credentials"));
    }

    fn execution_target(credentials: ResolvedCredentials, state_dir: Option<PathBuf>) -> ExecutionTarget {
        let (kind, secrets) = match credentials {
            ResolvedCredentials::GoogleOAuth { .. } => (
                AuthKind::GoogleOAuth,
                AuthSecretRefs::GoogleOAuth {
                    client_id: SecretRef::new("google.work_client_id").expect("secret ref should build"),
                    client_secret: SecretRef::new("google.work_client_secret").expect("secret ref should build"),
                    refresh_token: Some(SecretRef::new("google.work_refresh_token").expect("secret ref should build")),
                },
            ),
            ResolvedCredentials::GoogleOAuthFile { .. } => (
                AuthKind::GoogleOAuthFile,
                AuthSecretRefs::GoogleOAuthFile {
                    credentials: SecretRef::new("google.personal_credentials").expect("secret ref should build"),
                },
            ),
            ResolvedCredentials::GitHubToken { .. } => (
                AuthKind::GitHubToken,
                AuthSecretRefs::GitHubToken {
                    token: SecretRef::new("github.personal_token").expect("secret ref should build"),
                },
            ),
            ResolvedCredentials::GitHubCli => (AuthKind::GitHubCli, AuthSecretRefs::None),
            ResolvedCredentials::MyChartCli { .. } => (
                AuthKind::MyChartCli,
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
            ),
            ResolvedCredentials::SchwabCli { .. } => (
                AuthKind::SchwabCli,
                AuthSecretRefs::SchwabCli {
                    base_url: None,
                    market_data_base_url: None,
                    authorize_url: None,
                    token_url: None,
                    client_id: None,
                    client_secret: None,
                    third_party_id: None,
                    client_channel: None,
                    client_app_id: None,
                    client_function_id: None,
                    resource_version: None,
                    rrbus_pilot_rollout: None,
                    redirect_uri: None,
                    access_token: None,
                    refresh_token: None,
                },
            ),
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
            auth: ResolvedAuth::new("google.work_auth", kind.provider(), kind, "jess@example.com", secrets)
                .expect("auth should build"),
            credentials,
        }
    }
}
