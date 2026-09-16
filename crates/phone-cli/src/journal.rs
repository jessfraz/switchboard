//! Each record is independently encrypted and synced before the next event.
//! A killed process can lose its current record, but not earlier utterances.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use age::x25519;
use serde::{Deserialize, Serialize};

use crate::domain::{CallEvent, CallId, CallOutcome, CallRequest};
use crate::error::CallError;

const MAX_RECORD_BYTES: u64 = 2_097_152;
const MAX_RECORDS: usize = 20_000;
const MAX_TRANSCRIPT_BYTES: usize = 134_217_728;

#[derive(Serialize, Deserialize)]
pub struct JournalRecord {
    pub version: u32,
    pub call_id: CallId,
    pub sequence: u64,
    pub record: Record,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Record {
    Intent(CallRequest),
    Event(CallEvent),
    Outcome(CallOutcome),
}

pub struct Journal {
    directory: PathBuf,
    call_id: CallId,
    recipient: x25519::Recipient,
    sequence: u64,
}

pub fn recipient(value: &str) -> Result<x25519::Recipient, CallError> {
    value
        .parse()
        .map_err(|_| CallError::Configuration("transcript_recipient must be an age X25519 public key".into()))
}

pub fn check_encryption(value: &str) -> Result<(), CallError> {
    encrypt(&recipient(value)?, b"phone encryption preflight").map(|_| ())
}

fn encrypt(recipient: &x25519::Recipient, plaintext: &[u8]) -> Result<Vec<u8>, CallError> {
    age::encrypt(recipient, plaintext).map_err(|_| CallError::Storage("could not encrypt transcript record".into()))
}

impl Journal {
    pub fn create(root: &Path, recipient: &str, call_id: CallId) -> Result<Self, CallError> {
        let recipient = crate::journal::recipient(recipient)?;
        // Fail before reserving a call ID if encryption is unavailable.
        encrypt(&recipient, b"phone encryption preflight")?;
        private_directory(root)?;
        let directory = root.join(call_id.to_string());
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                CallError::DuplicateCall
            } else {
                CallError::Storage("could not reserve call directory".into())
            }
        })?;
        sync_directory(root)?;
        Ok(Self {
            directory,
            call_id,
            recipient,
            sequence: 0,
        })
    }

    pub fn path(&self) -> &Path {
        &self.directory
    }

    pub fn append(&mut self, record: Record) -> Result<(), CallError> {
        let record = JournalRecord {
            version: 1,
            call_id: self.call_id.clone(),
            sequence: self.sequence,
            record,
        };
        let plaintext =
            serde_json::to_vec(&record).map_err(|_| CallError::Storage("could not encode transcript record".into()))?;
        let ciphertext = encrypt(&self.recipient, &plaintext)?;
        let path = self.directory.join(format!("{:08}.age", self.sequence));
        let pending = self.directory.join(".record.pending");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options
            .open(&pending)
            .map_err(|_| CallError::Storage("could not create encrypted record".into()))?;
        file.write_all(&ciphertext)
            .and_then(|()| file.sync_all())
            .map_err(|_| CallError::Storage("could not persist encrypted record".into()))?;
        // Publish only a complete ciphertext, without replacing any committed
        // record. A crash leaves either the old prefix or the full new record.
        fs::hard_link(&pending, path).map_err(|_| CallError::Storage("could not publish encrypted record".into()))?;
        sync_directory(&self.directory)?;
        fs::remove_file(pending).map_err(|_| CallError::Storage("could not clean encrypted staging record".into()))?;
        sync_directory(&self.directory)?;
        self.sequence += 1;
        Ok(())
    }
}

fn sync_directory(path: &Path) -> Result<(), CallError> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| CallError::Storage("could not persist transcript directory".into()))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Symlink ancestors are rejected. The private root is not shared with other
/// users; concurrent callers can only claim distinct IDs using atomic mkdir.
pub fn private_directory(path: &Path) -> Result<(), CallError> {
    if !path.is_absolute() {
        return Err(CallError::Storage("transcript directory must be absolute".into()));
    }
    for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(CallError::Storage("transcript paths must not contain symlinks".into()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                if let Err(error) = builder.create(ancestor) {
                    if error.kind() != std::io::ErrorKind::AlreadyExists {
                        return Err(CallError::Storage("could not create transcript directory".into()));
                    }
                }
            }
            Err(_) => return Err(CallError::Storage("could not inspect transcript directory".into())),
        }
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|_| CallError::Storage("could not inspect transcript directory".into()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(CallError::Storage("transcript directory must not be a symlink".into()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(CallError::Storage("transcript directory must have mode 0700".into()));
        }
    }
    #[cfg(not(unix))]
    return Err(CallError::Storage(
        "encrypted journal permissions currently require Unix".into(),
    ));
    #[allow(unreachable_code)]
    Ok(())
}

pub fn list(root: &Path) -> Result<Vec<CallId>, CallError> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    private_directory(root)?;
    let mut calls = Vec::new();
    for entry in fs::read_dir(root).map_err(|_| CallError::Storage("could not list transcript directory".into()))? {
        let entry = entry.map_err(|_| CallError::Storage("could not read transcript entry".into()))?;
        let file_type = entry
            .file_type()
            .map_err(|_| CallError::Storage("could not inspect transcript entry".into()))?;
        if file_type.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                if let Ok(id) = name.parse() {
                    calls.push(id);
                }
            }
        }
    }
    calls.sort_by_key(ToString::to_string);
    Ok(calls)
}

pub fn read(root: &Path, call_id: &CallId, identity: &x25519::Identity) -> Result<Vec<JournalRecord>, CallError> {
    let directory = root.join(call_id.to_string());
    if !directory.exists() {
        return Err(CallError::Storage("call transcript does not exist".into()));
    }
    private_directory(&directory)?;
    let mut paths = fs::read_dir(&directory)
        .map_err(|_| CallError::Storage("could not list call records".into()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CallError::Storage("could not inspect call records".into()))?;
    paths.sort();
    paths.retain(|path| path.file_name().and_then(|name| name.to_str()) != Some(".record.pending"));
    if paths.len() > MAX_RECORDS {
        return Err(CallError::Storage("transcript contains too many records".into()));
    }
    let mut records = Vec::new();
    let mut total_bytes = 0;
    for (sequence, path) in paths.iter().enumerate() {
        if path.file_name().and_then(|name| name.to_str()) != Some(format!("{sequence:08}.age").as_str()) {
            return Err(CallError::Storage(
                "transcript has missing or unexpected records".into(),
            ));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(path)
            .map_err(|_| CallError::Storage("could not open encrypted record".into()))?;
        if !file
            .metadata()
            .map_err(|_| CallError::Storage("could not inspect encrypted record".into()))?
            .is_file()
        {
            return Err(CallError::Storage("transcript record must be a regular file".into()));
        }
        let mut encrypted = Vec::new();
        file.take(MAX_RECORD_BYTES + 1)
            .read_to_end(&mut encrypted)
            .map_err(|_| CallError::Storage("could not read encrypted record".into()))?;
        if encrypted.len() > MAX_RECORD_BYTES as usize {
            return Err(CallError::Storage("encrypted record exceeds size limit".into()));
        }
        total_bytes += encrypted.len();
        if total_bytes > MAX_TRANSCRIPT_BYTES {
            return Err(CallError::Storage("transcript exceeds total size limit".into()));
        }
        let plaintext = age::decrypt(identity, &encrypted)
            .map_err(|_| CallError::Storage("could not decrypt transcript record".into()))?;
        let record: JournalRecord =
            serde_json::from_slice(&plaintext).map_err(|_| CallError::Storage("invalid transcript record".into()))?;
        if record.version != 1 || record.call_id != *call_id || record.sequence != sequence as u64 {
            return Err(CallError::Storage(
                "transcript record identity or sequence mismatch".into(),
            ));
        }
        records.push(record);
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use crate::domain::{CallEvent, CallId};
    use crate::error::CallError;
    use crate::journal::{read, Journal, Record};

    #[test]
    fn encrypted_records_survive_reopen_and_duplicate_ids_never_redial() {
        let temp = tempfile::tempdir().expect("test fixture should succeed");
        let root = temp
            .path()
            .canonicalize()
            .expect("test fixture should succeed")
            .join("calls");
        let identity = age::x25519::Identity::generate();
        let id = CallId::new();
        let mut journal =
            Journal::create(&root, &identity.to_public().to_string(), id.clone()).expect("test fixture should succeed");
        journal
            .append(Record::Event(CallEvent::ApprovalRequired {
                reason: "private transcript text".into(),
            }))
            .expect("test fixture should succeed");
        drop(journal);
        let bytes = std::fs::read(root.join(id.to_string()).join("00000000.age")).expect("test fixture should succeed");
        assert!(!bytes.windows(23).any(|part| part == b"private transcript text"));
        let records = read(&root, &id, &identity).expect("test fixture should succeed");
        assert!(
            matches!(&records[0].record, Record::Event(CallEvent::ApprovalRequired { reason }) if reason == "private transcript text")
        );
        assert!(matches!(
            Journal::create(&root, &identity.to_public().to_string(), id),
            Err(CallError::DuplicateCall)
        ));
    }

    #[test]
    fn wrong_identity_and_reordered_ciphertext_fail_closed() {
        let temp = tempfile::tempdir().expect("test fixture should succeed");
        let root = temp
            .path()
            .canonicalize()
            .expect("test fixture should succeed")
            .join("calls");
        let identity = age::x25519::Identity::generate();
        let id = CallId::new();
        let mut journal =
            Journal::create(&root, &identity.to_public().to_string(), id.clone()).expect("test fixture should succeed");
        journal
            .append(Record::Event(CallEvent::Ready))
            .expect("test fixture should succeed");
        journal
            .append(Record::Event(CallEvent::Dialing))
            .expect("test fixture should succeed");
        assert!(read(&root, &id, &age::x25519::Identity::generate()).is_err());
        std::fs::copy(journal.path().join("00000001.age"), journal.path().join("00000000.age"))
            .expect("test fixture should succeed");
        assert!(read(&root, &id, &identity).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_storage_is_rejected() {
        let temp = tempfile::tempdir().expect("test fixture should succeed");
        let root = temp.path().canonicalize().expect("test fixture should succeed");
        std::os::unix::fs::symlink(&root, root.join("alias")).expect("test fixture should succeed");
        assert!(Journal::create(
            &root.join("alias/calls"),
            &age::x25519::Identity::generate().to_public().to_string(),
            CallId::new()
        )
        .is_err());
    }

    #[test]
    fn interrupted_ciphertext_staging_does_not_hide_committed_records() {
        let temp = tempfile::tempdir().expect("test fixture should succeed");
        let root = temp
            .path()
            .canonicalize()
            .expect("test fixture should succeed")
            .join("calls");
        let identity = age::x25519::Identity::generate();
        let id = CallId::new();
        let mut journal =
            Journal::create(&root, &identity.to_public().to_string(), id.clone()).expect("test fixture should succeed");
        journal
            .append(Record::Event(CallEvent::Ready))
            .expect("test fixture should succeed");
        std::fs::write(journal.path().join(".record.pending"), b"partial ciphertext")
            .expect("test fixture should succeed");
        assert_eq!(
            read(&root, &id, &identity).expect("test fixture should succeed").len(),
            1
        );
    }
}
