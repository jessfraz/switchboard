use std::{
    env, fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{Error, GlobalArgs, Result};

pub(crate) const DEFAULT_BASE_URL: &str = "https://mb-api.mindbodyonline.com/affiliate/api/v1";
pub(crate) const ENV_MINDBODY_CONFIG: &str = "MINDBODY_CONFIG";
pub(crate) const ENV_MINDBODY_BASE_URL: &str = "MINDBODY_BASE_URL";
pub(crate) const ENV_MINDBODY_API_KEY: &str = "MINDBODY_API_KEY";
pub(crate) const ENV_MINDBODY_CLIENT_KEY: &str = "MINDBODY_CLIENT_KEY";
pub(crate) const ENV_MINDBODY_CLIENT_SECRET: &str = "MINDBODY_CLIENT_SECRET";
pub(crate) const ENV_MINDBODY_USER_ID: &str = "MINDBODY_USER_ID";
pub(crate) const ENV_MINDBODY_APP_NAME: &str = "MINDBODY_APP_NAME";

#[derive(Clone, Debug, Default, Deserialize)]
struct MindbodyConfig {
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    client_key: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    app_name: Option<String>,
}

pub(crate) struct ResolvedContext {
    pub(crate) base_url: String,
    pub(crate) api_key: Option<String>,
    pub(crate) client_key: Option<String>,
    pub(crate) client_secret: Option<String>,
    pub(crate) user_id: Option<String>,
    pub(crate) app_name: String,
}

impl ResolvedContext {
    pub(crate) fn from_global(global: &GlobalArgs) -> Result<Self> {
        let config = load_config(global.config.as_deref())?;

        Ok(Self {
            base_url: pick(
                global.base_url.clone(),
                env_value(ENV_MINDBODY_BASE_URL),
                config.base_url,
            )
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned()),
            api_key: pick(global.api_key.clone(), env_value(ENV_MINDBODY_API_KEY), config.api_key),
            client_key: pick(
                global.client_key.clone(),
                env_value(ENV_MINDBODY_CLIENT_KEY),
                config.client_key,
            ),
            client_secret: pick(
                global.client_secret.clone(),
                env_value(ENV_MINDBODY_CLIENT_SECRET),
                config.client_secret,
            ),
            user_id: pick(global.user_id.clone(), env_value(ENV_MINDBODY_USER_ID), config.user_id),
            app_name: pick(
                global.app_name.clone(),
                env_value(ENV_MINDBODY_APP_NAME),
                config.app_name,
            )
            .unwrap_or_else(|| format!("mindbody-cli/{}", env!("CARGO_PKG_VERSION"))),
        })
    }

    pub(crate) fn require_credentials(&self) -> Result<(&str, &str, &str)> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or_else(|| Error::Config("missing API key, pass --api-key or set MINDBODY_API_KEY".into()))?;
        let client_key = self
            .client_key
            .as_deref()
            .ok_or_else(|| Error::Config("missing client key, pass --client-key or set MINDBODY_CLIENT_KEY".into()))?;
        let client_secret = self.client_secret.as_deref().ok_or_else(|| {
            Error::Config("missing client secret, pass --client-secret or set MINDBODY_CLIENT_SECRET".into())
        })?;

        Ok((api_key, client_key, client_secret))
    }

    pub(crate) fn require_user_id(&self) -> Result<&str> {
        let user_id = self
            .user_id
            .as_deref()
            .ok_or_else(|| Error::Config("missing user ID, pass --user-id or set MINDBODY_USER_ID".into()))?;
        validate_unique_user_id(user_id)?;
        Ok(user_id)
    }
}

fn pick(explicit: Option<String>, env_value: Option<String>, configured: Option<String>) -> Option<String> {
    explicit.or(env_value).or(configured)
}

fn env_value(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

fn load_config(explicit: Option<&Path>) -> Result<MindbodyConfig> {
    if let Some(path) = explicit {
        return read_config(path);
    }

    if let Some(path) = env_value(ENV_MINDBODY_CONFIG) {
        return read_config(Path::new(&path));
    }

    for candidate in default_config_candidates() {
        if candidate.exists() {
            return read_config(&candidate);
        }
    }

    Ok(MindbodyConfig::default())
}

fn read_config(path: &Path) -> Result<MindbodyConfig> {
    let contents = fs::read_to_string(path)
        .map_err(|error| Error::Io(format!("failed to read Mindbody config at {}: {error}", path.display())))?;
    serde_json::from_str(&contents).map_err(|error| {
        Error::Config(format!(
            "failed to parse Mindbody config at {}: {error}",
            path.display()
        ))
    })
}

fn default_config_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(xdg) = env_value("XDG_CONFIG_HOME") {
        candidates.push(PathBuf::from(xdg).join("mindbody").join("config.json"));
    }
    if let Some(home) = env_value("HOME") {
        candidates.push(PathBuf::from(home).join(".config").join("mindbody").join("config.json"));
    }

    candidates
}

pub(crate) fn validate_unique_user_id(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 64 {
        return Err(Error::Arguments(
            "Mindbody uniqueUserId must be between 1 and 64 characters".into(),
        ));
    }

    if value.chars().all(is_valid_user_id_char) {
        return Ok(());
    }

    Err(Error::Arguments(
        "Mindbody uniqueUserId may only contain lowercase letters, digits, _, -, ~, and .".into(),
    ))
}

fn is_valid_user_id_char(ch: char) -> bool {
    ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '-' | '~' | '.')
}
