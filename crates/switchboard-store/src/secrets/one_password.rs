use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::Connection;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use switchboard_core::{Error, ResolvedAuth, ResolvedSecret, Result, SecretRef, SecretSource, SecretString};

use crate::{
    one_password_config::{has_environment_session, has_external_auth},
    secrets::{env_secret::normalize_secret, SecretBackend},
    OnePasswordAuthMode, OnePasswordConfig,
};
use switchboard_core::process::output_with_timeout;

pub(super) struct OnePasswordSecretBackend {
    sessions: Mutex<BTreeMap<String, CachedSession>>,
    items: Mutex<BTreeMap<OnePasswordItemKey, BTreeMap<String, SecretString>>>,
    session_cache_path: Option<PathBuf>,
    item_cache_path: Option<PathBuf>,
    config: OnePasswordConfig,
    recovery: crate::secrets::recovery::RecoveryBudget,
}

impl Default for OnePasswordSecretBackend {
    fn default() -> Self {
        Self::new(None)
    }
}

impl OnePasswordSecretBackend {
    pub(super) fn new(session_cache_path: Option<PathBuf>) -> Self {
        Self::with_config(session_cache_path, OnePasswordConfig::default())
    }

    pub(super) fn with_config(session_cache_path: Option<PathBuf>, config: OnePasswordConfig) -> Self {
        Self::with_recovery_budget(session_cache_path, config, None)
    }

    pub(super) fn with_recovery_budget(
        session_cache_path: Option<PathBuf>,
        config: OnePasswordConfig,
        run_id: Option<String>,
    ) -> Self {
        let recovery = crate::secrets::recovery::RecoveryBudget::new(session_cache_path.as_deref(), run_id);
        let item_cache_path = item_cache_path(session_cache_path.as_deref());
        Self {
            sessions: Mutex::new(BTreeMap::new()),
            items: Mutex::new(BTreeMap::new()),
            session_cache_path,
            item_cache_path,
            config,
            recovery,
        }
    }
}

impl SecretBackend for OnePasswordSecretBackend {
    fn invalidate(&self, secret: &ResolvedSecret) -> Result<bool> {
        let SecretSource::OnePasswordItem {
            account, item, vault, ..
        } = &secret.source
        else {
            return Ok(false);
        };
        let key = OnePasswordItemKey::new(account, vault.as_deref(), item);
        let mut items = self
            .items
            .lock()
            .map_err(|_| Error::Config("credential cache lock was poisoned".into()))?;
        let Some(observed) = items.remove(&key) else {
            return Ok(false);
        };
        drop(items);
        // Compare the values this resolver actually used, not a later disk read:
        // another process may already have replaced the rejected credential.
        let observed = observed
            .into_iter()
            .map(|(field, value)| (field, value.expose().to_owned()))
            .collect::<BTreeMap<_, _>>();
        update_cache::<ItemCacheFile>(self.item_cache_path.as_deref(), |cache| {
            if cache
                .items
                .get(&key.cache_key())
                .is_some_and(|entry| entry.fields == observed)
            {
                cache.items.remove(&key.cache_key());
            }
        })
        .map_err(|error| Error::Config(format!("could not invalidate rejected credential cache: {error}")))?;
        Ok(true)
    }

    fn resolve_for_auth(&self, secret: &ResolvedSecret, auth: &ResolvedAuth) -> Result<SecretString> {
        let SecretSource::OnePasswordItem {
            account,
            item,
            field,
            vault,
        } = &secret.source
        else {
            return self.resolve(secret);
        };
        let key = OnePasswordItemKey::new(account, vault.as_deref(), item);
        if let Some(value) = cached_item_field(&self.items, &key, field) {
            return Ok(value);
        }
        if let Some(fields) = cached_item_fields_on_disk(self.item_cache_path.as_deref(), &key) {
            cache_item_fields(&self.items, &key, &fields);
            if let Some(value) = fields.get(field) {
                return Ok(value.clone());
            }
        }
        // Token/session access is tried with desktop prompts explicitly disabled.
        // A cached AppIntegration marker is not proof that the app remains unlocked.
        let session = env_session().or_else(|| {
            match cached_session(&self.sessions, account)
                .or_else(|| cached_session_on_disk(self.session_cache_path.as_deref(), account))
            {
                Some(CachedSession::Token(token)) => Some(token),
                _ => None,
            }
        });
        let args = item_json_args(account, vault.as_deref(), item);
        let mut config = self.config.clone();
        config.timeout_seconds = config.timeout_seconds.min(60);
        let run_item = |interactive: bool, timeout: Duration| -> Result<String> {
            let mut command = op_command(&config);
            command.env(
                "OP_BIOMETRIC_UNLOCK_ENABLED",
                if interactive { "true" } else { "false" },
            );
            if let Some(session) = session.as_deref().filter(|_| !interactive) {
                command.args(["--session", session]);
            }
            command.args(&args);
            let output = output_with_timeout(&mut command, timeout).map_err(|error| {
                if error.kind() == std::io::ErrorKind::TimedOut {
                    Error::AuthenticationTimeout {
                        seconds: config.timeout_seconds,
                    }
                } else {
                    Error::SecretResolution {
                        secret_ref: secret.id.to_string(),
                        reason: format!("1Password credential lookup failed: {error}"),
                    }
                }
            })?;
            if !output.status.success() {
                return Err(Error::SecretResolution {
                    secret_ref: secret.id.to_string(),
                    reason: format!(
                        "1Password credential lookup failed: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                });
            }
            String::from_utf8(output.stdout).map_err(|_| Error::SecretResolution {
                secret_ref: secret.id.to_string(),
                reason: "1Password returned non-UTF-8 output".into(),
            })
        };
        let mut recovery_attempt = None;
        let output = match run_item(false, config.timeout()) {
            Ok(output) => output,
            Err(error)
                if has_external_auth()
                    || matches!(
                        config.auth_mode,
                        OnePasswordAuthMode::ServiceAccount | OnePasswordAuthMode::Session
                    )
                    || env::var("OP_BIOMETRIC_UNLOCK_ENABLED")
                        .is_ok_and(|value| value.eq_ignore_ascii_case("false")) =>
            {
                return Err(error)
            }
            Err(_) => {
                let attempt = match self.recovery.claim(auth) {
                    Ok(attempt) => attempt,
                    Err(error @ Error::RecoveryExhausted(_)) => {
                        // A concurrent owner may have just published the credential.
                        if let Some(fields) = cached_item_fields_on_disk(self.item_cache_path.as_deref(), &key) {
                            cache_item_fields(&self.items, &key, &fields);
                            if let Some(value) = fields.get(field) {
                                return Ok(value.clone());
                            }
                        }
                        return Err(error);
                    }
                    Err(error) => return Err(error),
                };
                // One command owns the entire human-presence attempt. No whoami,
                // signin, or per-field retry can trigger another unlock afterward.
                let timeout = attempt.remaining_timeout(config.timeout())?;
                recovery_attempt = Some(attempt);
                run_item(true, timeout)?
            }
        };
        let ItemLookup::Fields(fields) = parse_item_fields(&output) else {
            return Err(Error::SecretResolution {
                secret_ref: secret.id.to_string(),
                reason: "1Password item response was not valid JSON; no interactive retry was attempted".into(),
            });
        };
        cache_item_fields(&self.items, &key, &fields);
        warn_cache_write(
            "item",
            write_item_cache_entry(self.item_cache_path.as_deref(), &key, Some(&fields)),
        );
        drop(recovery_attempt);
        fields.get(field).cloned().ok_or_else(|| Error::SecretResolution {
            secret_ref: secret.id.to_string(),
            reason: format!(
                "1Password item did not contain configured field {field:?}; no additional unlock was attempted"
            ),
        })
    }

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

        if let Some(fields) = cached_item_fields_on_disk(self.item_cache_path.as_deref(), &item_key) {
            cache_item_fields(&self.items, &item_key, &fields);
            if let Some(value) = fields.get(field) {
                return Ok(value.clone());
            }
        }

        let session = ensure_session(
            &secret.id,
            &self.sessions,
            self.session_cache_path.as_deref(),
            account,
            &self.config,
        )?;
        let config = session.command_config(&self.config);
        match fetch_item_fields(&secret.id, &item_key, session.token(), &config)? {
            ItemLookup::Fields(fields) => {
                cache_item_fields(&self.items, &item_key, &fields);
                warn_cache_write(
                    "item",
                    write_item_cache_entry(self.item_cache_path.as_deref(), &item_key, Some(&fields)),
                );
                if let Some(value) = fields.get(field) {
                    return Ok(value.clone());
                }
            }
            ItemLookup::ReadField => {}
        }

        let args = item_args(account, vault.as_deref(), item, field);
        let output = run(&secret.id, &args, session.token(), &config)?;
        let value = normalize_secret(&secret.id, output)?;
        cache_item_field(&self.items, &item_key, field, &value);
        let mut fields = BTreeMap::new();
        fields.insert(field.to_owned(), value.clone());
        warn_cache_write(
            "item",
            write_item_cache_entry(self.item_cache_path.as_deref(), &item_key, Some(&fields)),
        );
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

    fn cache_key(&self) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}",
            self.account,
            self.vault.as_deref().unwrap_or_default(),
            self.item
        )
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

#[derive(Debug, Default, Deserialize, Serialize)]
struct ItemCacheFile {
    #[serde(default)]
    items: BTreeMap<String, PersistedItemFields>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PersistedItemFields {
    expires_at_epoch_seconds: u64,
    #[serde(default)]
    fields: BTreeMap<String, String>,
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
enum SessionHandle {
    Token(String),
    NativeSession,
    #[default]
    CliDefault,
}

impl SessionHandle {
    fn token(&self) -> Option<&str> {
        match self {
            Self::Token(token) => Some(token),
            Self::NativeSession | Self::CliDefault => None,
        }
    }

    fn command_config(&self, configured: &OnePasswordConfig) -> OnePasswordConfig {
        let mut config = configured.clone();
        if *self == Self::NativeSession {
            config.auth_mode = OnePasswordAuthMode::Session;
        }
        config
    }
}

// JSON decoding can request a direct field lookup. CLI execution failures must
// propagate without retrying or relying on a human-facing error message.
enum ItemLookup {
    Fields(BTreeMap<String, SecretString>),
    ReadField,
}

fn fetch_item_fields(
    secret_ref: &SecretRef,
    item_key: &OnePasswordItemKey,
    session: Option<&str>,
    config: &OnePasswordConfig,
) -> Result<ItemLookup> {
    let args = item_json_args(&item_key.account, item_key.vault.as_deref(), &item_key.item);
    let output = run(secret_ref, &args, session, config)?;
    Ok(parse_item_fields(&output))
}

fn parse_item_fields(output: &str) -> ItemLookup {
    let Ok(item) = serde_json::from_str::<OnePasswordItem>(output) else {
        return ItemLookup::ReadField;
    };

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

    ItemLookup::Fields(fields)
}

fn ensure_session(
    secret_ref: &SecretRef,
    sessions: &Mutex<BTreeMap<String, CachedSession>>,
    session_cache_path: Option<&Path>,
    account: &str,
    config: &OnePasswordConfig,
) -> Result<SessionHandle> {
    if has_external_auth() {
        return Ok(SessionHandle::default());
    }
    if let Some(token) = env_session() {
        if cached_session(sessions, account) == Some(CachedSession::Token(token.clone())) {
            return Ok(SessionHandle::Token(token));
        }
        if whoami(account, Some(&token), config)? {
            cache_token_session(sessions, session_cache_path, account, &token);
            return Ok(SessionHandle::Token(token));
        }
        return Err(Error::SecretResolution {
            secret_ref: secret_ref.to_string(),
            reason: "OP_SESSION was supplied but is no longer valid; sign in again and refresh the session".into(),
        });
    }
    // Account-specific OP_SESSION_* names belong to op's registry. Probe the
    // requested account before preferring them over that account's cached login.
    if has_environment_session() {
        let native_config = SessionHandle::NativeSession.command_config(config);
        if whoami(account, None, &native_config)? {
            return Ok(SessionHandle::NativeSession);
        }
    }
    if config.auth_mode == OnePasswordAuthMode::ServiceAccount {
        return Err(Error::Config(
            "one_password.auth_mode = service_account requires OP_SERVICE_ACCOUNT_TOKEN".into(),
        ));
    }
    if let Some(session) = cached_session(sessions, account) {
        return match session {
            CachedSession::Token(token) => Ok(SessionHandle::Token(token)),
            CachedSession::AppIntegration => Ok(SessionHandle::CliDefault),
        };
    }

    if let Some(session) = cached_session_on_disk(session_cache_path, account) {
        match session {
            CachedSession::Token(token) => {
                if whoami(account, Some(&token), config)? {
                    cache_token_session(sessions, session_cache_path, account, &token);
                    return Ok(SessionHandle::Token(token));
                }
                forget_session(sessions, session_cache_path, account, &CachedSession::Token(token));
            }
            CachedSession::AppIntegration => {
                if let Some(token) = sign_in(secret_ref, account, config)? {
                    cache_token_session(sessions, session_cache_path, account, &token);
                    return Ok(SessionHandle::Token(token));
                }

                if whoami(account, None, config)? {
                    cache_app_session(sessions, session_cache_path, account);
                    return Ok(SessionHandle::CliDefault);
                }

                forget_session(sessions, session_cache_path, account, &CachedSession::AppIntegration);
            }
        }
    }

    if whoami(account, None, config)? {
        cache_app_session(sessions, session_cache_path, account);
        return Ok(SessionHandle::CliDefault);
    }

    let token = sign_in(secret_ref, account, config)?;
    if let Some(token) = token {
        cache_token_session(sessions, session_cache_path, account, &token);
        return Ok(SessionHandle::Token(token));
    }

    if whoami(account, None, config)? {
        cache_app_session(sessions, session_cache_path, account);
        return Ok(SessionHandle::CliDefault);
    }

    Err(Error::SecretResolution {
        secret_ref: secret_ref.to_string(),
        reason: format!(
            "`op signin --account {account} --raw` succeeded but did not authenticate the CLI for that account"
        ),
    })
}

fn sign_in(secret_ref: &SecretRef, account: &str, config: &OnePasswordConfig) -> Result<Option<String>> {
    let mut command = op_command(config);
    command.args(["signin", "--account", account, "--raw"]);
    let output = capture_op(&mut command, config).map_err(|error| Error::SecretResolution {
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

fn whoami(account: &str, session: Option<&str>, config: &OnePasswordConfig) -> Result<bool> {
    let mut command = op_command(config);
    if let Some(session) = session {
        command.args(["--session", session]);
    }
    command.args(["whoami", "--account", account]);

    let output = capture_op(&mut command, config)
        .map_err(|error| Error::Config(format!("failed to run `op whoami --account {account}`: {error}")))?;

    Ok(output.status.success())
}

fn run(secret_ref: &SecretRef, args: &[String], session: Option<&str>, config: &OnePasswordConfig) -> Result<String> {
    let mut command = op_command(config);
    if let Some(session) = session {
        command.args(["--session", session]);
    }
    command.args(args);

    let output = capture_op(&mut command, config).map_err(|error| Error::SecretResolution {
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

fn op_command(config: &OnePasswordConfig) -> Command {
    let mut command = match env::var_os("SWITCHBOARD_OP_BIN") {
        Some(path) => Command::new(path),
        None => Command::new("op"),
    };
    command.env_remove("OP_ACCOUNT");
    command.env_remove("OP_SESSION");
    if let Some(enabled) = config.desktop_integration() {
        command.env("OP_BIOMETRIC_UNLOCK_ENABLED", if enabled { "true" } else { "false" });
    }
    command
}

fn capture_op(command: &mut Command, config: &OnePasswordConfig) -> std::io::Result<std::process::Output> {
    output_with_timeout(command, config.timeout()).map_err(|error| {
        if error.kind() == std::io::ErrorKind::TimedOut {
            std::io::Error::new(error.kind(), format!(
                "1Password timed out after {} seconds; unlock the 1Password app and enable Settings > Developer > Integrate with 1Password CLI, or sign in to op in a terminal. Configure one_password.auth_mode and one_password.timeout_seconds for this environment",
                config.timeout_seconds
            ))
        } else {
            error
        }
    })
}

fn warn_cache_write(kind: &str, result: std::io::Result<()>) {
    if let Err(error) = result {
        eprintln!("warning: could not save 1Password {kind} cache ({:?}); authentication may repeat. Check the Switchboard state directory permissions with switchboard doctor", error.kind());
    }
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
    warn_cache_write("session", write_session_cache(session_cache_path, account, &session));
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
    warn_cache_write("session", write_session_cache(session_cache_path, account, &session));
}

fn forget_session(
    sessions: &Mutex<BTreeMap<String, CachedSession>>,
    session_cache_path: Option<&Path>,
    account: &str,
    observed: &CachedSession,
) {
    {
        let mut sessions = sessions.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if sessions.get(account) == Some(observed) {
            sessions.remove(account);
        }
    }

    warn_cache_write(
        "session",
        update_cache(session_cache_path, |cache: &mut SessionCacheFile| {
            let current = cache
                .sessions
                .get(account)
                .cloned()
                .and_then(PersistedSession::into_cached);
            if current.as_ref() == Some(observed) {
                cache.sessions.remove(account);
            }
        }),
    );
}

fn cached_session_on_disk(session_cache_path: Option<&Path>, account: &str) -> Option<CachedSession> {
    let cache = read_session_cache(session_cache_path?)?;
    cache.sessions.get(account).cloned()?.into_cached()
}

fn cached_item_fields_on_disk(
    item_cache_path: Option<&Path>,
    item_key: &OnePasswordItemKey,
) -> Option<BTreeMap<String, SecretString>> {
    let item_cache_path = item_cache_path?;
    let cache = read_item_cache(item_cache_path)?;
    let entry = cache.items.get(&item_key.cache_key())?;
    if entry.expires_at_epoch_seconds <= unix_timestamp_now() {
        warn_cache_write("item", write_item_cache_entry(Some(item_cache_path), item_key, None));
        return None;
    }

    Some(
        entry
            .fields
            .iter()
            .map(|(field, value)| (field.clone(), SecretString::from(value.clone())))
            .collect(),
    )
}

fn read_session_cache(path: &Path) -> Option<SessionCacheFile> {
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Validate the persisted session format and return only its entry count.
pub fn one_password_session_cache_entry_count(contents: &str) -> Option<usize> {
    serde_json::from_str::<SessionCacheFile>(contents)
        .ok()
        .map(|cache| cache.sessions.len())
}

/// Validate the persisted item format and return only expiration timestamps.
pub fn one_password_item_cache_expiries(contents: &str) -> Option<Vec<u64>> {
    serde_json::from_str::<ItemCacheFile>(contents).ok().map(|cache| {
        cache
            .items
            .into_values()
            .map(|entry| entry.expires_at_epoch_seconds)
            .collect()
    })
}

fn read_item_cache(path: &Path) -> Option<ItemCacheFile> {
    let contents = fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

fn cache_lock_path(path: &Path) -> PathBuf {
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock.sqlite3");
    PathBuf::from(lock_path)
}

fn open_cache_lock(path: &Path) -> std::io::Result<Connection> {
    let lock_path = cache_lock_path(path);
    let connection = Connection::open(lock_path).map_err(std::io::Error::other)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(std::io::Error::other)?;
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(std::io::Error::other)?;
    Ok(connection)
}

#[cfg(test)]
fn open_session_cache_lock(path: &Path) -> std::io::Result<Connection> {
    open_cache_lock(path)
}

/// Read, check, and update the latest snapshot while holding the cross-process
/// lock. A snapshot read before acquiring this lock cannot authorize deletion.
fn update_cache<T>(path: Option<&Path>, update: impl FnOnce(&mut T)) -> std::io::Result<()>
where
    T: Default + DeserializeOwned + Serialize,
{
    let Some(path) = path else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock_connection = open_cache_lock(path)?;
    let mut cache = fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default();
    update(&mut cache);

    let serialized = serde_json::to_vec(&cache).map_err(std::io::Error::other)?;
    let temp_path = path.with_extension("tmp");
    fs::write(&temp_path, serialized)?;
    set_owner_only_permissions(&temp_path)?;
    fs::rename(&temp_path, path)?;
    set_owner_only_permissions(path)?;
    lock_connection.execute_batch("COMMIT").map_err(std::io::Error::other)
}

fn write_session_cache(
    session_cache_path: Option<&Path>,
    account: &str,
    session: &CachedSession,
) -> std::io::Result<()> {
    let entry = PersistedSession::from_cached(session).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "cannot save an empty 1Password session",
        )
    })?;
    update_cache(session_cache_path, |cache: &mut SessionCacheFile| {
        cache.sessions.insert(account.to_owned(), entry);
    })
}

fn write_item_cache_entry(
    item_cache_path: Option<&Path>,
    item_key: &OnePasswordItemKey,
    fields: Option<&BTreeMap<String, SecretString>>,
) -> std::io::Result<()> {
    update_cache(item_cache_path, |cache: &mut ItemCacheFile| {
        prune_expired_item_cache_entries(cache);
        match fields {
            Some(fields) if !fields.is_empty() => {
                cache.items.insert(
                    item_key.cache_key(),
                    PersistedItemFields {
                        expires_at_epoch_seconds: unix_timestamp_now() + one_password_item_cache_ttl().as_secs(),
                        fields: fields
                            .iter()
                            .map(|(field, value)| (field.clone(), value.expose().to_owned()))
                            .collect(),
                    },
                );
            }
            Some(_) => {
                cache.items.remove(&item_key.cache_key());
            }
            // An expired reader only requests pruning. A concurrent writer may
            // already have refreshed this key while that reader waited for the lock.
            None => {}
        }
    })
}

fn prune_expired_item_cache_entries(cache: &mut ItemCacheFile) {
    let now = unix_timestamp_now();
    cache.items.retain(|_, entry| entry.expires_at_epoch_seconds > now);
}

fn item_cache_path(session_cache_path: Option<&Path>) -> Option<PathBuf> {
    let session_cache_path = session_cache_path?;
    let parent = session_cache_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    Some(parent.join("onepassword-items.json"))
}

fn one_password_item_cache_ttl() -> Duration {
    Duration::from_secs(60 * 60)
}

fn unix_timestamp_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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
        OnePasswordItemKey, OnePasswordSecretBackend, SecretBackend,
    };

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    static TEMP_FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn expired_item_observation_cannot_delete_a_concurrent_refresh() {
        let fixture = TempFixtureDir::new();
        let path = fixture.path.join("onepassword-items.json");
        let key = OnePasswordItemKey::new("account", None, "item");
        let expired = super::ItemCacheFile {
            items: std::collections::BTreeMap::from([(
                key.cache_key(),
                super::PersistedItemFields {
                    expires_at_epoch_seconds: 1,
                    fields: std::collections::BTreeMap::from([("credential".into(), "old-value".into())]),
                },
            )]),
        };
        fs::write(&path, serde_json::to_vec(&expired).expect("cache serializes")).expect("expired entry is saved");
        let observed = super::read_item_cache(&path).expect("reader observes the old cache");
        assert!(observed
            .items
            .values()
            .all(|entry| entry.expires_at_epoch_seconds <= super::unix_timestamp_now()));

        let replacement = std::collections::BTreeMap::from([(
            "credential".into(),
            switchboard_core::SecretString::from("fresh-value".to_owned()),
        )]);
        super::write_item_cache_entry(Some(&path), &key, Some(&replacement))
            .expect("another writer refreshes the entry");
        // Resume the expired reader's cleanup after the second writer committed.
        super::write_item_cache_entry(Some(&path), &key, None).expect("stale cleanup succeeds");

        let current = super::cached_item_fields_on_disk(Some(&path), &key).expect("fresh entry survives cleanup");
        assert_eq!(
            current.get("credential").expect("credential remains").expose(),
            "fresh-value"
        );
    }

    #[cfg(unix)]
    #[test]
    fn invalid_session_observation_cannot_delete_a_concurrent_replacement() {
        let _guard = ENV_LOCK.lock().expect("environment lock");
        let fixture = TempFixtureDir::new();
        let path = fixture.path.join("onepassword-sessions.json");
        let observed_path = fixture.path.join("observed");
        let continue_path = fixture.path.join("continue");
        let command = fixture.write_executable(
            "op",
            r#"#!/bin/sh
case "$*" in
  "--session observed-session whoami --account test.1password.com")
    : > "$(dirname "$0")/observed"
    while [ ! -e "$(dirname "$0")/continue" ]; do sleep 0.01; done
    ;;
esac
exit 1
"#,
        );
        let _op = EnvVarGuard::set("SWITCHBOARD_OP_BIN", command.into_os_string());
        let _generic = EnvVarGuard::remove("OP_SESSION");
        let _service = EnvVarGuard::remove("OP_SERVICE_ACCOUNT_TOKEN");
        let _connect_host = EnvVarGuard::remove("OP_CONNECT_HOST");
        let _connect_token = EnvVarGuard::remove("OP_CONNECT_TOKEN");
        let initial_sessions = Mutex::new(std::collections::BTreeMap::new());
        super::cache_token_session(&initial_sessions, Some(&path), "test.1password.com", "observed-session");
        let sessions = std::sync::Arc::new(Mutex::new(std::collections::BTreeMap::new()));
        let worker_sessions = std::sync::Arc::clone(&sessions);
        let worker_path = path.clone();
        let worker = thread::spawn(move || {
            let secret_ref = switchboard_core::SecretRef::new("secret").expect("secret ref builds");
            let config = crate::OnePasswordConfig {
                timeout_seconds: 5,
                ..crate::OnePasswordConfig::default()
            };
            super::ensure_session(
                &secret_ref,
                &worker_sessions,
                Some(&worker_path),
                "test.1password.com",
                &config,
            )
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !observed_path.exists() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let observed = observed_path.exists();
        super::cache_token_session(&sessions, Some(&path), "test.1password.com", "replacement-session");
        fs::write(&continue_path, "").expect("release the failed validation subprocess");
        assert!(worker.join().expect("validation worker completes").is_err());
        assert!(
            observed,
            "the validator must observe the old session before replacement"
        );

        let replacement = Some(CachedSession::Token("replacement-session".into()));
        assert_eq!(cached_session_on_disk(Some(&path), "test.1password.com"), replacement);
        assert_eq!(super::cached_session(&sessions, "test.1password.com"), replacement);
    }

    #[test]
    fn invalid_session_is_removed_only_when_it_still_matches_the_observation() {
        let fixture = TempFixtureDir::new();
        let path = fixture.path.join("sessions.json");
        let sessions = Mutex::new(std::collections::BTreeMap::new());
        super::cache_token_session(&sessions, Some(&path), "account", "invalid-session");
        super::cache_token_session(&sessions, Some(&path), "other-account", "other-session");
        super::forget_session(
            &sessions,
            Some(&path),
            "account",
            &CachedSession::Token("invalid-session".into()),
        );
        assert_eq!(super::cached_session(&sessions, "account"), None);
        assert_eq!(cached_session_on_disk(Some(&path), "account"), None);
        assert_eq!(
            cached_session_on_disk(Some(&path), "other-account"),
            Some(CachedSession::Token("other-session".into()))
        );
    }

    #[test]
    fn item_json_decoder_preserves_labels_ids_and_scalar_values() {
        let output = r#"{"fields":[
            {"id":"username","label":"login","value":"alice"},
            {"label":"number","value":42},
            {"label":"enabled","value":true},
            {"label":"ignored","value":[]}
        ]}"#;
        let super::ItemLookup::Fields(fields) = super::parse_item_fields(output) else {
            panic!("valid item JSON must provide fields");
        };
        let values = fields
            .iter()
            .map(|(key, value)| (key.as_str(), value.expose()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            values,
            std::collections::BTreeMap::from([
                ("username", "alice"),
                ("login", "alice"),
                ("number", "42"),
                ("enabled", "true")
            ])
        );
    }

    #[test]
    fn item_json_decoder_requests_direct_lookup_for_invalid_json_or_field_shape() {
        for output in ["not JSON", r#"{"fields":"unexpected shape"}"#] {
            assert!(matches!(super::parse_item_fields(output), super::ItemLookup::ReadField));
        }
    }

    #[cfg(unix)]
    #[test]
    fn item_process_failure_is_not_treated_as_a_json_decode_fallback() {
        let _guard = ENV_LOCK.lock().expect("environment lock");
        let _op = EnvVarGuard::set("SWITCHBOARD_OP_BIN", "/usr/bin/false".into());
        let secret_ref = switchboard_core::SecretRef::new("secret").expect("secret ref builds");
        let key = OnePasswordItemKey::new("account", None, "item");
        assert!(matches!(
            super::fetch_item_fields(&secret_ref, &key, None, &crate::OnePasswordConfig::default()),
            Err(switchboard_core::Error::SecretResolution { .. })
        ));
    }

    #[test]
    fn cache_write_failure_keeps_the_successful_in_memory_session() {
        let fixture = TempFixtureDir::new();
        let blocked_parent = fixture.path.join("not-a-directory");
        fs::write(&blocked_parent, "occupied").expect("create file blocking cache directory");
        let sessions = Mutex::new(std::collections::BTreeMap::new());
        super::cache_token_session(
            &sessions,
            Some(&blocked_parent.join("sessions.json")),
            "account",
            "session-value",
        );
        assert_eq!(
            super::cached_session(&sessions, "account"),
            Some(CachedSession::Token("session-value".into()))
        );
        assert!(!blocked_parent.join("sessions.json").exists());
    }

    #[test]
    fn unrelated_account_environment_session_preserves_target_cached_session() {
        let _guard = ENV_LOCK.lock().expect("environment lock");
        let _generic = EnvVarGuard::remove("OP_SESSION");
        let _personal = EnvVarGuard::set("OP_SESSION_PERSONAL", "personal-session".into());
        let _service = EnvVarGuard::remove("OP_SERVICE_ACCOUNT_TOKEN");
        let _connect_host = EnvVarGuard::remove("OP_CONNECT_HOST");
        let _connect_token = EnvVarGuard::remove("OP_CONNECT_TOKEN");
        // A failed native-account probe must not displace a valid target session.
        let _op = EnvVarGuard::set("SWITCHBOARD_OP_BIN", "/usr/bin/false".into());
        let sessions = Mutex::new(std::collections::BTreeMap::from([(
            "work.1password.com".into(),
            CachedSession::Token("work-session".into()),
        )]));
        let secret_ref = switchboard_core::SecretRef::new("secret").expect("secret ref builds");
        let session = super::ensure_session(
            &secret_ref,
            &sessions,
            None,
            "work.1password.com",
            &crate::OnePasswordConfig::default(),
        )
        .expect("cached target session remains usable");
        assert_eq!(session.token(), Some("work-session"));
    }

    #[test]
    fn external_authentication_uses_the_callers_environment_without_cached_sessions() {
        let _guard = ENV_LOCK.lock().expect("environment lock");
        let _service_account = EnvVarGuard::set("OP_SERVICE_ACCOUNT_TOKEN", "service-account-placeholder".into());
        let sessions = Mutex::new(std::collections::BTreeMap::from([(
            "account".into(),
            CachedSession::Token("old-session".into()),
        )]));
        let secret_ref = switchboard_core::SecretRef::new("secret").expect("secret ref builds");
        let session = super::ensure_session(
            &secret_ref,
            &sessions,
            None,
            "account",
            &crate::OnePasswordConfig::default(),
        )
        .expect("external authentication bypasses cached personal session");
        assert_eq!(session.token(), None);
    }

    #[test]
    fn explicit_desktop_environment_is_preserved_in_child_command() {
        let _guard = ENV_LOCK.lock().expect("environment lock");
        let _desktop = EnvVarGuard::set("OP_BIOMETRIC_UNLOCK_ENABLED", "false".into());
        let config = crate::OnePasswordConfig {
            auth_mode: crate::OnePasswordAuthMode::Desktop,
            ..crate::OnePasswordConfig::default()
        };
        let command = super::op_command(&config);
        assert!(!command
            .get_envs()
            .any(|(name, _)| name == "OP_BIOMETRIC_UNLOCK_ENABLED"));
    }

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
  *" --session persisted-session --account my.1password.com item get item-one --format json "*)
    cat <<'EOF'
{"fields":[
  {"label":"credential","value":"first-secret"}
]}
EOF
    ;;
  *" --session persisted-session --account my.1password.com item get item-two --format json "*)
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

        let first = OnePasswordSecretBackend::new(Some(cache_path.clone()));
        let second = OnePasswordSecretBackend::new(Some(cache_path.clone()));

        let first_value = first.resolve(&first_secret).expect("first backend should resolve");
        let second_value = second.resolve(&second_secret).expect("second backend should resolve");

        assert_eq!(first_value.expose(), "first-secret");
        assert_eq!(second_value.expose(), "second-secret");

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
    fn persists_item_cache_across_backend_instances_and_skips_second_op_call() {
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
  *" --session session-token --account my.1password.com item get gws cli --format json "*)
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
        let item_cache_path = fixture.path.join("onepassword-items.json");
        let log_path = fixture.path.join("op.log");
        fs::write(&log_path, "").expect("log file should exist");

        let _op_bin_guard = EnvVarGuard::set("SWITCHBOARD_OP_BIN", op_script.into_os_string());
        let _op_log_guard = EnvVarGuard::set("SWITCHBOARD_OP_LOG", log_path.clone().into_os_string());
        let _op_session_guard = EnvVarGuard::set("OP_SESSION", "session-token".into());

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
        let second = OnePasswordSecretBackend::new(Some(cache_path));

        let first_value = first.resolve(&secret).expect("first backend should resolve");
        let second_value = second
            .resolve(&secret)
            .expect("second backend should resolve from disk cache");

        assert_eq!(first_value.expose(), "personal-client-secret");
        assert_eq!(second_value.expose(), "personal-client-secret");
        assert!(item_cache_path.exists(), "item cache should be written");

        let log = fs::read_to_string(&log_path).expect("log should be readable");
        assert_eq!(
            log.lines()
                .filter(|line| line.contains("--session session-token whoami --account my.1password.com"))
                .count(),
            1
        );
        assert_eq!(
            log.lines()
                .filter(|line| line.contains("item get gws cli --format json"))
                .count(),
            1
        );
        assert!(
            !fs::read_to_string(&item_cache_path)
                .expect("item cache should remain readable")
                .contains("stale-secret"),
            "expired plaintext cache entry should be pruned on read"
        );
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
    fn ignores_expired_item_cache_entries() {
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
  *" --session session-token --account my.1password.com item get gws cli --format json "*)
    cat <<'EOF'
{"fields":[
  {"label":"credential","value":"fresh-secret"}
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
        let item_cache_path = fixture.path.join("onepassword-items.json");
        let log_path = fixture.path.join("op.log");
        fs::write(&log_path, "").expect("log file should exist");

        let item_key = serde_json::to_string(&OnePasswordItemKey::new("my.1password.com", None, "gws cli").cache_key())
            .expect("item cache key should serialize");
        let item_cache_contents = serde_json::json!({
            "items": {
                serde_json::from_str::<String>(&item_key).expect("item cache key should deserialize"): {
                    "expires_at_epoch_seconds": 1,
                    "fields": {
                        "credential": "stale-secret"
                    }
                }
            }
        });
        fs::write(
            &item_cache_path,
            serde_json::to_vec(&item_cache_contents).expect("item cache fixture should serialize"),
        )
        .expect("expired item cache should exist");

        let _op_bin_guard = EnvVarGuard::set("SWITCHBOARD_OP_BIN", op_script.into_os_string());
        let _op_log_guard = EnvVarGuard::set("SWITCHBOARD_OP_LOG", log_path.clone().into_os_string());
        let _op_session_guard = EnvVarGuard::set("OP_SESSION", "session-token".into());

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

        let value = backend.resolve(&secret).expect("expired disk cache should be ignored");

        assert_eq!(value.expose(), "fresh-secret");
        let log = fs::read_to_string(&log_path).expect("log should be readable");
        assert_eq!(
            log.lines()
                .filter(|line| line.contains("--session session-token whoami --account my.1password.com"))
                .count(),
            1
        );
        assert_eq!(
            log.lines()
                .filter(|line| line.contains("item get gws cli --format json"))
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
  *" signin --account my.1password.com --raw "*)
    ;;
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
            2
        );
        assert_eq!(log.lines().filter(|line| line.contains("signin --account")).count(), 1);
        assert_eq!(
            log.lines()
                .filter(|line| line.contains("--account my.1password.com item get"))
                .count(),
            2
        );
    }

    #[test]
    fn promotes_persisted_app_integration_sessions_to_token_sessions() {
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
  *" --session persisted-session --account my.1password.com item get item-one --format json "*)
    cat <<'EOF'
{"fields":[
  {"label":"credential","value":"promoted-secret"}
]}
EOF
    ;;
  *" --session persisted-session --account my.1password.com item get item-two --format json "*)
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
        fs::write(
            &cache_path,
            r#"{"sessions":{"my.1password.com":{"kind":"app_integration"}}}"#,
        )
        .expect("app integration cache should exist");

        let _op_bin_guard = EnvVarGuard::set("SWITCHBOARD_OP_BIN", op_script.into_os_string());
        let _op_log_guard = EnvVarGuard::set("SWITCHBOARD_OP_LOG", log_path.clone().into_os_string());
        let _op_session_guard = EnvVarGuard::remove("OP_SESSION");

        let first = OnePasswordSecretBackend::new(Some(cache_path.clone()));
        let second = OnePasswordSecretBackend::new(Some(cache_path.clone()));
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

        let first_value = first.resolve(&first_secret).expect("cached app session should upgrade");
        let second_value = second
            .resolve(&second_secret)
            .expect("promoted token should persist across backend instances");

        assert_eq!(first_value.expose(), "promoted-secret");
        assert_eq!(second_value.expose(), "second-secret");
        let log = fs::read_to_string(&log_path).expect("log should be readable");
        assert_eq!(log.lines().filter(|line| line.contains("signin --account")).count(), 1);
        assert_eq!(
            log.lines()
                .filter(|line| line.contains("--session persisted-session whoami --account my.1password.com"))
                .count(),
            1
        );
        assert_eq!(
            log.lines()
                .filter(|line| line.contains("--session persisted-session --account my.1password.com item get"))
                .count(),
            2
        );
        assert!(fs::read_to_string(&cache_path)
            .expect("cache file should exist")
            .contains("persisted-session"));
    }

    #[test]
    fn stale_app_integration_cache_revalidates_before_item_lookup() {
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
    ;;
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
        assert_eq!(log.lines().filter(|line| line.contains("signin --account")).count(), 1);
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
            1
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

        write_session_cache(Some(&cache_path), first_account, &first_session)
            .expect("first cache write should succeed");

        let lock_connection = open_session_cache_lock(&cache_path).expect("lock file should open");

        let (started_tx, started_rx) = mpsc::channel();
        let worker_cache_path = cache_path.clone();
        let worker = thread::spawn(move || {
            started_tx.send(()).expect("worker should signal start");
            write_session_cache(Some(&worker_cache_path), second_account, &second_session)
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

    #[test]
    fn rejected_credential_does_not_erase_a_concurrent_refresh() {
        let fixture = TempFixtureDir::new();
        let path = fixture.path.join("onepassword-sessions.json");
        let backend = OnePasswordSecretBackend::new(Some(path));
        let key = OnePasswordItemKey::new("account", None, "item");
        let old = std::collections::BTreeMap::from([(
            "credential".into(),
            switchboard_core::SecretString::from("old".to_owned()),
        )]);
        super::cache_item_fields(&backend.items, &key, &old);
        super::write_item_cache_entry(backend.item_cache_path.as_deref(), &key, Some(&old)).expect("old cache saved");
        let replacement = std::collections::BTreeMap::from([(
            "credential".into(),
            switchboard_core::SecretString::from("fresh".to_owned()),
        )]);
        super::write_item_cache_entry(backend.item_cache_path.as_deref(), &key, Some(&replacement))
            .expect("concurrent refresh saved");
        let secret = switchboard_core::ResolvedSecret::new(
            "token",
            switchboard_core::SecretSource::OnePasswordItem {
                account: "account".into(),
                item: "item".into(),
                field: "credential".into(),
                vault: None,
            },
        )
        .expect("secret configured");
        backend
            .invalidate(&secret)
            .expect("old rejected observation invalidated");
        let current =
            super::cached_item_fields_on_disk(backend.item_cache_path.as_deref(), &key).expect("fresh cache survives");
        assert_eq!(current.get("credential").expect("field present").expose(), "fresh");
    }

    #[test]
    fn human_presence_timeout_consumes_the_run_budget() {
        let _lock = ENV_LOCK.lock().expect("environment lock");
        let fixture = TempFixtureDir::new();
        let script = fixture.write_executable(
            "op.sh",
            r#"#!/bin/sh
[ "$OP_BIOMETRIC_UNLOCK_ENABLED" = true ] || exit 1
printf '%s\n' 'unlock' >> "$(dirname "$0")/attempts"
sleep 3
"#,
        );
        let _binary = EnvVarGuard::set("SWITCHBOARD_OP_BIN", script.into_os_string());
        let _desktop = EnvVarGuard::remove("OP_BIOMETRIC_UNLOCK_ENABLED");
        let _service = EnvVarGuard::remove("OP_SERVICE_ACCOUNT_TOKEN");
        let _connect = EnvVarGuard::remove("OP_CONNECT_HOST");
        let _connect_token = EnvVarGuard::remove("OP_CONNECT_TOKEN");
        let cache = fixture.path.join("onepassword-sessions.json");
        let secret = switchboard_core::ResolvedSecret::new(
            "token",
            switchboard_core::SecretSource::OnePasswordItem {
                account: "account".into(),
                item: "item".into(),
                field: "credential".into(),
                vault: None,
            },
        )
        .expect("secret configured");
        let auth = switchboard_core::ResolvedAuth::new(
            "github.personal",
            "example",
            switchboard_core::AuthSecretRefs::GitHubCli,
        )
        .expect("auth configured");
        let config = crate::OnePasswordConfig {
            timeout_seconds: 1,
            ..Default::default()
        };
        let first = OnePasswordSecretBackend::with_recovery_budget(
            Some(cache.clone()),
            config.clone(),
            Some("timeout-run".into()),
        );
        assert!(matches!(
            first.resolve_for_auth(&secret, &auth),
            Err(switchboard_core::Error::AuthenticationTimeout { seconds: 1 })
        ));
        let second = OnePasswordSecretBackend::with_recovery_budget(Some(cache), config, Some("timeout-run".into()));
        assert!(matches!(
            second.resolve_for_auth(&secret, &auth),
            Err(switchboard_core::Error::RecoveryExhausted(_))
        ));
        assert_eq!(
            fs::read_to_string(fixture.path.join("attempts"))
                .expect("attempt log")
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn scoped_recovery_coalesces_and_failed_attempts_are_not_repeated() {
        let _lock = ENV_LOCK.lock().expect("recovery fixture should succeed");
        let fixture = TempFixtureDir::new();
        let script = fixture.write_executable(
            "op.sh",
            r#"#!/bin/sh
printf '%s\n' "$OP_BIOMETRIC_UNLOCK_ENABLED" >> "$(dirname "$0")/attempts"
[ "$OP_BIOMETRIC_UNLOCK_ENABLED" = true ] || exit 1
[ ! -f "$(dirname "$0")/fail" ] || exit 1
sleep 0.2
printf '%s\n' '{"fields":[{"label":"credential","value":"fixture-value"}]}'
"#,
        );
        let _binary = EnvVarGuard::set("SWITCHBOARD_OP_BIN", script.into_os_string());
        let _session = EnvVarGuard::remove("OP_SESSION");
        let _service = EnvVarGuard::remove("OP_SERVICE_ACCOUNT_TOKEN");
        let _connect = EnvVarGuard::remove("OP_CONNECT_HOST");
        let _connect_token = EnvVarGuard::remove("OP_CONNECT_TOKEN");
        let cache = fixture.path.join("onepassword-sessions.json");
        let secret = switchboard_core::ResolvedSecret::new(
            "github-token",
            switchboard_core::SecretSource::OnePasswordItem {
                account: "account".into(),
                item: "item".into(),
                field: "credential".into(),
                vault: None,
            },
        )
        .expect("recovery fixture should succeed");
        let auth = switchboard_core::ResolvedAuth::new(
            "github.personal",
            "example",
            switchboard_core::AuthSecretRefs::GitHubCli,
        )
        .expect("recovery fixture should succeed");
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                OnePasswordSecretBackend::with_recovery_budget(
                    Some(cache.clone()),
                    crate::OnePasswordConfig::default(),
                    Some("run".into()),
                )
                .resolve_for_auth(&secret, &auth)
            });
            let second = scope.spawn(|| {
                OnePasswordSecretBackend::with_recovery_budget(
                    Some(cache.clone()),
                    crate::OnePasswordConfig::default(),
                    Some("run".into()),
                )
                .resolve_for_auth(&secret, &auth)
            });
            assert_eq!(
                first
                    .join()
                    .expect("recovery fixture should succeed")
                    .expect("recovery fixture should succeed")
                    .expose(),
                "fixture-value"
            );
            assert_eq!(
                second
                    .join()
                    .expect("recovery fixture should succeed")
                    .expect("recovery fixture should succeed")
                    .expose(),
                "fixture-value"
            );
        });
        let attempts = fs::read_to_string(fixture.path.join("attempts")).expect("recovery fixture should succeed");
        assert_eq!(attempts.lines().filter(|line| *line == "true").count(), 1);
        let reopened = OnePasswordSecretBackend::with_recovery_budget(
            Some(cache.clone()),
            crate::OnePasswordConfig::default(),
            Some("run".into()),
        );
        assert_eq!(
            reopened
                .resolve_for_auth(&secret, &auth)
                .expect("recovery fixture should succeed")
                .expose(),
            "fixture-value"
        );
        assert_eq!(
            fs::read_to_string(fixture.path.join("attempts")).expect("recovery fixture should succeed"),
            attempts
        );
        reopened.invalidate(&secret).expect("recovery fixture should succeed");
        fs::write(fixture.path.join("fail"), "").expect("recovery fixture should succeed");
        let failed = OnePasswordSecretBackend::with_recovery_budget(
            Some(cache.clone()),
            crate::OnePasswordConfig::default(),
            Some("failed-run".into()),
        );
        assert!(failed.resolve_for_auth(&secret, &auth).is_err());
        let retried = OnePasswordSecretBackend::with_recovery_budget(
            Some(cache),
            crate::OnePasswordConfig::default(),
            Some("failed-run".into()),
        );
        assert!(matches!(
            retried.resolve_for_auth(&secret, &auth),
            Err(switchboard_core::Error::RecoveryExhausted(_))
        ));
        assert_eq!(
            fs::read_to_string(fixture.path.join("attempts"))
                .expect("recovery fixture should succeed")
                .lines()
                .filter(|line| *line == "true")
                .count(),
            2
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
