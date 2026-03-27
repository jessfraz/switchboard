mod engine;
mod error;
mod operation;
mod traits;
mod types;

pub use crate::{
    engine::{AdapterRegistry, Switchboard, SwitchboardServices},
    error::{Error, Result},
    operation::{
        AggregateReadOutcome, AggregateReadRequest, AggregateReadResult, DispatchOutcome, OperationOutcome,
        OperationRequest,
    },
    traits::{
        Adapter, AuditSink, AuthStore, NamespaceStore, OperationStore, PolicyEngine, SecretResolver, SecretStore,
    },
    types::{
        AuditEvent, AuditOutcome, AuthKind, AuthRef, AuthSecretRefs, BackendKind, ExecutionMode, ExecutionTarget,
        NamespaceId, OperationEffect, OperationId, OperationStatus, PlannedAction, PlanningTarget, PolicyDecision,
        ProviderKind, ResolvedAuth, ResolvedCredentials, ResolvedNamespace, ResolvedSecret, SecretRef, SecretSource,
        SecretString, StoredOperation, ToolArgument, ToolArguments, ToolDescriptor, ToolKind, ToolName, ToolOutput,
        ToolRef, ToolRefKind, ToolRequest,
    },
};
