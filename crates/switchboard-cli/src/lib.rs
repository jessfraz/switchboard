use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use switchboard_core::{
    AuthStore, DispatchOutcome, NamespaceStore, SecretResolver, SecretStore, Switchboard, SwitchboardServices,
};
use switchboard_providers::default_registry;
use switchboard_store::{
    resolve_operation_store_path, LocalSecretResolver, SqliteAuditStore, SqliteOperationStore, SwitchboardConfig,
};

mod args;
mod output;

#[cfg(test)]
mod test_support;

use crate::{
    args::{AuditRuntimeCommand, AuditSelector, Cli, CommandKind, StoredOperationCommand, ToolCatalogRuntimeCommand},
    output::{
        operation_needs_attention, render_audit_events_human, render_audit_selection_human, render_clap_error,
        render_dispatch_human, render_json, render_json_dispatch, render_json_error, render_json_operation,
        render_namespaces_human, render_operation_human, render_operations_human, render_output_human,
        render_stored_operation_human, render_tool_detail_human, render_tools_human, AuditEventResponse,
        AuditListResponse, AuditOperationResponse, AuditSelection, NamespaceListResponse, StoredOperationListResponse,
        StoredOperationResponse, ToolCatalogDetail, ToolCatalogDetailResponse, ToolCatalogEntry,
        ToolCatalogListResponse,
    },
};

fn load_switchboard(config_path: Option<&Path>) -> Result<Switchboard> {
    let config_path = resolve_config_path(config_path)?;
    let config = SwitchboardConfig::from_file(&config_path).context("failed to load switchboard config")?;
    let policy = config.policy_engine();
    let (namespaces, auth, secrets) = config.into_stores();
    let state_db_path = resolve_operation_store_path(&config_path);
    let one_password_session_cache = state_db_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("onepassword-sessions.json");
    let operations = SqliteOperationStore::open(&state_db_path).context("failed to open operation store")?;
    let audit = SqliteAuditStore::open(&state_db_path).context("failed to open audit store")?;

    Ok(build_switchboard(
        Arc::new(namespaces),
        Arc::new(auth),
        Arc::new(secrets),
        Arc::new(LocalSecretResolver::with_one_password_session_cache(Some(
            one_password_session_cache,
        ))),
        Arc::new(policy),
        Arc::new(audit),
        Arc::new(operations),
    ))
}

fn build_switchboard(
    namespaces: Arc<dyn NamespaceStore>,
    auth: Arc<dyn AuthStore>,
    secrets: Arc<dyn SecretStore>,
    secret_resolver: Arc<dyn SecretResolver>,
    policy: Arc<dyn switchboard_core::PolicyEngine>,
    audit: Arc<dyn switchboard_core::AuditStore>,
    operations: Arc<dyn switchboard_core::OperationStore>,
) -> Switchboard {
    let adapters = default_registry();

    Switchboard::new(
        SwitchboardServices {
            namespaces,
            auth,
            secrets,
            secret_resolver,
            policy,
            audit,
            operations,
        },
        adapters,
    )
}

/// Run the Switchboard CLI and return a process exit code.
pub fn main_entry<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let json_requested = contains_flag(&args, "--json");
    let cli = match Cli::try_parse_from(args.clone()) {
        Ok(cli) => cli,
        Err(error) => return render_clap_error(error, json_requested),
    };

    match run(cli) {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            if json_requested {
                println!("{}", render_json_error(&error.to_string()));
            } else {
                eprintln!("{error:#}");
            }

            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<String> {
    let config_path = cli.config.clone();
    let json_requested = cli.json_requested();
    let switchboard = load_switchboard(config_path.as_deref());
    let switchboard = match switchboard {
        Ok(switchboard) => switchboard,
        Err(error) if json_requested => return Err(error),
        Err(error) => return Err(error.context("failed to initialize switchboard")),
    };

    match cli.command.into_runtime_command()? {
        CommandKind::NamespaceList => {
            let namespaces = switchboard.list_namespaces();
            if json_requested {
                render_json(&NamespaceListResponse { namespaces }, true)
            } else {
                Ok(render_namespaces_human(&namespaces))
            }
        }
        CommandKind::ToolCatalog(command) => run_tool_catalog_command(&switchboard, command),
        CommandKind::Audit(command) => run_audit_command(&switchboard, command),
        CommandKind::Operation(request) => {
            let outcome = switchboard.execute_operation(request)?;

            if json_requested {
                render_json_operation(&outcome)
            } else {
                Ok(render_operation_human(&outcome))
            }
        }
        CommandKind::StoredOperation(command) => run_stored_operation_command(&switchboard, command),
    }
}

fn run_audit_command(switchboard: &Switchboard, command: AuditRuntimeCommand) -> Result<String> {
    match command {
        AuditRuntimeCommand::List { operation_id, json } => {
            let events = match operation_id.as_ref() {
                Some(operation_id) => switchboard.list_audit_events_for_operation(operation_id),
                None => switchboard.list_audit_events(),
            };

            if json {
                render_json(
                    &AuditListResponse {
                        status: "ok",
                        events: &events,
                    },
                    true,
                )
            } else {
                Ok(render_audit_events_human(&events))
            }
        }
        AuditRuntimeCommand::Show { selector, json } => {
            let selection = match selector {
                AuditSelector::EventId(id) => AuditSelection::Single(
                    switchboard
                        .get_audit_event(&id)
                        .ok_or_else(|| anyhow!(switchboard_core::Error::UnknownAuditEvent(id.clone())))?,
                ),
                AuditSelector::OperationId(id) => {
                    AuditSelection::Operation(id.clone(), switchboard.list_audit_events_for_operation(&id))
                }
            };

            if json {
                match &selection {
                    AuditSelection::Single(event) => render_json(&AuditEventResponse { status: "ok", event }, true),
                    AuditSelection::Operation(operation_id, events) => render_json(
                        &AuditOperationResponse {
                            status: "ok",
                            operation_id,
                            events,
                        },
                        true,
                    ),
                }
            } else {
                Ok(render_audit_selection_human(&selection))
            }
        }
    }
}

fn run_tool_catalog_command(switchboard: &Switchboard, command: ToolCatalogRuntimeCommand) -> Result<String> {
    match command {
        ToolCatalogRuntimeCommand::List { json } => {
            let tools = switchboard.list_tools()?;
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
        ToolCatalogRuntimeCommand::Describe { tool, json } => {
            let descriptor = switchboard
                .describe_tool(&tool)
                .context("failed to resolve tool metadata")?
                .ok_or_else(|| anyhow!("unknown tool: {tool}"))?;
            let namespaces = switchboard
                .list_namespaces()
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

fn run_stored_operation_command(switchboard: &Switchboard, command: StoredOperationCommand) -> Result<String> {
    match command {
        StoredOperationCommand::List { pending_only, json } => {
            let operations = switchboard
                .list_operations()
                .into_iter()
                .filter(|operation| !pending_only || operation_needs_attention(operation))
                .collect::<Vec<_>>();
            if json {
                render_json(
                    &StoredOperationListResponse {
                        status: "ok",
                        operations: &operations,
                    },
                    true,
                )
            } else {
                Ok(render_operations_human(&operations))
            }
        }
        StoredOperationCommand::Show { id, json } => {
            let operation = switchboard
                .get_operation(&id)
                .ok_or_else(|| anyhow!("unknown operation id: {id}"))?;
            if json {
                render_json(
                    &StoredOperationResponse {
                        status: "ok",
                        operation: &operation,
                    },
                    true,
                )
            } else {
                Ok(render_stored_operation_human(&operation))
            }
        }
        StoredOperationCommand::Approve {
            id,
            actor,
            note,
            apply,
            json,
        } => {
            let operation = switchboard.approve_operation(&id, &actor, note.as_deref())?;
            if apply {
                let output = switchboard.apply_operation(&id)?;
                if json {
                    return render_json_dispatch(&DispatchOutcome::Executed(output));
                }

                return Ok(render_output_human(&output));
            }

            if json {
                render_json(
                    &StoredOperationResponse {
                        status: "approved",
                        operation: &operation,
                    },
                    true,
                )
            } else {
                Ok(render_stored_operation_human(&operation))
            }
        }
        StoredOperationCommand::Reject { id, actor, note, json } => {
            let operation = switchboard.reject_operation(&id, &actor, note.as_deref())?;
            if json {
                render_json(
                    &StoredOperationResponse {
                        status: "rejected",
                        operation: &operation,
                    },
                    true,
                )
            } else {
                Ok(render_stored_operation_human(&operation))
            }
        }
        StoredOperationCommand::Apply { id, json } => {
            let output = switchboard.apply_operation(&id)?;
            if json {
                render_json_dispatch(&DispatchOutcome::Executed(output))
            } else {
                Ok(render_output_human(&output))
            }
        }
        StoredOperationCommand::Undo { id, mode, json } => {
            let outcome = switchboard.undo_operation(&id, mode)?;
            if json {
                render_json_dispatch(&outcome)
            } else {
                Ok(render_dispatch_human(&outcome))
            }
        }
    }
}

#[derive(Debug, Default)]
struct ConfigPathCandidates {
    explicit: Option<PathBuf>,
    cwd: Option<PathBuf>,
    appdata: Option<PathBuf>,
    xdg: Option<PathBuf>,
    home: Option<PathBuf>,
}

fn resolve_config_path(config_path: Option<&Path>) -> Result<PathBuf> {
    let candidates = ConfigPathCandidates {
        explicit: config_path.map(Path::to_path_buf),
        cwd: existing_file(PathBuf::from("switchboard.toml")),
        appdata: env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("switchboard").join("config.toml"))
            .filter(|path| path.is_file()),
        xdg: env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .map(|path| path.join("switchboard").join("config.toml"))
            .filter(|path| path.is_file()),
        home: env::var_os("HOME")
            .map(PathBuf::from)
            .map(|path| path.join(".config").join("switchboard").join("config.toml"))
            .filter(|path| path.is_file()),
    };

    select_config_path(candidates)
}

fn select_config_path(candidates: ConfigPathCandidates) -> Result<PathBuf> {
    candidates
        .explicit
        .or(candidates.cwd)
        .or(candidates.appdata)
        .or(candidates.xdg)
        .or(candidates.home)
        .ok_or_else(|| {
            anyhow!(
                "no switchboard config found. Pass --config <path>, set SWITCHBOARD_CONFIG, create ./switchboard.toml, or place config at $XDG_CONFIG_HOME/switchboard/config.toml or $HOME/.config/switchboard/config.toml"
            )
        })
}

fn existing_file(path: PathBuf) -> Option<PathBuf> {
    path.is_file().then_some(path)
}

fn contains_flag(args: &[OsString], flag: &str) -> bool {
    args.iter().any(|value| value == flag)
}

pub fn args_from_env() -> Vec<OsString> {
    env::args_os().collect()
}

#[cfg(test)]
mod tests;
