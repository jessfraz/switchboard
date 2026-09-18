use switchboard_core::{ToolArgument, ToolArguments};

use crate::github::ci::{models::*, CiOptions};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn run(id: u64, attempt: u64, status: &str, conclusion: Option<&str>) -> WorkflowRun {
    WorkflowRun {
        id,
        workflow_id: 9,
        run_number: 1,
        run_attempt: attempt,
        name: Some("CI".into()),
        event: "push".into(),
        head_sha: SHA.into(),
        head_branch: Some("main".into()),
        status: status.into(),
        conclusion: conclusion.map(str::to_owned),
        html_url: format!("https://github.com/example/repo/actions/runs/{id}"),
    }
}

#[test]
fn a_rerun_replaces_the_old_attempt_and_changes_the_cursor() {
    let mut snapshot = Snapshot::default();
    snapshot.collect_runs(
        Ok(WorkflowRuns {
            total_count: 1,
            workflow_runs: vec![run(1, 1, "completed", Some("failure"))],
        }),
        SHA,
        30,
    );
    let previous = snapshot.cursor("scope:").expect("cursor");
    assert_eq!(snapshot.state(), CiState::Failure);
    snapshot.collect_runs(
        Ok(WorkflowRuns {
            total_count: 2,
            workflow_runs: vec![run(1, 1, "completed", Some("failure")), run(1, 2, "in_progress", None)],
        }),
        SHA,
        30,
    );
    assert_eq!(snapshot.runs.len(), 1);
    assert_eq!(snapshot.state(), CiState::Pending);
    assert_ne!(snapshot.cursor("scope:").expect("cursor"), previous);
}

#[test]
fn empty_truncated_wrong_commit_and_future_status_cannot_be_green() {
    let mut snapshot = Snapshot::default();
    assert_eq!(snapshot.state(), CiState::NoRuns);
    snapshot.collect_runs(
        Ok(WorkflowRuns {
            total_count: 2,
            workflow_runs: vec![run(1, 1, "completed", Some("success"))],
        }),
        SHA,
        1,
    );
    assert_eq!(snapshot.state(), CiState::Truncated);
    let mut snapshot = Snapshot::default();
    snapshot.collect_runs(
        Ok(WorkflowRuns {
            total_count: 1,
            workflow_runs: vec![run(1, 1, "future_status", None)],
        }),
        SHA,
        1,
    );
    assert_eq!(snapshot.state(), CiState::Unknown);
    let mut snapshot = Snapshot::default();
    snapshot.collect_runs(
        Ok(WorkflowRuns {
            total_count: 1,
            workflow_runs: vec![run(1, 1, "completed", Some("success"))],
        }),
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        1,
    );
    assert_eq!(snapshot.state(), CiState::Unknown);
    assert!(snapshot.runs.is_empty());
}

#[test]
fn malformed_missing_and_oversized_provider_pages_preserve_other_sources() {
    let mut snapshot = Snapshot::default();
    snapshot.collect_runs(
        Ok(WorkflowRuns {
            total_count: 1,
            workflow_runs: vec![run(1, 1, "completed", Some("success"))],
        }),
        SHA,
        1,
    );
    snapshot.collect_checks(
        Err(switchboard_core::Error::Execution("malformed checks".into())),
        SHA,
        1,
    );
    assert_eq!(snapshot.runs.len(), 1);
    assert_eq!(snapshot.state(), CiState::Unknown);
    let parsed = serde_json::from_str::<CheckRuns>(r#"{"check_runs":[]}"#);
    assert!(parsed.is_err(), "missing totals cannot establish complete coverage");
    snapshot.collect_statuses(
        Ok(CombinedStatus {
            sha: SHA.into(),
            total_count: 0,
            statuses: vec![CommitStatus {
                id: 1,
                context: "test".into(),
                state: "success".into(),
                target_url: None,
                description: None,
            }],
        }),
        SHA,
        1,
    );
    assert_eq!(snapshot.failures.len(), 2);
}

#[test]
fn cursor_is_order_independent_but_bound_to_namespace_repository_commit_and_limit() {
    let args = ToolArguments::new(vec![
        ToolArgument::option("repo", "example/repo").expect("repo"),
        ToolArgument::option("commit", SHA).expect("commit"),
    ]);
    let options = CiOptions::parse(&args).expect("options");
    assert_ne!(options.scope("github.personal"), options.scope("github.work"));
    let mut a = Snapshot::default();
    let mut b = Snapshot::default();
    let mut second = run(2, 1, "completed", Some("success"));
    second.workflow_id = 10;
    let first = run(1, 1, "completed", Some("success"));
    a.collect_runs(
        Ok(WorkflowRuns {
            total_count: 2,
            workflow_runs: vec![first.clone(), second.clone()],
        }),
        SHA,
        30,
    );
    b.collect_runs(
        Ok(WorkflowRuns {
            total_count: 2,
            workflow_runs: vec![second, first],
        }),
        SHA,
        30,
    );
    assert_eq!(a.cursor("scope:").expect("cursor"), b.cursor("scope:").expect("cursor"));
}

#[test]
fn exact_commit_and_wait_bounds_are_validated_before_execution() {
    for (commit, wait) in [("main", "0"), (SHA, "61"), ("abc123", "0")] {
        let args = ToolArguments::new(vec![
            ToolArgument::option("repo", "example/repo").expect("repo"),
            ToolArgument::option("commit", commit).expect("commit"),
            ToolArgument::option("wait", wait).expect("wait"),
        ]);
        assert!(CiOptions::parse(&args).is_err());
    }
}
