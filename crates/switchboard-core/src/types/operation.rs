use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::types::{
    AuthRef, BackendKind, NamespaceId, OperationId, PlannedAction, ToolArguments, ToolKind, ToolName, ToolRef,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    NotRequired,
    Pending,
    Approved,
    Rejected,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationApproval {
    pub state: ApprovalState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl OperationApproval {
    pub fn not_required() -> Self {
        Self {
            state: ApprovalState::NotRequired,
            actor: None,
            note: None,
        }
    }

    pub fn pending() -> Self {
        Self {
            state: ApprovalState::Pending,
            actor: None,
            note: None,
        }
    }

    pub fn approve(&mut self, actor: impl Into<String>, note: Option<String>) -> Result<()> {
        let actor = actor.into();
        crate::types::validate_non_empty("approval actor", &actor)?;
        if note.as_ref().is_some_and(|note| note.trim().is_empty()) {
            return Err(Error::InvalidArguments("approval note cannot be empty".into()));
        }

        self.state = ApprovalState::Approved;
        self.actor = Some(actor);
        self.note = note;
        Ok(())
    }

    pub fn reject(&mut self, actor: impl Into<String>, note: Option<String>) -> Result<()> {
        let actor = actor.into();
        crate::types::validate_non_empty("approval actor", &actor)?;
        if note.as_ref().is_some_and(|note| note.trim().is_empty()) {
            return Err(Error::InvalidArguments("approval note cannot be empty".into()));
        }

        self.state = ApprovalState::Rejected;
        self.actor = Some(actor);
        self.note = note;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationEffect {
    pub refs: Vec<ToolRef>,
    pub undoable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo_summary: Option<String>,
}

impl OperationEffect {
    pub fn new(undoable: bool) -> Self {
        Self {
            refs: Vec::new(),
            undoable,
            undo_summary: None,
        }
    }

    pub fn with_ref(mut self, tool_ref: ToolRef) -> Self {
        self.refs.push(tool_ref);
        self
    }

    pub fn with_refs(mut self, tool_refs: impl IntoIterator<Item = ToolRef>) -> Self {
        self.refs.extend(tool_refs);
        self
    }

    pub fn with_undo_summary(mut self, undo_summary: impl Into<String>) -> Result<Self> {
        let undo_summary = undo_summary.into();
        crate::types::validate_non_empty("undo summary", &undo_summary)?;
        self.undo_summary = Some(undo_summary);
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Planned,
    Applied,
    Failed,
    Compensated,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StoredOperation {
    pub id: OperationId,
    pub tool: ToolName,
    pub namespace: NamespaceId,
    pub auth_ref: AuthRef,
    pub kind: ToolKind,
    pub summary: String,
    pub backend: BackendKind,
    pub approval_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compensates_operation_id: Option<OperationId>,
    pub approval: OperationApproval,
    pub status: OperationStatus,
    pub args: ToolArguments,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<OperationEffect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

impl StoredOperation {
    pub fn from_plan(id: OperationId, plan: &PlannedAction) -> Self {
        Self {
            id,
            tool: plan.tool.clone(),
            namespace: plan.namespace.clone(),
            auth_ref: plan.auth_ref.clone(),
            kind: plan.kind,
            summary: plan.summary.clone(),
            backend: plan.backend,
            approval_required: plan.approval_required,
            approval_reason: plan.approval_reason.clone(),
            compensates_operation_id: plan.compensates_operation_id.clone(),
            approval: if plan.approval_required {
                OperationApproval::pending()
            } else {
                OperationApproval::not_required()
            },
            status: OperationStatus::Planned,
            args: plan.args.clone(),
            effect: None,
            failure_reason: None,
        }
    }

    pub fn mark_applied(&mut self, effect: Option<OperationEffect>) {
        self.status = OperationStatus::Applied;
        self.effect = effect;
        self.failure_reason = None;
    }

    pub fn mark_failed(&mut self, failure_reason: impl Into<String>) -> Result<()> {
        let failure_reason = failure_reason.into();
        crate::types::validate_non_empty("operation failure reason", &failure_reason)?;
        self.status = OperationStatus::Failed;
        self.failure_reason = Some(failure_reason);
        Ok(())
    }

    pub fn mark_compensated(&mut self) {
        self.status = OperationStatus::Compensated;
    }

    pub fn approve(&mut self, actor: impl Into<String>, note: Option<String>) -> Result<()> {
        if !self.approval_required {
            return Err(Error::Operation(format!(
                "operation {} does not require approval",
                self.id
            )));
        }
        if self.status == OperationStatus::Applied || self.status == OperationStatus::Compensated {
            return Err(Error::Operation(format!(
                "operation {} can no longer be approved",
                self.id
            )));
        }

        self.approval.approve(actor, note)
    }

    pub fn reject(&mut self, actor: impl Into<String>, note: Option<String>) -> Result<()> {
        if !self.approval_required {
            return Err(Error::Operation(format!(
                "operation {} does not require approval",
                self.id
            )));
        }
        if self.status == OperationStatus::Applied || self.status == OperationStatus::Compensated {
            return Err(Error::Operation(format!(
                "operation {} can no longer be rejected",
                self.id
            )));
        }

        self.approval.reject(actor, note)
    }

    pub fn can_apply(&self) -> Result<()> {
        match self.status {
            OperationStatus::Applied => {
                return Err(Error::Operation(format!(
                    "operation {} has already been applied",
                    self.id
                )));
            }
            OperationStatus::Compensated => {
                return Err(Error::Operation(format!(
                    "operation {} has already been compensated",
                    self.id
                )));
            }
            OperationStatus::Planned | OperationStatus::Failed => {}
        }

        match self.approval.state {
            ApprovalState::NotRequired | ApprovalState::Approved => Ok(()),
            ApprovalState::Pending => Err(Error::Operation(format!(
                "operation {} is still pending approval",
                self.id
            ))),
            ApprovalState::Rejected => Err(Error::Operation(format!(
                "operation {} was rejected and cannot be applied",
                self.id
            ))),
        }
    }

    pub fn can_undo(&self) -> Result<()> {
        if self.status != OperationStatus::Applied {
            return Err(Error::OperationNotUndoable(self.id.clone()));
        }

        match self.effect.as_ref() {
            Some(effect) if effect.undoable => Ok(()),
            _ => Err(Error::OperationNotUndoable(self.id.clone())),
        }
    }
}
