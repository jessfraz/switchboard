mod cli;
mod github;
mod google;
pub mod inventory;
pub mod inventory_generator;
mod mychart;
mod process_runtime;
mod schwab;
#[cfg(test)]
mod test_support;

use std::sync::Arc;

use switchboard_core::AdapterRegistry;

use crate::inventory::CliInventory;
pub use crate::{
    cli::diagnostics::{diagnose_one_password_cli, diagnose_provider_cli, CliBinaryDiagnostic},
    github::GitHubAdapter,
    google::GoogleWorkspaceAdapter,
    mychart::MyChartAdapter,
    schwab::SchwabAdapter,
};

/// Validate one provider manifest against the shared schema and embedded inventory model.
pub fn validate_manifest_json(manifest_json: &str, inventory: &CliInventory) -> switchboard_core::Result<()> {
    crate::cli::validate_manifest_json(manifest_json, inventory)
}

/// Build the default registry of provider adapters available in this workspace.
pub fn default_registry() -> switchboard_core::Result<AdapterRegistry> {
    let mut adapters = AdapterRegistry::default();
    adapters.register(Arc::new(GitHubAdapter::new()?));
    adapters.register(Arc::new(GoogleWorkspaceAdapter::new()?));
    adapters.register(Arc::new(MyChartAdapter::new()?));
    adapters.register(Arc::new(SchwabAdapter::new()?));
    Ok(adapters)
}
