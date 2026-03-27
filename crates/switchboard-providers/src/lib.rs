mod github;
mod google;
mod process_runtime;

use std::sync::Arc;

use switchboard_core::AdapterRegistry;

pub use crate::{github::GitHubAdapter, google::GoogleWorkspaceAdapter};

pub fn default_registry() -> AdapterRegistry {
    let mut adapters = AdapterRegistry::default();
    adapters.register(Arc::new(GitHubAdapter));
    adapters.register(Arc::new(GoogleWorkspaceAdapter::default()));
    adapters
}
