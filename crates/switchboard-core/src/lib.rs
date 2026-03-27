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
    traits::{Adapter, AuditSink, AuthStore, NamespaceStore, PolicyEngine, SecretResolver, SecretStore},
    types::{
        AuditEvent, AuditOutcome, AuthKind, AuthRef, AuthSecretRefs, BackendKind, ExecutionMode, ExecutionTarget,
        NamespaceId, PlannedAction, PlanningTarget, PolicyDecision, ProviderKind, ResolvedAuth, ResolvedCredentials,
        ResolvedNamespace, ResolvedSecret, SecretRef, SecretSource, SecretString, ToolDescriptor, ToolKind, ToolName,
        ToolOutput, ToolRequest,
    },
};
