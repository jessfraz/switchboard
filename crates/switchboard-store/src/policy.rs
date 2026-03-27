use switchboard_core::{PlannedAction, PolicyDecision, PolicyEngine, ResolvedNamespace, ToolKind, WritePolicy};

#[derive(Clone, Copy, Debug)]
pub struct ConfiguredPolicyEngine {
    write_policy: WritePolicy,
}

impl ConfiguredPolicyEngine {
    pub fn new(write_policy: WritePolicy) -> Self {
        Self { write_policy }
    }

    pub fn write_policy(&self) -> WritePolicy {
        self.write_policy
    }
}

impl Default for ConfiguredPolicyEngine {
    fn default() -> Self {
        Self::new(WritePolicy::RequireApproval)
    }
}

impl PolicyEngine for ConfiguredPolicyEngine {
    fn evaluate(&self, _namespace: &ResolvedNamespace, plan: &PlannedAction) -> PolicyDecision {
        match plan.kind {
            ToolKind::Read => PolicyDecision::Allow,
            ToolKind::Write => match self.write_policy {
                WritePolicy::Allow => PolicyDecision::Allow,
                WritePolicy::RequireApproval => PolicyDecision::RequireApproval {
                    reason: format!("{} stays draft-first until approval UX is wired", plan.tool),
                },
                WritePolicy::Deny => PolicyDecision::Deny {
                    reason: format!("writes are denied by policy for {}", plan.tool),
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use switchboard_core::{
        AuthKind, AuthSecretRefs, BackendKind, ExecutionMode, PlannedAction, PlanningTarget, PolicyDecision,
        PolicyEngine, ProviderKind, ResolvedAuth, ResolvedNamespace, SecretRef, ToolKind, ToolRequest, WritePolicy,
    };

    use crate::policy::ConfiguredPolicyEngine;

    #[test]
    fn allow_policy_lets_writes_through() {
        let engine = ConfiguredPolicyEngine::new(WritePolicy::Allow);
        let decision = engine.evaluate(&planning_target().namespace, &planned_write());

        assert_eq!(decision, PolicyDecision::Allow);
    }

    #[test]
    fn deny_policy_blocks_writes() {
        let engine = ConfiguredPolicyEngine::new(WritePolicy::Deny);
        let decision = engine.evaluate(&planning_target().namespace, &planned_write());

        assert!(matches!(decision, PolicyDecision::Deny { .. }));
    }

    fn planned_write() -> PlannedAction {
        PlannedAction::new(
            &ToolRequest::new(
                "google.calendar.create",
                "google.personal",
                ExecutionMode::Draft,
                vec![],
            )
            .expect("request should build"),
            &planning_target(),
            ToolKind::Write,
            "Draft calendar event",
            BackendKind::Cli,
        )
    }

    fn planning_target() -> PlanningTarget {
        PlanningTarget {
            namespace: ResolvedNamespace::new(
                "google.personal",
                ProviderKind::GoogleWorkspace,
                "Google personal",
                "google.personal_auth",
                false,
                None,
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "google.personal_auth",
                ProviderKind::GoogleWorkspace,
                AuthKind::GoogleOAuthFile,
                "me@gmail.com",
                AuthSecretRefs::GoogleOAuthFile {
                    credentials: SecretRef::new("google.personal_oauth").expect("secret ref should build"),
                },
            )
            .expect("auth should build"),
        }
    }
}
