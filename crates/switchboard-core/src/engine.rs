use std::{collections::HashMap, sync::Arc};

use crate::{
    error::{Error, Result},
    operation::{
        AggregateReadOutcome, AggregateReadRequest, AggregateReadResult, DispatchOutcome, OperationOutcome,
        OperationRequest,
    },
    traits::{Adapter, AuditSink, AuthStore, NamespaceStore, PolicyEngine, SecretResolver, SecretStore},
    types::{
        AuditEvent, AuditOutcome, AuthSecretRefs, ExecutionTarget, PlannedAction, PlanningTarget, ProviderKind,
        ResolvedCredentials, ResolvedNamespace, ToolKind,
    },
};

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
    auth: Arc<dyn AuthStore>,
    secrets: Arc<dyn SecretStore>,
    secret_resolver: Arc<dyn SecretResolver>,
    policy: Arc<dyn PolicyEngine>,
    audit: Arc<dyn AuditSink>,
    adapters: AdapterRegistry,
}

impl Switchboard {
    pub fn new(
        namespaces: Arc<dyn NamespaceStore>,
        auth: Arc<dyn AuthStore>,
        secrets: Arc<dyn SecretStore>,
        secret_resolver: Arc<dyn SecretResolver>,
        policy: Arc<dyn PolicyEngine>,
        audit: Arc<dyn AuditSink>,
        adapters: AdapterRegistry,
    ) -> Self {
        Self {
            namespaces,
            auth,
            secrets,
            secret_resolver,
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
        let auth = self
            .auth
            .get(&namespace.auth_ref)
            .ok_or_else(|| Error::MissingAuth(namespace.auth_ref.to_string()))?;
        if auth.provider != namespace.provider {
            return Err(Error::AuthProviderMismatch {
                auth_ref: namespace.auth_ref.to_string(),
                auth_provider: auth.provider,
                namespace_provider: namespace.provider,
            });
        }
        let target = PlanningTarget {
            namespace: namespace.clone(),
            auth,
        };
        let descriptor = adapter
            .find_tool(&request.tool)
            .ok_or_else(|| Error::UnsupportedTool(request.tool.to_string()))?;
        let mut plan = adapter.plan(&target, &request, descriptor)?;

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
            ToolKind::Read => self.finish_read(adapter.as_ref(), &target, plan),
            ToolKind::Write => self.finish_write(adapter.as_ref(), &target, plan),
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

    fn finish_read(
        &self,
        adapter: &dyn Adapter,
        target: &PlanningTarget,
        plan: PlannedAction,
    ) -> Result<DispatchOutcome> {
        match plan.mode {
            crate::ExecutionMode::Plan | crate::ExecutionMode::Draft => {
                self.audit
                    .record(&AuditEvent::from_plan(&plan, AuditOutcome::Planned))?;
                Ok(DispatchOutcome::Planned(plan))
            }
            crate::ExecutionMode::Auto | crate::ExecutionMode::Apply => {
                let target = self.resolve_execution_target(target)?;
                let output = adapter.execute(&target, &plan)?;
                self.audit
                    .record(&AuditEvent::from_plan(&plan, AuditOutcome::Executed))?;
                Ok(DispatchOutcome::Executed(output))
            }
        }
    }

    fn finish_write(
        &self,
        adapter: &dyn Adapter,
        target: &PlanningTarget,
        plan: PlannedAction,
    ) -> Result<DispatchOutcome> {
        let should_apply = matches!(plan.mode, crate::ExecutionMode::Apply) && !plan.approval_required;

        if should_apply {
            let target = self.resolve_execution_target(target)?;
            let output = adapter.execute(&target, &plan)?;
            self.audit
                .record(&AuditEvent::from_plan(&plan, AuditOutcome::Executed))?;
            return Ok(DispatchOutcome::Executed(output));
        }

        self.audit
            .record(&AuditEvent::from_plan(&plan, AuditOutcome::Planned))?;
        Ok(DispatchOutcome::Planned(plan))
    }

    fn resolve_execution_target(&self, target: &PlanningTarget) -> Result<ExecutionTarget> {
        let credentials = match &target.auth.secrets {
            AuthSecretRefs::None => ResolvedCredentials::GitHubCli,
            AuthSecretRefs::GitHubToken { token } => ResolvedCredentials::GitHubToken {
                token: self.resolve_secret(token)?,
            },
            AuthSecretRefs::GoogleOAuth {
                client_id,
                client_secret,
                refresh_token,
            } => ResolvedCredentials::GoogleOAuth {
                client_id: self.resolve_secret(client_id)?,
                client_secret: self.resolve_secret(client_secret)?,
                refresh_token: match refresh_token {
                    Some(refresh_token) => Some(self.resolve_secret(refresh_token)?),
                    None => None,
                },
            },
            AuthSecretRefs::GoogleOAuthFile { credentials } => ResolvedCredentials::GoogleOAuthFile {
                credentials: self.resolve_secret(credentials)?,
            },
        };

        Ok(ExecutionTarget {
            namespace: target.namespace.clone(),
            auth: target.auth.clone(),
            credentials,
        })
    }

    fn resolve_secret(&self, secret_ref: &crate::SecretRef) -> Result<crate::SecretString> {
        let secret = self
            .secrets
            .get(secret_ref)
            .ok_or_else(|| Error::MissingSecret(secret_ref.to_string()))?;

        self.secret_resolver.resolve(&secret)
    }
}
