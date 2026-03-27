use std::{collections::BTreeMap, env, io::IsTerminal, process::Command, sync::Mutex};

use switchboard_core::{Error, ResolvedSecret, Result, SecretRef, SecretSource, SecretString};

use crate::secrets::{env_secret::normalize_secret, SecretBackend};

#[derive(Default)]
pub(super) struct OnePasswordSecretBackend {
    sessions: Mutex<BTreeMap<String, String>>,
}

impl SecretBackend for OnePasswordSecretBackend {
    fn can_resolve(&self, secret: &ResolvedSecret) -> bool {
        matches!(secret.source, SecretSource::OnePasswordItem { .. })
    }

    fn resolve(&self, secret: &ResolvedSecret) -> Result<SecretString> {
        let SecretSource::OnePasswordItem {
            account,
            item,
            field,
            vault,
        } = &secret.source
        else {
            return Err(Error::SecretResolution {
                secret_ref: secret.id.to_string(),
                reason: "1Password backend received a non-1Password secret".into(),
            });
        };

        let session = ensure_session(&secret.id, &self.sessions, account)?;
        let args = item_args(account, vault.as_deref(), item, field);
        let output = run(&secret.id, &args, session.as_deref())?;

        normalize_secret(&secret.id, output)
    }
}

pub(crate) fn item_args(account: &str, vault: Option<&str>, item: &str, field: &str) -> Vec<String> {
    let mut args = vec![
        "--account".to_owned(),
        account.to_owned(),
        "item".to_owned(),
        "get".to_owned(),
    ];

    if let Some(vault) = vault {
        args.push("--vault".to_owned());
        args.push(vault.to_owned());
    }

    args.push(item.to_owned());
    args.push("--fields".to_owned());
    args.push(format!("label={field}"));
    args.push("--reveal".to_owned());
    args
}

fn ensure_session(
    secret_ref: &SecretRef,
    sessions: &Mutex<BTreeMap<String, String>>,
    account: &str,
) -> Result<Option<String>> {
    if env::var("OP_BIOMETRIC_UNLOCK_ENABLED").unwrap_or_default() != "false" {
        return Ok(None);
    }

    if let Some(token) = cached_session(sessions, account) {
        return Ok(Some(token));
    }

    if whoami(account, None)? {
        return Ok(env::var("OP_SESSION").ok().filter(|token| !token.trim().is_empty()));
    }

    if !std::io::stdin().is_terminal() {
        return Err(Error::SecretResolution {
            secret_ref: secret_ref.to_string(),
            reason: format!("1Password account {account} is not signed in and no TTY is available for `op signin`"),
        });
    }

    let output = Command::new("op")
        .args(["signin", "--account", account, "--raw"])
        .output()
        .map_err(|error| Error::SecretResolution {
            secret_ref: secret_ref.to_string(),
            reason: format!("failed to run `op signin --account {account} --raw`: {error}"),
        })?;

    if !output.status.success() {
        return Err(Error::SecretResolution {
            secret_ref: secret_ref.to_string(),
            reason: format!(
                "`op signin --account {account} --raw` failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }

    let token = String::from_utf8(output.stdout).map_err(|error| Error::SecretResolution {
        secret_ref: secret_ref.to_string(),
        reason: format!("`op signin --account {account} --raw` produced non-UTF-8 output: {error}"),
    })?;
    let token = normalize_secret(secret_ref, token)?;
    let token_value = token.expose().to_owned();
    cache_session(sessions, account, &token_value);

    Ok(Some(token_value))
}

fn whoami(account: &str, session: Option<&str>) -> Result<bool> {
    let mut command = Command::new("op");
    command.args(["whoami", "--account", account]);
    if let Some(session) = session {
        command.env("OP_SESSION", session);
    }

    let status = command
        .status()
        .map_err(|error| Error::Config(format!("failed to run `op whoami --account {account}`: {error}")))?;

    Ok(status.success())
}

fn run(secret_ref: &SecretRef, args: &[String], session: Option<&str>) -> Result<String> {
    let mut command = Command::new("op");
    command.args(args);
    if let Some(session) = session {
        command.env("OP_SESSION", session);
    }

    let output = command.output().map_err(|error| Error::SecretResolution {
        secret_ref: secret_ref.to_string(),
        reason: format!("failed to run `op {}`: {error}", args.join(" ")),
    })?;

    if !output.status.success() {
        return Err(Error::SecretResolution {
            secret_ref: secret_ref.to_string(),
            reason: format!(
                "`op {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }

    String::from_utf8(output.stdout).map_err(|error| Error::SecretResolution {
        secret_ref: secret_ref.to_string(),
        reason: format!("1Password CLI returned non-UTF-8 output: {error}"),
    })
}

fn cached_session(sessions: &Mutex<BTreeMap<String, String>>, account: &str) -> Option<String> {
    match sessions.lock() {
        Ok(sessions) => sessions.get(account).cloned(),
        Err(poisoned) => poisoned.into_inner().get(account).cloned(),
    }
}

fn cache_session(sessions: &Mutex<BTreeMap<String, String>>, account: &str, token: &str) {
    match sessions.lock() {
        Ok(mut sessions) => {
            sessions.insert(account.to_owned(), token.to_owned());
        }
        Err(poisoned) => {
            poisoned.into_inner().insert(account.to_owned(), token.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::item_args;

    #[test]
    fn item_args_include_optional_vault_and_label_selector() {
        let args = item_args("kittycadinc.1password.com", Some("Employee"), "gws cli", "credential");

        assert_eq!(
            args,
            vec![
                "--account",
                "kittycadinc.1password.com",
                "item",
                "get",
                "--vault",
                "Employee",
                "gws cli",
                "--fields",
                "label=credential",
                "--reveal",
            ]
        );
    }
}
