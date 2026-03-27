use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
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

    pub(crate) fn write_temp_file(
        &mut self,
        prefix: &str,
        extension: &str,
        contents: &str,
    ) -> Result<PathBuf> {
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
        fs::write(&path, contents)
            .map_err(|error| Error::Execution(format!("failed to write temp file {}: {error}", path.display())))?;
        self.temp_files.push(TempFileArtifact { path: path.clone() });

        Ok(path)
    }

    pub(crate) fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }

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
