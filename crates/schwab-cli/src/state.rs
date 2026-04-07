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
pub(crate) const ENV_SCHWAB_CLIENT_FUNCTION_ID: &str = "SCHWAB_CLIENT_FUNCTION_ID";
pub(crate) const ENV_SCHWAB_RRBUS_PILOT_ROLLOUT: &str = "SCHWAB_RRBUS_PILOT_ROLLOUT";
pub(crate) const ENV_SCHWAB_RESOURCE_VERSION: &str = "SCHWAB_RESOURCE_VERSION";
pub(crate) const ENV_SCHWAB_THIRD_PARTY_ID: &str = "SCHWAB_THIRD_PARTY_ID";
pub(crate) const ENV_SCHWAB_TOKEN_URL: &str = "SCHWAB_TOKEN_URL";
pub(crate) const ENV_SCHWAB_TRADER_CLIENT_APP_ID: &str = "SCHWAB_TRADER_CLIENT_APP_ID";
pub(crate) const ENV_SCHWAB_TRADER_CLIENT_CHANNEL: &str = "SCHWAB_TRADER_CLIENT_CHANNEL";

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
    pub(crate) third_party_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_function_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) resource_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rrbus_pilot_rollout: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_oauth_state: Option<String>,
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
    pub(crate) third_party_id: Option<String>,
    pub(crate) client_channel: Option<String>,
    pub(crate) client_app_id: Option<String>,
    pub(crate) client_function_id: Option<String>,
    pub(crate) resource_version: Option<String>,
    pub(crate) rrbus_pilot_rollout: Option<String>,
    pub(crate) redirect_uri: Option<String>,
    pub(crate) access_token: Option<String>,
    pub(crate) refresh_token: Option<String>,
    pub(crate) token_type: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) expires_at_epoch_seconds: Option<u64>,
    pub(crate) pending_oauth_state: Option<String>,
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
            third_party_id: pick(global.third_party_id.clone(), state.third_party_id.clone()),
            client_channel: pick(global.client_channel.clone(), state.client_channel.clone()),
            client_app_id: pick(global.client_app_id.clone(), state.client_app_id.clone()),
            client_function_id: pick(global.client_function_id.clone(), state.client_function_id.clone()),
            resource_version: pick(global.resource_version.clone(), state.resource_version.clone()),
            rrbus_pilot_rollout: pick(global.rrbus_pilot_rollout.clone(), state.rrbus_pilot_rollout.clone()),
            redirect_uri: pick(global.redirect_uri.clone(), state.redirect_uri.clone()),
            access_token: pick(global.access_token.clone(), state.access_token.clone()),
            refresh_token: pick(global.refresh_token.clone(), state.refresh_token.clone()),
            token_type: state.token_type.clone(),
            scope: state.scope.clone(),
            expires_at_epoch_seconds: state.expires_at_epoch_seconds,
            pending_oauth_state: state.pending_oauth_state.clone(),
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
        self.state.third_party_id = self.third_party_id.clone();
        self.state.client_channel = self.client_channel.clone();
        self.state.client_app_id = self.client_app_id.clone();
        self.state.client_function_id = self.client_function_id.clone();
        self.state.resource_version = self.resource_version.clone();
        self.state.rrbus_pilot_rollout = self.rrbus_pilot_rollout.clone();
        self.state.access_token = Some(access_token.clone());
        self.state.refresh_token = refresh_token.clone();
        self.state.token_type = value.get("token_type").and_then(Value::as_str).map(ToOwned::to_owned);
        self.state.scope = value.get("scope").and_then(Value::as_str).map(ToOwned::to_owned);
        self.state.expires_at_epoch_seconds = value
            .get("expires_in")
            .and_then(Value::as_u64)
            .map(|expires_in| current_epoch_seconds().saturating_add(expires_in));
        self.state.pending_oauth_state = None;

        self.access_token = Some(access_token);
        self.refresh_token = refresh_token;
        self.token_type = self.state.token_type.clone();
        self.scope = self.state.scope.clone();
        self.expires_at_epoch_seconds = self.state.expires_at_epoch_seconds;
        self.pending_oauth_state = None;
        self.store.save(&self.state)
    }

    pub(crate) fn remember_authorization_request(&mut self, redirect_uri: String, oauth_state: String) -> Result<()> {
        self.state.base_url = Some(self.base_url.clone());
        self.state.market_data_base_url = Some(self.market_data_base_url.clone());
        self.state.authorize_url = Some(self.authorize_url.clone());
        self.state.token_url = Some(self.token_url.clone());
        self.state.client_id = self.client_id.clone();
        self.state.client_secret = self.client_secret.clone();
        self.state.third_party_id = self.third_party_id.clone();
        self.state.client_channel = self.client_channel.clone();
        self.state.client_app_id = self.client_app_id.clone();
        self.state.client_function_id = self.client_function_id.clone();
        self.state.resource_version = self.resource_version.clone();
        self.state.rrbus_pilot_rollout = self.rrbus_pilot_rollout.clone();
        self.redirect_uri = Some(redirect_uri.clone());
        self.state.redirect_uri = Some(redirect_uri);
        self.pending_oauth_state = Some(oauth_state.clone());
        self.state.pending_oauth_state = Some(oauth_state);
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
        self.state.pending_oauth_state = None;
        self.access_token = None;
        self.refresh_token = None;
        self.token_type = None;
        self.scope = None;
        self.expires_at_epoch_seconds = None;
        self.pending_oauth_state = None;
        self.store.save(&self.state)
    }

    pub(crate) fn trader_headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![
            ("Accept".into(), "application/json".into()),
            ("Content-Type".into(), "application/json".into()),
            ("Schwab-Client-CorrelId".into(), correlation_id()),
        ];
        push_optional_header(&mut headers, "ThirdPartyId", self.third_party_id.clone());
        push_optional_header(&mut headers, "Schwab-Client-Channel", self.client_channel.clone());
        push_optional_header(&mut headers, "Schwab-Client-AppId", self.client_app_id.clone());
        push_optional_header(&mut headers, "Schwab-Resource-Version", self.resource_version.clone());
        push_optional_header(
            &mut headers,
            "Schwab-RRBus-PilotRollout",
            self.rrbus_pilot_rollout.clone(),
        );
        headers
    }

    pub(crate) fn market_headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![
            ("Accept".into(), "application/json".into()),
            ("Content-Type".into(), "application/json".into()),
            ("Schwab-Client-CorrelId".into(), correlation_id()),
        ];
        push_optional_header(&mut headers, "Schwab-Client-Channel", self.client_channel.clone());
        push_optional_header(&mut headers, "Schwab-Client-AppId", self.client_app_id.clone());
        push_optional_header(
            &mut headers,
            "Schwab-Client-FunctionId",
            self.client_function_id.clone(),
        );
        push_optional_header(&mut headers, "Schwab-Resource-Version", self.resource_version.clone());
        headers
    }
}

fn current_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn current_epoch_nanoseconds() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
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

fn push_optional_header(headers: &mut Vec<(String, String)>, key: &str, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        headers.push((key.to_owned(), value));
    }
}

fn correlation_id() -> String {
    let mixed = current_epoch_nanoseconds() ^ (u128::from(std::process::id()) << 32);
    let hex = format!("{mixed:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
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
