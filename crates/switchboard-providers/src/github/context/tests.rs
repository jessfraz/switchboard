use switchboard_core::{CoverageStatus, ToolArgument, ToolArguments, ToolName};

use crate::github::context::{
    models::{ContextNode, GraphResponse},
    ContextOptions,
};

fn options(include: &str, limit: u64, body_limit: u64) -> ContextOptions {
    ContextOptions::parse(
        &ToolName::new("github.pull_request.context").expect("tool"),
        &ToolArguments::new(vec![
            ToolArgument::option("repo", "example/repo").expect("repo"),
            ToolArgument::option("number", "7").expect("number"),
            ToolArgument::option("include", include).expect("include"),
            ToolArgument::option("limit", limit.to_string()).expect("limit"),
            ToolArgument::option("body-limit", body_limit.to_string()).expect("body limit"),
        ]),
    )
    .expect("options")
}

const CORE: &str = r#"{"number":7,"title":"A change","url":"https://github.com/example/repo/pull/7","state":"OPEN","body":"éééé","updatedAt":"2026-01-01T00:00:00Z","headRefOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","headRefName":"feature","baseRefName":"main","isDraft":false}"#;

#[test]
fn body_limit_respects_unicode_and_marks_incomplete_context() {
    let node: ContextNode = serde_json::from_str(CORE).expect("provider core");
    let result = node.into_context(&options("none", 10, 3), false).expect("context");
    assert_eq!(result.coverage(), CoverageStatus::Truncated);
    let encoded = serde_json::to_string(&result).expect("context JSON");
    #[derive(serde::Deserialize)]
    struct Body {
        body: String,
        body_truncated: bool,
    }
    let body: Body = serde_json::from_str(&encoded).expect("body");
    assert_eq!(body.body, "ééé");
    assert!(body.body_truncated);
}

#[test]
fn missing_requested_section_or_different_object_is_not_empty_success() {
    let node: ContextNode = serde_json::from_str(CORE).expect("provider core");
    assert!(node.into_context(&options("files", 10, 100), true).is_err());
    let node: ContextNode =
        serde_json::from_str(&CORE.replace("\"number\":7", "\"number\":8")).expect("different object");
    assert!(node.into_context(&options("none", 10, 100), false).is_err());
}

#[test]
fn graphql_error_payload_is_distinct_from_missing_object() {
    let envelope: GraphResponse<ContextNode> =
        serde_json::from_str(r#"{"data":null,"errors":[{"message":"Resource not accessible"}]}"#)
            .expect("error envelope");
    assert!(envelope.data.is_none());
    assert_eq!(envelope.errors.len(), 1);
}

#[test]
fn explicit_includes_only_request_bounded_selected_sections() {
    let selected = options("files", 2, 100);
    let query = selected.query(true);
    assert!(query.contains("files(first:$limit)"));
    assert!(!query.contains("comments("));
    assert!(!query.contains("commits("));
    let invalid = ToolArguments::new(vec![
        ToolArgument::option("repo", "example/repo").expect("repo"),
        ToolArgument::option("number", "7").expect("number"),
        ToolArgument::option("include", "files").expect("include"),
    ]);
    assert!(ContextOptions::parse(&ToolName::new("github.issue.context").expect("tool"), &invalid).is_err());
}
