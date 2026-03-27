use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::Write,
    path::PathBuf,
    process,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use switchboard_core::{Error, Result};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct ProcessContext {
    env: BTreeMap<String, String>,
    cleared_env: BTreeSet<String>,
    temp_files: Vec<TempFileArtifact>,
}

impl ProcessContext {
    pub(crate) fn new() -> Self {
        Self {
            env: BTreeMap::new(),
            cleared_env: BTreeSet::new(),
            temp_files: Vec::new(),
        }
    }

    pub(crate) fn set_env(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        self.cleared_env.remove(&key);
        self.env.insert(key, value.into());
    }

    pub(crate) fn clear_env(&mut self, key: impl Into<String>) {
        let key = key.into();
        self.env.remove(&key);
        self.cleared_env.insert(key);
    }

    pub(crate) fn write_temp_file(&mut self, prefix: &str, extension: &str, contents: &str) -> Result<PathBuf> {
        for _attempt in 0..16 {
            let path = env::temp_dir().join(format!(
                "{prefix}-{}-{}-{}.{}",
                process::id(),
                TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| Error::InvalidArguments(format!("system clock is before unix epoch: {error}")))?
                    .as_nanos(),
                extension
            ));

            match create_private_temp_file(&path, contents) {
                Ok(()) => {
                    self.temp_files.push(TempFileArtifact { path: path.clone() });
                    return Ok(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(Error::Execution(format!(
                        "failed to write temp file {}: {error}",
                        path.display()
                    )));
                }
            }
        }

        Err(Error::Execution(format!(
            "failed to allocate a unique temp file for prefix {prefix}"
        )))
    }

    #[cfg(test)]
    pub(crate) fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }

    #[cfg(test)]
    pub(crate) fn cleared_env(&self) -> &BTreeSet<String> {
        &self.cleared_env
    }
}

struct TempFileArtifact {
    path: PathBuf,
}

impl Drop for TempFileArtifact {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn create_private_temp_file(path: &PathBuf, contents: &str) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options.open(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    Ok(())
}
