use switchboard_core::{PlannedAction, PolicyDecision, PolicyEngine, ResolvedNamespace, ToolKind};

#[derive(Default, Debug)]
pub struct DefaultPolicyEngine;

impl PolicyEngine for DefaultPolicyEngine {
    fn evaluate(&self, _namespace: &ResolvedNamespace, plan: &PlannedAction) -> PolicyDecision {
        match plan.kind {
            ToolKind::Read => PolicyDecision::Allow,
            ToolKind::Write => PolicyDecision::RequireApproval {
                reason: format!("{} stays draft-first until approval UX is wired", plan.tool),
            },
        }
    }
}
