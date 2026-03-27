use serde::Serialize;

use crate::{
    error::{Error, Result},
    types::{ExecutionMode, NamespaceId, PlannedAction, ToolArguments, ToolKind, ToolName, ToolOutput, ToolRequest},
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum OperationRequest {
    Single(ToolRequest),
    AggregateRead(AggregateReadRequest),
}

impl OperationRequest {
    pub fn single(request: ToolRequest) -> Self {
        Self::Single(request)
    }

    pub fn aggregate_read(request: AggregateReadRequest) -> Self {
        Self::AggregateRead(request)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AggregateReadRequest {
    pub tool: ToolName,
    pub namespaces: Vec<NamespaceId>,
    pub mode: ExecutionMode,
    pub args: ToolArguments,
}

impl AggregateReadRequest {
    pub fn new(
        tool: impl Into<String>,
        namespaces: impl IntoIterator<Item = impl Into<String>>,
        mode: ExecutionMode,
        args: impl Into<ToolArguments>,
    ) -> Result<Self> {
        let tool = ToolName::new(tool)?;
        let namespaces = namespaces
            .into_iter()
            .map(NamespaceId::new)
            .collect::<Result<Vec<_>>>()?;

        if namespaces.is_empty() {
            return Err(Error::InvalidArguments(
                "aggregate reads require at least one namespace".into(),
            ));
        }

        Ok(Self {
            tool,
            namespaces,
            mode,
            args: args.into(),
        })
    }

    pub fn into_tool_requests(self) -> Vec<ToolRequest> {
        self.namespaces
            .into_iter()
            .map(|namespace| ToolRequest {
                tool: self.tool.clone(),
                namespace,
                mode: self.mode,
                args: self.args.clone(),
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum OperationOutcome {
    Single(DispatchOutcome),
    AggregateRead(AggregateReadOutcome),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum DispatchOutcome {
    Planned(PlannedAction),
    Executed(ToolOutput),
}

impl DispatchOutcome {
    pub fn kind(&self) -> ToolKind {
        match self {
            Self::Planned(plan) => plan.kind,
            Self::Executed(_) => ToolKind::Read,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AggregateReadOutcome {
    pub tool: ToolName,
    pub namespaces: Vec<NamespaceId>,
    pub results: Vec<AggregateReadResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AggregateReadResult {
    pub namespace: NamespaceId,
    pub outcome: DispatchOutcome,
}
