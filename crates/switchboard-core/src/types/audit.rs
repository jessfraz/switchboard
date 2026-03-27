use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::types::{
    AuthRef, BackendKind, NamespaceId, OperationId, PlannedAction, StoredOperation, ToolName,
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Planned,
    Approved,
    Rejected,
    Executed,
    Failed,
    Compensated,
    Blocked,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AuditEvent {
    pub tool: ToolName,
    pub namespace: NamespaceId,
    pub auth_ref: AuthRef,
    pub summary: String,
    pub backend: BackendKind,
    pub approval_required: bool,
    pub outcome: AuditOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compensates_operation_id: Option<OperationId>,
}

impl AuditEvent {
    pub fn from_plan(plan: &PlannedAction, outcome: AuditOutcome) -> Self {
        Self {
            tool: plan.tool.clone(),
            namespace: plan.namespace.clone(),
            auth_ref: plan.auth_ref.clone(),
            summary: plan.summary.clone(),
            backend: plan.backend,
            approval_required: plan.approval_required,
            outcome,
            operation_id: plan.operation_id.clone(),
            compensates_operation_id: plan.compensates_operation_id.clone(),
        }
    }

    pub fn from_operation(operation: &StoredOperation, outcome: AuditOutcome) -> Self {
        Self {
            tool: operation.tool.clone(),
            namespace: operation.namespace.clone(),
            auth_ref: operation.auth_ref.clone(),
            summary: operation.summary.clone(),
            backend: operation.backend,
            approval_required: operation.approval_required,
            outcome,
            operation_id: Some(operation.id.clone()),
            compensates_operation_id: operation.compensates_operation_id.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AuditEventId(String);

impl AuditEventId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        crate::types::validate_non_empty("audit event id", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AuditEventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredAuditEvent {
    pub id: AuditEventId,
    pub tool: ToolName,
    pub namespace: NamespaceId,
    pub auth_ref: AuthRef,
    pub summary: String,
    pub backend: BackendKind,
    pub approval_required: bool,
    pub outcome: AuditOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compensates_operation_id: Option<OperationId>,
    pub recorded_at: String,
}

impl StoredAuditEvent {
    pub fn from_event(id: AuditEventId, recorded_at: impl Into<String>, event: &AuditEvent) -> Self {
        Self {
            id,
            tool: event.tool.clone(),
            namespace: event.namespace.clone(),
            auth_ref: event.auth_ref.clone(),
            summary: event.summary.clone(),
            backend: event.backend,
            approval_required: event.approval_required,
            outcome: event.outcome.clone(),
            operation_id: event.operation_id.clone(),
            compensates_operation_id: event.compensates_operation_id.clone(),
            recorded_at: recorded_at.into(),
        }
    }
}
