use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, TryLockError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, ErrorCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use switchboard_core::{Error, Result};

use crate::secrets::one_password::{unix_timestamp_now, update_cache};

const QUOTA_REASON: &str = "1Password request quota is exhausted; no desktop fallback was attempted";

#[derive(Clone, Copy)]
pub(super) enum Scope {
    Profile,
    Account,
}

pub(super) struct Quota {
    pub scope: Option<Scope>,
    pub retry_after: Option<u64>,
}

/// Only documented rate-limit cues and numeric durations are retained. Child
/// output may contain bootstrap credentials and must never reach errors/storage.
pub(super) fn quota(stderr: &[u8]) -> Option<Quota> {
    let text = String::from_utf8_lossy(stderr);
    if !text.contains("(429) Too Many Requests:") {
        return None;
    }
    let scope = if text.contains("this service account is allowed to make.") {
        Some(Scope::Profile)
    } else if text.contains("this 1Password account is allowed to make.") {
        Some(Scope::Account)
    } else {
        None
    };
    let retry_after = text
        .split_once("Please retry in ")
        .and_then(|(_, rest)| rest.split_once(" or try other requests."))
        .and_then(|(duration, _)| parse_duration(duration));
    Some(Quota { scope, retry_after })
}

fn parse_duration(value: &str) -> Option<u64> {
    if value.len() > 80 {
        return None;
    }
    let words = value.split_whitespace().collect::<Vec<_>>();
    let mut words = words.as_slice();
    let mut previous_unit = u64::MAX;
    let mut total = 0u64;
    let mut count = 0;
    while let [number, unit, rest @ ..] = words {
        if number.len() > 2 || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let number = number.parse::<u64>().ok()?;
        let (unit, maximum) = match *unit {
            "hour" | "hours" => (3600, 24),
            "minute" | "minutes" => (60, 59),
            "second" | "seconds" => (1, 59),
            _ => return None,
        };
        if number > maximum || unit >= previous_unit || count == 3 {
            return None;
        }
        previous_unit = unit;
        total += number * unit;
        count += 1;
        words = match rest {
            [] => &[],
            ["and", remaining @ ..] if !remaining.is_empty() => remaining,
            _ => return None,
        };
    }
    (words.is_empty() && total > 0 && total <= 86_400).then_some(total)
}

pub(super) fn limited(seconds: u64) -> Error {
    Error::RateLimited {
        retry_after_seconds: seconds,
        reason: QUOTA_REASON.into(),
    }
}

#[derive(Default, Deserialize, Serialize)]
struct Cooldowns {
    deadlines: BTreeMap<String, u64>,
}

pub(super) struct Limits {
    directory: Option<PathBuf>,
    memory: Mutex<Cooldowns>,
    requests: Mutex<()>,
}

impl Limits {
    pub(super) fn new(cache_path: Option<&Path>) -> Self {
        Self {
            directory: cache_path.and_then(Path::parent).map(Path::to_path_buf),
            memory: Mutex::new(Cooldowns::default()),
            requests: Mutex::new(()),
        }
    }

    /// Different items using the same service profile share quota and therefore
    /// coordinate their cold lookups. The credential-cache write lock is separate.
    pub(super) fn acquire(&self, profile: &str, generation: &str, timeout: Duration) -> Result<Lease<'_>> {
        let started = Instant::now();
        let lease = if let Some(directory) = &self.directory {
            let directory = directory.join("onepassword-lookups");
            fs::create_dir_all(&directory).map_err(storage_error)?;
            let connection = Connection::open(directory.join(format!("{}.sqlite3", profile_key(profile, generation))))
                .map_err(storage_error)?;
            connection.busy_timeout(timeout).map_err(storage_error)?;
            connection.execute_batch("BEGIN IMMEDIATE").map_err(|error| {
                if matches!(
                    error.sqlite_error_code(),
                    Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                ) {
                    timed_out(timeout)
                } else {
                    storage_error(error)
                }
            })?;
            Lease {
                _connection: Some(connection),
                _memory: None,
                started,
                timeout,
            }
        } else {
            let guard = loop {
                match self.requests.try_lock() {
                    Ok(guard) => break guard,
                    Err(TryLockError::Poisoned(_)) => return Err(storage_error("lookup lock poisoned")),
                    Err(TryLockError::WouldBlock) if started.elapsed() < timeout => {
                        std::thread::sleep(Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())));
                    }
                    Err(TryLockError::WouldBlock) => return Err(timed_out(timeout)),
                }
            };
            Lease {
                _connection: None,
                _memory: Some(guard),
                started,
                timeout,
            }
        };
        lease.remaining()?;
        Ok(lease)
    }

    pub(super) fn check(&self, profile: &str, generation: &str, account: &str) -> Result<()> {
        let memory = self.memory.lock().map_err(|_| storage_error("quota lock poisoned"))?;
        let disk = self.read()?;
        let deadline = [profile_key(profile, generation), account_key(account)]
            .iter()
            .flat_map(|key| [memory.deadlines.get(key), disk.deadlines.get(key)])
            .flatten()
            .copied()
            .max()
            .unwrap_or_default();
        let remaining = deadline.saturating_sub(unix_timestamp_now());
        if remaining > 0 {
            Err(limited(remaining))
        } else {
            Ok(())
        }
    }

    pub(super) fn record(
        &self,
        scope: Scope,
        profile: &str,
        generation: &str,
        account: &str,
        seconds: u64,
    ) -> Result<()> {
        let key = match scope {
            Scope::Profile => profile_key(profile, generation),
            Scope::Account => account_key(account),
        };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let deadline = cooldown_deadline(now, seconds);
        let mut memory = self.memory.lock().map_err(|_| storage_error("quota lock poisoned"))?;
        memory
            .deadlines
            .entry(key.clone())
            .and_modify(|old| *old = (*old).max(deadline))
            .or_insert(deadline);
        let path = self
            .directory
            .as_ref()
            .map(|directory| directory.join("onepassword-limits.json"));
        update_cache(path.as_deref(), |limits: &mut Cooldowns| {
            limits.deadlines.retain(|_, deadline| *deadline > unix_timestamp_now());
            limits
                .deadlines
                .entry(key)
                .and_modify(|old| *old = (*old).max(deadline))
                .or_insert(deadline);
        })
        .map_err(storage_error)
    }

    fn read(&self) -> Result<Cooldowns> {
        let Some(directory) = &self.directory else {
            return Ok(Cooldowns::default());
        };
        match fs::read(directory.join("onepassword-limits.json")) {
            Ok(contents) => serde_json::from_slice(&contents).map_err(storage_error),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Cooldowns::default()),
            Err(error) => Err(storage_error(error)),
        }
    }
}

pub(super) struct Lease<'a> {
    _connection: Option<Connection>,
    _memory: Option<MutexGuard<'a, ()>>,
    started: Instant,
    timeout: Duration,
}

impl Lease<'_> {
    pub(super) fn remaining(&self) -> Result<Duration> {
        self.timeout
            .checked_sub(self.started.elapsed())
            .filter(|value| !value.is_zero())
            .ok_or_else(|| timed_out(self.timeout))
    }
}

fn profile_key(profile: &str, generation: &str) -> String {
    key(&["profile", profile, generation])
}

fn account_key(account: &str) -> String {
    key(&["account", account])
}

fn key(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part.len().to_string().as_bytes());
        hash.update(b":");
        hash.update(part.as_bytes());
    }
    hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn timed_out(timeout: Duration) -> Error {
    Error::AuthenticationTimeout {
        seconds: timeout.as_secs(),
    }
}

fn storage_error(error: impl std::fmt::Display) -> Error {
    Error::Config(format!("could not coordinate 1Password request quota: {error}"))
}

// Persist whole seconds rounded upward so another process never retries before
// the provider's full delay has elapsed, even across a subsecond boundary.
fn cooldown_deadline(now: Duration, seconds: u64) -> u64 {
    now.as_secs()
        .saturating_add(u64::from(now.subsec_nanos() != 0))
        .saturating_add(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_deadline_never_shortens_the_provider_delay() {
        for nanos in [0, 1, 900_000_000, 999_999_999] {
            let now = Duration::new(100, nanos);
            for seconds in [1, 3540, 86340] {
                let requested = now + Duration::from_secs(seconds);
                let persisted = Duration::from_secs(cooldown_deadline(now, seconds));
                assert!(persisted >= requested);
                assert!(persisted - requested < Duration::from_secs(1));
            }
        }
    }
}
