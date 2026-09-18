use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{
    error::{Error, Result},
    types::{
        AuthRef, BackendKind, NamespaceId, OperationId, PlannedAction, ToolArguments, ToolKind, ToolName, ToolRef,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    NotRequired,
    Pending,
    Approved,
    Rejected,
}

impl ApprovalState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

impl FromStr for ApprovalState {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(value))
            .map_err(|_| Error::InvalidArguments(format!("unknown approval state: {value}")))
    }
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
    Executing,
    Uncertain,
    Applied,
    Verified,
    Failed,
    Compensated,
}

impl OperationStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Executing => "executing",
            Self::Uncertain => "uncertain",
            Self::Applied => "applied",
            Self::Verified => "verified",
            Self::Failed => "failed",
            Self::Compensated => "compensated",
        }
    }
}

impl FromStr for OperationStatus {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(value))
            .map_err(|_| Error::InvalidArguments(format!("unknown operation status: {value}")))
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<crate::VerificationReceipt>,
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
            verification: None,
        }
    }

    pub fn mark_applied(&mut self, output: &crate::ToolOutput) -> Result<()> {
        self.require_executing()?;
        let mut effect = output.effect.clone().unwrap_or_else(|| OperationEffect::new(false));
        for reference in &output.refs {
            if !effect.refs.contains(reference) {
                effect.refs.push(reference.clone());
            }
        }
        self.status = OperationStatus::Applied;
        self.effect = Some(effect);
        self.failure_reason = None;
        Ok(())
    }

    pub fn claim_execution(&mut self) -> Result<()> {
        self.can_apply()?;
        self.status = OperationStatus::Executing;
        self.failure_reason = None;
        Ok(())
    }

    pub fn require_executing(&self) -> Result<()> {
        if self.status != OperationStatus::Executing {
            return Err(Error::Operation(format!(
                "operation {} is not claimed for execution",
                self.id
            )));
        }
        Ok(())
    }

    pub fn can_verify(&self) -> Result<()> {
        if !matches!(
            self.status,
            OperationStatus::Executing
                | OperationStatus::Uncertain
                | OperationStatus::Applied
                | OperationStatus::Verified
        ) {
            return Err(Error::Operation(format!("operation {} has not been executed", self.id)));
        }
        Ok(())
    }

    pub fn mark_uncertain(&mut self, reason: impl Into<String>) -> Result<()> {
        self.require_executing()?;
        let reason = reason.into();
        crate::types::validate_non_empty("uncertain outcome reason", &reason)?;
        self.status = OperationStatus::Uncertain;
        self.failure_reason = Some(reason);
        Ok(())
    }

    pub fn record_verification(&mut self, receipt: crate::VerificationReceipt) -> Result<()> {
        self.can_verify()?;
        if receipt.status == crate::VerificationStatus::Verified {
            self.status = OperationStatus::Verified;
            self.failure_reason = None;
            if let Some(effect) = &receipt.recovered_effect {
                self.effect = Some(effect.clone());
            }
        } else if self.status == OperationStatus::Verified {
            self.status = OperationStatus::Applied;
        }
        self.verification = Some(receipt);
        Ok(())
    }

    pub fn mark_failed(&mut self, failure_reason: impl Into<String>) -> Result<()> {
        self.require_executing()?;
        let failure_reason = failure_reason.into();
        crate::types::validate_non_empty("operation failure reason", &failure_reason)?;
        self.status = OperationStatus::Failed;
        self.failure_reason = Some(failure_reason);
        Ok(())
    }

    pub fn mark_compensated(&mut self) -> Result<()> {
        self.can_undo()?;
        self.status = OperationStatus::Compensated;
        Ok(())
    }

    pub fn approve(&mut self, actor: impl Into<String>, note: Option<String>) -> Result<()> {
        if !self.approval_required {
            return Err(Error::Operation(format!(
                "operation {} does not require approval",
                self.id
            )));
        }
        if !matches!(self.status, OperationStatus::Planned | OperationStatus::Failed) {
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
        if !matches!(self.status, OperationStatus::Planned | OperationStatus::Failed) {
            return Err(Error::Operation(format!(
                "operation {} can no longer be rejected",
                self.id
            )));
        }

        self.approval.reject(actor, note)
    }

    pub fn can_apply(&self) -> Result<()> {
        match self.status {
            OperationStatus::Applied | OperationStatus::Verified => {
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
            OperationStatus::Executing | OperationStatus::Uncertain => {
                return Err(Error::OutcomeUnknown {
                    operation_id: self.id.clone(),
                    reason: "execution has started; verify the remote state".into(),
                });
            }
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
        if !matches!(self.status, OperationStatus::Applied | OperationStatus::Verified) {
            return Err(Error::OperationNotUndoable(self.id.clone()));
        }

        match self.effect.as_ref() {
            Some(effect) if effect.undoable => Ok(()),
            _ => Err(Error::OperationNotUndoable(self.id.clone())),
        }
    }
}
