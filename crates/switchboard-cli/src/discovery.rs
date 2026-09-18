use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use switchboard_core::{
    NamespaceId, NamespaceStore, RegisteredTool, ResolvedNamespace, ToolExecutionSupport, ToolSurface,
};
use switchboard_providers::default_registry;
use switchboard_store::SwitchboardConfig;

use crate::{
    args::ToolCatalogRuntimeCommand,
    catalog::{tool_examples, ToolCatalogDetail, ToolCatalogEntry},
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

pub(crate) fn run(config_path: Option<&Path>, command: ToolCatalogRuntimeCommand, full: bool) -> Result<String> {
    let registry = default_registry().context("failed to load provider catalogs")?;
    match command {
        ToolCatalogRuntimeCommand::List {
            json,
            provider,
            namespace,
            executable,
            search,
            limit,
        } => {
            let available = namespaces(config_path, namespace.as_ref())?;
            let namespace_provider = if namespace.is_some() {
                available.first().map(|namespace| namespace.provider.clone())
            } else {
                None
            };
            let search = search.map(|search| search.to_lowercase());
            let mut tools = registry
                .list_tools()?
                .into_iter()
                .filter(|tool| {
                    provider.as_ref().map_or(true, |provider| tool.provider == *provider)
                        && namespace_provider
                            .as_ref()
                            .map_or(true, |provider| tool.provider == *provider)
                        && (!executable || tool.execution_support == ToolExecutionSupport::Executable)
                        && (full || tool.execution_support == ToolExecutionSupport::Executable)
                        && search.as_ref().map_or(true, |search| {
                            tool.name.as_str().to_lowercase().contains(search)
                                || tool.summary.to_lowercase().contains(search)
                        })
                })
                .collect::<Vec<_>>();
            tools.sort_by_key(|tool| {
                (
                    tool.execution_support != ToolExecutionSupport::Executable,
                    tool.surface != ToolSurface::Curated,
                    tool.name.clone(),
                )
            });
            let total = tools.len();
            tools.truncate(limit.map(usize::from).unwrap_or(if full { usize::MAX } else { 8 }));
            if full && json {
                render_json(
                    &ToolCatalogListResponse {
                        status: "ok",
                        tools: tools.iter().map(ToolCatalogEntry::from).collect(),
                    },
                    true,
                )
            } else if full {
                let mut text = render_tools_human(&tools);
                if total > tools.len() {
                    text.push_str(&format!("{} more matches; increase --limit\n", total - tools.len()));
                }
                Ok(text)
            } else {
                let matches = tools
                    .iter()
                    .map(|tool| ShortTool::new(tool, &available))
                    .collect::<Vec<_>>();
                if json {
                    #[derive(Serialize)]
                    struct Matches {
                        status: &'static str,
                        total: usize,
                        omitted: usize,
                        tools: Vec<ShortTool>,
                    }
                    render_json(
                        &Matches {
                            status: "ok",
                            total,
                            omitted: total - matches.len(),
                            tools: matches,
                        },
                        false,
                    )
                } else {
                    let mut text = String::new();
                    for item in &matches {
                        text.push_str(&format!("{}: {}\n  {}\n", item.name, item.summary, item.example));
                    }
                    if total > matches.len() {
                        text.push_str(&format!(
                            "{} more matches; refine --search or increase --limit\n",
                            total - matches.len()
                        ));
                    }
                    Ok(text)
                }
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
            if full {
                let detail = ToolCatalogDetail::new(&descriptor, &namespaces);
                return if json {
                    render_json(
                        &ToolCatalogDetailResponse {
                            status: "ok",
                            tool: detail,
                        },
                        true,
                    )
                } else {
                    Ok(render_tool_detail_human(&detail))
                };
            }
            let detail = ShortDetail::new(&descriptor, &namespaces);
            if json {
                #[derive(Serialize)]
                struct Detail<'a> {
                    status: &'static str,
                    tool: ShortDetail<'a>,
                }
                render_json(
                    &Detail {
                        status: "ok",
                        tool: detail,
                    },
                    false,
                )
            } else {
                let mut text = format!(
                    "{}: {}\nExecution: {:?}\n",
                    detail.name, detail.summary, detail.execution_support
                );
                for argument in detail.arguments {
                    text.push_str(&format!(
                        "  --{}{}{}\n",
                        argument.name,
                        if argument.required { " (required)" } else { "" },
                        if argument.repeated { " (repeatable)" } else { "" }
                    ));
                }
                for example in detail.examples.iter().take(2) {
                    text.push_str(&format!("{example}\n"));
                }
                text.push_str("Use --full for scopes, output schema, and native fallback.\n");
                Ok(text)
            }
        }
    }
}

#[derive(Serialize)]
struct ShortTool {
    name: switchboard_core::ToolName,
    summary: String,
    provider: switchboard_core::ProviderKind,
    execution_support: ToolExecutionSupport,
    example: String,
    namespaces: Vec<NamespaceId>,
}

impl ShortTool {
    fn new(tool: &RegisteredTool, available: &[ResolvedNamespace]) -> Self {
        let namespaces = available
            .iter()
            .filter(|namespace| namespace.provider == tool.provider)
            .map(|namespace| namespace.id.clone())
            .collect::<Vec<_>>();
        Self {
            name: tool.name.clone(),
            summary: tool.summary.clone(),
            provider: tool.provider.clone(),
            execution_support: tool.execution_support,
            example: tool_examples(tool, namespaces.first())
                .into_iter()
                .next()
                .unwrap_or_else(|| format!("switchboard {} --help", tool.name)),
            namespaces,
        }
    }
}

#[derive(Serialize)]
struct ShortDetail<'a> {
    name: &'a switchboard_core::ToolName,
    summary: &'a str,
    execution_support: ToolExecutionSupport,
    arguments: &'a [switchboard_core::ToolArgumentSpec],
    available_namespaces: Vec<NamespaceId>,
    examples: Vec<String>,
}

impl<'a> ShortDetail<'a> {
    fn new(tool: &'a RegisteredTool, namespaces: &[ResolvedNamespace]) -> Self {
        Self {
            name: &tool.name,
            summary: &tool.summary,
            execution_support: tool.execution_support,
            arguments: &tool.arguments,
            available_namespaces: namespaces.iter().map(|namespace| namespace.id.clone()).collect(),
            examples: tool_examples(tool, namespaces.first().map(|namespace| &namespace.id)),
        }
    }
}
