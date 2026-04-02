use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::{
    cache::{PlaidCacheStore, DEFAULT_CACHE_DB_FILE},
    Error, GlobalArgs, Result,
};

pub(crate) const DEFAULT_PLAID_VERSION: &str = "2020-09-14";
pub(crate) const ENV_PLAID_CONFIG: &str = "PLAID_CONFIG";
pub(crate) const ENV_PLAID_ENVIRONMENT: &str = "PLAID_ENVIRONMENT";
pub(crate) const ENV_PLAID_BASE_URL: &str = "PLAID_BASE_URL";
pub(crate) const ENV_PLAID_CLIENT_ID: &str = "PLAID_CLIENT_ID";
pub(crate) const ENV_PLAID_SECRET: &str = "PLAID_SECRET";
pub(crate) const ENV_PLAID_ACCESS_TOKEN: &str = "PLAID_ACCESS_TOKEN";
pub(crate) const ENV_PLAID_ITEM_ID: &str = "PLAID_ITEM_ID";
pub(crate) const ENV_PLAID_VERSION: &str = "PLAID_VERSION";
pub(crate) const ENV_PLAID_CLIENT_NAME: &str = "PLAID_CLIENT_NAME";
pub(crate) const ENV_PLAID_CACHE_DB: &str = "PLAID_CACHE_DB";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlaidEnvironment {
    #[default]
    Sandbox,
    Development,
    Production,
}

impl PlaidEnvironment {
    pub(crate) fn default_base_url(self) -> &'static str {
        match self {
            Self::Sandbox => "https://sandbox.plaid.com",
            Self::Development => "https://development.plaid.com",
            Self::Production => "https://production.plaid.com",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct PlaidState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) environment: Option<PlaidEnvironment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) item_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) plaid_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_name: Option<String>,
}

pub(crate) struct StateStore {
    path: PathBuf,
}

impl StateStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn load(&self) -> Result<PlaidState> {
        match fs::read_to_string(&self.path) {
            Ok(contents) => serde_json::from_str(&contents).map_err(|error| {
                Error::Config(format!(
                    "failed to parse Plaid state at {}: {error}",
                    self.path.display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(PlaidState::default()),
            Err(error) => Err(Error::Io(format!(
                "failed to read Plaid state at {}: {error}",
                self.path.display()
            ))),
        }
    }

    pub(crate) fn save(&self, state: &PlaidState) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::Config(format!("invalid Plaid state path {}", self.path.display())))?;
        fs::create_dir_all(parent).map_err(|error| {
            Error::Io(format!(
                "failed to create Plaid state directory {}: {error}",
                parent.display()
            ))
        })?;

        let temp_path = self.path.with_extension("tmp");
        let contents = serde_json::to_vec_pretty(state)
            .map_err(|error| Error::Config(format!("failed to serialize Plaid state: {error}")))?;
        write_private_file(&temp_path, &contents)?;
        fs::rename(&temp_path, &self.path).map_err(|error| {
            Error::Io(format!(
                "failed to move Plaid state into place at {}: {error}",
                self.path.display()
            ))
        })?;

        Ok(())
    }
}

pub(crate) struct ResolvedContext {
    pub(crate) environment: PlaidEnvironment,
    pub(crate) base_url_override: Option<String>,
    pub(crate) base_url: String,
    pub(crate) client_id: Option<String>,
    pub(crate) secret: Option<String>,
    pub(crate) access_token: Option<String>,
    pub(crate) item_id: Option<String>,
    pub(crate) plaid_version: String,
    pub(crate) client_name: String,
    pub(crate) cache: PlaidCacheStore,
    store: StateStore,
    state: PlaidState,
}

pub(crate) struct ForgetItemStateResult {
    pub(crate) access_token_cleared: bool,
    pub(crate) item_id_cleared: bool,
}

impl ResolvedContext {
    pub(crate) fn from_global(global: &GlobalArgs) -> Result<Self> {
        let path = resolve_state_path(global.config.as_deref())?;
        let cache_db_path = resolve_cache_db_path(global.cache_db.as_deref(), &path);
        let store = StateStore::new(path);
        let state = store.load()?;

        let environment = global.environment.or(state.environment).unwrap_or_default();
        let base_url_override = pick(global.base_url.clone(), state.base_url.clone());

        Ok(Self {
            environment,
            base_url: base_url_override
                .clone()
                .unwrap_or_else(|| environment.default_base_url().to_owned()),
            base_url_override,
            client_id: pick(global.client_id.clone(), state.client_id.clone()),
            secret: pick(global.secret.clone(), state.secret.clone()),
            access_token: pick(global.access_token.clone(), state.access_token.clone()),
            item_id: pick(global.item_id.clone(), state.item_id.clone()),
            plaid_version: pick(global.plaid_version.clone(), state.plaid_version.clone())
                .unwrap_or_else(|| DEFAULT_PLAID_VERSION.to_owned()),
            client_name: pick(global.client_name.clone(), state.client_name.clone())
                .unwrap_or_else(|| format!("plaid-cli/{}", env!("CARGO_PKG_VERSION"))),
            cache: PlaidCacheStore::open(cache_db_path)?,
            store,
            state,
        })
    }

    pub(crate) fn require_client_credentials(&self) -> Result<(&str, &str)> {
        let client_id = self
            .client_id
            .as_deref()
            .ok_or_else(|| Error::Config("missing client ID, pass --client-id or set PLAID_CLIENT_ID".into()))?;
        let secret = self
            .secret
            .as_deref()
            .ok_or_else(|| Error::Config("missing secret, pass --secret or set PLAID_SECRET".into()))?;

        Ok((client_id, secret))
    }

    pub(crate) fn require_access_token(&self) -> Result<&str> {
        self.access_token.as_deref().ok_or_else(|| {
            Error::Config("missing access token, run plaid auth exchange-public-token or pass --access-token".into())
        })
    }

    pub(crate) fn store_access_token(&mut self, access_token: String, item_id: Option<String>) -> Result<()> {
        self.state.environment = Some(self.environment);
        self.state.base_url = self.base_url_override.clone();
        self.state.client_id = self.client_id.clone();
        self.state.secret = self.secret.clone();
        self.state.access_token = Some(access_token.clone());
        self.state.item_id = item_id.clone().or_else(|| self.state.item_id.clone());
        self.state.plaid_version = Some(self.plaid_version.clone());
        self.state.client_name = Some(self.client_name.clone());

        self.access_token = Some(access_token);
        self.item_id = item_id.or_else(|| self.item_id.clone());
        self.store.save(&self.state)
    }

    pub(crate) fn remember_item_id(&mut self, item_id: impl Into<String>) -> Result<()> {
        let item_id = item_id.into();
        self.state.item_id = Some(item_id.clone());
        self.item_id = Some(item_id);
        self.store.save(&self.state)
    }

    pub(crate) fn forget_removed_item(
        &mut self,
        removed_item_id: Option<&str>,
        removed_access_token: Option<&str>,
    ) -> Result<ForgetItemStateResult> {
        let mut access_token_cleared = false;
        let mut item_id_cleared = false;

        if let Some(access_token) = removed_access_token {
            if self.state.access_token.as_deref() == Some(access_token) {
                self.state.access_token = None;
                access_token_cleared = true;
            }
            if self.access_token.as_deref() == Some(access_token) {
                self.access_token = None;
                access_token_cleared = true;
            }
        }

        if let Some(item_id) = removed_item_id {
            if self.state.item_id.as_deref() == Some(item_id) {
                self.state.item_id = None;
                item_id_cleared = true;
            }
            if self.item_id.as_deref() == Some(item_id) {
                self.item_id = None;
                item_id_cleared = true;
            }
        }

        if access_token_cleared || item_id_cleared {
            self.store.save(&self.state)?;
        }

        Ok(ForgetItemStateResult {
            access_token_cleared,
            item_id_cleared,
        })
    }

    pub(crate) fn clear_auth_state(&mut self) -> Result<()> {
        self.state.access_token = None;
        self.state.item_id = None;
        self.access_token = None;
        self.item_id = None;
        self.store.save(&self.state)
    }
}

fn pick<T>(explicit: Option<T>, persisted: Option<T>) -> Option<T> {
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
        return Ok(PathBuf::from(xdg).join("plaid").join("config.json"));
    }
    if let Some(home) = env_value("HOME") {
        return Ok(PathBuf::from(home).join(".config").join("plaid").join("config.json"));
    }

    Err(Error::Config(
        "could not resolve Plaid config path, pass --config or set PLAID_CONFIG".into(),
    ))
}

fn resolve_cache_db_path(explicit: Option<&Path>, state_path: &Path) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_path_buf();
    }

    state_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(DEFAULT_CACHE_DB_FILE)
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
        .map_err(|error| Error::Io(format!("failed to open Plaid state file {}: {error}", path.display())))?;
    file.write_all(contents)
        .map_err(|error| Error::Io(format!("failed to write Plaid state file {}: {error}", path.display())))?;
    file.sync_all()
        .map_err(|error| Error::Io(format!("failed to flush Plaid state file {}: {error}", path.display())))?;
    Ok(())
}
