mod engine;
mod error;
mod operation;
mod traits;
mod types;

pub use crate::{
    engine::{AdapterRegistry, Switchboard},
    error::{Error, Result},
    operation::{
        AggregateReadOutcome, AggregateReadRequest, AggregateReadResult, DispatchOutcome, OperationOutcome,
        OperationRequest,
    },
    traits::{Adapter, AuditSink, AuthStore, NamespaceStore, PolicyEngine},
    types::{
        AuditEvent, AuditOutcome, AuthKind, AuthRef, BackendKind, ExecutionMode, ExecutionTarget, NamespaceId,
        PlannedAction, PolicyDecision, ProviderKind, ResolvedAuth, ResolvedNamespace, ToolDescriptor, ToolKind,
        ToolName, ToolOutput, ToolRequest,
    },
};
