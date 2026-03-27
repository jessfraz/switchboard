use std::{
    env,
    path::{Path, PathBuf},
};

use switchboard_core::{Error, Result};

use crate::cli::command::CliBinarySpec;

pub(crate) trait CliLocator: Send + Sync {
    fn resolve(&self, binary: &CliBinarySpec) -> Result<PathBuf>;
}

pub(crate) struct DefaultCliLocator;

impl CliLocator for DefaultCliLocator {
    fn resolve(&self, binary: &CliBinarySpec) -> Result<PathBuf> {
        if let Some(env_override) = binary.env_override.as_deref() {
            if let Some(candidate) = env::var_os(env_override).filter(|value| !value.is_empty()) {
                let path = PathBuf::from(candidate);
                if is_executable_candidate(&path) {
                    return Ok(path);
                }

                return Err(Error::Execution(format!(
                    "binary override {env_override} points to {}, but that file is not executable",
                    path.display()
                )));
            }
        }

        resolve_on_path(&binary.program).ok_or_else(|| {
            Error::Execution(format!(
                "failed to locate {} on PATH{}",
                binary.program,
                binary
                    .env_override
                    .as_deref()
                    .map(|name| format!(" or via {name}"))
                    .unwrap_or_default()
            ))
        })
    }
}

fn resolve_on_path(program: &str) -> Option<PathBuf> {
    let program_path = Path::new(program);
    if program_path.components().count() > 1 {
        return is_executable_candidate(program_path).then(|| program_path.to_path_buf());
    }

    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .flat_map(|directory| candidate_paths(&directory, program))
        .find(|candidate| is_executable_candidate(candidate))
}

fn candidate_paths(directory: &Path, program: &str) -> Vec<PathBuf> {
    let base = directory.join(program);
    if has_extension(program) {
        return vec![base];
    }

    #[cfg(windows)]
    {
        let mut candidates = vec![base.clone()];
        if let Some(extensions) = env::var_os("PATHEXT") {
            for extension in env::split_paths(&extensions) {
                let extension = extension.to_string_lossy();
                if !extension.is_empty() {
                    candidates.push(directory.join(format!("{program}{extension}")));
                }
            }
        }
        candidates
    }

    #[cfg(not(windows))]
    {
        vec![base]
    }
}

fn has_extension(program: &str) -> bool {
    Path::new(program).extension().is_some()
}

fn is_executable_candidate(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        path::{Path, PathBuf},
        process,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{CliBinarySpec, CliLocator, DefaultCliLocator};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn resolves_binary_from_override_env_var() {
        let fixture = TempExecutable::new("switchboard-cli-locator-test");
        env::set_var("SWITCHBOARD_TEST_BIN", fixture.path());
        let locator = DefaultCliLocator;
        let binary = CliBinarySpec {
            program: "definitely-not-real".to_owned(),
            env_override: Some("SWITCHBOARD_TEST_BIN".to_owned()),
            version_args: vec!["--version".to_owned()],
        };

        let resolved = locator.resolve(&binary).expect("override path should resolve");

        assert_eq!(resolved, fixture.path());
    }

    struct TempExecutable {
        directory: PathBuf,
        path: PathBuf,
    }

    impl TempExecutable {
        fn new(name: &str) -> Self {
            let directory = env::temp_dir().join(format!(
                "switchboard-cli-locator-{}-{}-{}",
                process::id(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system time should be after unix epoch")
                    .as_nanos()
            ));
            fs::create_dir_all(&directory).expect("fixture directory should exist");
            let path = directory.join(name);
            fs::write(&path, "#!/bin/sh\nexit 0\n").expect("fixture script should be written");
            make_executable(&path);

            Self { directory, path }
        }

        fn path(&self) -> PathBuf {
            self.path.clone()
        }
    }

    impl Drop for TempExecutable {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)
            .expect("fixture script metadata should exist")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("fixture script should be executable");
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}
}
