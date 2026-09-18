use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection};
use switchboard_core::{Error, ResolvedAuth, Result};

/// A durable claim is made before human-presence authentication, so crashes and
/// parallel CLI processes cannot silently grant another attempt. Ordinary calls
/// share a one-minute recovery window; explicit runs keep their claim indefinitely.
pub(super) struct RecoveryBudget {
    run_id: Option<String>,
    path: Option<PathBuf>,
    attempted: Mutex<Vec<(String, String)>>,
}

impl RecoveryBudget {
    pub(super) fn new(cache_path: Option<&Path>, run_id: Option<String>) -> Self {
        Self {
            run_id,
            path: cache_path
                .and_then(Path::parent)
                .map(|path| path.join("auth-recovery.sqlite3")),
            attempted: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn claim(&self, auth: &ResolvedAuth) -> Result<RecoveryAttempt> {
        if self
            .run_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty() || id.len() > 256)
        {
            return Err(Error::InvalidArguments("run ID must contain 1 to 256 bytes".into()));
        }
        // Empty explicit IDs are invalid, so this key cannot collide with a run
        // supplied by an automation. Default callers need no shared environment.
        let run_id = self.run_id.as_deref().unwrap_or("");
        let key = (auth.provider().to_string(), auth.account_label().trim().to_lowercase());
        let exhausted = || {
            Error::RecoveryExhausted(match &self.run_id {
                Some(id) => format!("{} already used its one recovery attempt for run {id}; cached credentials remain usable, but further unlocks are blocked", auth.id()),
                None => format!("{} already attempted 1Password recovery recently; cached credentials remain usable, but wait up to 60 seconds before retrying an unlock", auth.id()),
            })
        };
        let started = Instant::now();
        let started_ms = epoch_ms();
        if let Some(path) = &self.path {
            let connection = Connection::open(path).map_err(storage_error)?;
            let wait = switchboard_core::process::remaining_timeout(Duration::from_secs(5))
                .map_err(|_| Error::AuthenticationTimeout { seconds: 5 })?;
            connection.busy_timeout(wait).map_err(storage_error)?;
            connection.execute_batch("CREATE TABLE IF NOT EXISTS recovery_attempts (run_id TEXT NOT NULL, provider TEXT NOT NULL, account TEXT NOT NULL, started_ms INTEGER NOT NULL, finished INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (run_id, provider, account));").map_err(storage_error)?;
            let inserted = connection
                .execute(
                    "INSERT INTO recovery_attempts (run_id, provider, account, started_ms) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (run_id, provider, account) DO UPDATE SET started_ms = excluded.started_ms, finished = 0
                     WHERE recovery_attempts.run_id = '' AND recovery_attempts.started_ms <= ?5",
                    params![run_id, key.0, key.1, started_ms, started_ms.saturating_sub(60_000)],
                )
                .map_err(storage_error)?;
            if inserted == 0 {
                let timeout = switchboard_core::process::remaining_timeout(Duration::from_secs(60))
                    .map_err(|error| Error::RecoveryExhausted(error.to_string()))?;
                let started = Instant::now();
                loop {
                    let (attempted_at, finished): (i64, bool) = connection.query_row(
                        "SELECT started_ms, finished FROM recovery_attempts WHERE run_id = ?1 AND provider = ?2 AND account = ?3",
                        params![run_id, key.0, key.1], |row| Ok((row.get(0)?, row.get(1)?)),
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
            run_id: run_id.to_owned(),
            provider: key.0,
            account: key.1,
            started,
            started_ms,
        })
    }
}

pub(super) struct RecoveryAttempt {
    path: Option<PathBuf>,
    run_id: String,
    provider: String,
    account: String,
    started: Instant,
    started_ms: i64,
}

impl RecoveryAttempt {
    pub(super) fn remaining_timeout(&self, limit: Duration) -> Result<Duration> {
        // Database contention and time spent preparing the command consume the
        // claim's lifetime too, so an unlock cannot outlive its recovery window.
        Duration::from_secs(60)
            .checked_sub(self.started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .map(|remaining| remaining.min(limit))
            .ok_or(Error::AuthenticationTimeout { seconds: 60 })
    }
}

impl Drop for RecoveryAttempt {
    fn drop(&mut self) {
        // The durable claim survives crashes. Waiters use its 60-second deadline
        // if this completion marker could not be written.
        if let Some(path) = &self.path {
            if let Ok(connection) = Connection::open(path) {
                let _ = connection.execute(
                    // An expired automatic claim may have a new owner already.
                    "UPDATE recovery_attempts SET finished = 1 WHERE run_id = ?1 AND provider = ?2 AND account = ?3 AND started_ms = ?4",
                    params![self.run_id, self.provider, self.account, self.started_ms],
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
    use std::process;

    use super::*;

    #[test]
    fn default_recovery_is_shared_without_a_run_id() {
        let directory = std::env::temp_dir().join(format!(
            "switchboard-default-budget-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test fixture should be valid")
                .as_nanos()
        ));
        std::fs::create_dir(&directory).expect("test fixture should be valid");
        let cache = directory.join("sessions.json");
        let auth = ResolvedAuth::new(
            "google.personal",
            "person@example.com",
            switchboard_core::AuthSecretRefs::GoogleCli,
        )
        .expect("test fixture should be valid");
        let first = RecoveryBudget::new(Some(&cache), None);
        drop(first.claim(&auth).expect("first recovery allowed"));
        let reopened = RecoveryBudget::new(Some(&cache), None);
        assert!(matches!(reopened.claim(&auth), Err(Error::RecoveryExhausted(_))));
        let alias = ResolvedAuth::new(
            "google.alias",
            " PERSON@example.com ",
            switchboard_core::AuthSecretRefs::GoogleCli,
        )
        .expect("test fixture should be valid");
        assert!(matches!(reopened.claim(&alias), Err(Error::RecoveryExhausted(_))));
        let other_account = ResolvedAuth::new(
            "google.work",
            "work@example.com",
            switchboard_core::AuthSecretRefs::GoogleCli,
        )
        .expect("test fixture should be valid");
        drop(
            reopened
                .claim(&other_account)
                .expect("other accounts remain independent"),
        );
        let other_provider = ResolvedAuth::new(
            "github.personal",
            "person@example.com",
            switchboard_core::AuthSecretRefs::GitHubCli,
        )
        .expect("test fixture should be valid");
        drop(
            reopened
                .claim(&other_provider)
                .expect("other providers remain independent"),
        );
        std::fs::remove_dir_all(directory).expect("test fixture should be valid");
    }

    #[test]
    fn expired_default_recovery_can_retry_without_the_old_owner_finishing_it() {
        let directory = std::env::temp_dir().join(format!(
            "switchboard-expired-budget-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("test fixture should be valid")
                .as_nanos()
        ));
        std::fs::create_dir(&directory).expect("test fixture should be valid");
        let cache = directory.join("sessions.json");
        let auth = ResolvedAuth::new(
            "google.personal",
            "person@example.com",
            switchboard_core::AuthSecretRefs::GoogleCli,
        )
        .expect("test fixture should be valid");
        let budget = RecoveryBudget::new(Some(&cache), None);
        let mut old_attempt = budget.claim(&auth).expect("first recovery allowed");
        let connection = Connection::open(directory.join("auth-recovery.sqlite3")).expect("recovery database");
        // Simulate a process that stopped before marking its claim complete.
        old_attempt.started_ms = epoch_ms() - 60_001;
        old_attempt.started = Instant::now() - Duration::from_secs(61);
        assert!(matches!(
            old_attempt.remaining_timeout(Duration::from_secs(60)),
            Err(Error::AuthenticationTimeout { seconds: 60 })
        ));
        connection
            .execute("UPDATE recovery_attempts SET started_ms = ?1", [old_attempt.started_ms])
            .expect("expire the unfinished claim");
        let reopened = RecoveryBudget::new(Some(&cache), None);
        let mut replacement = reopened.claim(&auth).expect("expired recovery can be retried");
        // Time spent waiting for SQLite must not give the unlock a fresh minute.
        replacement.started = Instant::now() - Duration::from_secs(59);
        assert!(
            replacement
                .remaining_timeout(Duration::from_secs(60))
                .expect("one second left")
                <= Duration::from_secs(1)
        );
        assert!(
            replacement
                .remaining_timeout(Duration::from_millis(1))
                .expect("configured timeout respected")
                <= Duration::from_millis(1)
        );
        drop(old_attempt);
        let finished: bool = connection
            .query_row("SELECT finished FROM recovery_attempts", [], |row| row.get(0))
            .expect("replacement recovery still present");
        assert!(!finished, "an old owner cannot finish a replacement attempt");
        drop(replacement);
        assert!(matches!(reopened.claim(&auth), Err(Error::RecoveryExhausted(_))));
        drop(connection);
        std::fs::remove_dir_all(directory).expect("test fixture should be valid");
    }

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
