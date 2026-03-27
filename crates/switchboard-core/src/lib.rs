mod engine;
mod error;
mod operation;
mod traits;
mod types;

pub use crate::engine::{AdapterRegistry, Switchboard};
pub use crate::error::{Error, Result};
pub use crate::operation::{
    AggregateReadOutcome, AggregateReadRequest, AggregateReadResult, DispatchOutcome, OperationOutcome,
    OperationRequest,
};
pub use crate::traits::{Adapter, AuditSink, NamespaceStore, PolicyEngine};
pub use crate::types::{
    AuditEvent, AuditOutcome, BackendKind, ExecutionMode, NamespaceId, PlannedAction, PolicyDecision, ProviderKind,
    ResolvedNamespace, ToolDescriptor, ToolKind, ToolName, ToolOutput, ToolRequest,
};
