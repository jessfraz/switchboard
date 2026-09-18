use std::collections::BTreeMap;

use serde::Serialize;
use switchboard_core::{
    BackendKind, NamespaceId, ProviderKind, RegisteredTool, ResolvedNamespace, ToolArgumentSpec, ToolArgumentValueKind,
    ToolExecutionSupport, ToolKind, ToolName, ToolSurface, ToolUndoSupport,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCatalogStatus {
    Stable,
    PlanningOnly,
    Raw,
}

impl ToolCatalogStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::PlanningOnly => "planning_only",
            Self::Raw => "raw",
        }
    }
}

pub fn tool_catalog_status(tool: &RegisteredTool) -> ToolCatalogStatus {
    if tool.surface == ToolSurface::Raw {
        ToolCatalogStatus::Raw
    } else if tool.execution_support == ToolExecutionSupport::PlanningOnly {
        ToolCatalogStatus::PlanningOnly
    } else {
        ToolCatalogStatus::Stable
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolCatalogEntry {
    pub name: ToolName,
    pub provider: ProviderKind,
    pub kind: ToolKind,
    pub backend: BackendKind,
    pub summary: String,
    pub surface: ToolSurface,
    pub aggregate_read_supported: bool,
    pub execution_support: ToolExecutionSupport,
    pub undo_support: ToolUndoSupport,
    pub status: ToolCatalogStatus,
}

impl From<&RegisteredTool> for ToolCatalogEntry {
    fn from(tool: &RegisteredTool) -> Self {
        Self {
            name: tool.name.clone(),
            provider: tool.provider.clone(),
            kind: tool.kind,
            backend: tool.backend,
            summary: tool.summary.to_owned(),
            surface: tool.surface,
            aggregate_read_supported: tool.aggregate_read_supported,
            execution_support: tool.execution_support,
            undo_support: tool.undo_support,
            status: tool_catalog_status(tool),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CatalogPagination {
    pub limit_argument: &'static str,
    pub cursor_argument: &'static str,
    pub coverage_field: &'static str,
    pub default_limit: u32,
    pub max_limit: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct RawFallback {
    /// A concrete example, not a substitution for the caller's original inputs.
    pub argv: Vec<String>,
    pub purpose: &'static str,
}

/// The execution envelope. Provider payloads whose shape depends on
/// native argv remain unconstrained instead of promising a made-up schema.
#[derive(Clone, Debug, Default, Serialize)]
pub struct OutputSchema {
    #[serde(rename = "type", skip_serializing_if = "Vec::is_empty")]
    types: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'static str>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    properties: BTreeMap<&'static str, OutputSchema>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    required: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    items: Option<Box<OutputSchema>>,
    #[serde(rename = "enum", skip_serializing_if = "Vec::is_empty")]
    values: Vec<&'static str>,
}

impl OutputSchema {
    fn value(types: &[&'static str]) -> Self {
        Self {
            types: types.to_vec(),
            ..Self::default()
        }
    }
    fn object(properties: impl IntoIterator<Item = (&'static str, Self)>, required: &[&'static str]) -> Self {
        Self {
            properties: properties.into_iter().collect(),
            required: required.to_vec(),
            ..Self::value(&["object"])
        }
    }
    fn array(items: Self) -> Self {
        Self {
            items: Some(Box::new(items)),
            ..Self::value(&["array"])
        }
    }
    fn enumeration(values: &[&'static str]) -> Self {
        Self {
            values: values.to_vec(),
            ..Self::value(&["string"])
        }
    }
}

fn output_schema(tool: &RegisteredTool) -> OutputSchema {
    let fields = if tool.name.as_str() == "google.mail.search" {
        let message = OutputSchema::object(
            [
                ("gmail_message_id", OutputSchema::value(&["string"])),
                ("from", OutputSchema::value(&["string", "null"])),
                ("subject", OutputSchema::value(&["string", "null"])),
                ("date", OutputSchema::value(&["string", "null"])),
                (
                    "labels",
                    OutputSchema {
                        types: vec!["array", "null"],
                        items: Some(Box::new(OutputSchema::value(&["string"]))),
                        ..OutputSchema::default()
                    },
                ),
            ],
            &["gmail_message_id", "from", "subject", "date", "labels"],
        );
        OutputSchema::object(
            [
                ("status", OutputSchema::enumeration(&["ok", "partial"])),
                ("query", OutputSchema::value(&["string"])),
                ("count", OutputSchema::value(&["integer"])),
                ("messages", OutputSchema::array(message)),
                ("result_size_estimate", OutputSchema::value(&["integer", "null"])),
                (
                    "failures",
                    OutputSchema::array(OutputSchema::object(
                        [
                            ("code", OutputSchema::value(&["string"])),
                            ("message", OutputSchema::value(&["string"])),
                            ("retryable", OutputSchema::value(&["boolean"])),
                        ],
                        &["code", "message", "retryable"],
                    )),
                ),
            ],
            &[
                "status",
                "query",
                "count",
                "messages",
                "result_size_estimate",
                "failures",
            ],
        )
    } else if tool.surface == ToolSurface::Raw {
        OutputSchema::object(
            [
                (
                    "response",
                    OutputSchema {
                        description: Some("Native JSON response, unconstrained by Switchboard."),
                        ..OutputSchema::default()
                    },
                ),
                ("stdout_text", OutputSchema::value(&["string"])),
                ("cli_stderr", OutputSchema::value(&["string"])),
            ],
            &[],
        )
    } else {
        OutputSchema {
            description: Some("Provider-specific fields; only the execution envelope is described here."),
            ..OutputSchema::value(&["object"])
        }
    };
    let mut schema = OutputSchema::object(
        [
            ("schema_version", OutputSchema::value(&["integer"])),
            ("status", OutputSchema::enumeration(&["executed", "partial"])),
            ("tool", OutputSchema::value(&["string"])),
            ("namespace", OutputSchema::value(&["string"])),
            ("summary", OutputSchema::value(&["string"])),
            ("fields", fields),
            (
                "refs",
                OutputSchema::array(OutputSchema::object(
                    [
                        ("provider", OutputSchema::value(&["string"])),
                        ("namespace", OutputSchema::value(&["string"])),
                        ("kind", OutputSchema::value(&["string"])),
                        ("id", OutputSchema::value(&["string"])),
                    ],
                    &["provider", "namespace", "kind", "id"],
                )),
            ),
        ],
        &[
            "schema_version",
            "status",
            "tool",
            "namespace",
            "summary",
            "fields",
            "refs",
        ],
    );
    schema.description = Some("Single-namespace execution, including partial results or failed readback with nonzero exit. Draft/plan, aggregate, and failure receipts have separate envelopes. Additional fields remain allowed for forward compatibility.");
    if tool.name.as_str() == "google.mail.search" {
        schema.properties.insert(
            "coverage",
            OutputSchema::object(
                [
                    (
                        "status",
                        OutputSchema::enumeration(&["complete", "truncated", "unknown"]),
                    ),
                    ("next_cursor", OutputSchema::value(&["string", "null"])),
                ],
                &["status", "next_cursor"],
            ),
        );
        schema.required.push("coverage");
    }
    schema
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolCatalogDetail {
    pub name: ToolName,
    pub provider: ProviderKind,
    pub kind: ToolKind,
    pub backend: BackendKind,
    pub summary: String,
    pub surface: ToolSurface,
    pub aggregate_read_supported: bool,
    pub execution_support: ToolExecutionSupport,
    pub undo_support: ToolUndoSupport,
    pub status: ToolCatalogStatus,
    pub arguments: Vec<ToolArgumentSpec>,
    pub available_namespaces: Vec<NamespaceId>,
    pub notes: Vec<String>,
    pub examples: Vec<String>,
    pub scope_guidance: Vec<String>,
    pub output_schema: OutputSchema,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pagination: Option<CatalogPagination>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_fallback: Option<RawFallback>,
}

impl ToolCatalogDetail {
    pub fn new(tool: &RegisteredTool, namespaces: &[ResolvedNamespace]) -> Self {
        let available_namespaces = namespaces
            .iter()
            .map(|namespace| namespace.id.clone())
            .collect::<Vec<_>>();
        let raw = tool.surface == ToolSurface::Raw;
        let example_namespace = namespaces
            .first()
            .map(|namespace| namespace.id.to_string())
            .unwrap_or_else(|| format!("{}.default", tool.provider));
        let mut notes = vec![
            "policy, auth isolation, and audit still apply".to_owned(),
            "repeat --ns for aggregate reads, writes stay single-namespace".to_owned(),
        ];
        if tool.execution_support == ToolExecutionSupport::PlanningOnly {
            notes.push("execution is not wired yet, this tool currently plans cleanly but will not apply".to_owned());
        }
        let scope_guidance = scope_guidance(tool);
        notes.extend(scope_guidance.iter().map(|scope| format!("Permissions: {scope}")));
        let pagination = (tool.name.as_str() == "google.mail.search").then_some(CatalogPagination {
            limit_argument: "max",
            cursor_argument: "cursor",
            coverage_field: "coverage",
            default_limit: 20,
            max_limit: 500,
        });
        if pagination.is_some() {
            notes.push("--max is 1..500 (default 20); continue with --cursor from coverage.next_cursor. coverage.status distinguishes complete, truncated, and unknown; fields.status=partial reports per-message failures.".into());
        }
        let raw_fallback = raw_fallback(tool, &example_namespace);
        if let Some(fallback) = &raw_fallback {
            notes.push(format!(
                "Raw fallback ({}): {}",
                fallback.purpose,
                shell_command(&fallback.argv)
            ));
        }
        let examples = if raw {
            notes.push(
                "put switchboard flags before --, everything after -- is forwarded to the provider CLI unchanged"
                    .to_owned(),
            );
            notes.push("for scripted calls, --argv-json accepts one JSON array of argv tokens".to_owned());
            notes.extend(raw_tool_notes(tool));
            raw_tool_examples(tool, &example_namespace)
        } else {
            curated_tool_examples(tool, &example_namespace)
        };

        Self {
            name: tool.name.clone(),
            provider: tool.provider.clone(),
            kind: tool.kind,
            backend: tool.backend,
            summary: tool.summary.to_owned(),
            surface: tool.surface,
            aggregate_read_supported: tool.aggregate_read_supported,
            execution_support: tool.execution_support,
            undo_support: tool.undo_support,
            status: tool_catalog_status(tool),
            arguments: tool.arguments.clone(),
            available_namespaces,
            notes,
            examples,
            scope_guidance,
            output_schema: output_schema(tool),
            pagination,
            raw_fallback,
        }
    }
}

fn curated_tool_examples(tool: &RegisteredTool, namespace: &str) -> Vec<String> {
    let mut argv = vec![
        "switchboard".into(),
        tool.name.to_string(),
        "--ns".into(),
        namespace.into(),
        match tool.kind {
            ToolKind::Read => "--json",
            ToolKind::Write => "--draft",
        }
        .into(),
    ];
    for argument in tool.arguments.iter().filter(|argument| argument.required) {
        argv.push(format!("--{}", argument.name));
        if argument.value_kind != ToolArgumentValueKind::Boolean {
            argv.push(example_value(&argument.name, argument.value_kind).into());
        }
    }
    if tool.name.as_str() == "google.drive.search" {
        argv.extend(["--query".into(), "name contains 'example'".into()]);
    }
    vec![shell_command(&argv)]
}

fn example_value(name: &str, kind: ToolArgumentValueKind) -> &str {
    if kind == ToolArgumentValueKind::Json {
        return "{}";
    }
    match name {
        "query" => "newer_than:7d",
        "max" | "limit" => "20",
        "repo" => "owner/repo",
        "number" => "123",
        "to" | "email" => "recipient@example.invalid",
        "subject" | "summary" | "title" => "Example reminder",
        "body" | "text" | "task" => "Ask for opening hours",
        "start" => "2026-10-01T09:00:00-05:00",
        "end" => "2026-10-01T10:00:00-05:00",
        "calendar" | "calendar-id" => "primary",
        "destination" => "+12125550100",
        "caller-name" => "Example",
        "max-duration-seconds" => "300",
        _ => "example-id",
    }
}

fn shell_command(argv: &[String]) -> String {
    argv.iter()
        .map(|argument| {
            if argument
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_./:@+=,-".contains(character))
            {
                argument.clone()
            } else {
                format!("'{}'", argument.replace('\'', "'\"'\"'"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn scope_guidance(tool: &RegisteredTool) -> Vec<String> {
    let guidance = match tool.name.as_str() {
        "google.mail.search" | "google.mail.read" => "Gmail read access: gmail.readonly or gmail.modify.",
        "google.mail.draft" => "Gmail draft access: gmail.compose or gmail.modify.",
        "google.calendar.list" => "Calendar read access: calendar.readonly or calendar.",
        "google.calendar.create" | "google.calendar.delete" => "Calendar event write access: calendar.events or calendar.",
        "google.drive.search" => "Drive metadata read access: drive.metadata.readonly, drive.readonly, or drive.",
        _ => match tool.provider {
            ProviderKind::GoogleWorkspace => "Scopes depend on the native service/method; inspect its native schema/help and use gws auth login service flags when authorizing.",
            ProviderKind::GitHub => "Token or GitHub App permissions depend on repository visibility and the requested endpoint; inspect native gh help for the endpoint.",
            ProviderKind::MyChart => "Access depends on the configured account's granted FHIR resources and scopes.",
            ProviderKind::Phone => "The configured phone namespace supplies provider credentials; no OAuth scopes are requested by catalog discovery.",
            _ => "Access depends on the configured namespace and native command.",
        },
    };
    vec![guidance.into()]
}

/// Return a concrete native CLI discovery or read example. This never executes
/// the fallback, and never claims to preserve the caller's query arguments.
pub fn raw_fallback(tool: &RegisteredTool, namespace: &str) -> Option<RawFallback> {
    if tool.surface == ToolSurface::Raw || tool.provider == ProviderKind::Phone {
        return None;
    }
    let (tail, purpose): (Vec<&str>, _) = match tool.name.as_str() {
        "google.mail.search" => (
            vec![
                "gmail",
                "users",
                "messages",
                "list",
                "--params",
                r#"{"userId":"me","q":"newer_than:7d","maxResults":20}"#,
            ],
            "read example; replace query, limit, and pageToken as needed",
        ),
        "google.drive.search" => (
            vec![
                "drive",
                "files",
                "list",
                "--params",
                r#"{"q":"trashed = false and name contains 'example'","pageSize":20}"#,
            ],
            "read example; replace Drive query and pageToken as needed",
        ),
        _ => (
            vec!["--help"],
            "native command discovery; select the corresponding method and arguments",
        ),
    };
    let mut argv = vec![
        "switchboard".into(),
        format!("{}.cli.read", tool.provider),
        "--ns".into(),
        namespace.into(),
        "--json".into(),
        "--".into(),
    ];
    argv.extend(tail.into_iter().map(str::to_owned));
    Some(RawFallback { argv, purpose })
}

fn raw_tool_examples(tool: &RegisteredTool, namespace: &str) -> Vec<String> {
    if let Some(examples) = mychart_raw_tool_examples(tool, namespace) {
        return examples;
    }

    if let Some(path) = inventory_raw_tool_path(&tool.name) {
        let mode = if tool.kind == ToolKind::Write {
            "--draft"
        } else {
            "--json"
        };
        return vec![
            format!("switchboard {} --ns {namespace} {mode} -- --help", tool.name),
            format!(
                "# fixed CLI path: {}; --help after -- is forwarded to the native command",
                path.join(" ")
            ),
        ];
    }

    match (tool.provider.clone(), tool.kind) {
        (ProviderKind::GoogleWorkspace, ToolKind::Read) => vec![format!(
            "switchboard {} --ns {namespace} --json -- gmail users messages list --params '{{\"userId\":\"me\",\"q\":\"newer_than:7d\",\"maxResults\":20}}'", tool.name)],
        (ProviderKind::GoogleWorkspace, ToolKind::Write) => vec![format!(
            "switchboard {} --ns {namespace} --draft -- calendar events insert --params '{{\"calendarId\":\"primary\"}}' --json '{{\"summary\":\"Example reminder\",\"start\":{{\"dateTime\":\"2026-10-01T09:00:00-05:00\"}},\"end\":{{\"dateTime\":\"2026-10-01T10:00:00-05:00\"}}}}'", tool.name)],
        (ProviderKind::GitHub, ToolKind::Read) => vec![
            format!(
                "switchboard {} --ns {namespace} --json -- repo view owner/repo --json name,visibility,defaultBranchRef",
                tool.name
            ),
            format!(
                "switchboard {} --ns {namespace} --argv-json '[\"search\",\"prs\",\"--repo\",\"owner/repo\",\"--state\",\"open\",\"--json\",\"number,title\"]' --json",
                tool.name
            ),
        ],
        (ProviderKind::GitHub, ToolKind::Write) => vec![
            format!(
                "switchboard {} --ns {namespace} --draft -- pr comment 123 --body 'needs tests'",
                tool.name
            ),
            format!(
                "switchboard {} --ns {namespace} --argv-json '[\"issue\",\"edit\",\"77\",\"--add-label\",\"triage\"]' --draft --json",
                tool.name
            ),
        ],
        (ProviderKind::MyChart, ToolKind::Read) => vec![
            format!(
                "switchboard {} --ns {namespace} --json -- notes search --query migraine",
                tool.name
            ),
            format!(
                "switchboard {} --ns {namespace} --argv-json '[\"appointments\",\"upcoming\",\"--limit\",\"5\"]' --json",
                tool.name
            ),
        ],
        (ProviderKind::MyChart, ToolKind::Write) => vec![
            format!(
                "switchboard {} --ns {} --draft -- login ucla",
                tool.name,
                mychart_example_namespace(namespace)
            ),
            format!(
                "switchboard {} --ns {} --draft -- finish '<auth-code>'",
                tool.name,
                mychart_example_namespace(namespace)
            ),
        ],
        (_, _) => vec![format!("switchboard {} --ns {namespace} --json -- --help", tool.name)],
    }
}

fn raw_tool_notes(tool: &RegisteredTool) -> Vec<String> {
    if tool.provider != ProviderKind::MyChart {
        return Vec::new();
    }

    let Some(path) = inventory_raw_tool_path(&tool.name) else {
        return vec![
            "for UCLA, use the preset login flow: `mychart login ucla`".to_owned(),
            "`mychart finish` is a fallback when the browser cannot reach the local login bridge".to_owned(),
        ];
    };
    let path = path.iter().map(String::as_str).collect::<Vec<_>>();
    match path.as_slice() {
        ["auth", "authorize-url"] | ["auth", "login"] | ["auth", "exchange-url"] => vec![
            "for UCLA, prefer the preset login flow: `mychart login ucla`".to_owned(),
            "low-level `mychart auth ...` commands are for custom FHIR endpoints and fallback plumbing".to_owned(),
        ],
        ["login"] => {
            vec!["for UCLA, pass `ucla`; the preset supplies the FHIR URL, client ID, and hosted callback".to_owned()]
        }
        ["finish"] => {
            vec!["`mychart finish` is a fallback when the browser cannot reach the local login bridge".to_owned()]
        }
        _ => Vec::new(),
    }
}

fn mychart_raw_tool_examples(tool: &RegisteredTool, namespace: &str) -> Option<Vec<String>> {
    if tool.provider != ProviderKind::MyChart {
        return None;
    }

    let namespace = mychart_example_namespace(namespace);
    let path = inventory_raw_tool_path(&tool.name)?;
    let path = path.iter().map(String::as_str).collect::<Vec<_>>();
    match path.as_slice() {
        ["auth", "authorize-url"] | ["auth", "login"] => Some(vec![
            format!("switchboard mychart.cli.login --ns {namespace} --draft -- ucla"),
            format!("switchboard mychart.cli.write --ns {namespace} --draft -- login ucla"),
            "# low-level auth commands are for custom FHIR endpoints, not the UCLA preset".to_owned(),
        ]),
        ["auth", "exchange-url"] => Some(vec![
            format!("switchboard mychart.cli.finish --ns {namespace} --draft -- '<auth-code>'"),
            "# auth exchange-url is the low-level fallback behind mychart finish".to_owned(),
        ]),
        ["login"] => Some(vec![
            format!("switchboard {} --ns {namespace} --draft -- ucla", tool.name),
            format!("switchboard mychart.cli.write --ns {namespace} --draft -- login ucla"),
        ]),
        ["finish"] => Some(vec![
            format!("switchboard {} --ns {namespace} --draft -- '<auth-code>'", tool.name),
            "# fallback only when the local login bridge does not receive the browser callback".to_owned(),
        ]),
        _ => None,
    }
}

fn mychart_example_namespace(namespace: &str) -> &str {
    if namespace == "mychart.default" {
        "mychart.ucla"
    } else {
        namespace
    }
}

fn inventory_raw_tool_path(tool: &ToolName) -> Option<Vec<String>> {
    let segments = tool.as_str().split('.').collect::<Vec<_>>();
    if segments.get(1).copied() != Some("cli") {
        return None;
    }
    if matches!(segments.get(2).copied(), Some("read" | "write")) && segments.len() == 3 {
        return None;
    }

    Some(segments.into_iter().skip(2).map(ToOwned::to_owned).collect())
}
