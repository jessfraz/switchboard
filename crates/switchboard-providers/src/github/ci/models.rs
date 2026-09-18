use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use switchboard_core::{CoverageStatus, Error, Failure, Result};

#[derive(Deserialize)]
pub(super) struct WorkflowRuns {
    pub total_count: usize,
    pub workflow_runs: Vec<WorkflowRun>,
}
#[derive(Deserialize, Serialize, Clone, Eq, PartialEq)]
pub(super) struct WorkflowRun {
    pub id: u64,
    pub workflow_id: u64,
    pub run_number: u64,
    pub run_attempt: u64,
    pub name: Option<String>,
    pub event: String,
    pub head_sha: String,
    pub head_branch: Option<String>,
    pub status: String,
    pub conclusion: Option<String>,
    pub html_url: String,
}
#[derive(Deserialize)]
pub(super) struct CheckRuns {
    pub total_count: usize,
    pub check_runs: Vec<CheckRun>,
}
#[derive(Deserialize, Serialize, Clone, Eq, PartialEq)]
pub(super) struct CheckRun {
    pub id: u64,
    pub name: String,
    pub head_sha: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub html_url: Option<String>,
    pub app: Option<App>,
    pub check_suite: CheckSuite,
}
#[derive(Deserialize, Serialize, Clone, Eq, PartialEq)]
pub(super) struct CheckSuite {
    pub id: u64,
}
#[derive(Deserialize, Serialize, Clone, Eq, PartialEq)]
pub(super) struct App {
    pub id: u64,
}
#[derive(Deserialize)]
pub(super) struct CombinedStatus {
    pub sha: String,
    pub total_count: usize,
    pub statuses: Vec<CommitStatus>,
}
#[derive(Deserialize, Serialize, Clone, Eq, PartialEq)]
pub(super) struct CommitStatus {
    pub id: u64,
    pub context: String,
    pub state: String,
    pub target_url: Option<String>,
    pub description: Option<String>,
}

#[derive(Default, Serialize)]
pub(super) struct Snapshot {
    pub runs: Vec<WorkflowRun>,
    pub checks: Vec<CheckRun>,
    pub statuses: Vec<CommitStatus>,
    pub failures: Vec<Failure>,
    pub truncated_sources: Vec<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CiState {
    NoRuns,
    Pending,
    Success,
    Failure,
    Truncated,
    Unknown,
}
impl CiState {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::NoRuns => "no_runs",
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Truncated => "truncated",
            Self::Unknown => "unknown",
        }
    }
}
#[derive(Serialize)]
pub(super) struct Counts {
    pub runs: usize,
    pub checks: usize,
    pub statuses: usize,
    pub pending: usize,
    pub failed: usize,
}

impl Snapshot {
    pub(super) fn collect_runs(&mut self, response: Result<WorkflowRuns>, sha: &str, limit: u64) {
        let result = response.and_then(|page| {
            validate_page(page.total_count, page.workflow_runs.len(), limit)?;
            if page
                .workflow_runs
                .iter()
                .any(|run| !run.head_sha.eq_ignore_ascii_case(sha))
            {
                return Err(wrong_commit());
            }
            if page.total_count > page.workflow_runs.len() {
                self.truncated_sources.push("actions");
            }
            let mut latest = BTreeMap::new();
            for run in page.workflow_runs {
                let key = (run.workflow_id, run.event.clone(), run.head_branch.clone());
                let previous = latest.entry(key).or_insert_with(|| run.clone());
                if (run.run_number, run.run_attempt, run.id) > (previous.run_number, previous.run_attempt, previous.id)
                {
                    *previous = run;
                }
            }
            self.runs = latest.into_values().collect();
            Ok(())
        });
        if let Err(error) = result {
            self.failures.push(Failure::from_error(&error));
        }
    }
    pub(super) fn collect_checks(&mut self, response: Result<CheckRuns>, sha: &str, limit: u64) {
        let result = response.and_then(|page| {
            validate_page(page.total_count, page.check_runs.len(), limit)?;
            if page
                .check_runs
                .iter()
                .any(|run| !run.head_sha.eq_ignore_ascii_case(sha))
            {
                return Err(wrong_commit());
            }
            if page.total_count > page.check_runs.len() {
                self.truncated_sources.push("checks");
            }
            let mut latest = BTreeMap::new();
            for run in page.check_runs {
                // Different suites may use the same app and job name. Only a
                // newer attempt inside that same suite can replace its result.
                let key = (run.check_suite.id, run.app.as_ref().map(|app| app.id), run.name.clone());
                let previous = latest.entry(key).or_insert_with(|| run.clone());
                if run.id > previous.id {
                    *previous = run;
                }
            }
            self.checks = latest.into_values().collect();
            Ok(())
        });
        if let Err(error) = result {
            self.failures.push(Failure::from_error(&error));
        }
    }
    pub(super) fn collect_statuses(&mut self, response: Result<CombinedStatus>, sha: &str, limit: u64) {
        let result = response.and_then(|page| {
            validate_page(page.total_count, page.statuses.len(), limit)?;
            if !page.sha.eq_ignore_ascii_case(sha) {
                return Err(wrong_commit());
            }
            if page.total_count > page.statuses.len() {
                self.truncated_sources.push("statuses");
            }
            let mut latest = BTreeMap::new();
            for status in page.statuses {
                let previous = latest.entry(status.context.clone()).or_insert_with(|| status.clone());
                if status.id > previous.id {
                    *previous = status;
                }
            }
            self.statuses = latest.into_values().collect();
            Ok(())
        });
        if let Err(error) = result {
            self.failures.push(Failure::from_error(&error));
        }
    }
    fn outcomes(&self) -> impl Iterator<Item = CiState> + '_ {
        self.runs
            .iter()
            .map(|run| outcome(&run.status, run.conclusion.as_deref()))
            .chain(
                self.checks
                    .iter()
                    .map(|run| outcome(&run.status, run.conclusion.as_deref())),
            )
            .chain(self.statuses.iter().map(|status| match status.state.as_str() {
                "success" => CiState::Success,
                "error" | "failure" => CiState::Failure,
                "pending" => CiState::Pending,
                _ => CiState::Unknown,
            }))
    }
    pub(super) fn state(&self) -> CiState {
        if !self.failures.is_empty() || self.outcomes().any(|state| state == CiState::Unknown) {
            return CiState::Unknown;
        }
        if !self.truncated_sources.is_empty() {
            return CiState::Truncated;
        }
        if self.runs.is_empty() && self.checks.is_empty() && self.statuses.is_empty() {
            return CiState::NoRuns;
        }
        if self.outcomes().any(|state| state == CiState::Failure) {
            return CiState::Failure;
        }
        if self.outcomes().any(|state| state == CiState::Pending) {
            return CiState::Pending;
        }
        CiState::Success
    }
    pub(super) fn coverage(&self) -> CoverageStatus {
        match self.state() {
            CiState::Unknown => CoverageStatus::Unknown,
            CiState::Truncated => CoverageStatus::Truncated,
            _ => CoverageStatus::Complete,
        }
    }
    pub(super) fn counts(&self) -> Counts {
        Counts {
            runs: self.runs.len(),
            checks: self.checks.len(),
            statuses: self.statuses.len(),
            pending: self.outcomes().filter(|state| *state == CiState::Pending).count(),
            failed: self.outcomes().filter(|state| *state == CiState::Failure).count(),
        }
    }
}

fn wrong_commit() -> Error {
    Error::Execution("GitHub returned CI results for a different commit".into())
}
fn validate_page(total: usize, count: usize, limit: u64) -> Result<()> {
    if count > limit as usize || count > total {
        Err(Error::Execution("GitHub returned an inconsistent CI page".into()))
    } else {
        Ok(())
    }
}
fn outcome(status: &str, conclusion: Option<&str>) -> CiState {
    if matches!(status, "queued" | "in_progress" | "waiting" | "pending" | "requested") {
        return CiState::Pending;
    }
    if status != "completed" {
        return CiState::Unknown;
    }
    match conclusion {
        Some("success" | "neutral" | "skipped") => CiState::Success,
        Some("failure" | "cancelled" | "timed_out" | "action_required" | "stale" | "startup_failure") => {
            CiState::Failure
        }
        _ => CiState::Unknown,
    }
}
