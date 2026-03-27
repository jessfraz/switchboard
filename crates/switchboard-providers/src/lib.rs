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

pub use crate::{github::GitHubAdapter, google::GoogleWorkspaceAdapter};
use crate::inventory::CliInventory;

pub fn validate_manifest_json(manifest_json: &str, inventory: &CliInventory) -> switchboard_core::Result<()> {
    crate::cli::manifest::validate_manifest_json(manifest_json, inventory)
}

pub fn default_registry() -> AdapterRegistry {
    let mut adapters = AdapterRegistry::default();
    adapters.register(Arc::new(GitHubAdapter::default()));
    adapters.register(Arc::new(GoogleWorkspaceAdapter::default()));
    adapters
}
