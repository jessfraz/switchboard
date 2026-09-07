use std::sync::Arc;

use switchboard_core::{
    Adapter, AdapterRegistry, BackendKind, Error, ExecutionTarget, PlannedAction, PlanningTarget, ProviderKind, Result,
    ToolDescriptor, ToolKind, ToolName, ToolOutput, ToolRequest,
};

struct CatalogAdapter {
    tools: Vec<ToolDescriptor>,
}

impl Adapter for CatalogAdapter {
    fn provider(&self) -> ProviderKind {
        ProviderKind::GitHub
    }

    fn tools(&self) -> &[ToolDescriptor] {
        &self.tools
    }

    fn plan(
        &self,
        target: &PlanningTarget,
        request: &ToolRequest,
        descriptor: &ToolDescriptor,
    ) -> Result<PlannedAction> {
        Ok(PlannedAction::new(
            request,
            target,
            descriptor.kind,
            &descriptor.summary,
            descriptor.backend,
        ))
    }

    fn execute(&self, _target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
        Err(Error::NotImplemented(action.tool.to_string()))
    }
}

#[test]
fn registry_owns_runtime_catalogs_and_releases_replaced_adapters() {
    let mut registry = AdapterRegistry::default();
    let first_name = ToolName::new("github.runtime.first").expect("tool name is valid");
    let adapter = Arc::new(CatalogAdapter {
        tools: vec![ToolDescriptor::new(
            first_name.as_str(),
            ToolKind::Read,
            "First runtime catalog",
            BackendKind::Cli,
        )
        .expect("descriptor is valid")],
    });
    let first_owner = Arc::downgrade(&adapter);
    registry.register(adapter);

    let first = registry
        .describe_tool(&first_name)
        .expect("catalog lookup succeeds")
        .expect("first tool exists");
    assert_eq!(first.summary, "First runtime catalog");
    assert!(first_owner.upgrade().is_some());

    registry.register(Arc::new(CatalogAdapter {
        tools: vec![ToolDescriptor::new(
            "github.runtime.second",
            ToolKind::Read,
            "Replacement runtime catalog",
            BackendKind::Cli,
        )
        .expect("descriptor is valid")],
    }));

    assert!(first_owner.upgrade().is_none());
    assert!(registry
        .describe_tool(&first_name)
        .expect("catalog lookup succeeds")
        .is_none());
    let tools = registry.list_tools().expect("catalog listing succeeds");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name.as_str(), "github.runtime.second");
    assert_eq!(first.summary, "First runtime catalog");
}
