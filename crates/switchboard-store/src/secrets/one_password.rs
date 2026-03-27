use std::{collections::BTreeMap, env, io::IsTerminal, process::Command, sync::Mutex};

use serde::Deserialize;
use switchboard_core::{Error, ResolvedSecret, Result, SecretRef, SecretSource, SecretString};

use crate::secrets::{env_secret::normalize_secret, SecretBackend};

#[derive(Default)]
pub(super) struct OnePasswordSecretBackend {
    sessions: Mutex<BTreeMap<String, String>>,
    items: Mutex<BTreeMap<OnePasswordItemKey, BTreeMap<String, SecretString>>>,
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

        let item_key = OnePasswordItemKey::new(account, vault.as_deref(), item);
        if let Some(value) = cached_item_field(&self.items, &item_key, field) {
            return Ok(value);
        }

        let session = ensure_session(&secret.id, &self.sessions, account)?;
        match fetch_item_fields(&secret.id, &item_key, session.as_deref()) {
            Ok(fields) => {
                cache_item_fields(&self.items, &item_key, &fields);
                if let Some(value) = fields.get(field) {
                    return Ok(value.clone());
                }
            }
            Err(Error::SecretResolution { reason, .. })
                if reason.starts_with("1Password CLI returned invalid item JSON") =>
            {
                // Fall back to the direct field lookup path if JSON item decoding fails.
            }
            Err(error) => return Err(error),
        }

        let args = item_args(account, vault.as_deref(), item, field);
        let output = run(&secret.id, &args, session.as_deref())?;
        let value = normalize_secret(&secret.id, output)?;
        cache_item_field(&self.items, &item_key, field, &value);
        Ok(value)
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

fn item_json_args(account: &str, vault: Option<&str>, item: &str) -> Vec<String> {
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
    args.push("--format".to_owned());
    args.push("json".to_owned());
    args
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct OnePasswordItemKey {
    account: String,
    vault: Option<String>,
    item: String,
}

impl OnePasswordItemKey {
    fn new(account: &str, vault: Option<&str>, item: &str) -> Self {
        Self {
            account: account.to_owned(),
            vault: vault.map(str::to_owned),
            item: item.to_owned(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct OnePasswordItem {
    #[serde(default)]
    fields: Vec<OnePasswordItemField>,
}

#[derive(Debug, Deserialize)]
struct OnePasswordItemField {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    value: Option<serde_json::Value>,
}

impl OnePasswordItemField {
    fn value_as_string(&self) -> Option<String> {
        match self.value.as_ref()? {
            serde_json::Value::String(value) => Some(value.clone()),
            serde_json::Value::Number(value) => Some(value.to_string()),
            serde_json::Value::Bool(value) => Some(value.to_string()),
            _ => None,
        }
    }
}

fn fetch_item_fields(
    secret_ref: &SecretRef,
    item_key: &OnePasswordItemKey,
    session: Option<&str>,
) -> Result<BTreeMap<String, SecretString>> {
    let args = item_json_args(&item_key.account, item_key.vault.as_deref(), &item_key.item);
    let output = run(secret_ref, &args, session)?;
    parse_item_fields(secret_ref, &output)
}

fn parse_item_fields(secret_ref: &SecretRef, output: &str) -> Result<BTreeMap<String, SecretString>> {
    let item: OnePasswordItem = serde_json::from_str(output).map_err(|error| Error::SecretResolution {
        secret_ref: secret_ref.to_string(),
        reason: format!("1Password CLI returned invalid item JSON: {error}"),
    })?;

    let mut fields = BTreeMap::new();
    for field in item.fields {
        let Some(value) = field.value_as_string() else {
            continue;
        };
        let value: SecretString = value.into();

        if let Some(label) = field.label.filter(|label| !label.trim().is_empty()) {
            fields.entry(label).or_insert_with(|| value.clone());
        }
        if let Some(id) = field.id.filter(|id| !id.trim().is_empty()) {
            fields.entry(id).or_insert(value);
        }
    }

    Ok(fields)
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

    let output = op_command()
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
    let mut command = op_command();
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
    let mut command = op_command();
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

fn op_command() -> Command {
    match env::var_os("SWITCHBOARD_OP_BIN") {
        Some(path) => Command::new(path),
        None => Command::new("op"),
    }
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

fn cached_item_field(
    items: &Mutex<BTreeMap<OnePasswordItemKey, BTreeMap<String, SecretString>>>,
    item_key: &OnePasswordItemKey,
    field: &str,
) -> Option<SecretString> {
    match items.lock() {
        Ok(items) => items.get(item_key).and_then(|fields| fields.get(field)).cloned(),
        Err(poisoned) => poisoned
            .into_inner()
            .get(item_key)
            .and_then(|fields| fields.get(field))
            .cloned(),
    }
}

fn cache_item_field(
    items: &Mutex<BTreeMap<OnePasswordItemKey, BTreeMap<String, SecretString>>>,
    item_key: &OnePasswordItemKey,
    field: &str,
    value: &SecretString,
) {
    match items.lock() {
        Ok(mut items) => {
            items
                .entry(item_key.clone())
                .or_default()
                .insert(field.to_owned(), value.clone());
        }
        Err(poisoned) => {
            poisoned
                .into_inner()
                .entry(item_key.clone())
                .or_default()
                .insert(field.to_owned(), value.clone());
        }
    }
}

fn cache_item_fields(
    items: &Mutex<BTreeMap<OnePasswordItemKey, BTreeMap<String, SecretString>>>,
    item_key: &OnePasswordItemKey,
    fields: &BTreeMap<String, SecretString>,
) {
    match items.lock() {
        Ok(mut items) => {
            items
                .entry(item_key.clone())
                .or_default()
                .extend(fields.iter().map(|(field, value)| (field.clone(), value.clone())));
        }
        Err(poisoned) => {
            poisoned
                .into_inner()
                .entry(item_key.clone())
                .or_default()
                .extend(fields.iter().map(|(field, value)| (field.clone(), value.clone())));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        path::PathBuf,
        process,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use switchboard_core::{ResolvedSecret, SecretSource};

    use super::{item_args, OnePasswordSecretBackend, SecretBackend};

    static TEMP_FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

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

    #[test]
    fn resolves_multiple_fields_from_the_same_item_with_one_op_call() {
        let fixture = TempFixtureDir::new();
        let op_script = fixture.write_executable(
            "op",
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SWITCHBOARD_OP_LOG"
case " $* " in
  *" --format json "*)
    cat <<'EOF'
{"fields":[
  {"label":"username","value":"personal-client-id"},
  {"label":"credential","value":"personal-client-secret"}
]}
EOF
    ;;
  *)
    echo "unexpected args: $*" >&2
    exit 1
    ;;
esac
"#,
        );
        let log_path = fixture.path.join("op.log");
        fs::write(&log_path, "").expect("log file should exist");

        let _op_bin_guard = EnvVarGuard::set("SWITCHBOARD_OP_BIN", op_script.into_os_string());
        let _op_log_guard = EnvVarGuard::set("SWITCHBOARD_OP_LOG", log_path.clone().into_os_string());
        let _biometric_guard = EnvVarGuard::remove("OP_BIOMETRIC_UNLOCK_ENABLED");

        let backend = OnePasswordSecretBackend::default();
        let client_id_secret = ResolvedSecret::new(
            "google_personal_client_id",
            SecretSource::OnePasswordItem {
                account: "my.1password.com".into(),
                vault: None,
                item: "gws cli".into(),
                field: "username".into(),
            },
        )
        .expect("secret should build");
        let client_secret_secret = ResolvedSecret::new(
            "google_personal_client_secret",
            SecretSource::OnePasswordItem {
                account: "my.1password.com".into(),
                vault: None,
                item: "gws cli".into(),
                field: "credential".into(),
            },
        )
        .expect("secret should build");

        let client_id = backend.resolve(&client_id_secret).expect("client id should resolve");
        let client_secret = backend
            .resolve(&client_secret_secret)
            .expect("client secret should resolve");

        assert_eq!(client_id.expose(), "personal-client-id");
        assert_eq!(client_secret.expose(), "personal-client-secret");
        assert_eq!(
            fs::read_to_string(&log_path)
                .expect("log should be readable")
                .lines()
                .count(),
            1
        );
    }

    struct TempFixtureDir {
        path: PathBuf,
    }

    impl TempFixtureDir {
        fn new() -> Self {
            let path = env::temp_dir().join(format!(
                "switchboard-op-test-{}-{}-{}",
                process::id(),
                TEMP_FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system time should be after unix epoch")
                    .as_nanos()
            ));
            fs::create_dir_all(&path).expect("temp fixture dir should exist");

            Self { path }
        }

        fn write_executable(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.path.join(name);
            fs::write(&path, contents).expect("fixture file should be written");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                let mut permissions = fs::metadata(&path)
                    .expect("fixture metadata should exist")
                    .permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&path, permissions).expect("fixture should be executable");
            }
            path
        }
    }

    impl Drop for TempFixtureDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: std::ffi::OsString) -> Self {
            let previous = env::var_os(key);
            env::set_var(key, value);
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = env::var_os(key);
            env::remove_var(key);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => env::set_var(self.key, value),
                None => env::remove_var(self.key),
            }
        }
    }
}
