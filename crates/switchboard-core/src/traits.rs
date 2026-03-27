use crate::{
    error::Result,
    types::{
        AuditEvent, AuthRef, ExecutionTarget, PlannedAction, PlanningTarget, PolicyDecision, ProviderKind,
        ResolvedAuth, ResolvedNamespace, ResolvedSecret, SecretRef, SecretString, ToolDescriptor, ToolName, ToolOutput,
        ToolRequest,
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

pub trait SecretStore: Send + Sync {
    fn get(&self, id: &SecretRef) -> Option<ResolvedSecret>;
    fn list(&self) -> Vec<ResolvedSecret>;
}

pub trait SecretResolver: Send + Sync {
    fn resolve(&self, secret: &ResolvedSecret) -> Result<SecretString>;
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
        target: &PlanningTarget,
        request: &ToolRequest,
        descriptor: &'static ToolDescriptor,
    ) -> Result<PlannedAction>;
    fn execute(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput>;

    fn find_tool(&self, name: &ToolName) -> Option<&'static ToolDescriptor> {
        self.tools().iter().find(|descriptor| descriptor.name == name.as_str())
    }
}
