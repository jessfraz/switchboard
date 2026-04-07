use switchboard_core::{Error, ExecutionTarget, ResolvedCredentials, Result};

use crate::{cli::CliRuntimeMaterializer, process_runtime::ProcessContext};

pub(crate) const CONFIG_DIR_ENV: &str = "GH_CONFIG_DIR";
pub(crate) const TOKEN_ENV: &str = "GH_TOKEN";
pub(crate) const LEGACY_TOKEN_ENV: &str = "GITHUB_TOKEN";
pub(crate) const ENTERPRISE_TOKEN_ENV: &str = "GH_ENTERPRISE_TOKEN";
pub(crate) const LEGACY_ENTERPRISE_TOKEN_ENV: &str = "GITHUB_ENTERPRISE_TOKEN";

pub(crate) struct DefaultGitHubCliMaterializer;

impl CliRuntimeMaterializer for DefaultGitHubCliMaterializer {
    fn prepare(&self, target: &ExecutionTarget) -> Result<ProcessContext> {
        let mut context = ProcessContext::new();

        if let Some(state_dir) = target.namespace.state_dir.as_ref() {
            context.set_env(CONFIG_DIR_ENV, state_dir.display().to_string());
        }

        match &target.credentials {
            ResolvedCredentials::GitHubCli => {
                context.clear_env(TOKEN_ENV);
                context.clear_env(LEGACY_TOKEN_ENV);
                context.clear_env(ENTERPRISE_TOKEN_ENV);
                context.clear_env(LEGACY_ENTERPRISE_TOKEN_ENV);
                Ok(context)
            }
            ResolvedCredentials::GitHubToken { token } => {
                context.set_env(TOKEN_ENV, token.expose());
                context.clear_env(LEGACY_TOKEN_ENV);
                context.clear_env(ENTERPRISE_TOKEN_ENV);
                context.clear_env(LEGACY_ENTERPRISE_TOKEN_ENV);
                Ok(context)
            }
            ResolvedCredentials::GoogleOAuth { .. }
            | ResolvedCredentials::GoogleOAuthFile { .. }
            | ResolvedCredentials::MyChartCli { .. }
            | ResolvedCredentials::SchwabCli { .. } => Err(Error::UnsupportedOperation(format!(
                "github cli materializer does not support {} credentials",
                target.auth.kind
            ))),
        }
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
        github::materializer::{
            DefaultGitHubCliMaterializer, CONFIG_DIR_ENV, ENTERPRISE_TOKEN_ENV, LEGACY_ENTERPRISE_TOKEN_ENV,
            LEGACY_TOKEN_ENV, TOKEN_ENV,
        },
    };

    #[test]
    fn token_auth_sets_gh_token_and_clears_legacy_env() {
        let materializer = DefaultGitHubCliMaterializer;
        let target = execution_target(
            ResolvedCredentials::GitHubToken {
                token: "ghp-test-token".to_owned().into(),
            },
            Some(PathBuf::from("/tmp/gh-personal")),
        );

        let process = materializer.prepare(&target).expect("materialization should succeed");

        assert_eq!(process.env().get(TOKEN_ENV).map(String::as_str), Some("ghp-test-token"));
        assert_eq!(
            process.env().get(CONFIG_DIR_ENV).map(String::as_str),
            Some("/tmp/gh-personal")
        );
        assert!(process.cleared_env().contains(LEGACY_TOKEN_ENV));
        assert!(process.cleared_env().contains(ENTERPRISE_TOKEN_ENV));
        assert!(process.cleared_env().contains(LEGACY_ENTERPRISE_TOKEN_ENV));
    }

    fn execution_target(credentials: ResolvedCredentials, state_dir: Option<PathBuf>) -> ExecutionTarget {
        ExecutionTarget {
            namespace: ResolvedNamespace::new(
                "github.personal",
                ProviderKind::GitHub,
                "GitHub personal",
                "github.personal_auth",
                false,
                state_dir,
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "github.personal_auth",
                ProviderKind::GitHub,
                AuthKind::GitHubToken,
                "jessfraz",
                AuthSecretRefs::GitHubToken {
                    token: SecretRef::new("github.personal_token").expect("secret ref should build"),
                },
            )
            .expect("auth should build"),
            credentials,
        }
    }
}
