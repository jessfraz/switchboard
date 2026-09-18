use std::{env, fs, process::Command};

use serde::Deserialize;
use switchboard_core::{Adapter, CoverageStatus, ExecutionMode, ToolArgument, ToolOutput, ToolRefKind, ToolRequest};

use super::*;
use crate::google::thread::{thread_fields, Part, ThreadParams};
use crate::{
    google::tests::{execution_target, planning_target},
    test_support::{lock_env, TempScript},
};

const MESSAGE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/google-gmail-read.json"
));
const THREAD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/cli/google-gmail-thread-context.json"
));

fn fixture() -> TempScript {
    let script = TempScript::new(
        "gws-context",
        r#"#!/bin/sh
set -eu
root=$(dirname "$0")
printf '%s\n' "$*" >> "$root/env.txt"
case "$*" in
  --version) printf 'gws 0.22.5\n';;
  *--help*) printf 'context fixture help\n';;
  'gmail users messages list '*) cat "$root/list.json";;
  'gmail users threads get '*) cat "$root/thread.json";;
  'gmail +read '*)
    if test -f "$root/fail-$4"; then cat "$root/fail-$4"; exit 1; fi
    if test -f "$root/$4.json"; then cat "$root/$4.json"; else cat "$root/message.json"; fi;;
  *) printf 'Unexpected invocation\n' >&2; exit 2;;
esac
"#,
    );
    let root = script.path().parent().expect("test fixture should be valid");
    fs::write(root.join("list.json"), r#"{"messages":[{"id":"message-1","threadId":"thread-1"},{"id":"message-2","threadId":"thread-1"}],"resultSizeEstimate":2}"#).expect("test fixture should be valid");
    fs::write(root.join("thread.json"), THREAD).expect("test fixture should be valid");
    fs::write(
        root.join("message.json"),
        MESSAGE.replace("1960thread123work", "thread-1"),
    )
    .expect("test fixture should be valid");
    env::set_var("SWITCHBOARD_GWS_BIN", script.path());
    script
}

fn execute(tool: &str, args: Vec<ToolArgument>) -> ToolOutput {
    let adapter = GoogleWorkspaceAdapter::new().expect("test fixture should be valid");
    let request =
        ToolRequest::new(tool, "google.work", ExecutionMode::Auto, args).expect("test fixture should be valid");
    let descriptor = adapter.find_tool(&request.tool).expect("test fixture should be valid");
    let action = adapter
        .plan(&planning_target(), &request, descriptor)
        .expect("test fixture should be valid");
    adapter
        .execute(&execution_target(), &action)
        .expect("test fixture should be valid")
}

fn option(name: &str, value: &str) -> ToolArgument {
    ToolArgument::option(name, value).expect("test fixture should be valid")
}
fn hydrate() -> Vec<ToolArgument> {
    vec![
        option("query", "in:inbox"),
        ToolArgument::flag("hydrate").expect("test fixture should be valid"),
    ]
}
fn messages(output: &ToolOutput) -> Vec<ContextMessage> {
    serde_json::from_value(
        output
            .fields
            .get("messages")
            .expect("test fixture should be valid")
            .clone(),
    )
    .expect("test fixture should be valid")
}

#[test]
fn hydrated_search_reads_each_body_once_without_metadata_gets() {
    let _guard = lock_env();
    let script = fixture();
    let output = execute("google.mail.search", hydrate());
    let rows = messages(&output);
    assert_eq!(rows.len(), 2);
    assert!(rows[0]
        .content
        .as_ref()
        .expect("test fixture should be valid")
        .body_text
        .contains("Hi Jess"));
    assert_eq!(rows[0].gmail_thread_id.as_deref(), Some("thread-1"));
    assert_eq!(
        output.coverage.expect("test fixture should be valid").status,
        CoverageStatus::Complete
    );
    let calls = script.capture_contents();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("gmail users messages list "))
            .count(),
        1
    );
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("gmail +read --id "))
            .count(),
        2
    );
    assert!(!calls.lines().any(|line| line.starts_with("gmail users messages get ")));
    assert_eq!(
        output
            .refs
            .iter()
            .filter(|reference| reference.kind == ToolRefKind::Message)
            .count(),
        2
    );
    assert_eq!(
        output
            .refs
            .iter()
            .filter(|reference| reference.kind == ToolRefKind::Thread)
            .count(),
        1
    );
    assert!(output
        .refs
        .iter()
        .all(|reference| reference.namespace.as_str() == "google.work"));
}

#[test]
fn native_decoded_message_flows_through_the_existing_reader_projection() {
    let _guard = lock_env();
    let script = fixture();
    let native = Command::new("gws")
        .args(["gmail", "+read", "--id", "message-1", "--format", "json", "--dry-run"])
        .output()
        .expect("test fixture should be valid");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let root = script.path().parent().expect("test fixture should be valid");
    fs::write(root.join("message.json"), native.stdout).expect("test fixture should be valid");
    fs::write(
        root.join("list.json"),
        r#"{"messages":[{"id":"message-1","threadId":"thread-message-1"}]}"#,
    )
    .expect("test fixture should be valid");
    let output = execute("google.mail.search", hydrate());
    let rows = messages(&output);
    assert_eq!(
        rows[0]
            .content
            .as_ref()
            .expect("test fixture should be valid")
            .body_text,
        "Original message body"
    );
    assert_eq!(
        rows[0]
            .content
            .as_ref()
            .expect("test fixture should be valid")
            .from
            .email,
        "sender@example.com"
    );
    assert_eq!(rows[0].coverage, CoverageStatus::Complete);
}

#[test]
fn body_bound_preserves_unicode_and_marks_coverage_truncated() {
    let _guard = lock_env();
    let script = fixture();
    let root = script.path().parent().expect("test fixture should be valid");
    // This fixture is the actual native helper's JSON shape; only its body changes.
    let mut native: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join("message.json")).expect("test fixture should be valid"))
            .expect("test fixture should be valid");
    native["body_text"] = serde_json::Value::String("aé🦀z".into());
    fs::write(
        root.join("message.json"),
        serde_json::to_vec(&native).expect("test fixture should be valid"),
    )
    .expect("test fixture should be valid");
    let mut args = hydrate();
    args.push(option("body-limit", "3"));
    let output = execute("google.mail.search", args);
    let rows = messages(&output);
    assert_eq!(
        rows[0]
            .content
            .as_ref()
            .expect("test fixture should be valid")
            .body_text,
        "aé🦀"
    );
    assert_eq!(rows[0].body_chars, Some(4));
    assert!(rows[0].body_truncated);
    assert_eq!(
        output.coverage.expect("test fixture should be valid").status,
        CoverageStatus::Truncated
    );
}

#[test]
fn thread_reads_latest_messages_in_provider_order_and_retains_attachment_metadata() {
    let _guard = lock_env();
    let script = fixture();
    let output = execute(
        "google.mail.thread",
        vec![
            option("thread-id", "thread-1"),
            option("max-messages", "2"),
            option("max-attachments", "1"),
        ],
    );
    let rows = messages(&output);
    assert_eq!(
        rows.iter().map(|row| row.gmail_message_id.as_str()).collect::<Vec<_>>(),
        ["message-2", "message-3"]
    );
    let attachments = rows[1].attachments.as_ref().expect("test fixture should be valid");
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].filename, "receipt.pdf");
    assert_eq!(attachments[0].attachment_id.as_deref(), Some("attachment-1"));
    assert_eq!(attachments[0].size, 123);
    assert_eq!(rows[1].attachments_omitted, 1);
    assert_eq!(rows[1].attachment_coverage, Some(CoverageStatus::Truncated));
    assert_eq!(
        serde_json::from_value::<usize>(output.fields["messages_omitted"].clone())
            .expect("test fixture should be valid"),
        1
    );
    assert!(serde_json::from_value::<bool>(output.fields["has_more"].clone()).expect("test fixture should be valid"));
    assert_eq!(
        output.coverage.expect("test fixture should be valid").status,
        CoverageStatus::Truncated
    );
    let calls = script.capture_contents();
    assert!(!calls
        .lines()
        .any(|line| line.starts_with("gmail +read --id message-1 ")));
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("gmail users threads get "))
            .count(),
        1
    );
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("gmail +read --id "))
            .count(),
        2
    );
}

#[test]
fn auth_failure_stops_later_waves_and_retains_all_source_ids() {
    let _guard = lock_env();
    let script = fixture();
    let root = script.path().parent().expect("test fixture should be valid");
    fs::write(root.join("list.json"), r#"{"messages":[{"id":"message-1"},{"id":"message-2"},{"id":"message-3"},{"id":"message-4"},{"id":"message-5"}],"nextPageToken":"next"}"#).expect("test fixture should be valid");
    fs::write(
        root.join("fail-message-1"),
        r#"{"error":{"code":401,"message":"credential rejected"}}"#,
    )
    .expect("test fixture should be valid");
    let output = execute("google.mail.search", hydrate());
    let rows = messages(&output);
    assert_eq!(rows.len(), 5);
    assert!(rows[0].failure.is_some());
    assert!(rows[1].content.is_some());
    assert_eq!(rows[4].gmail_message_id, "message-5");
    assert!(rows[4].content.is_none());
    let coverage = output.coverage.expect("test fixture should be valid");
    assert_eq!(coverage.status, CoverageStatus::Unknown);
    assert_eq!(coverage.next_cursor.as_deref(), Some("next"));
    assert_eq!(
        script
            .capture_contents()
            .lines()
            .filter(|line| line.starts_with("gmail +read --id "))
            .count(),
        4
    );
}

#[test]
fn mismatching_thread_and_malformed_message_preserve_other_results() {
    let _guard = lock_env();
    let script = fixture();
    let root = script.path().parent().expect("test fixture should be valid");
    fs::write(root.join("message-1.json"), MESSAGE).expect("test fixture should be valid");
    let output = execute("google.mail.search", hydrate());
    let rows = messages(&output);
    assert!(rows[0].failure.is_some());
    assert!(rows[1].content.is_some());
    assert_eq!(
        output.coverage.expect("test fixture should be valid").status,
        CoverageStatus::Unknown
    );
    fs::write(root.join("message-1.json"), "not JSON").expect("test fixture should be valid");
    let output = execute("google.mail.search", hydrate());
    assert!(messages(&output)[1].content.is_some());
    assert_eq!(
        output.coverage.expect("test fixture should be valid").status,
        CoverageStatus::Unknown
    );
}

#[test]
fn invalid_limits_fail_during_planning_before_provider_calls() {
    let adapter = GoogleWorkspaceAdapter::new().expect("test fixture should be valid");
    for args in [
        vec![
            option("query", "x"),
            ToolArgument::flag("hydrate").expect("test fixture should be valid"),
            option("max", "51"),
        ],
        vec![
            option("query", "x"),
            ToolArgument::flag("hydrate").expect("test fixture should be valid"),
            ToolArgument::flag("labels").expect("test fixture should be valid"),
        ],
        vec![option("query", "x"), option("body-limit", "1")],
        vec![
            option("query", "x"),
            ToolArgument::flag("hydrate").expect("test fixture should be valid"),
            option("body-limit", "0"),
        ],
    ] {
        let request = ToolRequest::new("google.mail.search", "google.work", ExecutionMode::Auto, args)
            .expect("test fixture should be valid");
        assert!(adapter
            .plan(
                &planning_target(),
                &request,
                adapter.find_tool(&request.tool).expect("test fixture should be valid")
            )
            .is_err());
    }
}

#[test]
fn expired_context_budget_preserves_sources_without_starting_more_reads() {
    let _guard = lock_env();
    let script = fixture();
    let adapter = GoogleWorkspaceAdapter::new().expect("catalog should load");
    let request = ToolRequest::new("google.mail.search", "google.work", ExecutionMode::Auto, hydrate())
        .expect("request should build");
    let action = adapter
        .plan(
            &planning_target(),
            &request,
            adapter.find_tool(&request.tool).expect("tool exists"),
        )
        .expect("plan should build");
    let limits = ContextLimits {
        body: 100,
        attachments: 20,
        started: Instant::now() - Duration::from_secs(121),
    };
    let sources = [ContextSource::message("message-1".into(), Some("thread-1".into()))];
    let collection = adapter
        .read_context(&execution_target(), &action, &sources, limits)
        .expect("partial collection should survive");
    assert_eq!(collection.messages.len(), 1);
    assert_eq!(collection.messages[0].gmail_message_id, "message-1");
    assert_eq!(collection.messages[0].coverage, CoverageStatus::Unknown);
    assert_eq!(collection.failures.len(), 1);
    assert_eq!(collection.coverage(None).status, CoverageStatus::Unknown);
    assert!(script.capture_contents().is_empty());
}

#[test]
fn native_gws_producer_accepts_body_free_thread_inventory_projection() {
    #[derive(Deserialize)]
    struct DryRun {
        method: String,
        query_params: Vec<(String, String)>,
        url: String,
    }
    let params = ThreadParams {
        user_id: "me",
        id: "thread-fixture",
        format: "full",
        fields: thread_fields(),
    };
    let output = Command::new("gws")
        .args([
            "gmail",
            "users",
            "threads",
            "get",
            "--params",
            &serde_json::to_string(&params).expect("test fixture should be valid"),
            "--dry-run",
        ])
        .output()
        .expect("test fixture should be valid");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let request: DryRun = serde_json::from_slice(&output.stdout).expect("test fixture should be valid");
    assert_eq!(request.method, "GET");
    assert!(request.url.ends_with("/threads/thread%2Dfixture"));
    let fields = &request
        .query_params
        .iter()
        .find(|(name, _)| name == "fields")
        .expect("test fixture should be valid")
        .1;
    assert_eq!(fields, &params.fields);
    assert!(!fields.contains("data"));
    assert!(fields.contains("attachmentId"));
}

#[test]
fn metadata_projection_depth_is_unknown_not_empty_attachment_list() {
    let part: Part = serde_json::from_str(r#"{"mimeType":"multipart/mixed","parts":[{"partId":"nested"}]}"#)
        .expect("test fixture should be valid");
    let mut items = Vec::new();
    let mut omitted = 0;
    let mut unknown = false;
    part.attachments(&mut items, &mut omitted, &mut unknown, 20);
    assert!(unknown);
    assert!(items.is_empty());
}
