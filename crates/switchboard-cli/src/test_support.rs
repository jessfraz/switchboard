use std::{
    env, fs,
    path::{Path, PathBuf},
    process,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static ENV_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct TempScript {
    directory: PathBuf,
    path: PathBuf,
    capture_path: PathBuf,
}

impl TempScript {
    pub(crate) fn new(name: &str, body: &str) -> Self {
        let directory = env::temp_dir().join(format!(
            "switchboard-cli-test-{}-{}-{}",
            process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&directory).expect("test fixture directory should exist");
        let path = directory.join(name);
        fs::write(&path, body).expect("test fixture script should be written");
        make_executable(&path);
        let capture_path = directory.join("env.txt");

        Self {
            directory,
            path,
            capture_path,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn capture_contents(&self) -> String {
        fs::read_to_string(&self.capture_path).unwrap_or_default()
    }
}

impl Drop for TempScript {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

pub(crate) fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    match ENV_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .expect("test fixture script metadata should exist")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("test fixture script should be executable");
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}
