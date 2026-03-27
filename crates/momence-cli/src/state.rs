use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, GlobalArgs, Result};

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.momence.com";
pub(crate) const ENV_MOMENCE_CONFIG: &str = "MOMENCE_CONFIG";
pub(crate) const ENV_MOMENCE_BASE_URL: &str = "MOMENCE_BASE_URL";
pub(crate) const ENV_MOMENCE_CLIENT_ID: &str = "MOMENCE_CLIENT_ID";
pub(crate) const ENV_MOMENCE_CLIENT_SECRET: &str = "MOMENCE_CLIENT_SECRET";
pub(crate) const ENV_MOMENCE_ACCESS_TOKEN: &str = "MOMENCE_ACCESS_TOKEN";
pub(crate) const ENV_MOMENCE_REFRESH_TOKEN: &str = "MOMENCE_REFRESH_TOKEN";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct MomenceState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) access_token_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) refresh_token_expires_at: Option<String>,
}

pub(crate) struct StateStore {
    path: PathBuf,
}

impl StateStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn load(&self) -> Result<MomenceState> {
        match fs::read_to_string(&self.path) {
            Ok(contents) => serde_json::from_str(&contents).map_err(|error| {
                Error::Config(format!(
                    "failed to parse Momence state at {}: {error}",
                    self.path.display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(MomenceState::default()),
            Err(error) => Err(Error::Io(format!(
                "failed to read Momence state at {}: {error}",
                self.path.display()
            ))),
        }
    }

    pub(crate) fn save(&self, state: &MomenceState) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::Config(format!("invalid Momence state path {}", self.path.display())))?;
        fs::create_dir_all(parent).map_err(|error| {
            Error::Io(format!(
                "failed to create Momence state directory {}: {error}",
                parent.display()
            ))
        })?;

        let temp_path = self.path.with_extension("tmp");
        let contents = serde_json::to_vec_pretty(state)
            .map_err(|error| Error::Config(format!("failed to serialize Momence state: {error}")))?;
        write_private_file(&temp_path, &contents)?;
        fs::rename(&temp_path, &self.path).map_err(|error| {
            Error::Io(format!(
                "failed to move Momence state into place at {}: {error}",
                self.path.display()
            ))
        })?;

        Ok(())
    }
}

pub(crate) struct ResolvedContext {
    pub(crate) base_url: String,
    pub(crate) client_id: Option<String>,
    pub(crate) client_secret: Option<String>,
    pub(crate) access_token: Option<String>,
    pub(crate) refresh_token: Option<String>,
    store: StateStore,
    state: MomenceState,
}

impl ResolvedContext {
    pub(crate) fn from_global(global: &GlobalArgs) -> Result<Self> {
        let path = resolve_state_path(global.config.as_deref())?;
        let store = StateStore::new(path);
        let state = store.load()?;

        Ok(Self {
            base_url: pick(
                global.base_url.clone(),
                env_value(ENV_MOMENCE_BASE_URL),
                state.base_url.clone(),
            )
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned()),
            client_id: pick(
                global.client_id.clone(),
                env_value(ENV_MOMENCE_CLIENT_ID),
                state.client_id.clone(),
            ),
            client_secret: pick(
                global.client_secret.clone(),
                env_value(ENV_MOMENCE_CLIENT_SECRET),
                state.client_secret.clone(),
            ),
            access_token: pick(
                global.access_token.clone(),
                env_value(ENV_MOMENCE_ACCESS_TOKEN),
                state.access_token.clone(),
            ),
            refresh_token: pick(
                global.refresh_token.clone(),
                env_value(ENV_MOMENCE_REFRESH_TOKEN),
                state.refresh_token.clone(),
            ),
            store,
            state,
        })
    }

    pub(crate) fn require_client_id(&self) -> Result<&str> {
        self.client_id
            .as_deref()
            .ok_or_else(|| Error::Config("missing client ID, pass --client-id or set MOMENCE_CLIENT_ID".into()))
    }

    pub(crate) fn require_client_credentials(&self) -> Result<(&str, &str)> {
        let client_id = self.require_client_id()?;
        let client_secret = self.client_secret.as_deref().ok_or_else(|| {
            Error::Config("missing client secret, pass --client-secret or set MOMENCE_CLIENT_SECRET".into())
        })?;

        Ok((client_id, client_secret))
    }

    pub(crate) fn require_access_token(&self) -> Result<&str> {
        self.access_token.as_deref().ok_or_else(|| {
            Error::Config("missing access token, run momence auth login-password or pass --access-token".into())
        })
    }

    pub(crate) fn store_tokens_from_response(&mut self, value: &Value) -> Result<()> {
        let access_token = required_string_field(value, &["access_token", "accessToken"])?;
        let refresh_token = required_string_field(value, &["refresh_token", "refreshToken"])?;
        let access_token_expires_at = required_string_field(value, &["accessTokenExpiresAt"])?;
        let refresh_token_expires_at = required_string_field(value, &["refreshTokenExpiresAt"])?;

        self.state.base_url = Some(self.base_url.clone());
        self.state.client_id = self.client_id.clone();
        self.state.client_secret = self.client_secret.clone();
        self.state.access_token = Some(access_token.clone());
        self.state.refresh_token = Some(refresh_token.clone());
        self.state.access_token_expires_at = Some(access_token_expires_at);
        self.state.refresh_token_expires_at = Some(refresh_token_expires_at);

        self.access_token = Some(access_token);
        self.refresh_token = Some(refresh_token);
        self.store.save(&self.state)
    }

    pub(crate) fn clear_tokens(&mut self) -> Result<()> {
        self.state.access_token = None;
        self.state.refresh_token = None;
        self.state.access_token_expires_at = None;
        self.state.refresh_token_expires_at = None;
        self.access_token = None;
        self.refresh_token = None;
        self.store.save(&self.state)
    }
}

fn pick(explicit: Option<String>, env_value: Option<String>, persisted: Option<String>) -> Option<String> {
    explicit.or(env_value).or(persisted)
}

fn env_value(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

fn resolve_state_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = env_value(ENV_MOMENCE_CONFIG) {
        return Ok(PathBuf::from(path));
    }
    if let Some(xdg) = env_value("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("momence").join("config.json"));
    }
    if let Some(home) = env_value("HOME") {
        return Ok(PathBuf::from(home).join(".config").join("momence").join("config.json"));
    }

    Err(Error::Config(
        "could not resolve Momence config path, pass --config or set MOMENCE_CONFIG".into(),
    ))
}

fn required_string_field(value: &Value, keys: &[&str]) -> Result<String> {
    keys.iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            Error::Config(format!(
                "Momence auth response is missing required field(s): {}",
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
        .map_err(|error| Error::Io(format!("failed to open Momence state file {}: {error}", path.display())))?;
    file.write_all(contents).map_err(|error| {
        Error::Io(format!(
            "failed to write Momence state file {}: {error}",
            path.display()
        ))
    })?;
    file.sync_all().map_err(|error| {
        Error::Io(format!(
            "failed to flush Momence state file {}: {error}",
            path.display()
        ))
    })?;
    Ok(())
}
