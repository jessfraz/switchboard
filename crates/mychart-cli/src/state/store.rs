use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

use super::MyChartState;
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
