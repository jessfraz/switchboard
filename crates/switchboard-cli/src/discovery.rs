use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use switchboard_core::{NamespaceId, NamespaceStore, ResolvedNamespace, ToolExecutionSupport};
use switchboard_providers::default_registry;
use switchboard_store::SwitchboardConfig;

use crate::{
    args::ToolCatalogRuntimeCommand,
    catalog::{ToolCatalogDetail, ToolCatalogEntry},
    output::{
        render_json, render_tool_detail_human, render_tools_human, ToolCatalogDetailResponse, ToolCatalogListResponse,
    },
    resolve_config_path,
};

fn namespaces(config_path: Option<&Path>, selected: Option<&NamespaceId>) -> Result<Vec<ResolvedNamespace>> {
    let load = || {
        let path = resolve_config_path(config_path)?;
        let config = SwitchboardConfig::from_file(path)
            .map_err(|_| anyhow!("Namespace configuration could not be read or validated"))?;
        Ok::<_, anyhow::Error>(config.into_stores().0.list())
    };
    let namespaces = match load() {
        Ok(namespaces) => namespaces,
        // Catalog help is available even on a fresh installation or with broken
        // configuration. An explicit namespace filter must never pretend empty.
        Err(error) if selected.is_some() => return Err(error),
        Err(_) => Vec::new(),
    };
    if let Some(selected) = selected {
        let namespace = namespaces
            .into_iter()
            .find(|namespace| namespace.id == *selected)
            .ok_or_else(|| anyhow!("namespace is not configured: {selected}"))?;
        Ok(vec![namespace])
    } else {
        Ok(namespaces)
    }
}

pub(crate) fn run(config_path: Option<&Path>, command: ToolCatalogRuntimeCommand) -> Result<String> {
    let registry = default_registry().context("failed to load provider catalogs")?;
    match command {
        ToolCatalogRuntimeCommand::List {
            json,
            provider,
            namespace,
            executable,
            search,
        } => {
            let namespace_provider = if namespace.is_some() {
                namespaces(config_path, namespace.as_ref())?
                    .first()
                    .map(|namespace| namespace.provider.clone())
            } else {
                None
            };
            let search = search.map(|search| search.to_lowercase());
            let tools = registry
                .list_tools()?
                .into_iter()
                .filter(|tool| {
                    provider.as_ref().map_or(true, |provider| tool.provider == *provider)
                        && namespace_provider
                            .as_ref()
                            .map_or(true, |provider| tool.provider == *provider)
                        && (!executable || tool.execution_support == ToolExecutionSupport::Executable)
                        && search.as_ref().map_or(true, |search| {
                            tool.name.as_str().to_lowercase().contains(search)
                                || tool.summary.to_lowercase().contains(search)
                        })
                })
                .collect::<Vec<_>>();
            if json {
                render_json(
                    &ToolCatalogListResponse {
                        status: "ok",
                        tools: tools.iter().map(ToolCatalogEntry::from).collect(),
                    },
                    true,
                )
            } else {
                Ok(render_tools_human(&tools))
            }
        }
        ToolCatalogRuntimeCommand::Describe { tool, json, namespace } => {
            let descriptor = registry
                .describe_tool(&tool)?
                .ok_or_else(|| anyhow!("unknown tool: {tool}"))?;
            let namespaces = namespaces(config_path, namespace.as_ref())?;
            if namespace.is_some()
                && namespaces
                    .iter()
                    .any(|namespace| namespace.provider != descriptor.provider)
            {
                bail!("namespace provider does not match tool {tool}");
            }
            let namespaces = namespaces
                .into_iter()
                .filter(|namespace| namespace.provider == descriptor.provider)
                .collect::<Vec<_>>();
            let detail = ToolCatalogDetail::new(&descriptor, &namespaces);
            if json {
                render_json(
                    &ToolCatalogDetailResponse {
                        status: "ok",
                        tool: detail,
                    },
                    true,
                )
            } else {
                Ok(render_tool_detail_human(&detail))
            }
        }
    }
}
