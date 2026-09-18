//! Exercise the real adapter/runtime boundary with isolated provider protocol
//! fixtures. No credentials, network, or external GitHub mutations are used.

use std::env;

use serde::Deserialize;
use switchboard_core::{
    Adapter, AuthSecretRefs, CoverageStatus, ExecutionMode, ExecutionTarget, PlanningTarget, ProviderKind,
    ResolvedAuth, ResolvedCredentials, ResolvedNamespace, SecretRef, ToolArgument, ToolOutput, ToolRequest,
};

use crate::{
    github::GitHubAdapter,
    test_support::{lock_env, TempScript},
};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CORE: &str = r#"{"data":{"repository":{"item":{"number":7,"title":"A change","url":"https://github.com/example/repo/pull/7","state":"OPEN","body":"Description","updatedAt":"2026-01-01T00:00:00Z","headRefOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","headRefName":"feature","baseRefName":"main","isDraft":false}}}}"#;
const DETAILS: &str = r#"{"data":{"repository":{"item":{"number":7,"title":"A change","url":"https://github.com/example/repo/pull/7","state":"OPEN","body":"Description","updatedAt":"2026-01-01T00:00:00Z","headRefOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","headRefName":"feature","baseRefName":"main","isDraft":false,"files":{"totalCount":2,"pageInfo":{"hasNextPage":true,"hasPreviousPage":false},"nodes":[{"path":"src/lib.rs","additions":2,"deletions":1}]}}}}}"#;
const RUNS: &str = r#"{"total_count":1,"workflow_runs":[{"id":11,"workflow_id":9,"run_number":1,"run_attempt":1,"name":"CI","event":"push","head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","head_branch":"main","status":"completed","conclusion":"success","html_url":"https://github.com/example/repo/actions/runs/11"}]}"#;

fn target() -> ExecutionTarget {
    ExecutionTarget {
        namespace: ResolvedNamespace::new(
            "github.fixture",
            ProviderKind::GitHub,
            "Fixture",
            "github.fixture-auth",
            false,
            None,
        )
        .expect("namespace"),
        auth: ResolvedAuth::new(
            "github.fixture-auth",
            "fixture",
            AuthSecretRefs::GitHubToken {
                token: SecretRef::new("fixture.token").expect("reference"),
            },
        )
        .expect("auth"),
        credentials: ResolvedCredentials::GitHubToken {
            token: "fixture-token".to_owned().into(),
        },
    }
}

fn execute(adapter: &GitHubAdapter, tool: &str, args: &[(&str, &str)]) -> switchboard_core::Result<ToolOutput> {
    let target = target();
    let request = ToolRequest::new(
        tool,
        "github.fixture",
        ExecutionMode::Auto,
        args.iter()
            .map(|(key, value)| ToolArgument::option(*key, *value).expect("argument"))
            .collect::<Vec<_>>(),
    )
    .expect("request");
    let descriptor = adapter.find_tool(&request.tool).expect("curated workflow must exist");
    let action = adapter.plan(
        &PlanningTarget {
            namespace: target.namespace.clone(),
            auth: target.auth.clone(),
        },
        &request,
        descriptor,
    )?;
    adapter.execute(&target, &action)
}

fn script(details: &str, checks: &str) -> TempScript {
    script_with_runs(details, checks, &format!("printf '%s\\n' '{RUNS}'"))
}

fn script_with_runs(details: &str, checks: &str, runs: &str) -> TempScript {
    TempScript::new(
        "gh-workflow",
        &format!(
            r#"#!/bin/sh
case "$*" in
  --version) echo 'gh version 2.93.0'; exit 0 ;;
  *--help*) echo 'gh help'; exit 0 ;;
esac
[ "$GH_TOKEN" = fixture-token ] || exit 91
[ -z "$GITHUB_TOKEN" ] || exit 92
printf '%s\n' "$*" >> "$(dirname "$0")/env.txt"
case "$*" in
  *'files(first:'*) {details} ;;
  'api graphql '*) printf '%s\n' '{CORE}' ;;
  *'/actions/runs?head_sha='*) {runs} ;;
  *'/check-runs?'*) {checks} ;;
  *'/status?'*) printf '%s\n' '{{"sha":"{SHA}","total_count":0,"statuses":[]}}' ;;
  *) echo unexpected-command >&2; exit 93 ;;
esac
"#
        ),
    )
}

#[test]
fn context_runtime_returns_bounded_files_and_core_in_one_result() {
    let _guard = lock_env();
    let fixture = script(&format!("printf '%s\\n' '{DETAILS}'"), "exit 94");
    env::set_var("SWITCHBOARD_GH_BIN", fixture.path());
    let adapter = GitHubAdapter::new().expect("catalog");
    let output = execute(
        &adapter,
        "github.pull_request.context",
        &[
            ("repo", "example/repo"),
            ("number", "7"),
            ("include", "files"),
            ("limit", "1"),
        ],
    )
    .expect("context");
    #[derive(Deserialize)]
    struct Fields {
        context: Context,
    }
    #[derive(Deserialize)]
    struct Context {
        number: u64,
        head_sha: String,
        files: Files,
    }
    #[derive(Deserialize)]
    struct Files {
        total: usize,
        items: Vec<File>,
    }
    #[derive(Deserialize)]
    struct File {
        path: String,
    }
    let fields: Fields =
        serde_json::from_value(serde_json::to_value(&output.fields).expect("fields")).expect("context fields");
    assert_eq!(fields.context.number, 7);
    assert_eq!(fields.context.head_sha, SHA);
    assert_eq!(fields.context.files.total, 2);
    assert_eq!(fields.context.files.items.len(), 1);
    assert_eq!(fields.context.files.items[0].path, "src/lib.rs");
    assert_eq!(output.coverage.expect("coverage").status, CoverageStatus::Truncated);
    assert_eq!(fixture.capture_contents().lines().count(), 2);
    assert!(!fixture.capture_contents().contains("comments("));
    env::remove_var("SWITCHBOARD_GH_BIN");
}

#[test]
fn context_runtime_preserves_core_when_optional_details_are_denied() {
    let _guard = lock_env();
    let fixture = script("echo 'Resource not accessible' >&2; exit 1", "exit 94");
    env::set_var("SWITCHBOARD_GH_BIN", fixture.path());
    let adapter = GitHubAdapter::new().expect("catalog");
    let output = execute(
        &adapter,
        "github.pull_request.context",
        &[("repo", "example/repo"), ("number", "7"), ("include", "files")],
    )
    .expect("partial context");
    #[derive(Deserialize)]
    struct Fields {
        context: Context,
        failures: Vec<switchboard_core::Failure>,
    }
    #[derive(Deserialize)]
    struct Context {
        number: u64,
        title: String,
    }
    let fields: Fields =
        serde_json::from_value(serde_json::to_value(&output.fields).expect("fields")).expect("partial fields");
    assert_eq!(fields.context.number, 7);
    assert_eq!(fields.context.title, "A change");
    assert_eq!(fields.failures.len(), 1);
    assert_eq!(output.coverage.expect("coverage").status, CoverageStatus::Unknown);
    env::remove_var("SWITCHBOARD_GH_BIN");
}

#[test]
fn ci_runtime_joins_exact_commit_and_suppresses_unchanged_details() {
    let _guard = lock_env();
    let fixture = script("exit 94", "printf '%s\\n' '{\"total_count\":0,\"check_runs\":[]}'");
    env::set_var("SWITCHBOARD_GH_BIN", fixture.path());
    let adapter = GitHubAdapter::new().expect("catalog");
    #[derive(Deserialize)]
    struct Fields {
        ci: Ci,
    }
    #[derive(Deserialize)]
    struct Ci {
        commit: String,
        status: String,
        changed: bool,
        cursor: String,
        #[serde(default)]
        runs: Vec<Run>,
    }
    #[derive(Deserialize)]
    struct Run {
        id: u64,
    }
    let output = execute(
        &adapter,
        "github.ci.status",
        &[("repo", "example/repo"), ("commit", SHA)],
    )
    .expect("CI snapshot");
    let fields: Fields =
        serde_json::from_value(serde_json::to_value(&output.fields).expect("fields")).expect("CI fields");
    assert_eq!(fields.ci.commit, SHA);
    assert_eq!(fields.ci.status, "success");
    assert!(fields.ci.changed);
    assert_eq!(fields.ci.runs[0].id, 11);
    let output = execute(
        &adapter,
        "github.ci.status",
        &[("repo", "example/repo"), ("commit", SHA), ("cursor", &fields.ci.cursor)],
    )
    .expect("unchanged snapshot");
    let next: Fields =
        serde_json::from_value(serde_json::to_value(&output.fields).expect("fields")).expect("CI fields");
    assert_eq!(next.ci.cursor, fields.ci.cursor);
    assert!(!next.ci.changed);
    assert!(next.ci.runs.is_empty());
    assert_eq!(fixture.capture_contents().lines().count(), 6);
    assert!(fixture
        .capture_contents()
        .lines()
        .all(|line| line.contains(SHA) && line.starts_with("api --method GET ")));
    env::remove_var("SWITCHBOARD_GH_BIN");
}

#[test]
fn ci_runtime_keeps_runs_when_check_payload_is_malformed() {
    let _guard = lock_env();
    let fixture = script("exit 94", "printf '%s\\n' '{\"check_runs\":[]}'");
    env::set_var("SWITCHBOARD_GH_BIN", fixture.path());
    let adapter = GitHubAdapter::new().expect("catalog");
    let output = execute(
        &adapter,
        "github.ci.status",
        &[("repo", "example/repo"), ("commit", SHA)],
    )
    .expect("partial CI");
    #[derive(Deserialize)]
    struct Fields {
        ci: Ci,
        failures: Vec<switchboard_core::Failure>,
    }
    #[derive(Deserialize)]
    struct Ci {
        status: String,
        runs: Vec<Run>,
    }
    #[derive(Deserialize)]
    struct Run {
        id: u64,
    }
    let fields: Fields =
        serde_json::from_value(serde_json::to_value(&output.fields).expect("fields")).expect("CI fields");
    assert_eq!(fields.ci.status, "unknown");
    assert_eq!(fields.ci.runs[0].id, 11);
    assert_eq!(fields.failures.len(), 1);
    assert_eq!(output.coverage.expect("coverage").status, CoverageStatus::Unknown);
    env::remove_var("SWITCHBOARD_GH_BIN");
}

#[test]
fn ci_wait_observes_a_completion_and_has_a_finite_unchanged_wait() {
    let _guard = lock_env();
    let pending = RUNS
        .replace("\"completed\"", "\"in_progress\"")
        .replace("\"success\"", "null");
    let runs = format!(
        "if [ -e \"$(dirname \"$0\")/polled\" ]; then printf '%s\\n' '{RUNS}'; else touch \"$(dirname \"$0\")/polled\"; printf '%s\\n' '{pending}'; fi"
    );
    let fixture = script_with_runs(
        "exit 94",
        "printf '%s\\n' '{\"total_count\":0,\"check_runs\":[]}'",
        &runs,
    );
    env::set_var("SWITCHBOARD_GH_BIN", fixture.path());
    let adapter = GitHubAdapter::new().expect("catalog");
    #[derive(Deserialize)]
    struct Fields {
        ci: Ci,
    }
    #[derive(Deserialize)]
    struct Ci {
        status: String,
        cursor: String,
        changed: bool,
    }
    let started = std::time::Instant::now();
    let output = execute(
        &adapter,
        "github.ci.status",
        &[("repo", "example/repo"), ("commit", SHA), ("wait", "4")],
    )
    .expect("completed CI");
    let fields: Fields =
        serde_json::from_value(serde_json::to_value(&output.fields).expect("fields")).expect("CI fields");
    assert_eq!(fields.ci.status, "success");
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    assert_eq!(fixture.capture_contents().lines().count(), 6);
    let started = std::time::Instant::now();
    let output = execute(
        &adapter,
        "github.ci.status",
        &[
            ("repo", "example/repo"),
            ("commit", SHA),
            ("wait", "1"),
            ("cursor", &fields.ci.cursor),
        ],
    )
    .expect("bounded unchanged wait");
    let next: Fields =
        serde_json::from_value(serde_json::to_value(&output.fields).expect("fields")).expect("CI fields");
    assert!(!next.ci.changed);
    assert_eq!(next.ci.cursor, fields.ci.cursor);
    assert!(started.elapsed() >= std::time::Duration::from_millis(900));
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    assert_eq!(fixture.capture_contents().lines().count(), 9);
    env::remove_var("SWITCHBOARD_GH_BIN");
}

#[test]
fn ci_runtime_keeps_distinct_suites_and_only_replaces_attempts_within_a_suite() {
    let _guard = lock_env();
    let checks = format!(
        r#"{{"total_count":3,"check_runs":[
        {{"id":1,"name":"test","head_sha":"{SHA}","status":"completed","conclusion":"failure","app":{{"id":42}},"check_suite":{{"id":101}}}},
        {{"id":2,"name":"test","head_sha":"{SHA}","status":"completed","conclusion":"failure","app":{{"id":42}},"check_suite":{{"id":102}}}},
        {{"id":3,"name":"test","head_sha":"{SHA}","status":"completed","conclusion":"success","app":{{"id":42}},"check_suite":{{"id":102}}}}
        ]}}"#
    );
    let fixture = script("exit 94", &format!("printf '%s\\n' '{checks}'"));
    env::set_var("SWITCHBOARD_GH_BIN", fixture.path());
    let adapter = GitHubAdapter::new().expect("catalog");
    let output = execute(
        &adapter,
        "github.ci.status",
        &[("repo", "example/repo"), ("commit", SHA)],
    )
    .expect("CI snapshot");
    #[derive(Deserialize)]
    struct Fields {
        ci: Ci,
    }
    #[derive(Deserialize)]
    struct Ci {
        status: String,
        checks: Vec<Check>,
        counts: Counts,
    }
    #[derive(Deserialize)]
    struct Check {
        id: u64,
    }
    #[derive(Deserialize)]
    struct Counts {
        checks: usize,
        failed: usize,
    }
    let fields: Fields =
        serde_json::from_value(serde_json::to_value(&output.fields).expect("fields")).expect("CI fields");
    assert_eq!(fields.ci.status, "failure");
    assert_eq!(fields.ci.counts.checks, 2);
    assert_eq!(fields.ci.counts.failed, 1);
    assert_eq!(
        fields.ci.checks.iter().map(|check| check.id).collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert_eq!(output.coverage.expect("coverage").status, CoverageStatus::Complete);
    env::remove_var("SWITCHBOARD_GH_BIN");
}

#[test]
fn ci_runtime_stops_after_native_github_auth_rejection() {
    let _guard = lock_env();
    let fixture = script_with_runs(
        "exit 94",
        "exit 94",
        r#"printf '%s\n' '{"message":"Bad credentials","documentation_url":"https://docs.github.com/rest","status":"401"}'; printf '%s\n' 'gh: Bad credentials (HTTP 401)' >&2; exit 1"#,
    );
    env::set_var("SWITCHBOARD_GH_BIN", fixture.path());
    let adapter = GitHubAdapter::new().expect("catalog");
    let output = execute(
        &adapter,
        "github.ci.status",
        &[("repo", "example/repo"), ("commit", SHA)],
    )
    .expect("partial CI snapshot");
    #[derive(Deserialize)]
    struct Fields {
        failures: Vec<switchboard_core::Failure>,
    }
    let fields: Fields =
        serde_json::from_value(serde_json::to_value(&output.fields).expect("fields")).expect("CI fields");
    assert_eq!(fixture.capture_contents().lines().count(), 1);
    assert_eq!(fields.failures.len(), 1);
    assert_eq!(fields.failures[0].phase, switchboard_core::FailurePhase::Authentication);
    assert_eq!(fields.failures[0].provider, Some(ProviderKind::GitHub));
    assert_eq!(
        fields.failures[0].namespace.as_ref().map(|ns| ns.as_str()),
        Some("github.fixture")
    );
    assert_eq!(output.coverage.expect("coverage").status, CoverageStatus::Unknown);
    env::remove_var("SWITCHBOARD_GH_BIN");
}

#[test]
fn ci_wait_does_not_start_a_snapshot_near_the_inherited_deadline() {
    let _guard = lock_env();
    let pending = RUNS
        .replace("\"completed\"", "\"in_progress\"")
        .replace("\"success\"", "null");
    let runs = format!(
        "if [ -e \"$(dirname \"$0\")/polled\" ]; then sleep 2; else touch \"$(dirname \"$0\")/polled\"; sleep 0.4; fi; printf '%s\\n' '{pending}'"
    );
    let fixture = script_with_runs(
        "exit 94",
        "printf '%s\\n' '{\"total_count\":0,\"check_runs\":[]}'",
        &runs,
    );
    env::set_var("SWITCHBOARD_GH_BIN", fixture.path());
    let previous_deadline = env::var_os("SWITCHBOARD_DEADLINE_UNIX_MS");
    let deadline = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_millis()
        + 3_000;
    env::set_var("SWITCHBOARD_DEADLINE_UNIX_MS", deadline.to_string());
    let started = std::time::Instant::now();
    let adapter = GitHubAdapter::new().expect("catalog");
    let output = execute(
        &adapter,
        "github.ci.status",
        &[("repo", "example/repo"), ("commit", SHA), ("wait", "10")],
    );
    match previous_deadline {
        Some(value) => env::set_var("SWITCHBOARD_DEADLINE_UNIX_MS", value),
        None => env::remove_var("SWITCHBOARD_DEADLINE_UNIX_MS"),
    }
    env::remove_var("SWITCHBOARD_GH_BIN");
    let output = output.expect("bounded CI wait");
    #[derive(Deserialize)]
    struct Fields {
        ci: Ci,
        failures: Vec<switchboard_core::Failure>,
    }
    #[derive(Deserialize)]
    struct Ci {
        status: String,
        runs: Vec<Run>,
    }
    #[derive(Deserialize)]
    struct Run {
        id: u64,
    }
    let fields: Fields =
        serde_json::from_value(serde_json::to_value(&output.fields).expect("fields")).expect("CI fields");
    assert_eq!(fields.ci.status, "pending");
    assert!(fields.failures.is_empty());
    assert_eq!(fields.ci.runs[0].id, 11);
    assert_eq!(output.coverage.expect("coverage").status, CoverageStatus::Complete);
    assert_eq!(fixture.capture_contents().lines().count(), 3);
    assert!(started.elapsed() < std::time::Duration::from_millis(3_500));
}
