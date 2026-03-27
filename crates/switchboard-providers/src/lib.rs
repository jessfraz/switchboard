mod cli;
mod github;
mod google;
pub mod inventory;
pub mod inventory_generator;
mod process_runtime;
#[cfg(test)]
mod test_support;

use std::sync::Arc;

use switchboard_core::AdapterRegistry;

use crate::inventory::CliInventory;
pub use crate::{github::GitHubAdapter, google::GoogleWorkspaceAdapter};

/// Validate one provider manifest against the shared schema and embedded inventory model.
pub fn validate_manifest_json(manifest_json: &str, inventory: &CliInventory) -> switchboard_core::Result<()> {
    crate::cli::validate_manifest_json(manifest_json, inventory)
}

/// Build the default registry of provider adapters available in this workspace.
pub fn default_registry() -> AdapterRegistry {
    let mut adapters = AdapterRegistry::default();
    adapters.register(Arc::new(GitHubAdapter::default()));
    adapters.register(Arc::new(GoogleWorkspaceAdapter::default()));
    adapters
}
