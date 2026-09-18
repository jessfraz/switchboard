use std::{
    path::{Path, PathBuf},
    process,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection};
use switchboard_core::{Error, ResolvedAuth, Result};

/// A durable claim is made before human-presence authentication, so crashes and
/// parallel CLI processes cannot silently grant another attempt in the same run.
pub(super) struct RecoveryBudget {
    run_id: String,
    path: Option<PathBuf>,
    attempted: Mutex<Vec<(String, String)>>,
}

impl RecoveryBudget {
    pub(super) fn new(cache_path: Option<&Path>, run_id: Option<String>) -> Self {
        let run_id = run_id.unwrap_or_else(|| {
            format!(
                "process-{}-{}",
                process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            )
        });
        Self {
            run_id,
            path: cache_path
                .and_then(Path::parent)
                .map(|path| path.join("auth-recovery.sqlite3")),
            attempted: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn claim(&self, auth: &ResolvedAuth) -> Result<RecoveryAttempt> {
        if self.run_id.trim().is_empty() || self.run_id.len() > 256 {
            return Err(Error::InvalidArguments("run ID must contain 1 to 256 bytes".into()));
        }
        let key = (auth.provider().to_string(), auth.account_label().trim().to_lowercase());
        let exhausted = || {
            Error::RecoveryExhausted(format!("{} already used its one recovery attempt for run {}; cached credentials remain usable, but further unlocks are blocked", auth.id(), self.run_id))
        };
        if let Some(path) = &self.path {
            let connection = Connection::open(path).map_err(storage_error)?;
            let wait = switchboard_core::process::remaining_timeout(Duration::from_secs(5))
                .map_err(|_| Error::AuthenticationTimeout { seconds: 5 })?;
            connection.busy_timeout(wait).map_err(storage_error)?;
            connection.execute_batch("CREATE TABLE IF NOT EXISTS recovery_attempts (run_id TEXT NOT NULL, provider TEXT NOT NULL, account TEXT NOT NULL, started_ms INTEGER NOT NULL, finished INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (run_id, provider, account));").map_err(storage_error)?;
            let inserted = connection
                .execute(
                    "INSERT OR IGNORE INTO recovery_attempts (run_id, provider, account, started_ms) VALUES (?1, ?2, ?3, ?4)",
                    params![self.run_id, key.0, key.1, epoch_ms()],
                )
                .map_err(storage_error)?;
            if inserted == 0 {
                let timeout = switchboard_core::process::remaining_timeout(Duration::from_secs(60))
                    .map_err(|error| Error::RecoveryExhausted(error.to_string()))?;
                let started = Instant::now();
                loop {
                    let (attempted_at, finished): (i64, bool) = connection.query_row(
                        "SELECT started_ms, finished FROM recovery_attempts WHERE run_id = ?1 AND provider = ?2 AND account = ?3",
                        params![self.run_id, key.0, key.1], |row| Ok((row.get(0)?, row.get(1)?)),
                    ).map_err(storage_error)?;
                    if finished || epoch_ms().saturating_sub(attempted_at) >= 60_000 || started.elapsed() >= timeout {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20).min(timeout.saturating_sub(started.elapsed())));
                }
                return Err(exhausted());
            }
        } else {
            let mut attempted = self
                .attempted
                .lock()
                .map_err(|_| Error::Config("auth recovery lock was poisoned".into()))?;
            if attempted.contains(&key) {
                return Err(exhausted());
            }
            attempted.push(key.clone());
        }
        Ok(RecoveryAttempt {
            path: self.path.clone(),
            run_id: self.run_id.clone(),
            provider: key.0,
            account: key.1,
        })
    }
}

pub(super) struct RecoveryAttempt {
    path: Option<PathBuf>,
    run_id: String,
    provider: String,
    account: String,
}

impl Drop for RecoveryAttempt {
    fn drop(&mut self) {
        // The durable claim survives crashes. Waiters use its 60-second deadline
        // if this completion marker could not be written.
        if let Some(path) = &self.path {
            if let Ok(connection) = Connection::open(path) {
                let _ = connection.execute(
                    "UPDATE recovery_attempts SET finished = 1 WHERE run_id = ?1 AND provider = ?2 AND account = ?3",
                    params![self.run_id, self.provider, self.account],
                );
            }
        }
    }
}

fn epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

fn storage_error(error: rusqlite::Error) -> Error {
    Error::Config(format!("could not persist authentication recovery budget: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_claim_survives_reopening_and_is_per_provider_account_run() {
        let directory = std::env::temp_dir().join(format!(
            "switchboard-budget-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test fixture should be valid")
                .as_nanos()
        ));
        std::fs::create_dir(&directory).expect("test fixture should be valid");
        let cache = directory.join("sessions.json");
        let namespace = ResolvedAuth::new(
            "google.personal",
            "person@example.com",
            switchboard_core::AuthSecretRefs::GoogleCli,
        )
        .expect("test fixture should be valid");
        let first = RecoveryBudget::new(Some(&cache), Some("task-1".into()));
        first.claim(&namespace).expect("test fixture should be valid");
        let reopened = RecoveryBudget::new(Some(&cache), Some("task-1".into()));
        assert!(matches!(reopened.claim(&namespace), Err(Error::RecoveryExhausted(_))));
        let alias = ResolvedAuth::new(
            "google.alias",
            " PERSON@example.com ",
            switchboard_core::AuthSecretRefs::GoogleCli,
        )
        .expect("test fixture should be valid");
        assert!(matches!(reopened.claim(&alias), Err(Error::RecoveryExhausted(_))));
        RecoveryBudget::new(Some(&cache), Some("task-2".into()))
            .claim(&namespace)
            .expect("test fixture should be valid");
        std::fs::remove_dir_all(directory).expect("test fixture should be valid");
    }
}
