use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Map, Value};
use switchboard_core::{
    AuthKind, AuthSecretRefs, ExecutionMode, ExecutionTarget, PlannedAction, PlanningTarget, ProviderKind,
    ResolvedAuth, ResolvedCredentials, ResolvedNamespace, ToolArgument, ToolKind, ToolOutput, ToolRefKind, ToolRequest,
};

use crate::cli::{
    command::CliResponse,
    declarative::{
        CliArgsSegment, CliArgsTemplate, CliComputedJsonValue, CliJsonArgumentField, CliJsonArgumentTemplate,
        CliJsonArgumentValue, CliJsonEffectSpec, CliJsonFieldMapping, CliJsonProjection, CliJsonProjectionConfig,
        CliJsonProjectionShape, CliJsonRefsSpec, CliProjectionTemplate, CliSummaryTemplate,
    },
};

#[test]
fn summary_template_renders_namespace_and_args() {
    let template = CliSummaryTemplate::parse("Search {arg:query} in {namespace}").expect("template should parse");
    let namespace = ResolvedNamespace::new(
        "github.personal",
        ProviderKind::GitHub,
        "GitHub personal",
        "github.personal_auth",
        true,
        None,
    )
    .expect("namespace should build");
    let request = ToolRequest::new(
        "github.pull_request.search",
        "github.personal",
        ExecutionMode::Auto,
        vec![ToolArgument::option("query", "is:open").expect("argument should build")],
    )
    .expect("request should build");

    let summary = template.render(&namespace, &request).expect("summary should render");
    assert_eq!(summary, "Search is:open in github.personal");
}

#[test]
fn summary_template_renders_mode_specific_verbs() {
    let template = CliSummaryTemplate::parse("{mode_verb:Draft delete of,Delete} event {arg:event-id} in {namespace}")
        .expect("template should parse");
    let namespace = ResolvedNamespace::new(
        "google.work",
        ProviderKind::GoogleWorkspace,
        "Google Workspace work",
        "google.work_auth",
        false,
        None,
    )
    .expect("namespace should build");
    let draft_request = ToolRequest::new(
        "google.calendar.delete",
        "google.work",
        ExecutionMode::Draft,
        vec![ToolArgument::option("event-id", "evt-123").expect("argument should build")],
    )
    .expect("request should build");
    let apply_request = ToolRequest::new(
        "google.calendar.delete",
        "google.work",
        ExecutionMode::Apply,
        vec![ToolArgument::option("event-id", "evt-123").expect("argument should build")],
    )
    .expect("request should build");

    assert_eq!(
        template
            .render(&namespace, &draft_request)
            .expect("draft summary should render"),
        "Draft delete of event evt-123 in google.work"
    );
    assert_eq!(
        template
            .render(&namespace, &apply_request)
            .expect("apply summary should render"),
        "Delete event evt-123 in google.work"
    );
}

#[test]
fn args_template_builds_positionals_options_and_flags() {
    let template = CliArgsTemplate::new(vec![
        CliArgsSegment::literal("search").expect("segment should build"),
        CliArgsSegment::literal("repos").expect("segment should build"),
        CliArgsSegment::required_positional(vec!["query".into()]).expect("segment should build"),
        CliArgsSegment::option("--limit", vec!["limit".into()], false, false).expect("segment should build"),
        CliArgsSegment::option("--topic", vec!["topic".into()], true, false).expect("segment should build"),
        CliArgsSegment::flag("--web", vec!["web".into()]).expect("segment should build"),
    ])
    .expect("template should build");
    let action = PlannedAction::new(
        &ToolRequest::new(
            "github.repository.search",
            "github.personal",
            ExecutionMode::Auto,
            vec![
                ToolArgument::option("query", "switchboard").expect("argument should build"),
                ToolArgument::option("limit", "10").expect("argument should build"),
                ToolArgument::option("topic", "rust").expect("argument should build"),
                ToolArgument::option("topic", "cli").expect("argument should build"),
                ToolArgument::flag("web").expect("argument should build"),
            ],
        )
        .expect("request should build"),
        &planning_target(),
        ToolKind::Read,
        "Search repos",
        switchboard_core::BackendKind::Cli,
    );

    let args = template.build_args(&action).expect("args should build");
    assert_eq!(
        args,
        vec![
            "search",
            "repos",
            "switchboard",
            "--limit",
            "10",
            "--topic",
            "rust",
            "--topic",
            "cli",
            "--web"
        ]
    );
}

#[test]
fn args_template_treats_boolean_option_values_as_flags() {
    let template = CliArgsTemplate::new(vec![
        CliArgsSegment::literal("search").expect("segment should build"),
        CliArgsSegment::literal("prs").expect("segment should build"),
        CliArgsSegment::required_positional(vec!["query".into()]).expect("segment should build"),
        CliArgsSegment::flag("--draft", vec!["draft".into()]).expect("segment should build"),
        CliArgsSegment::flag("--merged", vec!["merged".into()]).expect("segment should build"),
    ])
    .expect("template should build");
    let action = PlannedAction::new(
        &ToolRequest::new(
            "github.pull_request.search",
            "github.personal",
            ExecutionMode::Auto,
            vec![
                ToolArgument::option("query", "is:open").expect("argument should build"),
                ToolArgument::option("draft", "true").expect("argument should build"),
                ToolArgument::option("merged", "false").expect("argument should build"),
            ],
        )
        .expect("request should build"),
        &planning_target(),
        ToolKind::Read,
        "Search pull requests",
        switchboard_core::BackendKind::Cli,
    );

    let args = template.build_args(&action).expect("args should build");
    assert_eq!(args, vec!["search", "prs", "is:open", "--draft"]);
}

#[test]
fn args_template_builds_key_value_options() {
    let template = CliArgsTemplate::new(vec![
        CliArgsSegment::literal("api").expect("segment should build"),
        CliArgsSegment::literal("notifications").expect("segment should build"),
        CliArgsSegment::key_value_option("-F", "all", vec!["all".into()], false, false, true)
            .expect("segment should build"),
        CliArgsSegment::key_value_option("-F", "per_page", vec!["per_page".into()], false, false, false)
            .expect("segment should build"),
    ])
    .expect("template should build");
    let action = PlannedAction::new(
        &ToolRequest::new(
            "github.notifications.list",
            "github.personal",
            ExecutionMode::Auto,
            vec![
                ToolArgument::option("all", "true").expect("argument should build"),
                ToolArgument::option("per_page", "50").expect("argument should build"),
            ],
        )
        .expect("request should build"),
        &planning_target(),
        ToolKind::Read,
        "List notifications",
        switchboard_core::BackendKind::Cli,
    );

    let args = template.build_args(&action).expect("args should build");
    assert_eq!(
        args,
        vec!["api", "notifications", "-F", "all=true", "-F", "per_page=50"]
    );
}

#[test]
fn args_template_builds_json_segments_with_computed_values() {
    let template = CliArgsTemplate::new(vec![
        CliArgsSegment::literal("gmail").expect("segment should build"),
        CliArgsSegment::literal("users").expect("segment should build"),
        CliArgsSegment::literal("drafts").expect("segment should build"),
        CliArgsSegment::literal("create").expect("segment should build"),
        CliArgsSegment::literal("--json").expect("segment should build"),
        CliArgsSegment::json(CliJsonArgumentTemplate::new(CliJsonArgumentValue::Object {
            fields: vec![CliJsonArgumentField::new(
                "message",
                CliJsonArgumentValue::Object {
                    fields: vec![
                        CliJsonArgumentField::new(
                            "raw",
                            CliJsonArgumentValue::Computed(CliComputedJsonValue::GmailRawMessage),
                            false,
                        )
                        .expect("field should build"),
                        CliJsonArgumentField::new(
                            "threadId",
                            CliJsonArgumentValue::Argument {
                                aliases: vec!["thread-id".into()],
                                required: false,
                                repeated: false,
                                default: None,
                            },
                            true,
                        )
                        .expect("field should build"),
                    ],
                },
                false,
            )
            .expect("field should build")],
        })),
    ])
    .expect("template should build");
    let action = PlannedAction::new(
        &ToolRequest::new(
            "google.mail.draft",
            "google.work",
            ExecutionMode::Apply,
            vec![
                ToolArgument::option("to", "dogs@example.com").expect("argument should build"),
                ToolArgument::option("subject", "Boarding request").expect("argument should build"),
                ToolArgument::option("body-text", "Hi there").expect("argument should build"),
                ToolArgument::option("thread-id", "thread-123").expect("argument should build"),
            ],
        )
        .expect("request should build"),
        &google_planning_target(),
        ToolKind::Write,
        "Draft Gmail email to dogs@example.com in google.work",
        switchboard_core::BackendKind::Cli,
    );

    let args = template.build_args(&action).expect("args should build");
    let json_arg: Value = serde_json::from_str(&args[5]).expect("json segment should parse");
    assert_eq!(args[..5], ["gmail", "users", "drafts", "create", "--json"]);
    assert_eq!(json_arg["message"]["threadId"], "thread-123");
    assert!(json_arg["message"]["raw"].as_str().is_some());
}

#[test]
fn json_projection_decodes_array_response_and_refs() {
    let projection = CliJsonProjection::new(CliJsonProjectionConfig {
        response_field: "repositories".into(),
        source_pointer: None,
        shape: CliJsonProjectionShape::array(vec![
            CliJsonFieldMapping::from_pointer_with_items("name", "/name", None).expect("field should build"),
            CliJsonFieldMapping::from_pointer_with_items("full_name", "/fullName", None).expect("field should build"),
            CliJsonFieldMapping::from_pointer_with_items("url", "/url", None).expect("field should build"),
        ])
        .expect("shape should build"),
        count_field: Some("count".into()),
        extra_fields: Vec::new(),
        summary_template: Some(
            CliProjectionTemplate::parse("Found {count} repositories for {namespace}").expect("template should build"),
        ),
        refs: vec![CliJsonRefsSpec::new(
            ToolRefKind::Repository,
            "full_name",
            None,
            Some("name".into()),
            Some("url".into()),
        )],
        effect: None,
        empty_stdout_json: None,
    })
    .expect("projection should build");
    let action = PlannedAction::new(
        &ToolRequest::new(
            "github.repository.search",
            "github.personal",
            ExecutionMode::Auto,
            vec![ToolArgument::option("query", "switchboard").expect("argument should build")],
        )
        .expect("request should build"),
        &planning_target(),
        ToolKind::Read,
        "Search GitHub repositories matching switchboard",
        switchboard_core::BackendKind::Cli,
    );

    let output = projection
        .decode(
            &execution_target(),
            &action,
            CliResponse {
                program: PathBuf::from("gh"),
                version: "gh version 9.9.9-test".into(),
                stdout: r#"[{"name":"Switchboard","fullName":"KeepSafe/Switchboard","url":"https://github.com/KeepSafe/Switchboard"}]"#.into(),
                stderr: String::new(),
            },
        )
        .expect("projection should decode");

    assert_eq!(output.summary, "Found 1 repositories for github.personal");
    let fields: RepositoryProjectionFields = parse_output_fields(&output);
    assert_eq!(fields.count, 1);
    assert_eq!(fields.repositories[0].full_name, "KeepSafe/Switchboard");
    assert_eq!(output.refs.len(), 1);
    assert_eq!(output.refs[0].kind, ToolRefKind::Repository);
    assert_eq!(output.refs[0].id, "KeepSafe/Switchboard");
}

#[test]
fn json_projection_supports_argument_fields_and_effect_templates() {
    let projection = CliJsonProjection::new(CliJsonProjectionConfig {
        response_field: "event".into(),
        source_pointer: None,
        shape: CliJsonProjectionShape::object(vec![
            CliJsonFieldMapping::from_pointer_with_items("event_id", "/id", None).expect("field should build"),
            CliJsonFieldMapping::from_pointer_with_items("title", "/summary", None).expect("field should build"),
            CliJsonFieldMapping::from_argument("calendar", vec!["calendar".into()], Some("primary".into()))
                .expect("field should build"),
        ])
        .expect("shape should build"),
        count_field: None,
        extra_fields: Vec::new(),
        summary_template: Some(
            CliProjectionTemplate::parse("Created calendar event \"{field:title}\" for {namespace}")
                .expect("template should build"),
        ),
        refs: vec![CliJsonRefsSpec::new(
            ToolRefKind::Event,
            "event_id",
            Some("calendar".into()),
            Some("title".into()),
            None,
        )],
        effect: Some(CliJsonEffectSpec::new(
            true,
            true,
            Some(
                CliProjectionTemplate::parse("Delete calendar event \"{field:title}\" from {namespace}")
                    .expect("template should build"),
            ),
        )),
        empty_stdout_json: None,
    })
    .expect("projection should build");
    let action = PlannedAction::new(
        &ToolRequest::new(
            "google.calendar.create",
            "google.work",
            ExecutionMode::Apply,
            vec![
                ToolArgument::option("title", "Budget review").expect("argument should build"),
                ToolArgument::option("calendar", "primary").expect("argument should build"),
            ],
        )
        .expect("request should build"),
        &google_planning_target(),
        ToolKind::Write,
        "Create calendar event \"Budget review\" for google.work",
        switchboard_core::BackendKind::Cli,
    );

    let output = projection
        .decode(
            &google_execution_target(),
            &action,
            CliResponse {
                program: PathBuf::from("gws"),
                version: "gws 0.99.0-test".into(),
                stdout: r#"{"id":"event-1960budgetwork","summary":"Budget review"}"#.into(),
                stderr: String::new(),
            },
        )
        .expect("projection should decode");

    assert_eq!(
        output.summary,
        "Created calendar event \"Budget review\" for google.work"
    );
    assert_eq!(output.refs[0].id, "event-1960budgetwork");
    assert_eq!(output.refs[0].parent_id.as_deref(), Some("primary"));
    assert_eq!(output.effect.as_ref().map(|effect| effect.undoable), Some(true));
    assert_eq!(
        output.effect.as_ref().and_then(|effect| effect.undo_summary.as_deref()),
        Some("Delete calendar event \"Budget review\" from google.work")
    );
}

#[test]
fn json_projection_supports_array_field_extraction() {
    let projection = CliJsonProjection::new(CliJsonProjectionConfig {
        response_field: "pull_request".into(),
        source_pointer: None,
        shape: CliJsonProjectionShape::object(vec![
            CliJsonFieldMapping::from_pointer_with_items("title", "/title", None).expect("field should build"),
            CliJsonFieldMapping::from_pointer_with_items("assignees", "/assignees", Some("/login".into()))
                .expect("field should build"),
            CliJsonFieldMapping::from_pointer_with_items("labels", "/labels", Some("/name".into()))
                .expect("field should build"),
        ])
        .expect("shape should build"),
        count_field: None,
        extra_fields: Vec::new(),
        summary_template: Some(
            CliProjectionTemplate::parse("Read {field:title} for {namespace}").expect("template should build"),
        ),
        refs: Vec::new(),
        effect: None,
        empty_stdout_json: None,
    })
    .expect("projection should build");
    let action = PlannedAction::new(
        &ToolRequest::new(
            "github.pull_request.read",
            "github.personal",
            ExecutionMode::Auto,
            vec![
                ToolArgument::option("repo", "openai/codex").expect("argument should build"),
                ToolArgument::option("number", "1382").expect("argument should build"),
            ],
        )
        .expect("request should build"),
        &planning_target(),
        ToolKind::Read,
        "Read GitHub pull request openai/codex#1382 in github.personal",
        switchboard_core::BackendKind::Cli,
    );

    let output = projection
        .decode(
            &execution_target(),
            &action,
            CliResponse {
                program: PathBuf::from("gh"),
                version: "gh version 9.9.9-test".into(),
                stdout: r#"{"title":"Fix the thing","assignees":[{"login":"jessfraz"}],"labels":[{"name":"infra"},{"name":"tooling"}]}"#
                    .into(),
                stderr: String::new(),
            },
        )
        .expect("projection should decode");

    let fields: PullRequestProjectionFields = parse_output_fields(&output);
    assert_eq!(fields.pull_request.assignees, vec!["jessfraz"]);
    assert_eq!(fields.pull_request.labels, vec!["infra", "tooling"]);
}

#[test]
fn json_projection_supports_extra_fields_empty_stdout_and_multiple_refs() {
    let projection = CliJsonProjection::new(CliJsonProjectionConfig {
        response_field: "message".into(),
        source_pointer: None,
        shape: CliJsonProjectionShape::object(vec![
            CliJsonFieldMapping::from_argument("gmail_message_id", vec!["message-id".into()], None)
                .expect("field should build"),
            CliJsonFieldMapping::from_pointer_with_items("gmail_thread_id", "/thread_id", None)
                .expect("field should build"),
            CliJsonFieldMapping::from_pointer_with_items("subject", "/subject", None).expect("field should build"),
        ])
        .expect("shape should build"),
        count_field: None,
        extra_fields: vec![
            CliJsonFieldMapping::from_argument("query", vec!["query".into()], None).expect("field should build")
        ],
        summary_template: Some(
            CliProjectionTemplate::parse("Read Gmail message \"{field:subject}\" for {namespace}")
                .expect("template should build"),
        ),
        refs: vec![
            CliJsonRefsSpec::new(
                ToolRefKind::Message,
                "gmail_message_id",
                Some("gmail_thread_id".into()),
                Some("subject".into()),
                None,
            ),
            CliJsonRefsSpec::new(
                ToolRefKind::Thread,
                "gmail_thread_id",
                None,
                Some("subject".into()),
                None,
            ),
        ],
        effect: None,
        empty_stdout_json: Some(json!({
            "thread_id": "thread-123",
            "subject": "Dog hotel booking"
        })),
    })
    .expect("projection should build");
    let action = PlannedAction::new(
        &ToolRequest::new(
            "google.mail.read",
            "google.work",
            ExecutionMode::Auto,
            vec![
                ToolArgument::option("message-id", "msg-123").expect("argument should build"),
                ToolArgument::option("query", "from:doghotel").expect("argument should build"),
            ],
        )
        .expect("request should build"),
        &google_planning_target(),
        ToolKind::Read,
        "Read Gmail message msg-123 in google.work",
        switchboard_core::BackendKind::Cli,
    );

    let output = projection
        .decode(
            &google_execution_target(),
            &action,
            CliResponse {
                program: PathBuf::from("gws"),
                version: "gws 0.99.0-test".into(),
                stdout: String::new(),
                stderr: String::new(),
            },
        )
        .expect("projection should decode");

    let fields: MessageProjectionFields = parse_output_fields(&output);
    assert_eq!(fields.query, "from:doghotel");
    assert_eq!(fields.message.gmail_message_id, "msg-123");
    assert_eq!(fields.message.gmail_thread_id, "thread-123");
    assert_eq!(output.refs.len(), 2);
    assert_eq!(output.refs[0].kind, ToolRefKind::Message);
    assert_eq!(output.refs[0].parent_id.as_deref(), Some("thread-123"));
    assert_eq!(output.refs[1].kind, ToolRefKind::Thread);
}

#[test]
fn json_projection_supports_argument_values_and_presence_fields() {
    let projection = CliJsonProjection::new(CliJsonProjectionConfig {
        response_field: "draft".into(),
        source_pointer: None,
        shape: CliJsonProjectionShape::object(vec![
            CliJsonFieldMapping::from_pointer_with_items("draft_id", "/id", None).expect("field should build"),
            CliJsonFieldMapping::from_argument_values("to", vec!["to".into()]).expect("field should build"),
            CliJsonFieldMapping::from_argument_presence("has_body_text", vec!["body-text".into()])
                .expect("field should build"),
        ])
        .expect("shape should build"),
        count_field: None,
        extra_fields: Vec::new(),
        summary_template: None,
        refs: Vec::new(),
        effect: None,
        empty_stdout_json: None,
    })
    .expect("projection should build");
    let action = PlannedAction::new(
        &ToolRequest::new(
            "google.mail.draft",
            "google.work",
            ExecutionMode::Apply,
            vec![
                ToolArgument::option("to", "dogs@example.com").expect("argument should build"),
                ToolArgument::option("to", "frontdesk@example.com").expect("argument should build"),
                ToolArgument::option("body-text", "Hi there").expect("argument should build"),
            ],
        )
        .expect("request should build"),
        &google_planning_target(),
        ToolKind::Write,
        "Draft Gmail email to dogs@example.com, frontdesk@example.com in google.work",
        switchboard_core::BackendKind::Cli,
    );

    let output = projection
        .decode(
            &google_execution_target(),
            &action,
            CliResponse {
                program: PathBuf::from("gws"),
                version: "gws 0.99.0-test".into(),
                stdout: r#"{"id":"draft-123"}"#.into(),
                stderr: String::new(),
            },
        )
        .expect("projection should decode");

    let fields: MailDraftProjectionFields = parse_output_fields(&output);
    assert_eq!(fields.draft.draft_id, "draft-123");
    assert_eq!(fields.draft.to, vec!["dogs@example.com", "frontdesk@example.com"]);
    assert!(fields.draft.has_body_text);
}

#[derive(Debug, Deserialize)]
struct RepositoryProjectionFields {
    count: usize,
    repositories: Vec<RepositoryProjectionItem>,
}

#[derive(Debug, Deserialize)]
struct RepositoryProjectionItem {
    full_name: String,
}

#[derive(Debug, Deserialize)]
struct PullRequestProjectionFields {
    pull_request: PullRequestProjectionItem,
}

#[derive(Debug, Deserialize)]
struct PullRequestProjectionItem {
    assignees: Vec<String>,
    labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct MessageProjectionFields {
    query: String,
    message: MessageProjectionItem,
}

#[derive(Debug, Deserialize)]
struct MessageProjectionItem {
    gmail_message_id: String,
    gmail_thread_id: String,
}

#[derive(Debug, Deserialize)]
struct MailDraftProjectionFields {
    draft: MailDraftProjectionItem,
}

#[derive(Debug, Deserialize)]
struct MailDraftProjectionItem {
    draft_id: String,
    to: Vec<String>,
    has_body_text: bool,
}

fn parse_output_fields<T: for<'de> Deserialize<'de>>(output: &ToolOutput) -> T {
    serde_json::from_value(Value::Object(
        output
            .fields
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Map<String, Value>>(),
    ))
    .expect("output fields should deserialize")
}

fn planning_target() -> PlanningTarget {
    PlanningTarget {
        namespace: ResolvedNamespace::new(
            "github.personal",
            ProviderKind::GitHub,
            "GitHub personal",
            "github.personal_auth",
            true,
            None,
        )
        .expect("namespace should build"),
        auth: ResolvedAuth::new(
            "github.personal_auth",
            ProviderKind::GitHub,
            AuthKind::GitHubToken,
            "GitHub personal",
            AuthSecretRefs::GitHubToken {
                token: switchboard_core::SecretRef::new("github.personal.token").expect("secret ref should build"),
            },
        )
        .expect("auth should build"),
    }
}

fn execution_target() -> ExecutionTarget {
    ExecutionTarget {
        namespace: ResolvedNamespace::new(
            "github.personal",
            ProviderKind::GitHub,
            "GitHub personal",
            "github.personal_auth",
            true,
            Some(PathBuf::from("/tmp/gh-personal")),
        )
        .expect("namespace should build"),
        auth: ResolvedAuth::new(
            "github.personal_auth",
            ProviderKind::GitHub,
            AuthKind::GitHubToken,
            "GitHub personal",
            AuthSecretRefs::GitHubToken {
                token: switchboard_core::SecretRef::new("github.personal.token").expect("secret ref should build"),
            },
        )
        .expect("auth should build"),
        credentials: ResolvedCredentials::GitHubToken {
            token: "ghp-test-token".to_owned().into(),
        },
    }
}

fn google_planning_target() -> PlanningTarget {
    PlanningTarget {
        namespace: ResolvedNamespace::new(
            "google.work",
            ProviderKind::GoogleWorkspace,
            "Google Workspace work",
            "google.work_auth",
            true,
            Some(PathBuf::from("/tmp/gws-work")),
        )
        .expect("namespace should build"),
        auth: ResolvedAuth::new(
            "google.work_auth",
            ProviderKind::GoogleWorkspace,
            AuthKind::GoogleOAuth,
            "Google Workspace work",
            AuthSecretRefs::GoogleOAuth {
                client_id: switchboard_core::SecretRef::new("google.work.client_id").expect("secret ref should build"),
                client_secret: switchboard_core::SecretRef::new("google.work.client_secret")
                    .expect("secret ref should build"),
                refresh_token: None,
            },
        )
        .expect("auth should build"),
    }
}

fn google_execution_target() -> ExecutionTarget {
    ExecutionTarget {
        namespace: ResolvedNamespace::new(
            "google.work",
            ProviderKind::GoogleWorkspace,
            "Google Workspace work",
            "google.work_auth",
            true,
            Some(PathBuf::from("/tmp/gws-work")),
        )
        .expect("namespace should build"),
        auth: ResolvedAuth::new(
            "google.work_auth",
            ProviderKind::GoogleWorkspace,
            AuthKind::GoogleOAuth,
            "Google Workspace work",
            AuthSecretRefs::GoogleOAuth {
                client_id: switchboard_core::SecretRef::new("google.work.client_id").expect("secret ref should build"),
                client_secret: switchboard_core::SecretRef::new("google.work.client_secret")
                    .expect("secret ref should build"),
                refresh_token: None,
            },
        )
        .expect("auth should build"),
        credentials: ResolvedCredentials::GoogleOAuth {
            client_id: "client-id".to_owned().into(),
            client_secret: "client-secret".to_owned().into(),
            refresh_token: None,
        },
    }
}
