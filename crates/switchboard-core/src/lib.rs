mod coverage;
mod engine;
mod error;
mod failure;
mod operation;
pub mod process;
mod traits;
mod types;
mod verification;

pub use crate::{
    coverage::{CoverageStatus, ReadCoverage},
    engine::{AdapterRegistry, Switchboard, SwitchboardServices},
    error::{Error, Result},
    failure::{Failure, FailureCode, FailurePhase, RecoveryAction},
    operation::{
        AggregateReadOutcome, AggregateReadRequest, AggregateReadResult, DispatchOutcome, OperationOutcome,
        OperationRequest,
    },
    traits::{
        Adapter, AuditStore, AuthStore, NamespaceStore, OperationStore, PolicyEngine, SecretResolver, SecretStore,
    },
    types::{
        ApprovalState, AuditEvent, AuditEventId, AuditOutcome, AuthKind, AuthRef, AuthScopeProfile, AuthSecretRefs,
        BackendKind, ExecutionMode, ExecutionTarget, ExecutionTimings, FileChecksum, NamespaceId, OperationApproval,
        OperationEffect, OperationId, OperationStatus, PlannedAction, PlanningTarget, PolicyDecision, ProviderKind,
        RegisteredTool, ResolvedAuth, ResolvedCredentials, ResolvedNamespace, ResolvedSecret, SecretRef, SecretSource,
        SecretString, StoredAuditEvent, StoredOperation, ToolArgument, ToolArgumentSpec, ToolArgumentTransport,
        ToolArgumentValueKind, ToolArguments, ToolDescriptor, ToolExecutionSupport, ToolKind, ToolName, ToolOutput,
        ToolRef, ToolRefKind, ToolRequest, ToolSurface, ToolUndoSupport, WritePolicy,
    },
    verification::{VerificationReceipt, VerificationStatus},
};
