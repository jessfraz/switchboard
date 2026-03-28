use switchboard_core::{Error, ExecutionTarget, ResolvedCredentials, Result};

use crate::{cli::CliRuntimeMaterializer, process_runtime::ProcessContext};

pub(crate) const CONFIG_ENV: &str = "MYCHART_CONFIG";
pub(crate) const ACCOUNT_ENV: &str = "MYCHART_ACCOUNT";
pub(crate) const BASE_URL_ENV: &str = "MYCHART_BASE_URL";
pub(crate) const PORTAL_BASE_URL_ENV: &str = "MYCHART_PORTAL_BASE_URL";
pub(crate) const CLIENT_ID_ENV: &str = "MYCHART_CLIENT_ID";
pub(crate) const CLIENT_SECRET_ENV: &str = "MYCHART_CLIENT_SECRET";
pub(crate) const REDIRECT_URI_ENV: &str = "MYCHART_REDIRECT_URI";
pub(crate) const ACCESS_TOKEN_ENV: &str = "MYCHART_ACCESS_TOKEN";
pub(crate) const REFRESH_TOKEN_ENV: &str = "MYCHART_REFRESH_TOKEN";
pub(crate) const USERNAME_ENV: &str = "MYCHART_USERNAME";
pub(crate) const DEBUG_AUTH_ENV: &str = "MYCHART_DEBUG_AUTH";

pub(crate) struct DefaultMyChartCliMaterializer;

impl CliRuntimeMaterializer for DefaultMyChartCliMaterializer {
    fn prepare(&self, target: &ExecutionTarget) -> Result<ProcessContext> {
        let mut context = ProcessContext::new();

        if let Some(state_dir) = target.namespace.state_dir.as_ref() {
            context.set_env(CONFIG_ENV, state_dir.join("config.json").display().to_string());
        } else {
            context.clear_env(CONFIG_ENV);
        }

        context.set_env(ACCOUNT_ENV, target.auth.account_label.clone());
        context.clear_env(DEBUG_AUTH_ENV);

        match &target.credentials {
            ResolvedCredentials::MyChartCli {
                base_url,
                portal_base_url,
                client_id,
                client_secret,
                redirect_uri,
                access_token,
                refresh_token,
                username,
            } => {
                apply_optional_secret(&mut context, BASE_URL_ENV, base_url.as_ref());
                apply_optional_secret(&mut context, PORTAL_BASE_URL_ENV, portal_base_url.as_ref());
                apply_optional_secret(&mut context, CLIENT_ID_ENV, client_id.as_ref());
                apply_optional_secret(&mut context, CLIENT_SECRET_ENV, client_secret.as_ref());
                apply_optional_secret(&mut context, REDIRECT_URI_ENV, redirect_uri.as_ref());
                apply_optional_secret(&mut context, ACCESS_TOKEN_ENV, access_token.as_ref());
                apply_optional_secret(&mut context, REFRESH_TOKEN_ENV, refresh_token.as_ref());
                apply_optional_secret(&mut context, USERNAME_ENV, username.as_ref());
                Ok(context)
            }
            ResolvedCredentials::GitHubCli
            | ResolvedCredentials::GitHubToken { .. }
            | ResolvedCredentials::GoogleOAuth { .. }
            | ResolvedCredentials::GoogleOAuthFile { .. } => Err(Error::UnsupportedOperation(format!(
                "mychart cli materializer does not support {} credentials",
                target.auth.kind
            ))),
        }
    }
}

fn apply_optional_secret(
    context: &mut ProcessContext,
    env_name: &str,
    secret: Option<&switchboard_core::SecretString>,
) {
    match secret {
        Some(secret) => context.set_env(env_name, secret.expose()),
        None => context.clear_env(env_name),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use switchboard_core::{
        AuthKind, AuthSecretRefs, ExecutionTarget, ProviderKind, ResolvedAuth, ResolvedCredentials, ResolvedNamespace,
        SecretRef,
    };

    use crate::{
        cli::CliRuntimeMaterializer,
        mychart::materializer::{
            DefaultMyChartCliMaterializer, ACCESS_TOKEN_ENV, ACCOUNT_ENV, BASE_URL_ENV, CLIENT_ID_ENV,
            CLIENT_SECRET_ENV, CONFIG_ENV, DEBUG_AUTH_ENV, PORTAL_BASE_URL_ENV, REDIRECT_URI_ENV, REFRESH_TOKEN_ENV,
            USERNAME_ENV,
        },
    };

    #[test]
    fn cli_managed_credentials_set_scoped_config_account_and_overrides() {
        let materializer = DefaultMyChartCliMaterializer;
        let target = execution_target(
            ResolvedCredentials::MyChartCli {
                base_url: Some("https://fhir.example.org/api/FHIR/R4".to_owned().into()),
                portal_base_url: None,
                client_id: Some("client-id".to_owned().into()),
                client_secret: Some("client-secret".to_owned().into()),
                redirect_uri: Some("http://127.0.0.1:8910/callback".to_owned().into()),
                access_token: None,
                refresh_token: Some("refresh-token".to_owned().into()),
                username: Some("jess@example.com".to_owned().into()),
            },
            Some(PathBuf::from("/tmp/mychart-ucla")),
        );

        let process = materializer.prepare(&target).expect("materialization should succeed");

        assert_eq!(
            process.env().get(CONFIG_ENV).map(String::as_str),
            Some("/tmp/mychart-ucla/config.json")
        );
        assert_eq!(process.env().get(ACCOUNT_ENV).map(String::as_str), Some("ucla"));
        assert_eq!(
            process.env().get(BASE_URL_ENV).map(String::as_str),
            Some("https://fhir.example.org/api/FHIR/R4")
        );
        assert_eq!(process.env().get(CLIENT_ID_ENV).map(String::as_str), Some("client-id"));
        assert_eq!(
            process.env().get(CLIENT_SECRET_ENV).map(String::as_str),
            Some("client-secret")
        );
        assert_eq!(
            process.env().get(REDIRECT_URI_ENV).map(String::as_str),
            Some("http://127.0.0.1:8910/callback")
        );
        assert_eq!(
            process.env().get(REFRESH_TOKEN_ENV).map(String::as_str),
            Some("refresh-token")
        );
        assert_eq!(
            process.env().get(USERNAME_ENV).map(String::as_str),
            Some("jess@example.com")
        );
        assert!(process.cleared_env().contains(PORTAL_BASE_URL_ENV));
        assert!(process.cleared_env().contains(ACCESS_TOKEN_ENV));
        assert!(process.cleared_env().contains(DEBUG_AUTH_ENV));
    }

    #[test]
    fn cli_managed_credentials_clear_host_env_when_no_overrides_exist() {
        let materializer = DefaultMyChartCliMaterializer;
        let target = execution_target(
            ResolvedCredentials::MyChartCli {
                base_url: None,
                portal_base_url: None,
                client_id: None,
                client_secret: None,
                redirect_uri: None,
                access_token: None,
                refresh_token: None,
                username: None,
            },
            None,
        );

        let process = materializer.prepare(&target).expect("materialization should succeed");

        assert_eq!(process.env().get(ACCOUNT_ENV).map(String::as_str), Some("ucla"));
        assert!(process.cleared_env().contains(CONFIG_ENV));
        assert!(process.cleared_env().contains(BASE_URL_ENV));
        assert!(process.cleared_env().contains(PORTAL_BASE_URL_ENV));
        assert!(process.cleared_env().contains(CLIENT_ID_ENV));
        assert!(process.cleared_env().contains(CLIENT_SECRET_ENV));
        assert!(process.cleared_env().contains(REDIRECT_URI_ENV));
        assert!(process.cleared_env().contains(ACCESS_TOKEN_ENV));
        assert!(process.cleared_env().contains(REFRESH_TOKEN_ENV));
        assert!(process.cleared_env().contains(USERNAME_ENV));
        assert!(process.cleared_env().contains(DEBUG_AUTH_ENV));
    }

    #[test]
    fn materializer_rejects_non_mychart_credentials() {
        let materializer = DefaultMyChartCliMaterializer;
        let target = execution_target(ResolvedCredentials::GitHubCli, Some(PathBuf::from("/tmp/mychart-ucla")));

        let error = match materializer.prepare(&target) {
            Ok(_) => panic!("github credentials should be rejected"),
            Err(error) => error,
        };

        assert!(error
            .to_string()
            .contains("mychart cli materializer does not support gh_cli credentials"));
    }

    fn execution_target(credentials: ResolvedCredentials, state_dir: Option<PathBuf>) -> ExecutionTarget {
        let secrets = match credentials {
            ResolvedCredentials::MyChartCli { .. } => AuthSecretRefs::MyChartCli {
                base_url: Some(SecretRef::new("mychart.ucla_base_url").expect("secret ref should build")),
                portal_base_url: None,
                client_id: Some(SecretRef::new("mychart.ucla_client_id").expect("secret ref should build")),
                client_secret: Some(SecretRef::new("mychart.ucla_client_secret").expect("secret ref should build")),
                redirect_uri: Some(SecretRef::new("mychart.ucla_redirect_uri").expect("secret ref should build")),
                access_token: None,
                refresh_token: Some(SecretRef::new("mychart.ucla_refresh_token").expect("secret ref should build")),
                username: Some(SecretRef::new("mychart.ucla_username").expect("secret ref should build")),
            },
            ResolvedCredentials::GitHubCli => AuthSecretRefs::None,
            ResolvedCredentials::GitHubToken { .. }
            | ResolvedCredentials::GoogleOAuth { .. }
            | ResolvedCredentials::GoogleOAuthFile { .. } => AuthSecretRefs::None,
        };

        ExecutionTarget {
            namespace: ResolvedNamespace::new(
                "mychart.ucla",
                ProviderKind::MyChart,
                "UCLA Health",
                "mychart_ucla",
                false,
                state_dir,
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "mychart_ucla",
                ProviderKind::MyChart,
                match credentials {
                    ResolvedCredentials::MyChartCli { .. } => AuthKind::MyChartCli,
                    ResolvedCredentials::GitHubCli => AuthKind::GitHubCli,
                    ResolvedCredentials::GitHubToken { .. } => AuthKind::GitHubToken,
                    ResolvedCredentials::GoogleOAuth { .. } => AuthKind::GoogleOAuth,
                    ResolvedCredentials::GoogleOAuthFile { .. } => AuthKind::GoogleOAuthFile,
                },
                "ucla",
                secrets,
            )
            .expect("auth should build"),
            credentials,
        }
    }
}
