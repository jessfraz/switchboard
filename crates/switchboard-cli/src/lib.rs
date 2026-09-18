use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use anyhow::{anyhow, bail, Context, Result};
use clap::{CommandFactory, Parser};
use switchboard_core::{
    AuthStore, DispatchOutcome, NamespaceStore, SecretResolver, SecretStore, Switchboard, SwitchboardServices,
    ToolKind, ToolRequest,
};
use switchboard_providers::default_registry;
use switchboard_store::{
    resolve_operation_store_path, LocalSecretResolver, SqliteAuditStore, SqliteOperationStore, SwitchboardConfig,
};

pub mod catalog;

mod args;
mod auth;
mod batch;
mod deadline;
mod discovery;
mod doctor;
mod output;
mod presentation;

#[cfg(test)]
mod test_support;

use crate::{
    args::{AuditRuntimeCommand, AuditSelector, Cli, CommandKind, StoredOperationCommand},
    output::{
        operation_needs_attention, render_audit_events_human, render_audit_selection_human, render_clap_error,
        render_dispatch_human, render_json, render_json_dispatch, render_namespaces_human, render_operations_human,
        render_stored_operation_human, AuditEventResponse, AuditListResponse, AuditOperationResponse, AuditSelection,
        NamespaceListResponse, StoredOperationListResponse, StoredOperationResponse,
    },
};

pub fn command() -> clap::Command {
    args::Cli::command()
}

fn load_switchboard(config_path: Option<&Path>) -> Result<Switchboard> {
    load_switchboard_with_run(config_path, std::env::var("SWITCHBOARD_RUN_ID").ok().as_deref())
}

fn load_switchboard_with_run(config_path: Option<&Path>, run_id: Option<&str>) -> Result<Switchboard> {
    let config_path = resolve_config_path(config_path)?;
    let config = SwitchboardConfig::from_file(&config_path).context("failed to load switchboard config")?;
    let policy = config.policy_engine();
    let one_password = config.one_password.clone();
    let (namespaces, auth, secrets) = config.into_stores();
    let state_db_path = resolve_operation_store_path(&config_path);
    let one_password_session_cache = state_db_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("onepassword-sessions.json");
    let operations = SqliteOperationStore::open(&state_db_path).context("failed to open operation store")?;
    let audit = SqliteAuditStore::open(&state_db_path).context("failed to open audit store")?;

    build_switchboard(
        Arc::new(namespaces),
        Arc::new(auth),
        Arc::new(secrets),
        Arc::new(LocalSecretResolver::with_recovery_budget(
            Some(one_password_session_cache),
            one_password,
            run_id.map(str::to_owned),
        )),
        Arc::new(policy),
        Arc::new(audit),
        Arc::new(operations),
    )
}

fn build_switchboard(
    namespaces: Arc<dyn NamespaceStore>,
    auth: Arc<dyn AuthStore>,
    secrets: Arc<dyn SecretStore>,
    secret_resolver: Arc<dyn SecretResolver>,
    policy: Arc<dyn switchboard_core::PolicyEngine>,
    audit: Arc<dyn switchboard_core::AuditStore>,
    operations: Arc<dyn switchboard_core::OperationStore>,
) -> Result<Switchboard> {
    let adapters = default_registry().context("failed to load provider catalogs")?;

    Ok(Switchboard::new(
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
    ))
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

    let requested_tool = match &cli.command {
        args::Commands::Tool(tokens) => tokens
            .first()
            .and_then(|token| token.to_str())
            .and_then(|name| switchboard_core::ToolName::new(name).ok()),
        _ => None,
    };
    match run(cli) {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            if let Some(incomplete) = error.downcast_ref::<output::IncompleteOutput>() {
                println!("{}", incomplete.0);
                return ExitCode::FAILURE;
            }
            if json_requested {
                let namespace = args
                    .iter()
                    .take_while(|value| value.as_os_str() != "--")
                    .enumerate()
                    .find_map(|(index, value)| {
                        let value = value.to_str()?;
                        let namespace = if value == "--ns" {
                            args.get(index + 1)?.to_str()?
                        } else {
                            value.strip_prefix("--ns=")?
                        };
                        switchboard_core::NamespaceId::new(namespace).ok()
                    });
                println!(
                    "{}",
                    output::render_json_error_for_tool(&error, namespace, requested_tool.as_ref())
                );
            } else {
                eprintln!("{error:#}");
            }

            ExitCode::FAILURE
        }
    }
}

fn run(mut cli: Cli) -> Result<String> {
    let config_path = cli.config.clone();
    let json_requested = cli.json_requested();
    if let args::Commands::Tool(tokens) = &mut cli.command {
        cli.presentation.extract(tokens)?;
    }
    cli.presentation.validate()?;
    let presentation = cli.presentation;
    let command = cli.command.into_runtime_command()?;
    presentation.validate_command(&command)?;
    if let CommandKind::Doctor(arguments) = command {
        return doctor::run(config_path.as_deref(), arguments);
    }
    if let CommandKind::ToolCatalog(command) = command {
        return discovery::run(config_path.as_deref(), command, presentation.full);
    }
    if let CommandKind::Auth(arguments) = command {
        return auth::run(config_path.as_deref(), arguments);
    }
    let _deadline = deadline::Deadline::configure(&command)?;
    let switchboard = load_switchboard(config_path.as_deref());
    let switchboard = match switchboard {
        Ok(switchboard) => switchboard,
        Err(error) if json_requested => return Err(error),
        Err(error) => return Err(error.context("failed to initialize switchboard")),
    };

    match command {
        CommandKind::Doctor(arguments) => doctor::run(config_path.as_deref(), arguments),
        CommandKind::Auth(arguments) => auth::run(config_path.as_deref(), arguments),
        CommandKind::ReadBatch(arguments) => batch::run(&switchboard, config_path.as_deref(), arguments, &presentation),
        CommandKind::NamespaceList => {
            let namespaces = switchboard.list_namespaces();
            if json_requested {
                render_json(&NamespaceListResponse { namespaces }, true)
            } else {
                Ok(render_namespaces_human(&namespaces))
            }
        }
        CommandKind::ToolCatalog(command) => discovery::run(config_path.as_deref(), command, presentation.full),
        CommandKind::Audit(command) => run_audit_command(&switchboard, command),
        CommandKind::Operation(request) => {
            let outcome = switchboard.execute_operation(request)?;

            let text = presentation.operation(&outcome, json_requested)?;
            output::require_complete(text, output::operation_complete(&outcome))
        }
        CommandKind::ApproveAndApply(request) => {
            let output = approve_and_apply(&switchboard, request)?;
            let complete = output::output_complete(&output);
            let text = presentation.dispatch(&DispatchOutcome::Executed(output), json_requested)?;
            output::require_complete(text, complete)
        }
        CommandKind::StoredOperation(command) => run_stored_operation_command(&switchboard, command),
    }
}

fn approve_and_apply(switchboard: &Switchboard, request: ToolRequest) -> Result<switchboard_core::ToolOutput> {
    let descriptor = switchboard
        .describe_tool(&request.tool)?
        .ok_or_else(|| anyhow!("unknown tool: {}", request.tool))?;
    if descriptor.kind != ToolKind::Write {
        bail!("--approve-and-apply requires a write tool");
    }
    // The parser forces Draft, so policy and persistence finish before the
    // exact resulting operation is approved or any provider write can run.
    let DispatchOutcome::Planned(plan) = switchboard.dispatch(request)? else {
        bail!("--approve-and-apply expected a persisted write plan");
    };
    let id = plan
        .operation_id
        .ok_or_else(|| anyhow!("write plan has no operation ID"))?;
    if plan.approval_required {
        switchboard
            .approve_operation(&id, &args::default_actor(), None)
            .with_context(|| format!("failed to approve operation {id}"))?;
    }
    switchboard
        .apply_operation(&id)
        .with_context(|| format!("failed to apply operation {id}; inspect it before retrying"))
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

fn run_stored_operation_command(switchboard: &Switchboard, command: StoredOperationCommand) -> Result<String> {
    match command {
        StoredOperationCommand::List { pending_only, json } => {
            let operations = switchboard
                .list_operations()?
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
                .get_operation(&id)?
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
                return output::render_output_result(&output, json);
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
            output::render_output_result(&output, json)
        }
        StoredOperationCommand::Verify { id, json } => {
            let operation = switchboard.verify_operation(&id)?;
            let verified = operation
                .verification
                .as_ref()
                .is_some_and(|receipt| receipt.status == switchboard_core::VerificationStatus::Verified);
            let text = if json {
                render_json(
                    &StoredOperationResponse {
                        status: if verified { "verified" } else { "unverified" },
                        operation: &operation,
                    },
                    true,
                )?
            } else {
                render_stored_operation_human(&operation)
            };
            output::require_complete(text, verified)
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
    args.iter()
        .take_while(|value| value.as_os_str() != "--")
        .any(|value| value == flag)
}

pub fn args_from_env() -> Vec<OsString> {
    env::args_os().collect()
}

#[cfg(test)]
mod tests;
