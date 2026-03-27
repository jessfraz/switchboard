use std::{
    collections::BTreeMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    client::{cookie_names, StoredCookie},
    Error, GlobalArgs, Result,
};

pub(crate) const ENV_MYCHART_CONFIG: &str = "MYCHART_CONFIG";
pub(crate) const ENV_MYCHART_BASE_URL: &str = "MYCHART_BASE_URL";
pub(crate) const ENV_MYCHART_PORTAL_BASE_URL: &str = "MYCHART_PORTAL_BASE_URL";
pub(crate) const ENV_MYCHART_CLIENT_ID: &str = "MYCHART_CLIENT_ID";
pub(crate) const ENV_MYCHART_CLIENT_SECRET: &str = "MYCHART_CLIENT_SECRET";
pub(crate) const ENV_MYCHART_REDIRECT_URI: &str = "MYCHART_REDIRECT_URI";
pub(crate) const ENV_MYCHART_ACCESS_TOKEN: &str = "MYCHART_ACCESS_TOKEN";
pub(crate) const ENV_MYCHART_REFRESH_TOKEN: &str = "MYCHART_REFRESH_TOKEN";
pub(crate) const ENV_MYCHART_USERNAME: &str = "MYCHART_USERNAME";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct MyChartState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) api_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) portal_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    legacy_base_url: Option<String>,
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
    pub(crate) patient_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expires_at_epoch_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_oauth_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_code_verifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) cookies: Vec<StoredCookie>,
}

pub(crate) struct StateStore {
    path: PathBuf,
}

impl StateStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn load(&self) -> Result<MyChartState> {
        match fs::read_to_string(&self.path) {
            Ok(contents) => serde_json::from_str(&contents).map_err(|error| {
                Error::Config(format!(
                    "failed to parse MyChart state at {}: {error}",
                    self.path.display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(MyChartState::default()),
            Err(error) => Err(Error::Io(format!(
                "failed to read MyChart state at {}: {error}",
                self.path.display()
            ))),
        }
    }

    pub(crate) fn save(&self, state: &MyChartState) -> Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::Config(format!("invalid MyChart state path {}", self.path.display())))?;
        fs::create_dir_all(parent).map_err(|error| {
            Error::Io(format!(
                "failed to create MyChart state directory {}: {error}",
                parent.display()
            ))
        })?;

        let temp_path = self.path.with_extension("tmp");
        let contents = serde_json::to_vec_pretty(state)
            .map_err(|error| Error::Config(format!("failed to serialize MyChart state: {error}")))?;
        write_private_file(&temp_path, &contents)?;
        fs::rename(&temp_path, &self.path).map_err(|error| {
            Error::Io(format!(
                "failed to move MyChart state into place at {}: {error}",
                self.path.display()
            ))
        })?;

        Ok(())
    }
}

pub(crate) struct ResolvedContext {
    pub(crate) api_base_url: Option<String>,
    pub(crate) portal_base_url: Option<String>,
    pub(crate) client_id: Option<String>,
    pub(crate) client_secret: Option<String>,
    pub(crate) redirect_uri: Option<String>,
    pub(crate) access_token: Option<String>,
    pub(crate) refresh_token: Option<String>,
    pub(crate) token_type: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) patient_id: Option<String>,
    pub(crate) expires_at_epoch_seconds: Option<u64>,
    pub(crate) pending_oauth_state: Option<String>,
    pub(crate) pending_code_verifier: Option<String>,
    pub(crate) username: Option<String>,
    pub(crate) device_id: String,
    pub(crate) cookies: BTreeMap<String, String>,
    store: StateStore,
    state: MyChartState,
}

impl ResolvedContext {
    pub(crate) fn from_global(global: &GlobalArgs) -> Result<Self> {
        let path = resolve_state_path(global.config.as_deref())?;
        let store = StateStore::new(path);
        let mut state = store.load()?;
        if state.portal_base_url.is_none() && state.legacy_base_url.is_some() {
            state.portal_base_url = state.legacy_base_url.clone();
        }

        Ok(Self {
            api_base_url: pick(
                global.base_url.clone(),
                env_value(ENV_MYCHART_BASE_URL),
                state.api_base_url.clone(),
            ),
            portal_base_url: pick(
                global.portal_base_url.clone(),
                env_value(ENV_MYCHART_PORTAL_BASE_URL),
                state.portal_base_url.clone(),
            ),
            client_id: pick(
                global.client_id.clone(),
                env_value(ENV_MYCHART_CLIENT_ID),
                state.client_id.clone(),
            ),
            client_secret: pick(
                global.client_secret.clone(),
                env_value(ENV_MYCHART_CLIENT_SECRET),
                state.client_secret.clone(),
            ),
            redirect_uri: pick(
                global.redirect_uri.clone(),
                env_value(ENV_MYCHART_REDIRECT_URI),
                state.redirect_uri.clone(),
            ),
            access_token: pick(
                global.access_token.clone(),
                env_value(ENV_MYCHART_ACCESS_TOKEN),
                state.access_token.clone(),
            ),
            refresh_token: pick(
                global.refresh_token.clone(),
                env_value(ENV_MYCHART_REFRESH_TOKEN),
                state.refresh_token.clone(),
            ),
            token_type: state.token_type.clone(),
            scope: state.scope.clone(),
            patient_id: state.patient_id.clone(),
            expires_at_epoch_seconds: state.expires_at_epoch_seconds,
            pending_oauth_state: state.pending_oauth_state.clone(),
            pending_code_verifier: state.pending_code_verifier.clone(),
            username: pick(
                global.username.clone(),
                env_value(ENV_MYCHART_USERNAME),
                state.username.clone(),
            ),
            device_id: state.device_id.clone().unwrap_or_else(generate_device_id),
            cookies: state
                .cookies
                .iter()
                .map(|cookie| (cookie.name.clone(), cookie.value.clone()))
                .collect(),
            store,
            state,
        })
    }

    pub(crate) fn require_api_base_url(&self) -> Result<String> {
        self.api_base_url.clone().ok_or_else(|| {
            Error::Config(
                "missing MyChart FHIR base URL, pass --base-url, set MYCHART_BASE_URL, or store it during auth authorize-url"
                    .into(),
            )
        })
    }

    pub(crate) fn require_portal_base_url(&self) -> Result<String> {
        self.portal_base_url.clone().ok_or_else(|| {
            Error::Config(
                "missing MyChart portal base URL, pass --portal-base-url, set MYCHART_PORTAL_BASE_URL, or store it during portal auth login-password"
                    .into(),
            )
        })
    }

    pub(crate) fn require_client_id(&self) -> Result<String> {
        self.client_id
            .clone()
            .ok_or_else(|| Error::Config("missing client id, pass --client-id or set MYCHART_CLIENT_ID".into()))
    }

    pub(crate) fn require_redirect_uri(&self, explicit: Option<String>) -> Result<String> {
        explicit.or_else(|| self.redirect_uri.clone()).ok_or_else(|| {
            Error::Config("missing redirect URI, pass --redirect-uri or set MYCHART_REDIRECT_URI".into())
        })
    }

    pub(crate) fn require_access_token(&self, explicit: Option<String>) -> Result<String> {
        explicit.or_else(|| self.access_token.clone()).ok_or_else(|| {
            Error::Config("missing access token, run mychart auth exchange-code or pass --access-token".into())
        })
    }

    pub(crate) fn require_refresh_token(&self, explicit: Option<String>) -> Result<String> {
        explicit.or_else(|| self.refresh_token.clone()).ok_or_else(|| {
            Error::Config("missing refresh token, run mychart auth exchange-code or pass --refresh-token".into())
        })
    }

    pub(crate) fn require_code_verifier(&self, explicit: Option<String>) -> Result<String> {
        explicit.or_else(|| self.pending_code_verifier.clone()).ok_or_else(|| {
            Error::Config(
                "missing PKCE code verifier, pass --code-verifier or generate one with mychart auth authorize-url"
                    .into(),
            )
        })
    }

    pub(crate) fn api_authenticated(&self) -> bool {
        self.access_token.is_some()
    }

    pub(crate) fn has_portal_session(&self) -> bool {
        !self.cookies.is_empty()
    }

    pub(crate) fn require_portal_session(&self) -> Result<()> {
        if self.has_portal_session() {
            Ok(())
        } else {
            Err(Error::Config(
                "missing MyChart portal session, run mychart portal auth login-password first".into(),
            ))
        }
    }

    pub(crate) fn require_username(&self, explicit: Option<String>) -> Result<String> {
        explicit
            .or_else(|| self.username.clone())
            .ok_or_else(|| Error::Config("missing username, pass --username or set MYCHART_USERNAME".into()))
    }

    pub(crate) fn store_pending_oauth(
        &mut self,
        base_url: String,
        client_id: String,
        client_secret: Option<String>,
        redirect_uri: String,
        oauth_state: String,
        code_verifier: String,
    ) -> Result<()> {
        self.api_base_url = Some(base_url.clone());
        self.client_id = Some(client_id.clone());
        self.client_secret = client_secret.clone();
        self.redirect_uri = Some(redirect_uri.clone());
        self.pending_oauth_state = Some(oauth_state.clone());
        self.pending_code_verifier = Some(code_verifier.clone());
        self.state.api_base_url = Some(base_url);
        self.state.client_id = Some(client_id);
        self.state.client_secret = client_secret;
        self.state.redirect_uri = Some(redirect_uri);
        self.state.pending_oauth_state = Some(oauth_state);
        self.state.pending_code_verifier = Some(code_verifier);
        self.store.save(&self.state)
    }

    pub(crate) fn store_api_tokens(
        &mut self,
        base_url: String,
        client_id: String,
        client_secret: Option<String>,
        redirect_uri: String,
        access_token: String,
        refresh_token: Option<String>,
        token_type: Option<String>,
        scope: Option<String>,
        patient_id: Option<String>,
        expires_at_epoch_seconds: Option<u64>,
    ) -> Result<()> {
        self.api_base_url = Some(base_url.clone());
        self.client_id = Some(client_id.clone());
        self.client_secret = client_secret.clone();
        self.redirect_uri = Some(redirect_uri.clone());
        self.access_token = Some(access_token.clone());
        self.refresh_token = refresh_token.clone();
        self.token_type = token_type.clone();
        self.scope = scope.clone();
        self.patient_id = patient_id.clone();
        self.expires_at_epoch_seconds = expires_at_epoch_seconds;
        self.pending_oauth_state = None;
        self.pending_code_verifier = None;

        self.state.api_base_url = Some(base_url);
        self.state.client_id = Some(client_id);
        self.state.client_secret = client_secret;
        self.state.redirect_uri = Some(redirect_uri);
        self.state.access_token = Some(access_token);
        self.state.refresh_token = refresh_token;
        self.state.token_type = token_type;
        self.state.scope = scope;
        self.state.patient_id = patient_id;
        self.state.expires_at_epoch_seconds = expires_at_epoch_seconds;
        self.state.pending_oauth_state = None;
        self.state.pending_code_verifier = None;
        self.store.save(&self.state)
    }

    pub(crate) fn clear_api_session(&mut self) -> Result<()> {
        self.access_token = None;
        self.refresh_token = None;
        self.token_type = None;
        self.scope = None;
        self.patient_id = None;
        self.expires_at_epoch_seconds = None;
        self.pending_oauth_state = None;
        self.pending_code_verifier = None;

        self.state.access_token = None;
        self.state.refresh_token = None;
        self.state.token_type = None;
        self.state.scope = None;
        self.state.patient_id = None;
        self.state.expires_at_epoch_seconds = None;
        self.state.pending_oauth_state = None;
        self.state.pending_code_verifier = None;
        self.store.save(&self.state)
    }

    pub(crate) fn update_cookies(&mut self, cookies: BTreeMap<String, String>) {
        self.cookies = cookies;
    }

    pub(crate) fn store_portal_session(&mut self, portal_base_url: String, username: Option<String>) -> Result<()> {
        if let Some(username) = username {
            self.username = Some(username);
        }

        self.portal_base_url = Some(portal_base_url.clone());
        self.state.portal_base_url = Some(portal_base_url);
        self.state.username = self.username.clone();
        self.state.device_id = Some(self.device_id.clone());
        self.state.cookies = self
            .cookies
            .iter()
            .map(|(name, value)| StoredCookie {
                name: name.clone(),
                value: value.clone(),
            })
            .collect();
        self.store.save(&self.state)
    }

    pub(crate) fn clear_portal_session(&mut self) -> Result<()> {
        self.cookies.clear();
        self.state.portal_base_url = self.portal_base_url.clone();
        self.state.username = self.username.clone();
        self.state.device_id = Some(self.device_id.clone());
        self.state.cookies = Vec::new();
        self.store.save(&self.state)
    }

    pub(crate) fn cookie_names(&self) -> Vec<String> {
        cookie_names(&self.cookies)
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
    if let Some(path) = env_value(ENV_MYCHART_CONFIG) {
        return Ok(PathBuf::from(path));
    }
    if let Some(xdg) = env_value("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("mychart").join("config.json"));
    }
    if let Some(home) = env_value("HOME") {
        return Ok(PathBuf::from(home).join(".config").join("mychart").join("config.json"));
    }

    Err(Error::Config(
        "could not resolve MyChart config path, pass --config or set MYCHART_CONFIG".into(),
    ))
}

fn generate_device_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("codex-{nanos:x}-{:x}", std::process::id())
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
        .map_err(|error| Error::Io(format!("failed to open MyChart state file {}: {error}", path.display())))?;
    file.write_all(contents).map_err(|error| {
        Error::Io(format!(
            "failed to write MyChart state file {}: {error}",
            path.display()
        ))
    })?;
    file.sync_all().map_err(|error| {
        Error::Io(format!(
            "failed to flush MyChart state file {}: {error}",
            path.display()
        ))
    })?;
    Ok(())
}
