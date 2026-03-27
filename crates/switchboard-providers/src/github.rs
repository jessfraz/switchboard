use switchboard_core::{
    Adapter, BackendKind, Error, ExecutionTarget, PlannedAction, ProviderKind, ResolvedNamespace, Result,
    ToolDescriptor, ToolKind, ToolOutput, ToolRequest,
};

const TOOLS: &[ToolDescriptor] = &[
    ToolDescriptor {
        name: "github.notifications.list",
        kind: ToolKind::Read,
        summary: "List notifications for a GitHub namespace",
        backend: BackendKind::Cli,
    },
    ToolDescriptor {
        name: "github.pull_request.read",
        kind: ToolKind::Read,
        summary: "Read a pull request",
        backend: BackendKind::Cli,
    },
    ToolDescriptor {
        name: "github.pull_request.search",
        kind: ToolKind::Read,
        summary: "Search pull requests",
        backend: BackendKind::Cli,
    },
    ToolDescriptor {
        name: "github.pull_request.comment",
        kind: ToolKind::Write,
        summary: "Draft or send a pull request comment",
        backend: BackendKind::Cli,
    },
    ToolDescriptor {
        name: "github.issue.read",
        kind: ToolKind::Read,
        summary: "Read an issue",
        backend: BackendKind::Cli,
    },
    ToolDescriptor {
        name: "github.issue.comment",
        kind: ToolKind::Write,
        summary: "Draft or send an issue comment",
        backend: BackendKind::Cli,
    },
    ToolDescriptor {
        name: "github.repository.search",
        kind: ToolKind::Read,
        summary: "Search repositories",
        backend: BackendKind::Cli,
    },
];

#[derive(Default)]
pub struct GitHubAdapter;

impl GitHubAdapter {
    fn arg<'a>(request: &'a ToolRequest, key: &str) -> Option<&'a str> {
        request.args.get(key).map(String::as_str)
    }

    fn required_arg<'a>(request: &'a ToolRequest, key: &str) -> Result<&'a str> {
        Self::arg(request, key)
            .ok_or_else(|| Error::InvalidArguments(format!("missing required argument --{key} for {}", request.tool)))
    }

    fn summary(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
        let summary = match request.tool.as_str() {
            "github.notifications.list" => format!("List GitHub notifications for {}", namespace.id),
            "github.pull_request.read" => {
                let repo = Self::required_arg(request, "repo")?;
                let number = Self::required_arg(request, "number")?;
                format!("Read pull request {repo}#{number}")
            }
            "github.pull_request.search" => {
                let query = Self::required_arg(request, "query")?;
                format!("Search GitHub pull requests matching {query:?}")
            }
            "github.pull_request.comment" => {
                let repo = Self::required_arg(request, "repo")?;
                let number = Self::required_arg(request, "number")?;
                format!("Draft comment for pull request {repo}#{number}")
            }
            "github.issue.read" => {
                let repo = Self::required_arg(request, "repo")?;
                let number = Self::required_arg(request, "number")?;
                format!("Read issue {repo}#{number}")
            }
            "github.issue.comment" => {
                let repo = Self::required_arg(request, "repo")?;
                let number = Self::required_arg(request, "number")?;
                format!("Draft comment for issue {repo}#{number}")
            }
            "github.repository.search" => {
                let query = Self::required_arg(request, "query")?;
                format!("Search GitHub repositories matching {query:?}")
            }
            _ => {
                return Err(Error::UnsupportedTool(request.tool.to_string()));
            }
        };

        Ok(summary)
    }
}

impl Adapter for GitHubAdapter {
    fn provider(&self) -> ProviderKind {
        ProviderKind::GitHub
    }

    fn tools(&self) -> &'static [ToolDescriptor] {
        TOOLS
    }

    fn plan(
        &self,
        target: &ExecutionTarget,
        request: &ToolRequest,
        descriptor: &'static ToolDescriptor,
    ) -> Result<PlannedAction> {
        let summary = Self::summary(&target.namespace, request)?;
        Ok(PlannedAction::new(
            request,
            target,
            descriptor.kind,
            summary,
            descriptor.backend,
        ))
    }

    fn execute(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
        if matches!(action.kind, ToolKind::Write) {
            return Err(Error::NotImplemented(format!(
                "{} apply path is not wired to GitHub yet",
                action.tool
            )));
        }

        Ok(ToolOutput::new(
            action.tool.clone(),
            action.namespace.clone(),
            format!("{} via {} (stub)", action.summary, action.backend),
        )
        .with_field("status", "stub")
        .with_field("backend", action.backend.to_string())
        .with_field("auth", target.auth.id.to_string())
        .with_field("note", "remote GitHub execution is not wired yet"))
    }
}
