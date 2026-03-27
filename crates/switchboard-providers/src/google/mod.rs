mod backend;
mod materializer;

use switchboard_core::{
    Adapter, BackendKind, Error, ExecutionTarget, PlannedAction, PlanningTarget, ProviderKind, ResolvedNamespace,
    Result, ToolDescriptor, ToolRequest,
};

use crate::google::backend::{GoogleWorkspaceBackend, GoogleWorkspaceCliBackend};

const TOOLS: &[ToolDescriptor] = &[
    ToolDescriptor {
        name: "google.mail.search",
        kind: switchboard_core::ToolKind::Read,
        summary: "Search Gmail",
        backend: BackendKind::Cli,
    },
    ToolDescriptor {
        name: "google.mail.read",
        kind: switchboard_core::ToolKind::Read,
        summary: "Read a Gmail message",
        backend: BackendKind::Cli,
    },
    ToolDescriptor {
        name: "google.mail.draft",
        kind: switchboard_core::ToolKind::Write,
        summary: "Draft an email",
        backend: BackendKind::Cli,
    },
    ToolDescriptor {
        name: "google.mail.send",
        kind: switchboard_core::ToolKind::Write,
        summary: "Send an email",
        backend: BackendKind::Cli,
    },
    ToolDescriptor {
        name: "google.calendar.list",
        kind: switchboard_core::ToolKind::Read,
        summary: "List calendar events",
        backend: BackendKind::Cli,
    },
    ToolDescriptor {
        name: "google.calendar.create",
        kind: switchboard_core::ToolKind::Write,
        summary: "Draft or create a calendar event",
        backend: BackendKind::Cli,
    },
    ToolDescriptor {
        name: "google.drive.search",
        kind: switchboard_core::ToolKind::Read,
        summary: "Search Drive files",
        backend: BackendKind::Cli,
    },
];

pub struct GoogleWorkspaceAdapter {
    backend: Box<dyn GoogleWorkspaceBackend>,
}

impl Default for GoogleWorkspaceAdapter {
    fn default() -> Self {
        Self::new(Box::new(GoogleWorkspaceCliBackend::default()))
    }
}

impl GoogleWorkspaceAdapter {
    fn new(backend: Box<dyn GoogleWorkspaceBackend>) -> Self {
        Self { backend }
    }

    fn arg<'a>(request: &'a ToolRequest, key: &str) -> Option<&'a str> {
        request.args.get(key).map(String::as_str)
    }

    fn required_arg<'a>(request: &'a ToolRequest, key: &str) -> Result<&'a str> {
        Self::arg(request, key)
            .ok_or_else(|| Error::InvalidArguments(format!("missing required argument --{key} for {}", request.tool)))
    }

    fn summary(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
        let summary = match request.tool.as_str() {
            "google.mail.search" => {
                let query = Self::required_arg(request, "query")?;
                format!("Search Gmail in {} for {query:?}", namespace.id)
            }
            "google.mail.read" => {
                let message_id = Self::required_arg(request, "message-id")?;
                format!("Read Gmail message {message_id}")
            }
            "google.mail.draft" | "google.mail.send" => {
                let to = Self::required_arg(request, "to")?;
                format!("Draft email to {to} from {}", namespace.id)
            }
            "google.calendar.list" => format!("List calendar events for {}", namespace.id),
            "google.calendar.create" => {
                let title = Self::required_arg(request, "title")?;
                let start = Self::required_arg(request, "start")?;
                format!("Draft calendar event {title:?} starting at {start}")
            }
            "google.drive.search" => {
                let query = Self::required_arg(request, "query")?;
                format!("Search Google Drive in {} for {query:?}", namespace.id)
            }
            _ => {
                return Err(Error::UnsupportedTool(request.tool.to_string()));
            }
        };

        Ok(summary)
    }
}

impl Adapter for GoogleWorkspaceAdapter {
    fn provider(&self) -> ProviderKind {
        ProviderKind::GoogleWorkspace
    }

    fn tools(&self) -> &'static [ToolDescriptor] {
        TOOLS
    }

    fn plan(
        &self,
        target: &PlanningTarget,
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

    fn execute(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<switchboard_core::ToolOutput> {
        self.backend.execute(target, action)
    }
}
