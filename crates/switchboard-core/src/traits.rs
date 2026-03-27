use crate::{
    error::Result,
    types::{
        AuditEvent, AuthRef, ExecutionTarget, PlannedAction, PolicyDecision, ProviderKind, ResolvedAuth,
        ResolvedNamespace, ToolDescriptor, ToolName, ToolOutput, ToolRequest,
    },
    NamespaceId,
};

pub trait NamespaceStore: Send + Sync {
    fn get(&self, id: &NamespaceId) -> Option<ResolvedNamespace>;
    fn list(&self) -> Vec<ResolvedNamespace>;
}

pub trait AuthStore: Send + Sync {
    fn get(&self, id: &AuthRef) -> Option<ResolvedAuth>;
    fn list(&self) -> Vec<ResolvedAuth>;
}

pub trait PolicyEngine: Send + Sync {
    fn evaluate(&self, namespace: &ResolvedNamespace, plan: &PlannedAction) -> PolicyDecision;
}

pub trait AuditSink: Send + Sync {
    fn record(&self, event: &AuditEvent) -> Result<()>;
}

pub trait Adapter: Send + Sync {
    fn provider(&self) -> ProviderKind;
    fn tools(&self) -> &'static [ToolDescriptor];
    fn plan(
        &self,
        target: &ExecutionTarget,
        request: &ToolRequest,
        descriptor: &'static ToolDescriptor,
    ) -> Result<PlannedAction>;
    fn execute(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput>;

    fn find_tool(&self, name: &ToolName) -> Option<&'static ToolDescriptor> {
        self.tools().iter().find(|descriptor| descriptor.name == name.as_str())
    }
}
