use serde::Serialize;
use switchboard_core::{
    BackendKind, NamespaceId, ProviderKind, RegisteredTool, ResolvedNamespace, ToolArgumentSpec, ToolExecutionSupport,
    ToolKind, ToolName, ToolSurface, ToolUndoSupport,
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
        }
    }
}

fn curated_tool_examples(tool: &RegisteredTool, namespace: &str) -> Vec<String> {
    let mode_flag = match tool.kind {
        ToolKind::Read => "--json",
        ToolKind::Write => "--draft",
    };
    vec![format!("switchboard {} --ns {namespace} {mode_flag} ...", tool.name)]
}

fn raw_tool_examples(tool: &RegisteredTool, namespace: &str) -> Vec<String> {
    if let Some(examples) = mychart_raw_tool_examples(tool, namespace) {
        return examples;
    }

    if let Some(path) = inventory_raw_tool_path(&tool.name) {
        let command = path.join(" ");
        return match tool.kind {
            ToolKind::Read => vec![
                format!("switchboard {} --ns {namespace} --json -- --format json", tool.name),
                format!(
                    "switchboard {} --ns {namespace} --argv-json '[\"--format\",\"json\"]' --json",
                    tool.name
                ),
                format!("# fixed CLI path: {command}"),
            ],
            ToolKind::Write => vec![
                format!(
                    "switchboard {} --ns {namespace} --draft -- --format json ...",
                    tool.name
                ),
                format!(
                    "switchboard {} --ns {namespace} --argv-json '[\"--format\",\"json\",...]' --apply --json",
                    tool.name
                ),
                format!("# fixed CLI path: {command}"),
            ],
        };
    }

    match (tool.provider.clone(), tool.kind) {
        (ProviderKind::GoogleWorkspace, ToolKind::Read) => vec![
            format!(
                "switchboard {} --ns {namespace} --json -- calendar +agenda --format json --today",
                tool.name
            ),
            format!(
                "switchboard {} --ns {namespace} --argv-json '[\"gmail\",\"users\",\"messages\",\"list\",\"--query\",\"from:finance\",\"--format\",\"json\"]' --json",
                tool.name
            ),
        ],
        (ProviderKind::GoogleWorkspace, ToolKind::Write) => vec![
            format!(
                "switchboard {} --ns {namespace} --draft -- gmail users drafts create --params '{{\"userId\":\"me\"}}' --json '{{\"message\":{{\"raw\":\"SGVsbG8=\"}}}}' --format json",
                tool.name
            ),
            format!(
                "switchboard {} --ns {namespace} --argv-json '[\"calendar\",\"events\",\"insert\",\"--summary\",\"Vet visit\",\"--start\",\"2026-04-01T09:00:00-07:00\",\"--end\",\"2026-04-01T10:00:00-07:00\",\"--format\",\"json\"]' --apply --json",
                tool.name
            ),
        ],
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
                "switchboard {} --ns {namespace} --argv-json '[\"issue\",\"edit\",\"77\",\"--add-label\",\"triage\"]' --apply --json",
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
        (_, _) => vec![format!("switchboard {} --ns {namespace} -- ...", tool.name)],
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
