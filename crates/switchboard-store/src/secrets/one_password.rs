use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::Duration,
};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use switchboard_core::{Error, ResolvedSecret, Result, SecretRef, SecretSource, SecretString};

use crate::secrets::{env_secret::normalize_secret, SecretBackend};

pub(super) struct OnePasswordSecretBackend {
    sessions: Mutex<BTreeMap<String, CachedSession>>,
    items: Mutex<BTreeMap<OnePasswordItemKey, BTreeMap<String, SecretString>>>,
    session_cache_path: Option<PathBuf>,
}

impl Default for OnePasswordSecretBackend {
    fn default() -> Self {
        Self::new(None)
    }
}

impl OnePasswordSecretBackend {
    pub(super) fn new(session_cache_path: Option<PathBuf>) -> Self {
        Self {
            sessions: Mutex::new(BTreeMap::new()),
            items: Mutex::new(BTreeMap::new()),
            session_cache_path,
        }
    }
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

        let mut session = ensure_session(&secret.id, &self.sessions, self.session_cache_path.as_deref(), account)?;
        match fetch_item_fields_with_retry(
            &secret.id,
            &item_key,
            &mut session,
            &self.sessions,
            self.session_cache_path.as_deref(),
        ) {
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
        let output = run_with_retry(
            &secret.id,
            account,
            &args,
            &mut session,
            &self.sessions,
            self.session_cache_path.as_deref(),
        )?;
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

#[derive(Debug, Default, Deserialize, Serialize)]
struct SessionCacheFile {
    #[serde(default)]
    sessions: BTreeMap<String, PersistedSession>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CachedSession {
    Token(String),
    AppIntegration,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum PersistedSession {
    LegacyToken(String),
    Entry(PersistedSessionEntry),
}

impl PersistedSession {
    fn from_cached(session: &CachedSession) -> Option<Self> {
        match session {
            CachedSession::Token(token) if !token.trim().is_empty() => Some(Self::Entry(PersistedSessionEntry {
                kind: PersistedSessionKind::Token,
                token: Some(token.clone()),
            })),
            CachedSession::Token(_) => None,
            CachedSession::AppIntegration => Some(Self::Entry(PersistedSessionEntry {
                kind: PersistedSessionKind::AppIntegration,
                token: None,
            })),
        }
    }

    fn into_cached(self) -> Option<CachedSession> {
        match self {
            Self::LegacyToken(token) => {
                let token = token.trim().to_owned();
                if token.is_empty() {
                    None
                } else {
                    Some(CachedSession::Token(token))
                }
            }
            Self::Entry(entry) => entry.into_cached(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedSessionEntry {
    kind: PersistedSessionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
}

impl PersistedSessionEntry {
    fn into_cached(self) -> Option<CachedSession> {
        match self.kind {
            PersistedSessionKind::Token => self
                .token
                .map(|token| token.trim().to_owned())
                .filter(|token| !token.is_empty())
                .map(CachedSession::Token),
            PersistedSessionKind::AppIntegration => Some(CachedSession::AppIntegration),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum PersistedSessionKind {
    Token,
    AppIntegration,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct SessionHandle {
    token: Option<String>,
    optimistic_app_integration: bool,
}

impl SessionHandle {
    fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    fn optimistic_app_integration(&self) -> bool {
        self.optimistic_app_integration
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
    sessions: &Mutex<BTreeMap<String, CachedSession>>,
    session_cache_path: Option<&Path>,
    account: &str,
) -> Result<SessionHandle> {
    if let Some(session) = cached_session(sessions, account) {
        return match session {
            CachedSession::Token(token) => Ok(SessionHandle {
                token: Some(token),
                optimistic_app_integration: false,
            }),
            CachedSession::AppIntegration => Ok(SessionHandle {
                token: None,
                optimistic_app_integration: false,
            }),
        };
    }

    if let Some(session) = cached_session_on_disk(session_cache_path, account) {
        match session {
            CachedSession::Token(token) => {
                if whoami(account, Some(&token))? {
                    cache_token_session(sessions, session_cache_path, account, &token);
                    return Ok(SessionHandle {
                        token: Some(token),
                        optimistic_app_integration: false,
                    });
                }
                forget_session(sessions, session_cache_path, account);
            }
            CachedSession::AppIntegration => {
                cache_app_session(sessions, session_cache_path, account);
                return Ok(SessionHandle {
                    token: None,
                    optimistic_app_integration: true,
                });
            }
        }
    }

    if let Some(token) = env_session() {
        if whoami(account, Some(&token))? {
            cache_token_session(sessions, session_cache_path, account, &token);
            return Ok(SessionHandle {
                token: Some(token),
                optimistic_app_integration: false,
            });
        }
    }

    if whoami(account, None)? {
        cache_app_session(sessions, session_cache_path, account);
        return Ok(SessionHandle {
            token: None,
            optimistic_app_integration: false,
        });
    }

    let token = sign_in(secret_ref, account)?;
    if let Some(token) = token {
        cache_token_session(sessions, session_cache_path, account, &token);
        return Ok(SessionHandle {
            token: Some(token),
            optimistic_app_integration: false,
        });
    }

    if whoami(account, None)? {
        cache_app_session(sessions, session_cache_path, account);
        return Ok(SessionHandle {
            token: None,
            optimistic_app_integration: false,
        });
    }

    Err(Error::SecretResolution {
        secret_ref: secret_ref.to_string(),
        reason: format!(
            "`op signin --account {account} --raw` succeeded but did not authenticate the CLI for that account"
        ),
    })
}

fn sign_in(secret_ref: &SecretRef, account: &str) -> Result<Option<String>> {
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

    if token.trim_end_matches(['\n', '\r']).is_empty() {
        return Ok(None);
    }

    let token = normalize_secret(secret_ref, token)?;
    Ok(Some(token.expose().to_owned()))
}

fn whoami(account: &str, session: Option<&str>) -> Result<bool> {
    let mut command = op_command();
    if let Some(session) = session {
        command.args(["--session", session]);
    }
    command.args(["whoami", "--account", account]);

    let output = command
        .output()
        .map_err(|error| Error::Config(format!("failed to run `op whoami --account {account}`: {error}")))?;

    Ok(output.status.success())
}

fn run(secret_ref: &SecretRef, args: &[String], session: Option<&str>) -> Result<String> {
    let mut command = op_command();
    if let Some(session) = session {
        command.args(["--session", session]);
    }
    command.args(args);

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

fn fetch_item_fields_with_retry(
    secret_ref: &SecretRef,
    item_key: &OnePasswordItemKey,
    session: &mut SessionHandle,
    sessions: &Mutex<BTreeMap<String, CachedSession>>,
    session_cache_path: Option<&Path>,
) -> Result<BTreeMap<String, SecretString>> {
    match fetch_item_fields(secret_ref, item_key, session.token()) {
        Ok(fields) => Ok(fields),
        Err(_) if session.optimistic_app_integration() => {
            *session = refresh_session(secret_ref, sessions, session_cache_path, &item_key.account)?;
            fetch_item_fields(secret_ref, item_key, session.token())
        }
        Err(error) => Err(error),
    }
}

fn run_with_retry(
    secret_ref: &SecretRef,
    account: &str,
    args: &[String],
    session: &mut SessionHandle,
    sessions: &Mutex<BTreeMap<String, CachedSession>>,
    session_cache_path: Option<&Path>,
) -> Result<String> {
    match run(secret_ref, args, session.token()) {
        Ok(output) => Ok(output),
        Err(_) if session.optimistic_app_integration() => {
            *session = refresh_session(secret_ref, sessions, session_cache_path, account)?;
            run(secret_ref, args, session.token())
        }
        Err(error) => Err(error),
    }
}

fn refresh_session(
    secret_ref: &SecretRef,
    sessions: &Mutex<BTreeMap<String, CachedSession>>,
    session_cache_path: Option<&Path>,
    account: &str,
) -> Result<SessionHandle> {
    forget_session(sessions, session_cache_path, account);
    ensure_session(secret_ref, sessions, session_cache_path, account)
}

fn op_command() -> Command {
    let mut command = match env::var_os("SWITCHBOARD_OP_BIN") {
        Some(path) => Command::new(path),
        None => Command::new("op"),
    };
    command.env_remove("OP_ACCOUNT");
    command.env_remove("OP_SESSION");
    command
}

fn env_session() -> Option<String> {
    env::var("OP_SESSION").ok().filter(|token| !token.trim().is_empty())
}

fn cached_session(sessions: &Mutex<BTreeMap<String, CachedSession>>, account: &str) -> Option<CachedSession> {
    match sessions.lock() {
        Ok(sessions) => sessions.get(account).cloned(),
        Err(poisoned) => poisoned.into_inner().get(account).cloned(),
    }
}

fn cache_token_session(
    sessions: &Mutex<BTreeMap<String, CachedSession>>,
    session_cache_path: Option<&Path>,
    account: &str,
    token: &str,
) {
    match sessions.lock() {
        Ok(mut sessions) => {
            sessions.insert(account.to_owned(), CachedSession::Token(token.to_owned()));
        }
        Err(poisoned) => {
            poisoned
                .into_inner()
                .insert(account.to_owned(), CachedSession::Token(token.to_owned()));
        }
    }

    let session = CachedSession::Token(token.to_owned());
    let _ = write_session_cache(session_cache_path, account, Some(&session));
}

fn cache_app_session(
    sessions: &Mutex<BTreeMap<String, CachedSession>>,
    session_cache_path: Option<&Path>,
    account: &str,
) {
    match sessions.lock() {
        Ok(mut sessions) => {
            sessions.insert(account.to_owned(), CachedSession::AppIntegration);
        }
        Err(poisoned) => {
            poisoned
                .into_inner()
                .insert(account.to_owned(), CachedSession::AppIntegration);
        }
    }

    let session = CachedSession::AppIntegration;
    let _ = write_session_cache(session_cache_path, account, Some(&session));
}

fn forget_session(sessions: &Mutex<BTreeMap<String, CachedSession>>, session_cache_path: Option<&Path>, account: &str) {
    match sessions.lock() {
        Ok(mut sessions) => {
            sessions.remove(account);
        }
        Err(poisoned) => {
            poisoned.into_inner().remove(account);
        }
    }

    let _ = write_session_cache(session_cache_path, account, None);
}

fn cached_session_on_disk(session_cache_path: Option<&Path>, account: &str) -> Option<CachedSession> {
    let cache = read_session_cache(session_cache_path?)?;
    cache.sessions.get(account).cloned()?.into_cached()
}

fn read_session_cache(path: &Path) -> Option<SessionCacheFile> {
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

fn session_cache_lock_path(path: &Path) -> PathBuf {
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock.sqlite3");
    PathBuf::from(lock_path)
}

fn open_session_cache_lock(path: &Path) -> std::io::Result<Connection> {
    let lock_path = session_cache_lock_path(path);
    let connection = Connection::open(lock_path).map_err(std::io::Error::other)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(std::io::Error::other)?;
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(std::io::Error::other)?;
    Ok(connection)
}

fn write_session_cache(
    session_cache_path: Option<&Path>,
    account: &str,
    session: Option<&CachedSession>,
) -> std::io::Result<()> {
    let Some(path) = session_cache_path else {
        return Ok(());
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let lock_connection = open_session_cache_lock(path)?;

    let mut cache = read_session_cache(path).unwrap_or_default();
    match session.and_then(PersistedSession::from_cached) {
        Some(session) => {
            cache.sessions.insert(account.to_owned(), session);
        }
        _ => {
            cache.sessions.remove(account);
        }
    }

    let serialized = serde_json::to_vec(&cache).map_err(std::io::Error::other)?;
    let temp_path = path.with_extension("tmp");
    fs::write(&temp_path, serialized)?;
    set_owner_only_permissions(&temp_path)?;
    fs::rename(&temp_path, path)?;
    set_owner_only_permissions(path)?;
    lock_connection.execute_batch("COMMIT").map_err(std::io::Error::other)
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
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
        sync::{
            atomic::{AtomicU64, Ordering},
            mpsc, Mutex,
        },
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use switchboard_core::{ResolvedSecret, SecretSource};

    use super::{
        cached_session_on_disk, item_args, open_session_cache_lock, write_session_cache, CachedSession,
        OnePasswordSecretBackend, SecretBackend,
    };

    static ENV_LOCK: Mutex<()> = Mutex::new(());
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
    fn resolves_multiple_fields_from_the_same_item_with_one_item_get_call() {
        let _guard = match ENV_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let fixture = TempFixtureDir::new();
        let op_script = fixture.write_executable(
            "op",
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SWITCHBOARD_OP_LOG"
case " $* " in
  *" --session session-token whoami --account my.1password.com "*)
    ;;
  *" --session session-token --account my.1password.com item get "*" --format json "*)
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
        let _op_session_guard = EnvVarGuard::set("OP_SESSION", "session-token".into());

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
                .filter(|line| line.contains("item get"))
                .count(),
            1
        );
    }

    #[test]
    fn reuses_cached_token_sessions_across_item_lookups() {
        let _guard = match ENV_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let fixture = TempFixtureDir::new();
        let op_script = fixture.write_executable(
            "op",
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SWITCHBOARD_OP_LOG"
case " $* " in
  *" --session session-token whoami --account my.1password.com "*)
    ;;
  *" --session session-token --account my.1password.com item get item-one --format json "*)
    cat <<'EOF'
{"fields":[
  {"label":"credential","value":"first-secret"}
]}
EOF
    ;;
  *" --session session-token --account my.1password.com item get item-two --format json "*)
    cat <<'EOF'
{"fields":[
  {"label":"credential","value":"second-secret"}
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
        let _op_session_guard = EnvVarGuard::set("OP_SESSION", "session-token".into());

        let backend = OnePasswordSecretBackend::default();
        let first_secret = ResolvedSecret::new(
            "first_secret",
            SecretSource::OnePasswordItem {
                account: "my.1password.com".into(),
                vault: None,
                item: "item-one".into(),
                field: "credential".into(),
            },
        )
        .expect("secret should build");
        let second_secret = ResolvedSecret::new(
            "second_secret",
            SecretSource::OnePasswordItem {
                account: "my.1password.com".into(),
                vault: None,
                item: "item-two".into(),
                field: "credential".into(),
            },
        )
        .expect("secret should build");

        let first = backend.resolve(&first_secret).expect("first secret should resolve");
        let second = backend.resolve(&second_secret).expect("second secret should resolve");

        assert_eq!(first.expose(), "first-secret");
        assert_eq!(second.expose(), "second-secret");

        let log = fs::read_to_string(&log_path).expect("log should be readable");
        assert_eq!(
            log.lines()
                .filter(|line| line.contains("--session session-token whoami --account my.1password.com"))
                .count(),
            1
        );
        assert_eq!(log.lines().filter(|line| line.contains("signin --account")).count(), 0);
        assert_eq!(log.lines().filter(|line| line.contains("item get")).count(), 2);
    }

    #[test]
    fn persists_sessions_across_backend_instances() {
        let _guard = match ENV_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let fixture = TempFixtureDir::new();
        let op_script = fixture.write_executable(
            "op",
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SWITCHBOARD_OP_LOG"
case " $* " in
  *" signin --account my.1password.com --raw "*)
    printf 'persisted-session\n'
    ;;
  *" --session persisted-session whoami --account my.1password.com "*)
    ;;
  *" --session persisted-session --account my.1password.com item get "*" --format json "*)
    cat <<'EOF'
{"fields":[
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
        let cache_path = fixture.path.join("onepassword-sessions.json");
        let log_path = fixture.path.join("op.log");
        fs::write(&log_path, "").expect("log file should exist");

        let _op_bin_guard = EnvVarGuard::set("SWITCHBOARD_OP_BIN", op_script.into_os_string());
        let _op_log_guard = EnvVarGuard::set("SWITCHBOARD_OP_LOG", log_path.clone().into_os_string());
        let _op_session_guard = EnvVarGuard::remove("OP_SESSION");

        let secret = ResolvedSecret::new(
            "google_personal_client_secret",
            SecretSource::OnePasswordItem {
                account: "my.1password.com".into(),
                vault: None,
                item: "gws cli".into(),
                field: "credential".into(),
            },
        )
        .expect("secret should build");

        let first = OnePasswordSecretBackend::new(Some(cache_path.clone()));
        let second = OnePasswordSecretBackend::new(Some(cache_path.clone()));

        let first_value = first.resolve(&secret).expect("first backend should resolve");
        let second_value = second.resolve(&secret).expect("second backend should resolve");

        assert_eq!(first_value.expose(), "personal-client-secret");
        assert_eq!(second_value.expose(), "personal-client-secret");

        let log = fs::read_to_string(&log_path).expect("log should be readable");
        assert_eq!(log.lines().filter(|line| line.contains("signin --account")).count(), 1);
        assert_eq!(
            log.lines()
                .filter(|line| line.contains("--session persisted-session whoami --account my.1password.com"))
                .count(),
            1
        );
        assert!(fs::read_to_string(&cache_path)
            .expect("cache file should exist")
            .contains("persisted-session"));
    }

    #[test]
    fn reads_legacy_token_session_cache_files() {
        let _guard = match ENV_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let fixture = TempFixtureDir::new();
        let op_script = fixture.write_executable(
            "op",
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SWITCHBOARD_OP_LOG"
case " $* " in
  *" --session legacy-session whoami --account my.1password.com "*)
    ;;
  *" --session legacy-session --account my.1password.com item get "*" --format json "*)
    cat <<'EOF'
{"fields":[
  {"label":"credential","value":"legacy-secret"}
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
        let cache_path = fixture.path.join("onepassword-sessions.json");
        let log_path = fixture.path.join("op.log");
        fs::write(&log_path, "").expect("log file should exist");
        fs::write(&cache_path, r#"{"sessions":{"my.1password.com":"legacy-session"}}"#)
            .expect("legacy cache should exist");

        let _op_bin_guard = EnvVarGuard::set("SWITCHBOARD_OP_BIN", op_script.into_os_string());
        let _op_log_guard = EnvVarGuard::set("SWITCHBOARD_OP_LOG", log_path.clone().into_os_string());
        let _op_session_guard = EnvVarGuard::remove("OP_SESSION");

        let backend = OnePasswordSecretBackend::new(Some(cache_path));
        let secret = ResolvedSecret::new(
            "google_personal_client_secret",
            SecretSource::OnePasswordItem {
                account: "my.1password.com".into(),
                vault: None,
                item: "gws cli".into(),
                field: "credential".into(),
            },
        )
        .expect("secret should build");

        let value = backend.resolve(&secret).expect("legacy cached secret should resolve");

        assert_eq!(value.expose(), "legacy-secret");
        let log = fs::read_to_string(&log_path).expect("log should be readable");
        assert_eq!(log.lines().filter(|line| line.contains("signin --account")).count(), 0);
        assert_eq!(
            log.lines()
                .filter(|line| line.contains("--session legacy-session whoami --account my.1password.com"))
                .count(),
            1
        );
    }

    #[test]
    fn persists_app_integration_sessions_across_backend_instances() {
        let _guard = match ENV_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let fixture = TempFixtureDir::new();
        let op_script = fixture.write_executable(
            "op",
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SWITCHBOARD_OP_LOG"
case " $* " in
  *" whoami --account my.1password.com "*)
    ;;
  *" --account my.1password.com item get item-one --format json "*)
    cat <<'EOF'
{"fields":[
  {"label":"credential","value":"first-secret"}
]}
EOF
    ;;
  *" --account my.1password.com item get item-two --format json "*)
    cat <<'EOF'
{"fields":[
  {"label":"credential","value":"second-secret"}
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
        let cache_path = fixture.path.join("onepassword-sessions.json");
        let log_path = fixture.path.join("op.log");
        fs::write(&log_path, "").expect("log file should exist");

        let _op_bin_guard = EnvVarGuard::set("SWITCHBOARD_OP_BIN", op_script.into_os_string());
        let _op_log_guard = EnvVarGuard::set("SWITCHBOARD_OP_LOG", log_path.clone().into_os_string());
        let _op_session_guard = EnvVarGuard::remove("OP_SESSION");

        let first = OnePasswordSecretBackend::new(Some(cache_path.clone()));
        let second = OnePasswordSecretBackend::new(Some(cache_path));

        let first_secret = ResolvedSecret::new(
            "first_secret",
            SecretSource::OnePasswordItem {
                account: "my.1password.com".into(),
                vault: None,
                item: "item-one".into(),
                field: "credential".into(),
            },
        )
        .expect("secret should build");
        let second_secret = ResolvedSecret::new(
            "second_secret",
            SecretSource::OnePasswordItem {
                account: "my.1password.com".into(),
                vault: None,
                item: "item-two".into(),
                field: "credential".into(),
            },
        )
        .expect("secret should build");

        let first_value = first.resolve(&first_secret).expect("first backend should resolve");
        let second_value = second.resolve(&second_secret).expect("second backend should resolve");

        assert_eq!(first_value.expose(), "first-secret");
        assert_eq!(second_value.expose(), "second-secret");

        let log = fs::read_to_string(&log_path).expect("log should be readable");
        assert_eq!(
            log.lines()
                .filter(|line| *line == "whoami --account my.1password.com")
                .count(),
            1
        );
        assert_eq!(
            log.lines()
                .filter(|line| line.contains("--account my.1password.com item get"))
                .count(),
            2
        );
    }

    #[test]
    fn stale_app_integration_cache_revalidates_and_retries() {
        let _guard = match ENV_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let fixture = TempFixtureDir::new();
        let op_script = fixture.write_executable(
            "op",
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SWITCHBOARD_OP_LOG"
case " $* " in
  *" whoami --account my.1password.com "*)
    : > "$SWITCHBOARD_ALLOW_ITEM_GET"
    ;;
  *" --account my.1password.com item get item-one --format json "*)
    if [ -f "$SWITCHBOARD_ALLOW_ITEM_GET" ]; then
      cat <<'EOF'
{"fields":[
  {"label":"credential","value":"revalidated-secret"}
]}
EOF
    else
      echo "not signed in" >&2
      exit 1
    fi
    ;;
  *)
    echo "unexpected args: $*" >&2
    exit 1
    ;;
esac
"#,
        );
        let cache_path = fixture.path.join("onepassword-sessions.json");
        let log_path = fixture.path.join("op.log");
        let allow_item_get_path = fixture.path.join("allow-item-get");
        fs::write(&log_path, "").expect("log file should exist");
        fs::write(
            &cache_path,
            r#"{"sessions":{"my.1password.com":{"kind":"app_integration"}}}"#,
        )
        .expect("app integration cache should exist");

        let _op_bin_guard = EnvVarGuard::set("SWITCHBOARD_OP_BIN", op_script.into_os_string());
        let _op_log_guard = EnvVarGuard::set("SWITCHBOARD_OP_LOG", log_path.clone().into_os_string());
        let _allow_item_get_guard = EnvVarGuard::set(
            "SWITCHBOARD_ALLOW_ITEM_GET",
            allow_item_get_path.clone().into_os_string(),
        );
        let _op_session_guard = EnvVarGuard::remove("OP_SESSION");
        fs::remove_file(&allow_item_get_path).ok();

        let backend = OnePasswordSecretBackend::new(Some(cache_path));
        let secret = ResolvedSecret::new(
            "google_personal_client_secret",
            SecretSource::OnePasswordItem {
                account: "my.1password.com".into(),
                vault: None,
                item: "item-one".into(),
                field: "credential".into(),
            },
        )
        .expect("secret should build");

        let value = backend.resolve(&secret).expect("stale app cache should recover");

        assert_eq!(value.expose(), "revalidated-secret");
        let log = fs::read_to_string(&log_path).expect("log should be readable");
        assert_eq!(
            log.lines()
                .filter(|line| *line == "whoami --account my.1password.com")
                .count(),
            1
        );
        assert_eq!(
            log.lines()
                .filter(|line| line.contains("--account my.1password.com item get item-one --format json"))
                .count(),
            2
        );
    }

    #[test]
    fn concurrent_session_cache_writes_do_not_drop_other_accounts() {
        let _guard = match ENV_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let fixture = TempFixtureDir::new();
        let cache_path = fixture.path.join("onepassword-sessions.json");
        let first_account = "my.1password.com";
        let second_account = "kittycadinc.1password.com";
        let first_session = CachedSession::Token("persisted-session".into());
        let second_session = CachedSession::AppIntegration;

        write_session_cache(Some(&cache_path), first_account, Some(&first_session))
            .expect("first cache write should succeed");

        let lock_connection = open_session_cache_lock(&cache_path).expect("lock file should open");

        let (started_tx, started_rx) = mpsc::channel();
        let worker_cache_path = cache_path.clone();
        let worker = thread::spawn(move || {
            started_tx.send(()).expect("worker should signal start");
            write_session_cache(Some(&worker_cache_path), second_account, Some(&second_session))
                .expect("second cache write should succeed");
        });

        started_rx.recv().expect("worker start should be observed");
        thread::sleep(Duration::from_millis(100));
        assert!(
            !worker.is_finished(),
            "writer should block while the cache lock is held"
        );

        lock_connection
            .execute_batch("COMMIT")
            .expect("lock should be released");
        worker.join().expect("worker should finish");

        assert_eq!(
            cached_session_on_disk(Some(&cache_path), first_account),
            Some(CachedSession::Token("persisted-session".into()))
        );
        assert_eq!(
            cached_session_on_disk(Some(&cache_path), second_account),
            Some(CachedSession::AppIntegration)
        );
    }

    #[test]
    fn reuses_existing_app_integration_sessions_across_item_lookups() {
        let _guard = match ENV_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let fixture = TempFixtureDir::new();
        let op_script = fixture.write_executable(
            "op",
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SWITCHBOARD_OP_LOG"
case " $* " in
  *" whoami --account my.1password.com "*)
    ;;
  *" --account my.1password.com item get item-one --format json "*)
    cat <<'EOF'
{"fields":[
  {"label":"credential","value":"first-secret"}
]}
EOF
    ;;
  *" --account my.1password.com item get item-two --format json "*)
    cat <<'EOF'
{"fields":[
  {"label":"credential","value":"second-secret"}
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
        let _op_session_guard = EnvVarGuard::remove("OP_SESSION");

        let backend = OnePasswordSecretBackend::default();
        let first_secret = ResolvedSecret::new(
            "first_secret",
            SecretSource::OnePasswordItem {
                account: "my.1password.com".into(),
                vault: None,
                item: "item-one".into(),
                field: "credential".into(),
            },
        )
        .expect("secret should build");
        let second_secret = ResolvedSecret::new(
            "second_secret",
            SecretSource::OnePasswordItem {
                account: "my.1password.com".into(),
                vault: None,
                item: "item-two".into(),
                field: "credential".into(),
            },
        )
        .expect("secret should build");

        let first = backend.resolve(&first_secret).expect("first secret should resolve");
        let second = backend.resolve(&second_secret).expect("second secret should resolve");

        assert_eq!(first.expose(), "first-secret");
        assert_eq!(second.expose(), "second-secret");

        let log = fs::read_to_string(&log_path).expect("log should be readable");
        assert_eq!(
            log.lines()
                .filter(|line| *line == "whoami --account my.1password.com")
                .count(),
            1
        );
        assert_eq!(log.lines().filter(|line| line.contains("signin --account")).count(), 0);
        assert_eq!(log.lines().filter(|line| line.contains("item get")).count(), 2);
    }

    #[test]
    fn resolves_items_via_app_integration_when_signin_returns_no_session_token() {
        let _guard = match ENV_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let fixture = TempFixtureDir::new();
        let op_script = fixture.write_executable(
            "op",
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SWITCHBOARD_OP_LOG"
case " $* " in
  *" whoami --account my.1password.com "*)
    if [ -f "$SWITCHBOARD_APP_SIGNED_IN" ]; then
      exit 0
    fi
    exit 1
    ;;
  *" signin --account my.1password.com --raw "*)
    : > "$SWITCHBOARD_APP_SIGNED_IN"
    ;;
  *" --account my.1password.com item get item-one --format json "*)
    cat <<'EOF'
{"fields":[
  {"label":"credential","value":"first-secret"}
]}
EOF
    ;;
  *" --account my.1password.com item get item-two --format json "*)
    cat <<'EOF'
{"fields":[
  {"label":"credential","value":"second-secret"}
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
        let signed_in_path = fixture.path.join("signed-in");
        let log_path = fixture.path.join("op.log");
        fs::write(&log_path, "").expect("log file should exist");

        let _op_bin_guard = EnvVarGuard::set("SWITCHBOARD_OP_BIN", op_script.into_os_string());
        let _op_log_guard = EnvVarGuard::set("SWITCHBOARD_OP_LOG", log_path.clone().into_os_string());
        let _signed_in_guard = EnvVarGuard::set("SWITCHBOARD_APP_SIGNED_IN", signed_in_path.clone().into_os_string());
        let _op_session_guard = EnvVarGuard::remove("OP_SESSION");

        let backend = OnePasswordSecretBackend::default();
        let first_secret = ResolvedSecret::new(
            "google_personal_client_secret_one",
            SecretSource::OnePasswordItem {
                account: "my.1password.com".into(),
                vault: None,
                item: "item-one".into(),
                field: "credential".into(),
            },
        )
        .expect("secret should build");
        let second_secret = ResolvedSecret::new(
            "google_personal_client_secret_two",
            SecretSource::OnePasswordItem {
                account: "my.1password.com".into(),
                vault: None,
                item: "item-two".into(),
                field: "credential".into(),
            },
        )
        .expect("secret should build");

        let first = backend.resolve(&first_secret).expect("first secret should resolve");
        let second = backend.resolve(&second_secret).expect("second secret should resolve");

        assert_eq!(first.expose(), "first-secret");
        assert_eq!(second.expose(), "second-secret");

        let log = fs::read_to_string(&log_path).expect("log should be readable");
        assert_eq!(log.lines().filter(|line| line.contains("signin --account")).count(), 1);
        assert!(
            log.lines()
                .filter(|line| *line == "whoami --account my.1password.com")
                .count()
                == 2,
            "expected one whoami probe before app sign-in and one after it"
        );
        assert!(
            log.lines()
                .filter(|line| line.contains("--account my.1password.com item get"))
                .count()
                == 2,
            "expected both item lookups to run without a session token"
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
