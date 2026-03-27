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
    traits::{Adapter, AuditSink, NamespaceStore, PolicyEngine},
    types::{
        AuditEvent, AuditOutcome, AuthRef, BackendKind, ExecutionMode, NamespaceId, PlannedAction, PolicyDecision,
        ProviderKind,
        ResolvedNamespace, ToolDescriptor, ToolKind, ToolName, ToolOutput, ToolRequest,
    },
};
