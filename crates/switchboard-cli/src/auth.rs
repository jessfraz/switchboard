use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use switchboard_core::{
    AuthKind, AuthSecretRefs, AuthStore, DispatchOutcome, Error, ExecutionMode, NamespaceId, NamespaceStore,
    ProviderKind, ResolvedAuth, Switchboard, ToolArgument, ToolRequest,
};
use switchboard_store::SwitchboardConfig;

#[derive(Debug, Args)]
pub(crate) struct AuthCommand {
    #[command(subcommand)]
    command: AuthSubcommand,
}

#[derive(Debug, Subcommand)]
enum AuthSubcommand {
    /// Perform a read-only provider identity request and verify the configured account.
    Check(CheckArgs),
    /// Show an opt-in Google CLI migration without modifying configuration or credentials.
    MigrationPreview(MigrationArgs),
}

#[derive(Debug, Args)]
struct CheckArgs {
    #[arg(long = "ns")]
    namespace: String,
    /// Share the one-recovery-attempt budget across commands in this task.
    #[arg(long, env = "SWITCHBOARD_RUN_ID")]
    run_id: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct MigrationArgs {
    #[arg(long = "ns")]
    namespace: String,
    /// Verify the saved CLI session through a live read before changing config.
    #[arg(long)]
    verify: bool,
    #[arg(long)]
    json: bool,
}

impl AuthCommand {
    pub(crate) fn json_requested(&self) -> bool {
        match &self.command {
            AuthSubcommand::Check(args) => args.json,
            AuthSubcommand::MigrationPreview(args) => args.json,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct AuthReceipt {
    pub namespace: NamespaceId,
    pub provider: ProviderKind,
    pub expected_account: String,
    pub authenticated_account: String,
    pub identity_verified: bool,
}

#[derive(Serialize)]
struct ProfileRequest {
    #[serde(rename = "userId")]
    user_id: &'static str,
}

#[derive(Deserialize)]
struct GoogleIdentity {
    #[serde(rename = "emailAddress")]
    email_address: String,
}

#[derive(Deserialize)]
struct GitHubIdentity {
    login: String,
}

/// Resolve credentials serially before a caller starts parallel provider reads.
pub(crate) fn check_namespace(switchboard: &Switchboard, id: &NamespaceId) -> Result<AuthReceipt> {
    let namespace = switchboard
        .list_namespaces()
        .into_iter()
        .find(|namespace| namespace.id == *id)
        .ok_or_else(|| Error::UnknownNamespace(id.to_string()))?;
    let (tool, argv) = match namespace.provider {
        ProviderKind::GoogleWorkspace => (
            "google.cli.read",
            vec![
                "gmail".into(),
                "users".into(),
                "getProfile".into(),
                "--params".into(),
                serde_json::to_string(&ProfileRequest { user_id: "me" })?,
                "--format".into(),
                "json".into(),
            ],
        ),
        ProviderKind::GitHub => ("github.cli.read", vec!["api".into(), "user".into()]),
        _ => {
            return Err(Error::UnsupportedOperation(format!(
                "identity verification is not implemented for {}",
                namespace.provider
            ))
            .into())
        }
    };
    let configured_auth = switchboard.namespace_auth(id)?;
    let request = ToolRequest::new(
        tool,
        id.as_str(),
        ExecutionMode::Auto,
        vec![ToolArgument::option("argv-json", serde_json::to_string(&argv)?)?],
    )?;
    let outcome = match switchboard.dispatch(request.clone()) {
        Err(Error::AuthenticationRejected { .. }) if switchboard.invalidate_rejected_credentials(id)? => {
            switchboard.dispatch(request)?
        }
        outcome => outcome?,
    };
    let DispatchOutcome::Executed(output) = outcome else {
        return Err(anyhow!("identity request did not execute"));
    };
    let response = output
        .fields
        .get("response")
        .ok_or_else(|| anyhow!("identity request returned no JSON response"))?;
    let actual = match namespace.provider {
        ProviderKind::GoogleWorkspace => serde_json::from_value::<GoogleIdentity>(response.clone())?.email_address,
        ProviderKind::GitHub => serde_json::from_value::<GitHubIdentity>(response.clone())?.login,
        _ => return Err(anyhow!("unsupported identity response")),
    };
    let expected = configured_auth.account_label().trim();
    if actual.trim().is_empty() || !actual.eq_ignore_ascii_case(expected) {
        return Err(Error::AccountMismatch {
            expected: expected.to_owned(),
            actual,
        }
        .into());
    }
    Ok(AuthReceipt {
        namespace: id.clone(),
        provider: namespace.provider,
        expected_account: expected.to_owned(),
        authenticated_account: actual,
        identity_verified: true,
    })
}

#[derive(Debug, Serialize)]
struct MigrationPreview {
    namespace: NamespaceId,
    auth_ref: String,
    current_auth_mode: AuthKind,
    proposed_auth_mode: AuthKind,
    state_dir: Option<PathBuf>,
    saved_credentials_present: bool,
    identity_verified: bool,
    configuration_change: String,
    verification_command: String,
}

pub(crate) fn run(config_path: Option<&Path>, arguments: AuthCommand) -> Result<String> {
    match arguments.command {
        AuthSubcommand::Check(args) => {
            let switchboard = crate::load_switchboard_with_run(config_path, args.run_id.as_deref())?;
            let receipt = check_namespace(&switchboard, &NamespaceId::new(args.namespace)?)?;
            if args.json {
                crate::output::render_json(&receipt, true)
            } else {
                Ok(format!(
                    "{}: verified authenticated account {}\n",
                    receipt.namespace, receipt.authenticated_account
                ))
            }
        }
        AuthSubcommand::MigrationPreview(args) => {
            let path = crate::resolve_config_path(config_path)?;
            let config = SwitchboardConfig::from_file(&path).context("failed to load configuration")?;
            let policy = config.policy_engine();
            let (namespaces, auth_store, secrets) = config.into_stores();
            let id = NamespaceId::new(args.namespace)?;
            let namespace = namespaces
                .get(&id)
                .ok_or_else(|| Error::UnknownNamespace(id.to_string()))?;
            if namespace.provider != ProviderKind::GoogleWorkspace {
                return Err(Error::UnsupportedOperation(
                    "saved-session migration currently supports Google namespaces".into(),
                )
                .into());
            }
            let auth = auth_store
                .get(&namespace.auth_ref)
                .ok_or_else(|| Error::MissingAuth(namespace.auth_ref.to_string()))?;
            let saved_credentials_present = namespace.state_dir.as_ref().is_some_and(|directory| {
                ["credentials.enc", "credentials.json"]
                    .iter()
                    .any(|name| directory.join(name).is_file())
            });
            let identity_verified = if args.verify {
                let candidate_auth =
                    ResolvedAuth::new(auth.id().as_str(), auth.account_label(), AuthSecretRefs::GoogleCli)?;
                let candidate = crate::build_switchboard(
                    Arc::new(namespaces),
                    Arc::new(switchboard_store::StaticAuthStore::new([candidate_auth])),
                    Arc::new(secrets),
                    Arc::new(switchboard_store::LocalSecretResolver::default()),
                    Arc::new(policy),
                    Arc::new(switchboard_store::MemoryAuditStore::default()),
                    Arc::new(switchboard_store::MemoryOperationStore::default()),
                )?;
                check_namespace(&candidate, &id)?.identity_verified
            } else {
                false
            };
            let preview = MigrationPreview {
                namespace: id.clone(), auth_ref: namespace.auth_ref.to_string(), current_auth_mode: auth.kind(),
                proposed_auth_mode: AuthKind::GoogleCli, state_dir: namespace.state_dir,
                saved_credentials_present, identity_verified,
                configuration_change: format!("Replace auth.{} with provider = \"google\", kind = \"google_cli\", account = {:?}; preserve the namespace state_dir. This auth entry may be shared by other namespaces. Keep the old configuration for rollback.", namespace.auth_ref, auth.account_label()),
                verification_command: format!("switchboard auth check --ns {id} --json"),
            };
            if args.json {
                crate::output::render_json(&preview, true)
            } else {
                Ok(format!("{}: {} -> google_cli\nSaved credentials present: {} (identity verified: {})\n{}\nAfter opting in, verify: {}\n", id, preview.current_auth_mode, preview.saved_credentials_present, preview.identity_verified, preview.configuration_change, preview.verification_command))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{lock_env, TempScript};

    #[test]
    fn auth_check_verifies_provider_identity_and_migration_preserves_configuration() {
        let _guard = lock_env();
        let script = TempScript::new(
            "gws",
            r#"#!/bin/sh
case "$1" in
 --version) echo 'gws 0.22.5'; exit 0;;
 --help) echo 'gws help'; exit 0;;
esac
[ "$1 $2 $3" = 'gmail users getProfile' ] || exit 5
printf '%s\n' "$*" >> "$(dirname "$0")/env.txt"
cat "$(dirname "$0")/identity.json"
"#,
        );
        let directory = script.path().parent().expect("test fixture should be valid");
        let identity = directory.join("identity.json");
        std::fs::write(&identity, r#"{"emailAddress":"correct@example.com"}"#).expect("test fixture should be valid");
        let config_path = directory.join("config.toml");
        let source = format!(
            r#"[namespace.google.personal]
provider = "google"
account = "Personal Google"
auth = "google_account"
state_dir = {directory:?}
[auth.google_account]
provider = "google"
kind = "google_cli"
account = "correct@example.com"
"#
        );
        std::fs::write(&config_path, &source).expect("test fixture should be valid");
        let previous = std::env::var_os("SWITCHBOARD_GWS_BIN");
        std::env::set_var("SWITCHBOARD_GWS_BIN", script.path());
        let result = (|| -> Result<()> {
            let switchboard = crate::load_switchboard(Some(&config_path))?;
            let receipt = check_namespace(&switchboard, &NamespaceId::new("google.personal")?)?;
            assert!(receipt.identity_verified);
            assert_eq!(receipt.expected_account, "correct@example.com");
            assert!(script.capture_contents().contains("gmail users getProfile --params"));
            assert!(script.capture_contents().contains("--format json"));
            let preview = run(
                Some(&config_path),
                AuthCommand {
                    command: AuthSubcommand::MigrationPreview(MigrationArgs {
                        namespace: "google.personal".into(),
                        verify: true,
                        json: false,
                    }),
                },
            )?;
            assert!(preview.contains("identity verified: true"));
            assert_eq!(std::fs::read_to_string(&config_path)?, source);
            std::fs::write(&identity, r#"{"emailAddress":"wrong@example.com"}"#)?;
            let error = check_namespace(&switchboard, &NamespaceId::new("google.personal")?)
                .expect_err("authentication should be rejected");
            assert!(matches!(
                error.downcast_ref::<Error>(),
                Some(Error::AccountMismatch { .. })
            ));
            Ok(())
        })();
        match previous {
            Some(value) => std::env::set_var("SWITCHBOARD_GWS_BIN", value),
            None => std::env::remove_var("SWITCHBOARD_GWS_BIN"),
        }
        result.expect("test fixture should be valid");
    }
    #[test]
    fn auth_check_refreshes_rejected_cached_vault_credentials_once() {
        let _guard = lock_env();
        let provider = TempScript::new(
            "gh",
            r#"#!/bin/sh
case "$1" in
 --version) echo 'gh version 2.93.0'; exit 0;;
 --help) echo 'gh help'; exit 0;;
esac
if [ "$GH_TOKEN" = "fresh-fixture" ]; then
  printf '%s\n' '{"login":"expected-user"}'
else
  printf '%s\n' '{"error":{"code":401,"message":"Bad credentials"}}' >&2
  exit 1
fi
"#,
        );
        let vault = TempScript::new(
            "op",
            r#"#!/bin/sh
printf '%s\n' 'lookup' >> "$(dirname "$0")/env.txt"
cat "$(dirname "$0")/item.json"
"#,
        );
        let directory = provider.path().parent().expect("test fixture should be valid");
        let item = vault
            .path()
            .parent()
            .expect("test fixture should be valid")
            .join("item.json");
        std::fs::write(&item, r#"{"fields":[{"label":"token","value":"revoked-fixture"}]}"#)
            .expect("test fixture should be valid");
        let config_path = directory.join("config.toml");
        std::fs::write(
            &config_path,
            r#"[one_password]
auth_mode = "session"
[namespace.github.personal]
provider = "github"
account = "Display label"
auth = "github_account"
[auth.github_account]
provider = "github"
kind = "github_token"
account = "expected-user"
token = "github_token"
[secret.github_token]
kind = "onepassword_item"
account = "example"
item = "fixture"
field = "token"
"#,
        )
        .expect("test fixture should be valid");
        let previous_gh = std::env::var_os("SWITCHBOARD_GH_BIN");
        let previous_op = std::env::var_os("SWITCHBOARD_OP_BIN");
        std::env::set_var("SWITCHBOARD_GH_BIN", provider.path());
        std::env::set_var("SWITCHBOARD_OP_BIN", vault.path());
        let result = (|| -> Result<()> {
            let switchboard = crate::load_switchboard(Some(&config_path))?;
            let id = NamespaceId::new("github.personal")?;
            let initial = check_namespace(&switchboard, &id).expect_err("authentication should be rejected");
            assert!(matches!(
                initial.downcast_ref::<Error>(),
                Some(Error::AuthenticationRejected { .. })
            ));
            std::fs::write(&item, r#"{"fields":[{"label":"token","value":"fresh-fixture"}]}"#)?;
            let receipt = check_namespace(&switchboard, &id)?;
            assert!(receipt.identity_verified);
            assert_eq!(receipt.authenticated_account, "expected-user");
            let lookups = vault.capture_contents();
            assert_eq!(
                lookups.lines().count(),
                3,
                "one initial resolution, one first-check refresh, one second-check refresh"
            );
            Ok(())
        })();
        for (key, previous) in [("SWITCHBOARD_GH_BIN", previous_gh), ("SWITCHBOARD_OP_BIN", previous_op)] {
            match previous {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        result.expect("test fixture should be valid");
    }
}
