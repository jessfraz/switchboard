use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::operation::{
    AggregateReadOutcome, AggregateReadRequest, AggregateReadResult, DispatchOutcome, OperationOutcome,
    OperationRequest,
};
use crate::traits::{Adapter, AuditSink, NamespaceStore, PolicyEngine};
use crate::types::{AuditEvent, AuditOutcome, PlannedAction, ProviderKind, ResolvedNamespace, ToolKind};

#[derive(Default)]
pub struct AdapterRegistry {
    adapters: HashMap<ProviderKind, Arc<dyn Adapter>>,
}

impl AdapterRegistry {
    pub fn register(&mut self, adapter: Arc<dyn Adapter>) {
        self.adapters.insert(adapter.provider(), adapter);
    }

    pub fn get(&self, provider: &ProviderKind) -> Option<Arc<dyn Adapter>> {
        self.adapters.get(provider).cloned()
    }
}

pub struct Switchboard {
    namespaces: Arc<dyn NamespaceStore>,
    policy: Arc<dyn PolicyEngine>,
    audit: Arc<dyn AuditSink>,
    adapters: AdapterRegistry,
}

impl Switchboard {
    pub fn new(
        namespaces: Arc<dyn NamespaceStore>,
        policy: Arc<dyn PolicyEngine>,
        audit: Arc<dyn AuditSink>,
        adapters: AdapterRegistry,
    ) -> Self {
        Self {
            namespaces,
            policy,
            audit,
            adapters,
        }
    }

    pub fn list_namespaces(&self) -> Vec<ResolvedNamespace> {
        self.namespaces.list()
    }

    pub fn execute_operation(&self, request: OperationRequest) -> Result<OperationOutcome> {
        match request {
            OperationRequest::Single(request) => self.dispatch(request).map(OperationOutcome::Single),
            OperationRequest::AggregateRead(request) => self
                .dispatch_aggregate_read(request)
                .map(OperationOutcome::AggregateRead),
        }
    }

    pub fn dispatch(&self, request: crate::ToolRequest) -> Result<DispatchOutcome> {
        let namespace = self
            .namespaces
            .get(&request.namespace)
            .ok_or_else(|| Error::UnknownNamespace(request.namespace.to_string()))?;
        let requested_provider = request.tool.provider()?;

        if namespace.provider != requested_provider {
            return Err(Error::ProviderMismatch {
                namespace: namespace.id.to_string(),
                namespace_provider: namespace.provider,
                requested_provider,
            });
        }

        let adapter = self
            .adapters
            .get(&namespace.provider)
            .ok_or_else(|| Error::MissingAdapter(namespace.provider.clone()))?;
        let descriptor = adapter
            .find_tool(&request.tool)
            .ok_or_else(|| Error::UnsupportedTool(request.tool.to_string()))?;
        let mut plan = adapter.plan(&namespace, &request, descriptor)?;

        match self.policy.evaluate(&namespace, &plan) {
            crate::PolicyDecision::Allow => {}
            crate::PolicyDecision::RequireApproval { reason } => {
                plan.approval_required = true;
                plan.approval_reason = Some(reason);
            }
            crate::PolicyDecision::Deny { reason } => {
                let _ = self.audit.record(&AuditEvent::from_plan(&plan, AuditOutcome::Blocked));
                return Err(Error::PolicyDenied(reason));
            }
        }

        match descriptor.kind {
            ToolKind::Read => self.finish_read(adapter.as_ref(), plan),
            ToolKind::Write => self.finish_write(adapter.as_ref(), plan),
        }
    }

    fn dispatch_aggregate_read(&self, request: AggregateReadRequest) -> Result<AggregateReadOutcome> {
        let provider = request.tool.provider()?;
        let adapter = self
            .adapters
            .get(&provider)
            .ok_or_else(|| Error::MissingAdapter(provider.clone()))?;
        let descriptor = adapter
            .find_tool(&request.tool)
            .ok_or_else(|| Error::UnsupportedTool(request.tool.to_string()))?;

        if descriptor.kind != ToolKind::Read {
            return Err(Error::UnsupportedOperation(format!(
                "aggregate reads require a read tool, got {}",
                request.tool
            )));
        }

        let tool = request.tool.clone();
        let namespaces = request.namespaces.clone();
        let mut results = Vec::with_capacity(namespaces.len());

        for tool_request in request.into_tool_requests() {
            let namespace = tool_request.namespace.clone();
            let outcome = self.dispatch(tool_request)?;
            results.push(AggregateReadResult { namespace, outcome });
        }

        Ok(AggregateReadOutcome {
            tool,
            namespaces,
            results,
        })
    }

    fn finish_read(&self, adapter: &dyn Adapter, plan: PlannedAction) -> Result<DispatchOutcome> {
        match plan.mode {
            crate::ExecutionMode::Plan | crate::ExecutionMode::Draft => {
                self.audit
                    .record(&AuditEvent::from_plan(&plan, AuditOutcome::Planned))?;
                Ok(DispatchOutcome::Planned(plan))
            }
            crate::ExecutionMode::Auto | crate::ExecutionMode::Apply => {
                let output = adapter.execute(&plan)?;
                self.audit
                    .record(&AuditEvent::from_plan(&plan, AuditOutcome::Executed))?;
                Ok(DispatchOutcome::Executed(output))
            }
        }
    }

    fn finish_write(&self, adapter: &dyn Adapter, plan: PlannedAction) -> Result<DispatchOutcome> {
        let should_apply = matches!(plan.mode, crate::ExecutionMode::Apply) && !plan.approval_required;

        if should_apply {
            let output = adapter.execute(&plan)?;
            self.audit
                .record(&AuditEvent::from_plan(&plan, AuditOutcome::Executed))?;
            return Ok(DispatchOutcome::Executed(output));
        }

        self.audit
            .record(&AuditEvent::from_plan(&plan, AuditOutcome::Planned))?;
        Ok(DispatchOutcome::Planned(plan))
    }
}
