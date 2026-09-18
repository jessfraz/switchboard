use std::{
    env, fs,
    path::{Path, PathBuf},
};

use crate::state::MyChartState;
use switchboard_cli_support::private_file::write_private_file;

use crate::{Error, Result};

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
        self.path
            .parent()
            .ok_or_else(|| Error::Config(format!("invalid MyChart state path {}", self.path.display())))?;
        let contents = serde_json::to_vec_pretty(state)
            .map_err(|error| Error::Config(format!("failed to serialize MyChart state: {error}")))?;
        write_private_file(&self.path, &contents).map_err(|error| {
            Error::Io(format!(
                "failed to save MyChart state at {}: {error}",
                self.path.display()
            ))
        })
    }

    pub(crate) fn sibling_path(&self, name: &str) -> Result<PathBuf> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::Config(format!("invalid MyChart state path {}", self.path.display())))?;
        Ok(parent.join(name))
    }
}

pub(super) fn resolve_state_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
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

fn env_value(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}
