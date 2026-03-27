use std::{collections::HashMap, sync::Arc};

use crate::{
    error::{Error, Result},
    operation::{
        AggregateReadOutcome, AggregateReadRequest, AggregateReadResult, DispatchOutcome, OperationOutcome,
        OperationRequest,
    },
    traits::{
        Adapter, AuditStore, AuthStore, NamespaceStore, OperationStore, PolicyEngine, SecretResolver, SecretStore,
    },
    types::{
        AuditEvent, AuditEventId, AuditOutcome, AuthSecretRefs, ExecutionMode, ExecutionTarget, OperationId,
        PlannedAction, PlanningTarget, ProviderKind, ResolvedCredentials, ResolvedNamespace, StoredAuditEvent,
        StoredOperation, ToolKind, ToolOutput, ToolRequest,
    },
};

#[derive(Default)]
/// In-memory registry of provider adapters keyed by provider kind.
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

    pub fn list_tools(&self) -> Result<Vec<crate::RegisteredTool>> {
        let mut tools = self
            .adapters
            .values()
            .flat_map(|adapter| adapter.tools().iter())
            .map(crate::RegisteredTool::from_descriptor)
            .collect::<Result<Vec<_>>>()?;
        tools.sort_by(|left, right| left.name.as_str().cmp(right.name.as_str()));
        Ok(tools)
    }

    pub fn describe_tool(&self, name: &crate::ToolName) -> Result<Option<crate::RegisteredTool>> {
        let provider = name.provider()?;
        self.get(&provider)
            .and_then(|adapter| adapter.find_tool(name).cloned())
            .map(|descriptor| crate::RegisteredTool::from_descriptor(&descriptor))
            .transpose()
    }
}

/// Top-level orchestrator for namespaces, policy, audit, and provider dispatch.
pub struct Switchboard {
    services: SwitchboardServices,
    adapters: AdapterRegistry,
}

/// Dependency bundle required to construct one Switchboard instance.
pub struct SwitchboardServices {
    pub namespaces: Arc<dyn NamespaceStore>,
    pub auth: Arc<dyn AuthStore>,
    pub secrets: Arc<dyn SecretStore>,
    pub secret_resolver: Arc<dyn SecretResolver>,
    pub policy: Arc<dyn PolicyEngine>,
    pub audit: Arc<dyn AuditStore>,
    pub operations: Arc<dyn OperationStore>,
}

impl Switchboard {
    pub fn new(services: SwitchboardServices, adapters: AdapterRegistry) -> Self {
        Self { services, adapters }
    }

    pub fn list_namespaces(&self) -> Vec<ResolvedNamespace> {
        self.services.namespaces.list()
    }

    pub fn list_audit_events(&self) -> Vec<StoredAuditEvent> {
        self.services.audit.list()
    }

    pub fn get_audit_event(&self, id: &AuditEventId) -> Option<StoredAuditEvent> {
        self.services.audit.get(id)
    }

    pub fn list_audit_events_for_operation(&self, id: &OperationId) -> Vec<StoredAuditEvent> {
        self.services
            .audit
            .list()
            .into_iter()
            .filter(|event| event.operation_id.as_ref() == Some(id))
            .collect()
    }

    pub fn list_tools(&self) -> Result<Vec<crate::RegisteredTool>> {
        self.adapters.list_tools()
    }

    pub fn describe_tool(&self, name: &crate::ToolName) -> Result<Option<crate::RegisteredTool>> {
        self.adapters.describe_tool(name)
    }

    pub fn list_operations(&self) -> Vec<StoredOperation> {
        self.services.operations.list()
    }

    pub fn get_operation(&self, id: &OperationId) -> Option<StoredOperation> {
        self.services.operations.get(id)
    }

    pub fn approve_operation(&self, id: &OperationId, actor: &str, note: Option<&str>) -> Result<StoredOperation> {
        let operation = self.services.operations.mark_approved(id, actor, note)?;
        self.services
            .audit
            .record(&AuditEvent::from_operation(&operation, AuditOutcome::Approved))?;
        Ok(operation)
    }

    pub fn reject_operation(&self, id: &OperationId, actor: &str, note: Option<&str>) -> Result<StoredOperation> {
        let operation = self.services.operations.mark_rejected(id, actor, note)?;
        self.services
            .audit
            .record(&AuditEvent::from_operation(&operation, AuditOutcome::Rejected))?;
        Ok(operation)
    }

    pub fn apply_operation(&self, id: &OperationId) -> Result<ToolOutput> {
        let operation = self
            .services
            .operations
            .get(id)
            .ok_or_else(|| Error::Operation(format!("unknown operation id: {id}")))?;
        operation.can_apply()?;

        self.execute_stored_operation(operation)
    }

    pub fn undo_operation(&self, id: &OperationId, mode: ExecutionMode) -> Result<DispatchOutcome> {
        let operation = self
            .services
            .operations
            .get(id)
            .ok_or_else(|| Error::Operation(format!("unknown operation id: {id}")))?;
        operation.can_undo()?;
        let provider = operation.tool.provider()?;
        let adapter = self
            .adapters
            .get(&provider)
            .ok_or_else(|| Error::MissingAdapter(provider.clone()))?;
        let request = adapter
            .compensation_request(&operation, mode)?
            .ok_or_else(|| Error::UndoUnsupported(operation.tool.clone()))?;

        self.dispatch_with_compensation(request, Some(operation.id))
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
        self.dispatch_with_compensation(request, None)
    }

    fn dispatch_with_compensation(
        &self,
        request: ToolRequest,
        compensates_operation_id: Option<OperationId>,
    ) -> Result<DispatchOutcome> {
        let namespace = self
            .services
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
            .services
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
        if let Some(compensates_operation_id) = compensates_operation_id {
            plan = plan.with_compensates_operation_id(compensates_operation_id);
        }

        match self.services.policy.evaluate(&namespace, &plan) {
            crate::PolicyDecision::Allow => {}
            crate::PolicyDecision::RequireApproval { reason } => {
                plan.approval_required = true;
                plan.approval_reason = Some(reason);
            }
            crate::PolicyDecision::Deny { reason } => {
                let _ = self
                    .services
                    .audit
                    .record(&AuditEvent::from_plan(&plan, AuditOutcome::Blocked));
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
            return Err(Error::AggregateReadRequiresReadTool(request.tool.clone()));
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
                self.services
                    .audit
                    .record(&AuditEvent::from_plan(&plan, AuditOutcome::Planned))?;
                Ok(DispatchOutcome::Planned(plan))
            }
            crate::ExecutionMode::Auto | crate::ExecutionMode::Apply => {
                let target = self.resolve_execution_target(target)?;
                let output = adapter.execute(&target, &plan)?;
                self.services
                    .audit
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
        let operation = self.services.operations.create(&plan)?;
        let plan = plan.with_operation_id(operation.id.clone());
        let should_apply = matches!(plan.mode, crate::ExecutionMode::Apply) && !plan.approval_required;

        if should_apply {
            let target = self.resolve_execution_target(target)?;
            let operation_id = operation.id.clone();
            let output = match adapter.execute(&target, &plan) {
                Ok(output) => output.with_operation_id(operation_id.clone()),
                Err(error) => {
                    self.services
                        .operations
                        .mark_failed(&operation_id, &error.to_string())?;
                    self.services
                        .audit
                        .record(&AuditEvent::from_plan(&plan, AuditOutcome::Failed))?;
                    return Err(error);
                }
            };
            let applied = self.services.operations.mark_applied(&operation_id, &output)?;
            self.finalize_compensation(&applied)?;
            self.services
                .audit
                .record(&AuditEvent::from_plan(&plan, AuditOutcome::Executed))?;
            return Ok(DispatchOutcome::Executed(output));
        }

        self.services
            .audit
            .record(&AuditEvent::from_plan(&plan, AuditOutcome::Planned))?;
        Ok(DispatchOutcome::Planned(plan))
    }

    fn execute_stored_operation(&self, operation: StoredOperation) -> Result<ToolOutput> {
        let namespace = self
            .services
            .namespaces
            .get(&operation.namespace)
            .ok_or_else(|| Error::UnknownNamespace(operation.namespace.to_string()))?;
        let requested_provider = operation.tool.provider()?;
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
            .services
            .auth
            .get(&operation.auth_ref)
            .ok_or_else(|| Error::MissingAuth(operation.auth_ref.to_string()))?;
        if auth.provider != namespace.provider {
            return Err(Error::AuthProviderMismatch {
                auth_ref: operation.auth_ref.to_string(),
                auth_provider: auth.provider,
                namespace_provider: namespace.provider,
            });
        }

        let descriptor = adapter
            .find_tool(&operation.tool)
            .ok_or_else(|| Error::UnsupportedTool(operation.tool.to_string()))?;
        if descriptor.kind != operation.kind {
            return Err(Error::Operation(format!(
                "stored operation {} expected tool kind {:?}, but {} is registered as {:?}",
                operation.id, operation.kind, operation.tool, descriptor.kind
            )));
        }

        let planning_target = PlanningTarget { namespace, auth };
        let plan = PlannedAction {
            tool: operation.tool.clone(),
            namespace: operation.namespace.clone(),
            auth_ref: operation.auth_ref.clone(),
            kind: operation.kind,
            mode: ExecutionMode::Apply,
            summary: operation.summary.clone(),
            backend: operation.backend,
            approval_required: operation.approval_required,
            approval_reason: operation.approval_reason.clone(),
            args: operation.args.clone(),
            operation_id: Some(operation.id.clone()),
            compensates_operation_id: operation.compensates_operation_id.clone(),
        };
        let execution_target = self.resolve_execution_target(&planning_target)?;

        let output = match adapter.execute(&execution_target, &plan) {
            Ok(output) => output.with_operation_id(operation.id.clone()),
            Err(error) => {
                self.services
                    .operations
                    .mark_failed(&operation.id, &error.to_string())?;
                self.services
                    .audit
                    .record(&AuditEvent::from_plan(&plan, AuditOutcome::Failed))?;
                return Err(error);
            }
        };

        let applied = self.services.operations.mark_applied(&operation.id, &output)?;
        self.finalize_compensation(&applied)?;
        self.services
            .audit
            .record(&AuditEvent::from_plan(&plan, AuditOutcome::Executed))?;

        Ok(output)
    }

    fn finalize_compensation(&self, operation: &StoredOperation) -> Result<()> {
        let Some(original_operation_id) = operation.compensates_operation_id.as_ref() else {
            return Ok(());
        };

        let original = self.services.operations.mark_compensated(original_operation_id)?;
        self.services
            .audit
            .record(&AuditEvent::from_operation(&original, AuditOutcome::Compensated))?;

        Ok(())
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
            .services
            .secrets
            .get(secret_ref)
            .ok_or_else(|| Error::MissingSecret(secret_ref.to_string()))?;

        self.services.secret_resolver.resolve(&secret)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc, Mutex,
        },
    };

    use crate::{
        engine::{AdapterRegistry, Switchboard, SwitchboardServices},
        traits::{
            Adapter, AuditStore, AuthStore, NamespaceStore, OperationStore, PolicyEngine, SecretResolver, SecretStore,
        },
        AuditEvent, AuditEventId, AuditOutcome, AuthKind, AuthRef, AuthSecretRefs, BackendKind, DispatchOutcome, Error,
        ExecutionMode, ExecutionTarget, NamespaceId, OperationEffect, OperationId, OperationStatus, PlannedAction,
        PlanningTarget, PolicyDecision, ProviderKind, ResolvedAuth, ResolvedNamespace, ResolvedSecret, Result,
        SecretRef, SecretSource, SecretString, StoredAuditEvent, StoredOperation, ToolDescriptor, ToolKind, ToolOutput,
        ToolRef, ToolRefKind, ToolRequest,
    };

    #[test]
    fn planned_writes_get_operation_ids_and_planned_audit_events() {
        let audit = Arc::new(TestAuditSink::default());
        let operations = Arc::new(TestOperationStore::default());
        let switchboard = test_switchboard(
            Arc::new(RequireApprovalPolicy),
            audit.clone(),
            operations.clone(),
            Arc::new(TestAdapter { fail_execution: false }),
        );

        let outcome = switchboard
            .dispatch(
                ToolRequest::new(
                    "github.issue.comment",
                    "github.personal",
                    ExecutionMode::Draft,
                    vec![
                        crate::ToolArgument::option("repo", "openai/codex").expect("repo arg should build"),
                        crate::ToolArgument::option("number", "77").expect("number arg should build"),
                        crate::ToolArgument::option("body", "ship it").expect("body arg should build"),
                    ],
                )
                .expect("request should build"),
            )
            .expect("dispatch should succeed");

        let plan = match outcome {
            DispatchOutcome::Planned(plan) => plan,
            DispatchOutcome::Executed(_) => panic!("write should stay planned"),
        };

        assert!(plan.operation_id.is_some());
        let operation_id = plan.operation_id.expect("operation id should exist");
        let stored = operations
            .get(&operation_id)
            .expect("planned operation should be stored");
        assert_eq!(stored.status, OperationStatus::Planned);

        let audit_events = audit.snapshot();
        assert_eq!(audit_events.len(), 1);
        assert_eq!(audit_events[0].outcome, AuditOutcome::Planned);
        assert_eq!(audit_events[0].operation_id.as_ref(), Some(&operation_id));
    }

    #[test]
    fn failed_apply_marks_operation_failed_and_audits_failure() {
        let audit = Arc::new(TestAuditSink::default());
        let operations = Arc::new(TestOperationStore::default());
        let switchboard = test_switchboard(
            Arc::new(AllowPolicy),
            audit.clone(),
            operations.clone(),
            Arc::new(TestAdapter { fail_execution: true }),
        );

        let error = switchboard
            .dispatch(
                ToolRequest::new(
                    "github.issue.comment",
                    "github.personal",
                    ExecutionMode::Apply,
                    vec![
                        crate::ToolArgument::option("repo", "openai/codex").expect("repo arg should build"),
                        crate::ToolArgument::option("number", "77").expect("number arg should build"),
                        crate::ToolArgument::option("body", "ship it").expect("body arg should build"),
                    ],
                )
                .expect("request should build"),
            )
            .expect_err("execution should fail");

        assert_eq!(error, Error::Execution("adapter blew up".into()));

        let stored = operations.list();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].status, OperationStatus::Failed);
        assert_eq!(
            stored[0].failure_reason.as_deref(),
            Some("execution failure: adapter blew up")
        );

        let audit_events = audit.snapshot();
        assert_eq!(audit_events.len(), 1);
        assert_eq!(audit_events[0].outcome, AuditOutcome::Failed);
        assert_eq!(audit_events[0].operation_id.as_ref(), Some(&stored[0].id));
    }

    fn test_switchboard(
        policy: Arc<dyn PolicyEngine>,
        audit: Arc<dyn AuditStore>,
        operations: Arc<dyn OperationStore>,
        adapter: Arc<dyn Adapter>,
    ) -> Switchboard {
        let namespace = ResolvedNamespace::new(
            "github.personal",
            ProviderKind::GitHub,
            "GitHub personal",
            "github.personal_auth",
            false,
            None,
        )
        .expect("namespace should build");
        let auth = ResolvedAuth::new(
            "github.personal_auth",
            ProviderKind::GitHub,
            AuthKind::GitHubCli,
            "jessfraz",
            AuthSecretRefs::None,
        )
        .expect("auth should build");

        let mut adapters = AdapterRegistry::default();
        adapters.register(adapter);

        Switchboard::new(
            SwitchboardServices {
                namespaces: Arc::new(TestNamespaceStore { namespace }),
                auth: Arc::new(TestAuthStore { auth }),
                secrets: Arc::new(TestSecretStore),
                secret_resolver: Arc::new(TestSecretResolver),
                policy,
                audit,
                operations,
            },
            adapters,
        )
    }

    struct TestAdapter {
        fail_execution: bool,
    }

    impl Adapter for TestAdapter {
        fn provider(&self) -> ProviderKind {
            ProviderKind::GitHub
        }

        fn tools(&self) -> &'static [ToolDescriptor] {
            static TOOLS: std::sync::OnceLock<Vec<ToolDescriptor>> = std::sync::OnceLock::new();
            TOOLS.get_or_init(|| {
                vec![ToolDescriptor::new(
                    "github.issue.comment",
                    ToolKind::Write,
                    "Comment on a GitHub issue",
                    BackendKind::Cli,
                )
                .expect("test tool descriptor should build")]
            })
        }

        fn plan(
            &self,
            target: &PlanningTarget,
            request: &ToolRequest,
            descriptor: &'static ToolDescriptor,
        ) -> Result<PlannedAction> {
            Ok(PlannedAction::new(
                request,
                target,
                descriptor.kind,
                "Draft comment for GitHub issue",
                descriptor.backend,
            ))
        }

        fn execute(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
            if self.fail_execution {
                return Err(Error::Execution("adapter blew up".into()));
            }

            Ok(ToolOutput::new(
                action.tool.clone(),
                action.namespace.clone(),
                "Created GitHub issue comment",
            )
            .with_ref(
                ToolRef::new(ProviderKind::GitHub, action.namespace.clone(), ToolRefKind::Issue, "77")?
                    .with_parent_id("openai/codex")?,
            )
            .with_effect(
                OperationEffect::new(true)
                    .with_ref(
                        ToolRef::new(ProviderKind::GitHub, action.namespace.clone(), ToolRefKind::Issue, "77")?
                            .with_parent_id("openai/codex")?,
                    )
                    .with_undo_summary(format!("Delete comment in {}", target.namespace.id))?,
            ))
        }
    }

    struct AllowPolicy;

    impl PolicyEngine for AllowPolicy {
        fn evaluate(&self, _namespace: &ResolvedNamespace, _plan: &PlannedAction) -> PolicyDecision {
            PolicyDecision::Allow
        }
    }

    struct RequireApprovalPolicy;

    impl PolicyEngine for RequireApprovalPolicy {
        fn evaluate(&self, _namespace: &ResolvedNamespace, _plan: &PlannedAction) -> PolicyDecision {
            PolicyDecision::RequireApproval {
                reason: "writes need approval".into(),
            }
        }
    }

    struct TestNamespaceStore {
        namespace: ResolvedNamespace,
    }

    impl NamespaceStore for TestNamespaceStore {
        fn get(&self, id: &NamespaceId) -> Option<ResolvedNamespace> {
            (self.namespace.id == *id).then_some(self.namespace.clone())
        }

        fn list(&self) -> Vec<ResolvedNamespace> {
            vec![self.namespace.clone()]
        }
    }

    struct TestAuthStore {
        auth: ResolvedAuth,
    }

    impl AuthStore for TestAuthStore {
        fn get(&self, id: &AuthRef) -> Option<ResolvedAuth> {
            (self.auth.id == *id).then_some(self.auth.clone())
        }

        fn list(&self) -> Vec<ResolvedAuth> {
            vec![self.auth.clone()]
        }
    }

    #[derive(Default)]
    struct TestSecretStore;

    impl SecretStore for TestSecretStore {
        fn get(&self, _id: &SecretRef) -> Option<ResolvedSecret> {
            None
        }

        fn list(&self) -> Vec<ResolvedSecret> {
            Vec::new()
        }
    }

    struct TestSecretResolver;

    impl SecretResolver for TestSecretResolver {
        fn resolve(&self, secret: &ResolvedSecret) -> Result<SecretString> {
            match &secret.source {
                SecretSource::Env { name } => Ok(format!("resolved:{name}").into()),
                SecretSource::File { path } => Ok(path.display().to_string().into()),
                SecretSource::OnePasswordItem { item, .. } => Ok(format!("op:{item}").into()),
            }
        }
    }

    struct TestAuditSink {
        next_id: AtomicU64,
        events: Mutex<Vec<StoredAuditEvent>>,
    }

    impl Default for TestAuditSink {
        fn default() -> Self {
            Self {
                next_id: AtomicU64::new(1),
                events: Mutex::new(Vec::new()),
            }
        }
    }

    impl TestAuditSink {
        fn snapshot(&self) -> Vec<StoredAuditEvent> {
            match self.events.lock() {
                Ok(events) => events.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }

        fn next_audit_event_id(&self) -> Result<AuditEventId> {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
            AuditEventId::new(format!("audit_test_{id:04}"))
        }
    }

    impl AuditStore for TestAuditSink {
        fn record(&self, event: &AuditEvent) -> Result<()> {
            let stored = StoredAuditEvent::from_event(self.next_audit_event_id()?, "test", event);
            match self.events.lock() {
                Ok(mut events) => events.push(stored),
                Err(poisoned) => poisoned.into_inner().push(stored),
            }

            Ok(())
        }

        fn get(&self, id: &AuditEventId) -> Option<StoredAuditEvent> {
            match self.events.lock() {
                Ok(events) => events.iter().find(|event| event.id == *id).cloned(),
                Err(poisoned) => poisoned.into_inner().iter().find(|event| event.id == *id).cloned(),
            }
        }

        fn list(&self) -> Vec<StoredAuditEvent> {
            self.snapshot()
        }
    }

    #[derive(Default)]
    struct TestOperationStore {
        next_id: AtomicU64,
        operations: Mutex<BTreeMap<OperationId, StoredOperation>>,
    }

    impl TestOperationStore {
        fn next_operation_id(&self) -> Result<OperationId> {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
            OperationId::new(format!("op_test_{id:04}"))
        }
    }

    impl OperationStore for TestOperationStore {
        fn create(&self, plan: &PlannedAction) -> Result<StoredOperation> {
            let operation = StoredOperation::from_plan(self.next_operation_id()?, plan);

            match self.operations.lock() {
                Ok(mut operations) => {
                    operations.insert(operation.id.clone(), operation.clone());
                }
                Err(poisoned) => {
                    poisoned.into_inner().insert(operation.id.clone(), operation.clone());
                }
            }

            Ok(operation)
        }

        fn mark_applied(&self, id: &OperationId, output: &ToolOutput) -> Result<StoredOperation> {
            self.with_operation_mut(id, |operation| {
                operation.mark_applied(output.effect.clone());
                Ok(operation.clone())
            })
        }

        fn mark_approved(&self, id: &OperationId, actor: &str, note: Option<&str>) -> Result<StoredOperation> {
            self.with_operation_mut(id, |operation| {
                operation.approve(actor, note.map(str::to_owned))?;
                Ok(operation.clone())
            })
        }

        fn mark_rejected(&self, id: &OperationId, actor: &str, note: Option<&str>) -> Result<StoredOperation> {
            self.with_operation_mut(id, |operation| {
                operation.reject(actor, note.map(str::to_owned))?;
                Ok(operation.clone())
            })
        }

        fn mark_failed(&self, id: &OperationId, reason: &str) -> Result<StoredOperation> {
            self.with_operation_mut(id, |operation| {
                operation.mark_failed(reason)?;
                Ok(operation.clone())
            })
        }

        fn mark_compensated(&self, id: &OperationId) -> Result<StoredOperation> {
            self.with_operation_mut(id, |operation| {
                operation.mark_compensated();
                Ok(operation.clone())
            })
        }

        fn get(&self, id: &OperationId) -> Option<StoredOperation> {
            match self.operations.lock() {
                Ok(operations) => operations.get(id).cloned(),
                Err(poisoned) => poisoned.into_inner().get(id).cloned(),
            }
        }

        fn list(&self) -> Vec<StoredOperation> {
            match self.operations.lock() {
                Ok(operations) => operations.values().cloned().collect(),
                Err(poisoned) => poisoned.into_inner().values().cloned().collect(),
            }
        }
    }

    impl TestOperationStore {
        fn with_operation_mut<T, F>(&self, id: &OperationId, mut update: F) -> Result<T>
        where
            F: FnMut(&mut StoredOperation) -> Result<T>,
        {
            match self.operations.lock() {
                Ok(mut operations) => {
                    let operation = operations
                        .get_mut(id)
                        .ok_or_else(|| Error::Operation(format!("unknown operation id: {id}")))?;
                    update(operation)
                }
                Err(poisoned) => {
                    let mut operations = poisoned.into_inner();
                    let operation = operations
                        .get_mut(id)
                        .ok_or_else(|| Error::Operation(format!("unknown operation id: {id}")))?;
                    update(operation)
                }
            }
        }
    }
}
