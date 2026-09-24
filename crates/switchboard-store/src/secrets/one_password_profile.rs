use std::{fs, io::Read, path::Path};

use sha2::{Digest, Sha256};
use switchboard_core::{Error, Result, SecretString};

/// A freshly checked bootstrap credential. Never persist or print its contents.
pub(super) struct ProfileToken {
    pub token: SecretString,
    pub generation: String,
}

pub(super) fn read_token(path: &Path) -> Result<ProfileToken> {
    let invalid = || {
        Error::Config(format!(
        "1Password token file {} must be a regular, owner-only file in an owner-only directory owned by the current user",
        path.display()
    ))
    };
    let before = fs::symlink_metadata(path).map_err(|_| invalid())?;
    if !before.is_file() || before.len() > 16_384 {
        return Err(invalid());
    }
    let parent = path.parent().ok_or_else(invalid)?;
    let directory = fs::symlink_metadata(parent).map_err(|_| invalid())?;
    if !directory.is_dir() {
        return Err(invalid());
    }
    let file = fs::File::open(path).map_err(|_| invalid())?;
    let metadata = file.metadata().map_err(|_| invalid())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        extern "C" {
            fn geteuid() -> u32;
        }
        // SAFETY: geteuid has no arguments and returns this process's effective UID.
        let uid = unsafe { geteuid() };
        if !metadata.is_file()
            || before.dev() != metadata.dev()
            || before.ino() != metadata.ino()
            || metadata.uid() != uid
            || directory.uid() != uid
            || metadata.mode() & 0o077 != 0
            || directory.mode() & 0o077 != 0
        {
            return Err(invalid());
        }
    }
    #[cfg(not(unix))]
    {
        return Err(Error::Config(
            "1Password token-file profiles require Unix file permission validation".into(),
        ));
    }
    let mut token = String::new();
    file.take(16_385).read_to_string(&mut token).map_err(|_| invalid())?;
    let token = token.trim();
    if token.is_empty() || token.len() > 16_384 || token.chars().any(char::is_whitespace) {
        return Err(Error::Config(format!(
            "1Password token file {} is empty or invalid",
            path.display()
        )));
    }
    Ok(ProfileToken {
        generation: Sha256::digest(token.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        token: token.to_owned().into(),
    })
}
