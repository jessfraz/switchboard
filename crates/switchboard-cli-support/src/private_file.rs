use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
#[error("failed to {operation} {}: {source}", path.display())]
pub struct PrivateFileError {
    operation: &'static str,
    path: PathBuf,
    #[source]
    source: io::Error,
}

impl PrivateFileError {
    fn new(operation: &'static str, path: &Path, source: io::Error) -> Self {
        Self {
            operation,
            path: path.to_owned(),
            source,
        }
    }
}

/// Replace a state file after flushing its complete contents, with mode 0600 on Unix.
/// The temporary file is on the same filesystem and belongs to this writer alone.
pub fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), PrivateFileError> {
    let parent = path.parent().ok_or_else(|| {
        PrivateFileError::new(
            "resolve parent of",
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "no parent directory"),
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| PrivateFileError::new("create directory", parent, error))?;
    let (temp_path, mut file) = create_temp_file(parent)?;
    let result = (|| {
        file.write_all(contents)
            .map_err(|error| PrivateFileError::new("write", &temp_path, error))?;
        file.sync_all()
            .map_err(|error| PrivateFileError::new("flush", &temp_path, error))?;
        // Close before replacement so the same lifecycle works on Windows.
        drop(file);
        fs::rename(&temp_path, path).map_err(|error| PrivateFileError::new("replace", path, error))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn create_temp_file(parent: &Path) -> Result<(PathBuf, File), PrivateFileError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    // Exclusive creation prevents concurrent saves or stale files from sharing a
    // staging file. Retrying collisions does not modify a file owned by another writer.
    for attempt in 0..100 {
        let temp_path = parent.join(format!(".switchboard-state.{}.{attempt}.tmp", std::process::id()));
        match options.open(&temp_path) {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(PrivateFileError::new("create temporary file", &temp_path, error)),
        }
    }
    Err(PrivateFileError::new(
        "create temporary file in",
        parent,
        io::Error::new(io::ErrorKind::AlreadyExists, "temporary file names are occupied"),
    ))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn test_directory(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("valid clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("switchboard-private-{name}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).expect("create fixture directory");
        path
    }

    #[test]
    fn replacement_is_complete_private_and_leaves_no_staging_file() {
        let root = test_directory("replace");
        let path = root.join("nested/state.json");
        write_private_file(&path, b"old and longer contents").expect("initial state");
        write_private_file(&path, b"new").expect("replace state");
        assert_eq!(fs::read(&path).expect("read state"), b"new");
        assert_eq!(
            fs::read_dir(path.parent().expect("parent"))
                .expect("list directory")
                .count(),
            1
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).expect("state metadata").permissions().mode() & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn failed_replacement_preserves_destination_and_removes_temporary_file() {
        let root = test_directory("failure");
        let destination = root.join("existing-directory");
        fs::create_dir(&destination).expect("create destination");
        let sentinel = destination.join("existing");
        fs::write(&sentinel, b"untouched").expect("write sentinel");
        let error = write_private_file(&destination, b"new state").expect_err("cannot replace directory");
        assert_eq!(error.operation, "replace");
        assert_eq!(fs::read(&sentinel).expect("read sentinel"), b"untouched");
        assert_eq!(fs::read_dir(&root).expect("list directory").count(), 1);
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn concurrent_writers_publish_whole_files() {
        let root = test_directory("concurrent");
        let path = root.join("state.json");
        std::thread::scope(|scope| {
            for byte in 0..8 {
                let path = &path;
                scope.spawn(move || {
                    let contents = vec![byte; 64 * 1024];
                    for _ in 0..8 {
                        write_private_file(path, &contents).expect("save concurrently");
                    }
                });
            }
        });
        let contents = fs::read(&path).expect("read state");
        assert_eq!(contents.len(), 64 * 1024);
        assert!(contents.iter().all(|byte| *byte == contents[0]));
        assert_eq!(fs::read_dir(&root).expect("list directory").count(), 1);
        fs::remove_dir_all(root).expect("remove fixture");
    }
}
