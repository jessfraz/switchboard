use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, GlobalArgs, Result};

pub(crate) const DEFAULT_AUTHORIZE_URL: &str = "https://api.schwabapi.com/v1/oauth/authorize";
pub(crate) const DEFAULT_TOKEN_URL: &str = "https://api.schwabapi.com/v1/oauth/token";
pub(crate) const DEFAULT_TRADER_BASE_URL: &str = "https://api.schwabapi.com/trader/v1";
pub(crate) const DEFAULT_MARKET_DATA_BASE_URL: &str = "https://api.schwabapi.com/marketdata/v1";
pub(crate) const ENV_SCHWAB_ACCESS_TOKEN: &str = "SCHWAB_ACCESS_TOKEN";
pub(crate) const ENV_SCHWAB_AUTHORIZE_URL: &str = "SCHWAB_AUTHORIZE_URL";
pub(crate) const ENV_SCHWAB_BASE_URL: &str = "SCHWAB_BASE_URL";
pub(crate) const ENV_SCHWAB_CLIENT_ID: &str = "SCHWAB_CLIENT_ID";
pub(crate) const ENV_SCHWAB_CLIENT_SECRET: &str = "SCHWAB_CLIENT_SECRET";
pub(crate) const ENV_SCHWAB_CONFIG: &str = "SCHWAB_CONFIG";
pub(crate) const ENV_SCHWAB_MARKET_DATA_BASE_URL: &str = "SCHWAB_MARKETDATA_BASE_URL";
pub(crate) const ENV_SCHWAB_REDIRECT_URI: &str = "SCHWAB_REDIRECT_URI";
pub(crate) const ENV_SCHWAB_REFRESH_TOKEN: &str = "SCHWAB_REFRESH_TOKEN";
pub(crate) const ENV_SCHWAB_TOKEN_URL: &str = "SCHWAB_TOKEN_URL";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct AccountNumberHashEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) account_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) hash_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) synced_at_epoch_seconds: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct SchwabState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) market_data_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) authorize_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) token_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) redirect_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) token_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expires_at_epoch_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) account_numbers: Vec<AccountNumberHashEntry>,
}

pub(crate) struct StateStore {
    path: PathBuf,
}

impl StateStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn load(&self) -> Result<SchwabState> {
        match fs::read_to_string(&self.path) {
            Ok(contents) => serde_json::from_str(&contents).map_err(|error| {
                Error::Config(format!(
                    "failed to parse Schwab state at {}: {error}",
                    self.path.display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SchwabState::default()),
            Err(error) => Err(Error::Io(format!(
                "failed to read Schwab state at {}: {error}",
                self.path.display()
            ))),
        }
    }

    pub(crate) fn save(&self, state: &SchwabState) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::Config(format!("invalid Schwab state path {}", self.path.display())))?;
        fs::create_dir_all(parent).map_err(|error| {
            Error::Io(format!(
                "failed to create Schwab state directory {}: {error}",
                parent.display()
            ))
        })?;

        let temp_path = self.path.with_extension("tmp");
        let contents = serde_json::to_vec_pretty(state)
            .map_err(|error| Error::Config(format!("failed to serialize Schwab state: {error}")))?;
        write_private_file(&temp_path, &contents)?;
        fs::rename(&temp_path, &self.path).map_err(|error| {
            Error::Io(format!(
                "failed to move Schwab state into place at {}: {error}",
                self.path.display()
            ))
        })?;

        Ok(())
    }
}

pub(crate) struct ResolvedContext {
    pub(crate) base_url: String,
    pub(crate) market_data_base_url: String,
    pub(crate) authorize_url: String,
    pub(crate) token_url: String,
    pub(crate) client_id: Option<String>,
    pub(crate) client_secret: Option<String>,
    pub(crate) redirect_uri: Option<String>,
    pub(crate) access_token: Option<String>,
    pub(crate) refresh_token: Option<String>,
    pub(crate) token_type: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) expires_at_epoch_seconds: Option<u64>,
    store: StateStore,
    state: SchwabState,
}

impl ResolvedContext {
    pub(crate) fn from_global(global: &GlobalArgs) -> Result<Self> {
        let path = resolve_state_path(global.config.as_deref())?;
        let store = StateStore::new(path);
        let state = store.load()?;

        Ok(Self {
            base_url: pick(global.base_url.clone(), state.base_url.clone())
                .unwrap_or_else(|| DEFAULT_TRADER_BASE_URL.to_owned()),
            market_data_base_url: pick(global.market_data_base_url.clone(), state.market_data_base_url.clone())
                .unwrap_or_else(|| DEFAULT_MARKET_DATA_BASE_URL.to_owned()),
            authorize_url: pick(global.authorize_url.clone(), state.authorize_url.clone())
                .unwrap_or_else(|| DEFAULT_AUTHORIZE_URL.to_owned()),
            token_url: pick(global.token_url.clone(), state.token_url.clone())
                .unwrap_or_else(|| DEFAULT_TOKEN_URL.to_owned()),
            client_id: pick(global.client_id.clone(), state.client_id.clone()),
            client_secret: pick(global.client_secret.clone(), state.client_secret.clone()),
            redirect_uri: pick(global.redirect_uri.clone(), state.redirect_uri.clone()),
            access_token: pick(global.access_token.clone(), state.access_token.clone()),
            refresh_token: pick(global.refresh_token.clone(), state.refresh_token.clone()),
            token_type: state.token_type.clone(),
            scope: state.scope.clone(),
            expires_at_epoch_seconds: state.expires_at_epoch_seconds,
            store,
            state,
        })
    }

    pub(crate) fn require_client_id(&self) -> Result<&str> {
        self.client_id
            .as_deref()
            .ok_or_else(|| Error::Config("missing client ID, pass --client-id or set SCHWAB_CLIENT_ID".into()))
    }

    pub(crate) fn require_client_credentials(&self) -> Result<(&str, &str)> {
        let client_id = self.require_client_id()?;
        let client_secret = self.client_secret.as_deref().ok_or_else(|| {
            Error::Config("missing client secret, pass --client-secret or set SCHWAB_CLIENT_SECRET".into())
        })?;
        Ok((client_id, client_secret))
    }

    pub(crate) fn require_redirect_uri(&self, redirect_uri: Option<String>) -> Result<String> {
        pick(redirect_uri, self.redirect_uri.clone())
            .ok_or_else(|| Error::Config("missing redirect URI, pass --redirect-uri or set SCHWAB_REDIRECT_URI".into()))
    }

    pub(crate) fn require_access_token(&self) -> Result<&str> {
        self.access_token.as_deref().ok_or_else(|| {
            Error::Config("missing access token, run schwab auth exchange-code or pass --access-token".into())
        })
    }

    pub(crate) fn store_oauth_token_response(&mut self, value: &Value) -> Result<()> {
        let access_token = required_string_field(value, &["access_token"])?;
        let refresh_token = value
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| self.refresh_token.clone());

        self.state.base_url = Some(self.base_url.clone());
        self.state.market_data_base_url = Some(self.market_data_base_url.clone());
        self.state.authorize_url = Some(self.authorize_url.clone());
        self.state.token_url = Some(self.token_url.clone());
        self.state.client_id = self.client_id.clone();
        self.state.client_secret = self.client_secret.clone();
        self.state.redirect_uri = self.redirect_uri.clone();
        self.state.access_token = Some(access_token.clone());
        self.state.refresh_token = refresh_token.clone();
        self.state.token_type = value.get("token_type").and_then(Value::as_str).map(ToOwned::to_owned);
        self.state.scope = value.get("scope").and_then(Value::as_str).map(ToOwned::to_owned);
        self.state.expires_at_epoch_seconds = value
            .get("expires_in")
            .and_then(Value::as_u64)
            .map(|expires_in| current_epoch_seconds().saturating_add(expires_in));

        self.access_token = Some(access_token);
        self.refresh_token = refresh_token;
        self.token_type = self.state.token_type.clone();
        self.scope = self.state.scope.clone();
        self.expires_at_epoch_seconds = self.state.expires_at_epoch_seconds;
        self.store.save(&self.state)
    }

    pub(crate) fn remember_redirect_uri(&mut self, redirect_uri: String) -> Result<()> {
        self.redirect_uri = Some(redirect_uri.clone());
        self.state.redirect_uri = Some(redirect_uri);
        self.store.save(&self.state)
    }

    pub(crate) fn remember_account_numbers(&mut self, value: &Value) -> Result<()> {
        let array = value
            .as_array()
            .ok_or_else(|| Error::Config("Schwab account-number response was not the expected array payload".into()))?;

        let synced_at_epoch_seconds = current_epoch_seconds();
        let mut entries = Vec::new();
        for item in array {
            let account_number = item.get("accountNumber").and_then(Value::as_str).map(ToOwned::to_owned);
            let hash_value = item.get("hashValue").and_then(Value::as_str).map(ToOwned::to_owned);

            if account_number.is_none() && hash_value.is_none() {
                continue;
            }

            entries.push(AccountNumberHashEntry {
                account_number,
                hash_value,
                synced_at_epoch_seconds: Some(synced_at_epoch_seconds),
            });
        }

        self.state.account_numbers = entries;
        self.store.save(&self.state)
    }

    pub(crate) fn account_hash_for_plain_text(&self, account_number: &str) -> Option<&str> {
        self.state.account_numbers.iter().find_map(|entry| {
            if entry.account_number.as_deref() == Some(account_number) {
                entry.hash_value.as_deref()
            } else {
                None
            }
        })
    }

    pub(crate) fn account_number_cache(&self) -> &[AccountNumberHashEntry] {
        &self.state.account_numbers
    }

    pub(crate) fn clear_auth_state(&mut self) -> Result<()> {
        self.state.access_token = None;
        self.state.refresh_token = None;
        self.state.token_type = None;
        self.state.scope = None;
        self.state.expires_at_epoch_seconds = None;
        self.access_token = None;
        self.refresh_token = None;
        self.token_type = None;
        self.scope = None;
        self.expires_at_epoch_seconds = None;
        self.store.save(&self.state)
    }
}

fn current_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn pick(explicit: Option<String>, persisted: Option<String>) -> Option<String> {
    explicit.or(persisted)
}

fn env_value(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

fn resolve_state_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Some(xdg) = env_value("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("schwab").join("config.json"));
    }
    if let Some(home) = env_value("HOME") {
        return Ok(PathBuf::from(home).join(".config").join("schwab").join("config.json"));
    }

    Err(Error::Config(
        "could not resolve Schwab config path, pass --config or set SCHWAB_CONFIG".into(),
    ))
}

fn required_string_field(value: &Value, keys: &[&str]) -> Result<String> {
    keys.iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            Error::Config(format!(
                "Schwab auth response is missing required field(s): {}",
                keys.join(", ")
            ))
        })
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(path)
        .map_err(|error| Error::Io(format!("failed to open Schwab state file {}: {error}", path.display())))?;
    file.write_all(contents)
        .map_err(|error| Error::Io(format!("failed to write Schwab state file {}: {error}", path.display())))?;
    file.sync_all()
        .map_err(|error| Error::Io(format!("failed to flush Schwab state file {}: {error}", path.display())))?;
    Ok(())
}
