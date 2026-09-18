mod models;
#[cfg(test)]
mod tests;

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};

use serde::Serialize;
use switchboard_core::{
    Error, ExecutionTarget, FailurePhase, PlannedAction, ReadCoverage, Result, ToolArguments, ToolOutput,
};

use crate::github::{
    api::{bounded, encode, repository},
    ci::models::*,
    GitHubAdapter,
};

pub(super) struct CiOptions {
    repo: String,
    commit: String,
    limit: u64,
    wait: u64,
    cursor: Option<String>,
}

impl CiOptions {
    pub(super) fn parse(args: &ToolArguments) -> Result<Self> {
        let commit = args
            .value("commit")
            .ok_or_else(|| Error::InvalidArguments("missing --commit (full 40-character SHA)".into()))?;
        if commit.len() != 40 || !commit.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::InvalidArguments(
                "--commit must be a full 40-character commit SHA, not a branch or abbreviated SHA".into(),
            ));
        }
        let cursor = args.value("cursor").map(str::to_owned);
        if let Some(cursor) = &cursor {
            let parts = cursor.split(':').collect::<Vec<_>>();
            if parts.len() != 3
                || parts[0] != "v1"
                || parts[1..]
                    .iter()
                    .any(|part| part.len() != 16 || !part.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(Error::InvalidArguments("invalid CI change cursor".into()));
            }
        }
        Ok(Self {
            repo: repository(args)?,
            commit: commit.to_ascii_lowercase(),
            limit: bounded(args, "limit", 30, 1, 100)?,
            wait: bounded(args, "wait", 0, 0, 60)?,
            cursor,
        })
    }

    fn scope(&self, namespace: &str) -> String {
        format!(
            "v1:{:016x}:",
            fingerprint(&(namespace, self.repo.to_ascii_lowercase(), &self.commit, self.limit))
        )
    }
}

// A cursor is a small change detector, not a credential or an authorization
// token. A different implementation version may simply produce a fresh snapshot.
fn fingerprint(value: &impl Hash) -> u64 {
    let mut state = DefaultHasher::new();
    value.hash(&mut state);
    state.finish()
}

#[derive(Serialize)]
struct CiReport<'a> {
    repository: &'a str,
    commit: &'a str,
    url: String,
    status: CiState,
    counts: Counts,
    changed: bool,
    cursor: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    runs: Vec<WorkflowRun>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    checks: Vec<CheckRun>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    statuses: Vec<CommitStatus>,
    truncated_sources: Vec<&'static str>,
}

impl GitHubAdapter {
    fn ci_snapshot(&self, target: &ExecutionTarget, options: &CiOptions) -> Snapshot {
        let endpoint = |path: String| vec!["api".into(), "--method".into(), "GET".into(), path];
        let mut snapshot = Snapshot::default();
        let runs = self.read_json::<WorkflowRuns>(
            target,
            endpoint(format!(
                "repos/{}/actions/runs?head_sha={}&per_page={}",
                options.repo, options.commit, options.limit
            )),
        );
        snapshot.collect_runs(runs, &options.commit, options.limit);
        if !snapshot.authentication_failed() {
            let checks = self.read_json::<CheckRuns>(
                target,
                endpoint(format!(
                    "repos/{}/commits/{}/check-runs?filter=latest&per_page={}",
                    options.repo, options.commit, options.limit
                )),
            );
            snapshot.collect_checks(checks, &options.commit, options.limit);
        }
        if !snapshot.authentication_failed() {
            let statuses = self.read_json::<CombinedStatus>(
                target,
                endpoint(format!(
                    "repos/{}/commits/{}/status?per_page={}",
                    options.repo, options.commit, options.limit
                )),
            );
            snapshot.collect_statuses(statuses, &options.commit, options.limit);
        }
        snapshot
    }

    pub(super) fn ci_status(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
        let options = CiOptions::parse(&action.args)?;
        let scope = options.scope(action.namespace.as_str());
        if options
            .cursor
            .as_ref()
            .is_some_and(|cursor| !cursor.starts_with(&scope))
        {
            return Err(Error::InvalidArguments(
                "CI cursor belongs to a different namespace, repository, commit, or limit".into(),
            ));
        }
        let started = Instant::now();
        let budget = Duration::from_secs(options.wait);
        let mut snapshot = self.ci_snapshot(target, &options);
        let mut snapshot_duration = started.elapsed();
        let mut poll_delay = Duration::from_secs(2);
        loop {
            let state = snapshot.state();
            let current_cursor = snapshot.cursor(&scope)?;
            let changed = options.cursor.as_ref().is_some_and(|cursor| &current_cursor != cursor);
            if options.wait == 0
                || changed
                || matches!(state, CiState::Unknown | CiState::Truncated)
                || (options.cursor.is_none() && matches!(state, CiState::Success | CiState::Failure))
                || started.elapsed() >= budget
            {
                break;
            }
            let remaining = budget.saturating_sub(started.elapsed());
            let remaining = match switchboard_core::process::remaining_timeout(remaining) {
                Ok(remaining) => remaining,
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => break,
                Err(error) => return Err(Error::Execution(format!("invalid CI deadline: {error}"))),
            };
            // Each snapshot reads three endpoints. Back off while CI is unchanged.
            std::thread::sleep(poll_delay.min(remaining));
            poll_delay = (poll_delay * 2).min(Duration::from_secs(10));
            let remaining = match switchboard_core::process::remaining_timeout(budget.saturating_sub(started.elapsed()))
            {
                Ok(remaining) => remaining,
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => break,
                Err(error) => return Err(Error::Execution(format!("invalid CI deadline: {error}"))),
            };
            // Leave room for all three reads, including ordinary latency changes.
            // Ending the wait is normal; starting a poll we cannot finish is not.
            if remaining < snapshot_duration.saturating_mul(2).max(Duration::from_secs(1)) {
                break;
            }
            let poll_started = Instant::now();
            let next = self.ci_snapshot(target, &options);
            snapshot_duration = poll_started.elapsed();
            if next
                .failures
                .iter()
                .any(|failure| failure.code == switchboard_core::FailureCode::Timeout)
            {
                // A deadline in the next poll must not erase evidence already
                // collected. Mark it incomplete rather than returning stale green.
                snapshot.failures.extend(next.failures);
                break;
            }
            snapshot = next;
        }
        let state = snapshot.state();
        let cursor = snapshot.cursor(&scope)?;
        let changed = options.cursor.as_ref() != Some(&cursor);
        let coverage = snapshot.coverage();
        let counts = snapshot.counts();
        let summary = format!(
            "{}@{}: {} ({} runs, {} checks, {} statuses{})",
            options.repo,
            &options.commit[..12],
            state.as_str(),
            counts.runs,
            counts.checks,
            counts.statuses,
            if changed { "" } else { ", unchanged" }
        );
        let failures = snapshot
            .failures
            .into_iter()
            .map(|failure| failure.with_namespace(action.namespace.clone()))
            .collect::<Vec<_>>();
        let report = CiReport {
            repository: &options.repo,
            commit: &options.commit,
            url: format!("https://github.com/{}/commit/{}/checks", options.repo, options.commit),
            status: state,
            counts,
            changed,
            cursor,
            runs: if changed { snapshot.runs } else { vec![] },
            checks: if changed { snapshot.checks } else { vec![] },
            statuses: if changed { snapshot.statuses } else { vec![] },
            truncated_sources: snapshot.truncated_sources,
        };
        let mut output = ToolOutput::new(action.tool.clone(), action.namespace.clone(), summary)
            .with_value_field("ci", encode(&report)?)
            .with_value_field("failures", encode(&failures)?);
        // Change cursors are separate from pagination cursors. Never ask the
        // generic batch paginator to repeat an incomplete CI snapshot.
        output.coverage = Some(ReadCoverage {
            status: coverage,
            next_cursor: None,
        });
        Ok(output)
    }
}

impl Snapshot {
    fn cursor(&self, scope: &str) -> Result<String> {
        let bytes =
            serde_json::to_vec(self).map_err(|error| Error::Execution(format!("cannot encode CI cursor: {error}")))?;
        Ok(format!("{scope}{:016x}", fingerprint(&bytes)))
    }

    fn authentication_failed(&self) -> bool {
        self.failures
            .iter()
            .any(|failure| failure.phase == FailurePhase::Authentication)
    }
}
