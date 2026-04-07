use switchboard_core::{Error, ExecutionTarget, ResolvedCredentials, Result};

use crate::{cli::CliRuntimeMaterializer, process_runtime::ProcessContext};

pub(crate) const CONFIG_ENV: &str = "SCHWAB_CONFIG";
pub(crate) const BASE_URL_ENV: &str = "SCHWAB_BASE_URL";
pub(crate) const MARKET_DATA_BASE_URL_ENV: &str = "SCHWAB_MARKETDATA_BASE_URL";
pub(crate) const AUTHORIZE_URL_ENV: &str = "SCHWAB_AUTHORIZE_URL";
pub(crate) const TOKEN_URL_ENV: &str = "SCHWAB_TOKEN_URL";
pub(crate) const CLIENT_ID_ENV: &str = "SCHWAB_CLIENT_ID";
pub(crate) const CLIENT_SECRET_ENV: &str = "SCHWAB_CLIENT_SECRET";
pub(crate) const THIRD_PARTY_ID_ENV: &str = "SCHWAB_THIRD_PARTY_ID";
pub(crate) const CLIENT_CHANNEL_ENV: &str = "SCHWAB_TRADER_CLIENT_CHANNEL";
pub(crate) const CLIENT_APP_ID_ENV: &str = "SCHWAB_TRADER_CLIENT_APP_ID";
pub(crate) const CLIENT_FUNCTION_ID_ENV: &str = "SCHWAB_CLIENT_FUNCTION_ID";
pub(crate) const RESOURCE_VERSION_ENV: &str = "SCHWAB_RESOURCE_VERSION";
pub(crate) const RRBUS_PILOT_ROLLOUT_ENV: &str = "SCHWAB_RRBUS_PILOT_ROLLOUT";
pub(crate) const REDIRECT_URI_ENV: &str = "SCHWAB_REDIRECT_URI";
pub(crate) const ACCESS_TOKEN_ENV: &str = "SCHWAB_ACCESS_TOKEN";
pub(crate) const REFRESH_TOKEN_ENV: &str = "SCHWAB_REFRESH_TOKEN";

pub(crate) struct DefaultSchwabCliMaterializer;

impl CliRuntimeMaterializer for DefaultSchwabCliMaterializer {
    fn prepare(&self, target: &ExecutionTarget) -> Result<ProcessContext> {
        let mut context = ProcessContext::new();

        if let Some(state_dir) = target.namespace.state_dir.as_ref() {
            context.set_env(CONFIG_ENV, state_dir.join("config.json").display().to_string());
        } else {
            context.clear_env(CONFIG_ENV);
        }

        match &target.credentials {
            ResolvedCredentials::SchwabCli {
                base_url,
                market_data_base_url,
                authorize_url,
                token_url,
                client_id,
                client_secret,
                third_party_id,
                client_channel,
                client_app_id,
                client_function_id,
                resource_version,
                rrbus_pilot_rollout,
                redirect_uri,
                access_token,
                refresh_token,
            } => {
                apply_optional_secret(&mut context, BASE_URL_ENV, base_url.as_ref());
                apply_optional_secret(&mut context, MARKET_DATA_BASE_URL_ENV, market_data_base_url.as_ref());
                apply_optional_secret(&mut context, AUTHORIZE_URL_ENV, authorize_url.as_ref());
                apply_optional_secret(&mut context, TOKEN_URL_ENV, token_url.as_ref());
                apply_optional_secret(&mut context, CLIENT_ID_ENV, client_id.as_ref());
                apply_optional_secret(&mut context, CLIENT_SECRET_ENV, client_secret.as_ref());
                apply_optional_secret(&mut context, THIRD_PARTY_ID_ENV, third_party_id.as_ref());
                apply_optional_secret(&mut context, CLIENT_CHANNEL_ENV, client_channel.as_ref());
                apply_optional_secret(&mut context, CLIENT_APP_ID_ENV, client_app_id.as_ref());
                apply_optional_secret(&mut context, CLIENT_FUNCTION_ID_ENV, client_function_id.as_ref());
                apply_optional_secret(&mut context, RESOURCE_VERSION_ENV, resource_version.as_ref());
                apply_optional_secret(&mut context, RRBUS_PILOT_ROLLOUT_ENV, rrbus_pilot_rollout.as_ref());
                apply_optional_secret(&mut context, REDIRECT_URI_ENV, redirect_uri.as_ref());
                apply_optional_secret(&mut context, ACCESS_TOKEN_ENV, access_token.as_ref());
                apply_optional_secret(&mut context, REFRESH_TOKEN_ENV, refresh_token.as_ref());
                Ok(context)
            }
            ResolvedCredentials::GitHubCli
            | ResolvedCredentials::GitHubToken { .. }
            | ResolvedCredentials::GoogleOAuth { .. }
            | ResolvedCredentials::GoogleOAuthFile { .. }
            | ResolvedCredentials::MyChartCli { .. } => Err(Error::UnsupportedOperation(format!(
                "schwab cli materializer does not support {} credentials",
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
        schwab::materializer::{
            DefaultSchwabCliMaterializer, ACCESS_TOKEN_ENV, AUTHORIZE_URL_ENV, BASE_URL_ENV, CLIENT_APP_ID_ENV,
            CLIENT_CHANNEL_ENV, CLIENT_FUNCTION_ID_ENV, CLIENT_ID_ENV, CLIENT_SECRET_ENV, CONFIG_ENV,
            MARKET_DATA_BASE_URL_ENV, REDIRECT_URI_ENV, REFRESH_TOKEN_ENV, RESOURCE_VERSION_ENV,
            RRBUS_PILOT_ROLLOUT_ENV, THIRD_PARTY_ID_ENV, TOKEN_URL_ENV,
        },
    };

    #[test]
    fn cli_managed_credentials_set_scoped_config_and_overrides() {
        let materializer = DefaultSchwabCliMaterializer;
        let target = execution_target(
            ResolvedCredentials::SchwabCli {
                base_url: Some("https://api.schwabapi.com/trader/v1".to_owned().into()),
                market_data_base_url: Some("https://api.schwabapi.com/marketdata/v1".to_owned().into()),
                authorize_url: Some("https://api.schwabapi.com/v1/oauth/authorize".to_owned().into()),
                token_url: Some("https://api.schwabapi.com/v1/oauth/token".to_owned().into()),
                client_id: Some("client-id".to_owned().into()),
                client_secret: Some("client-secret".to_owned().into()),
                third_party_id: Some("third-party".to_owned().into()),
                client_channel: Some("Y2".to_owned().into()),
                client_app_id: Some("AD00001234".to_owned().into()),
                client_function_id: Some("TR123".to_owned().into()),
                resource_version: Some("1".to_owned().into()),
                rrbus_pilot_rollout: Some("pilot".to_owned().into()),
                redirect_uri: Some(
                    "https://jessfraz.github.io/switchboard/schwab-callback/"
                        .to_owned()
                        .into(),
                ),
                access_token: None,
                refresh_token: Some("refresh-token".to_owned().into()),
            },
            Some(PathBuf::from("/tmp/schwab-personal")),
        );

        let process = materializer.prepare(&target).expect("materialization should succeed");

        assert_eq!(
            process.env().get(CONFIG_ENV).map(String::as_str),
            Some("/tmp/schwab-personal/config.json")
        );
        assert_eq!(
            process.env().get(BASE_URL_ENV).map(String::as_str),
            Some("https://api.schwabapi.com/trader/v1")
        );
        assert_eq!(
            process.env().get(MARKET_DATA_BASE_URL_ENV).map(String::as_str),
            Some("https://api.schwabapi.com/marketdata/v1")
        );
        assert_eq!(
            process.env().get(AUTHORIZE_URL_ENV).map(String::as_str),
            Some("https://api.schwabapi.com/v1/oauth/authorize")
        );
        assert_eq!(
            process.env().get(TOKEN_URL_ENV).map(String::as_str),
            Some("https://api.schwabapi.com/v1/oauth/token")
        );
        assert_eq!(process.env().get(CLIENT_ID_ENV).map(String::as_str), Some("client-id"));
        assert_eq!(
            process.env().get(CLIENT_SECRET_ENV).map(String::as_str),
            Some("client-secret")
        );
        assert_eq!(
            process.env().get(THIRD_PARTY_ID_ENV).map(String::as_str),
            Some("third-party")
        );
        assert_eq!(process.env().get(CLIENT_CHANNEL_ENV).map(String::as_str), Some("Y2"));
        assert_eq!(
            process.env().get(CLIENT_APP_ID_ENV).map(String::as_str),
            Some("AD00001234")
        );
        assert_eq!(
            process.env().get(CLIENT_FUNCTION_ID_ENV).map(String::as_str),
            Some("TR123")
        );
        assert_eq!(process.env().get(RESOURCE_VERSION_ENV).map(String::as_str), Some("1"));
        assert_eq!(
            process.env().get(RRBUS_PILOT_ROLLOUT_ENV).map(String::as_str),
            Some("pilot")
        );
        assert_eq!(
            process.env().get(REDIRECT_URI_ENV).map(String::as_str),
            Some("https://jessfraz.github.io/switchboard/schwab-callback/")
        );
        assert_eq!(
            process.env().get(REFRESH_TOKEN_ENV).map(String::as_str),
            Some("refresh-token")
        );
        assert!(process.cleared_env().contains(ACCESS_TOKEN_ENV));
    }

    #[test]
    fn cli_managed_credentials_clear_host_env_when_no_overrides_exist() {
        let materializer = DefaultSchwabCliMaterializer;
        let target = execution_target(
            ResolvedCredentials::SchwabCli {
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
            None,
        );

        let process = materializer.prepare(&target).expect("materialization should succeed");

        assert!(process.cleared_env().contains(CONFIG_ENV));
        assert!(process.cleared_env().contains(BASE_URL_ENV));
        assert!(process.cleared_env().contains(MARKET_DATA_BASE_URL_ENV));
        assert!(process.cleared_env().contains(AUTHORIZE_URL_ENV));
        assert!(process.cleared_env().contains(TOKEN_URL_ENV));
        assert!(process.cleared_env().contains(CLIENT_ID_ENV));
        assert!(process.cleared_env().contains(CLIENT_SECRET_ENV));
        assert!(process.cleared_env().contains(THIRD_PARTY_ID_ENV));
        assert!(process.cleared_env().contains(CLIENT_CHANNEL_ENV));
        assert!(process.cleared_env().contains(CLIENT_APP_ID_ENV));
        assert!(process.cleared_env().contains(CLIENT_FUNCTION_ID_ENV));
        assert!(process.cleared_env().contains(RESOURCE_VERSION_ENV));
        assert!(process.cleared_env().contains(RRBUS_PILOT_ROLLOUT_ENV));
        assert!(process.cleared_env().contains(REDIRECT_URI_ENV));
        assert!(process.cleared_env().contains(ACCESS_TOKEN_ENV));
        assert!(process.cleared_env().contains(REFRESH_TOKEN_ENV));
    }

    #[test]
    fn materializer_rejects_non_schwab_credentials() {
        let materializer = DefaultSchwabCliMaterializer;
        let target = execution_target(
            ResolvedCredentials::GitHubCli,
            Some(PathBuf::from("/tmp/schwab-personal")),
        );

        let error = match materializer.prepare(&target) {
            Ok(_) => panic!("github credentials should be rejected"),
            Err(error) => error,
        };

        assert!(error
            .to_string()
            .contains("schwab cli materializer does not support gh_cli credentials"));
    }

    fn execution_target(credentials: ResolvedCredentials, state_dir: Option<PathBuf>) -> ExecutionTarget {
        let secrets = match credentials {
            ResolvedCredentials::SchwabCli { .. } => AuthSecretRefs::SchwabCli {
                base_url: Some(SecretRef::new("schwab.personal_base_url").expect("secret ref should build")),
                market_data_base_url: Some(
                    SecretRef::new("schwab.personal_market_data_base_url").expect("secret ref should build"),
                ),
                authorize_url: Some(SecretRef::new("schwab.personal_authorize_url").expect("secret ref should build")),
                token_url: Some(SecretRef::new("schwab.personal_token_url").expect("secret ref should build")),
                client_id: Some(SecretRef::new("schwab.personal_client_id").expect("secret ref should build")),
                client_secret: Some(SecretRef::new("schwab.personal_client_secret").expect("secret ref should build")),
                third_party_id: Some(
                    SecretRef::new("schwab.personal_third_party_id").expect("secret ref should build"),
                ),
                client_channel: Some(
                    SecretRef::new("schwab.personal_client_channel").expect("secret ref should build"),
                ),
                client_app_id: Some(SecretRef::new("schwab.personal_client_app_id").expect("secret ref should build")),
                client_function_id: Some(
                    SecretRef::new("schwab.personal_client_function_id").expect("secret ref should build"),
                ),
                resource_version: Some(
                    SecretRef::new("schwab.personal_resource_version").expect("secret ref should build"),
                ),
                rrbus_pilot_rollout: Some(
                    SecretRef::new("schwab.personal_rrbus_pilot_rollout").expect("secret ref should build"),
                ),
                redirect_uri: Some(SecretRef::new("schwab.personal_redirect_uri").expect("secret ref should build")),
                access_token: None,
                refresh_token: Some(SecretRef::new("schwab.personal_refresh_token").expect("secret ref should build")),
            },
            ResolvedCredentials::GitHubCli => AuthSecretRefs::None,
            ResolvedCredentials::GitHubToken { .. }
            | ResolvedCredentials::GoogleOAuth { .. }
            | ResolvedCredentials::GoogleOAuthFile { .. }
            | ResolvedCredentials::MyChartCli { .. } => AuthSecretRefs::None,
        };

        ExecutionTarget {
            namespace: ResolvedNamespace::new(
                "schwab.personal",
                ProviderKind::Schwab,
                "jessfraz",
                "schwab_personal",
                false,
                state_dir,
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "schwab_personal",
                ProviderKind::Schwab,
                match credentials {
                    ResolvedCredentials::SchwabCli { .. } => AuthKind::SchwabCli,
                    ResolvedCredentials::GitHubCli => AuthKind::GitHubCli,
                    ResolvedCredentials::GitHubToken { .. } => AuthKind::GitHubToken,
                    ResolvedCredentials::GoogleOAuth { .. } => AuthKind::GoogleOAuth,
                    ResolvedCredentials::GoogleOAuthFile { .. } => AuthKind::GoogleOAuthFile,
                    ResolvedCredentials::MyChartCli { .. } => AuthKind::MyChartCli,
                },
                "jessfraz",
                secrets,
            )
            .expect("auth should build"),
            credentials,
        }
    }
}
