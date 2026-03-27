use switchboard_core::{
    Adapter, BackendKind, Error, PlannedAction, ProviderKind, ResolvedNamespace, Result, ToolDescriptor, ToolKind,
    ToolOutput, ToolRequest,
};

const TOOLS: &[ToolDescriptor] = &[
    ToolDescriptor {
        name: "google.mail.search",
        kind: ToolKind::Read,
        summary: "Search Gmail",
        backend: BackendKind::Api,
    },
    ToolDescriptor {
        name: "google.mail.read",
        kind: ToolKind::Read,
        summary: "Read a Gmail message",
        backend: BackendKind::Api,
    },
    ToolDescriptor {
        name: "google.mail.draft",
        kind: ToolKind::Write,
        summary: "Draft an email",
        backend: BackendKind::Api,
    },
    ToolDescriptor {
        name: "google.mail.send",
        kind: ToolKind::Write,
        summary: "Send an email",
        backend: BackendKind::Api,
    },
    ToolDescriptor {
        name: "google.calendar.list",
        kind: ToolKind::Read,
        summary: "List calendar events",
        backend: BackendKind::Api,
    },
    ToolDescriptor {
        name: "google.calendar.create",
        kind: ToolKind::Write,
        summary: "Draft or create a calendar event",
        backend: BackendKind::Api,
    },
    ToolDescriptor {
        name: "google.drive.search",
        kind: ToolKind::Read,
        summary: "Search Drive files",
        backend: BackendKind::Api,
    },
];

#[derive(Default)]
pub struct GoogleWorkspaceAdapter;

impl GoogleWorkspaceAdapter {
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
        namespace: &ResolvedNamespace,
        request: &ToolRequest,
        descriptor: &'static ToolDescriptor,
    ) -> Result<PlannedAction> {
        let summary = Self::summary(namespace, request)?;
        Ok(PlannedAction::new(
            request,
            descriptor.kind,
            summary,
            descriptor.backend,
        ))
    }

    fn execute(&self, action: &PlannedAction) -> Result<ToolOutput> {
        if matches!(action.kind, ToolKind::Write) {
            return Err(Error::NotImplemented(format!(
                "{} apply path is not wired to Google Workspace yet",
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
        .with_field("note", "remote Google Workspace execution is not wired yet"))
    }
}
