mod audit;
mod auth;
mod context;
mod identity;
mod operation;
mod policy;
mod tool;

pub use crate::types::{
    audit::{AuditEvent, AuditEventId, AuditOutcome, StoredAuditEvent},
    auth::{
        AuthKind, AuthRef, AuthSecretRefs, ResolvedAuth, ResolvedCredentials, ResolvedSecret, SecretRef,
        SecretSource, SecretString,
    },
    context::{BackendKind, ExecutionTarget, PlanningTarget, ResolvedNamespace},
    identity::{NamespaceId, OperationId, ProviderKind, ToolName},
    operation::{ApprovalState, OperationApproval, OperationEffect, OperationStatus, StoredOperation},
    policy::{PolicyDecision, WritePolicy},
    tool::{
        ExecutionMode, PlannedAction, RegisteredTool, ToolArgument, ToolArgumentSpec, ToolArgumentTransport,
        ToolArgumentValueKind, ToolArguments, ToolDescriptor, ToolExecutionSupport, ToolKind, ToolOutput, ToolRef,
        ToolRefKind, ToolRequest, ToolSurface, ToolUndoSupport,
    },
};

use crate::error::{Error, Result};

fn validate_non_empty(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::InvalidArguments(format!("{label} cannot be empty")));
    }

    Ok(())
}

fn validate_argument_name(name: String) -> Result<String> {
    if name.trim().is_empty() {
        return Err(Error::InvalidArguments("tool argument name cannot be empty".into()));
    }

    Ok(name)
}

fn normalize_argument_aliases(name: &str, aliases: Vec<String>) -> Result<Vec<String>> {
    let mut normalized = Vec::new();
    for alias in aliases {
        let alias = validate_argument_name(alias)?;
        if alias == name || normalized.contains(&alias) {
            continue;
        }
        normalized.push(alias);
    }

    Ok(normalized)
}
