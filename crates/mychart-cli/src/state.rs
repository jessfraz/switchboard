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
pub(crate) const ENV_MYCHART_USERNAME: &str = "MYCHART_USERNAME";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct MyChartState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) base_url: Option<String>,
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
    pub(crate) base_url: String,
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
        let state = store.load()?;

        Ok(Self {
            base_url: pick(global.base_url.clone(), env_value(ENV_MYCHART_BASE_URL), state.base_url.clone()).ok_or_else(
                || {
                    Error::Config(
                        "missing MyChart base URL, pass --base-url, set MYCHART_BASE_URL, or log in once with --base-url so it can be persisted"
                            .into(),
                    )
                },
            )?,
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

    pub(crate) fn has_session(&self) -> bool {
        !self.cookies.is_empty()
    }

    pub(crate) fn require_session(&self) -> Result<()> {
        if self.has_session() {
            Ok(())
        } else {
            Err(Error::Config(
                "missing MyChart session, run mychart auth login-password first".into(),
            ))
        }
    }

    pub(crate) fn require_username(&self, explicit: Option<String>) -> Result<String> {
        explicit
            .or_else(|| self.username.clone())
            .ok_or_else(|| Error::Config("missing username, pass --username or set MYCHART_USERNAME".into()))
    }

    pub(crate) fn update_cookies(&mut self, cookies: BTreeMap<String, String>) {
        self.cookies = cookies;
    }

    pub(crate) fn store_session(&mut self, username: Option<String>) -> Result<()> {
        if let Some(username) = username {
            self.username = Some(username);
        }

        self.state.base_url = Some(self.base_url.clone());
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

    pub(crate) fn clear_session(&mut self) -> Result<()> {
        self.cookies.clear();
        self.state.base_url = Some(self.base_url.clone());
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
