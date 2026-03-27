use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::json;
use switchboard_core::ProviderKind;
use switchboard_providers::{
    inventory::CliInventory,
    inventory_generator::{generate_inventory, CliInventoryTarget, ProcessCliHelpRunner},
    validate_manifest_json,
};

#[derive(Parser)]
#[command(name = "switchboard-catalog-gen")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Generate {
        #[arg(long)]
        provider: Option<String>,
    },
    Check {
        #[arg(long)]
        provider: Option<String>,
    },
    Scaffold {
        #[arg(long)]
        provider: String,
        #[arg(long)]
        program: Option<String>,
        #[arg(long)]
        inventory_path: Option<PathBuf>,
        #[arg(long)]
        manifest_path: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let runner = ProcessCliHelpRunner;

    match cli.command {
        Command::Generate { provider } => {
            for target in resolve_targets(provider.as_deref())? {
                let inventory = generate_inventory(&target, &runner)
                    .with_context(|| format!("failed to generate inventory for {}", target.provider))?;
                write_inventory_file(&target, &inventory)?;
            }
        }
        Command::Check { provider } => {
            for target in resolve_targets(provider.as_deref())? {
                let inventory = generate_inventory(&target, &runner)
                    .with_context(|| format!("failed to generate inventory for {}", target.provider))?;
                check_inventory_file(&target, &inventory)?;
                validate_manifest_file(&target, &inventory)?;
            }
        }
        Command::Scaffold {
            provider,
            program,
            inventory_path,
            manifest_path,
            force,
        } => {
            scaffold_manifest(
                parse_provider(&provider)?,
                program,
                inventory_path,
                manifest_path,
                force,
            )?;
        }
    }

    Ok(())
}

fn resolve_targets(provider: Option<&str>) -> Result<Vec<CliInventoryTarget>> {
    let mut targets = CliInventoryTarget::default_targets();
    if let Some(provider) = provider {
        let provider = parse_provider(provider)?;
        targets.retain(|target| target.provider == provider);
        if targets.is_empty() {
            return Err(anyhow!("no inventory target configured for provider {provider}"));
        }
    }

    Ok(targets)
}

fn parse_provider(value: &str) -> Result<ProviderKind> {
    ProviderKind::from_identifier(value).ok_or_else(|| anyhow!("unsupported provider {value}"))
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn provider_hint(provider: &ProviderKind) -> String {
    format!(" --provider {provider}")
}

fn default_manifest_path(provider: &ProviderKind) -> PathBuf {
    PathBuf::from(format!("crates/switchboard-providers/manifests/{provider}.json"))
}

fn default_target(provider: &ProviderKind) -> Option<CliInventoryTarget> {
    CliInventoryTarget::default_targets()
        .into_iter()
        .find(|target| &target.provider == provider)
}

fn write_inventory_file(target: &CliInventoryTarget, inventory: &CliInventory) -> Result<()> {
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(inventory).context("failed to serialize inventory")?
    );
    let output_path = workspace_root().join(&target.output_path);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&output_path, rendered).with_context(|| format!("failed to write {}", output_path.display()))?;
    println!("updated {}", output_path.display());
    Ok(())
}

fn check_inventory_file(target: &CliInventoryTarget, inventory: &CliInventory) -> Result<()> {
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(inventory).context("failed to serialize inventory")?
    );
    let output_path = workspace_root().join(&target.output_path);
    let existing =
        fs::read_to_string(&output_path).with_context(|| format!("failed to read {}", output_path.display()))?;
    if existing != rendered {
        return Err(anyhow!(
            "inventory file {} is stale, run `cargo run -p switchboard-providers --bin switchboard-catalog-gen -- generate{}`",
            output_path.display(),
            provider_hint(&target.provider)
        ));
    }
    println!("ok {}", output_path.display());
    Ok(())
}

fn validate_manifest_file(target: &CliInventoryTarget, inventory: &CliInventory) -> Result<()> {
    let manifest_path = workspace_root().join(default_manifest_path(&target.provider));
    let manifest_json =
        fs::read_to_string(&manifest_path).with_context(|| format!("failed to read {}", manifest_path.display()))?;
    validate_manifest_json(&manifest_json, inventory)
        .with_context(|| format!("manifest {} is invalid", manifest_path.display()))?;
    println!("ok {}", manifest_path.display());
    Ok(())
}

fn scaffold_manifest(
    provider: ProviderKind,
    program: Option<String>,
    inventory_path: Option<PathBuf>,
    manifest_path: Option<PathBuf>,
    force: bool,
) -> Result<()> {
    let default_target = default_target(&provider);
    let program = program
        .or_else(|| default_target.as_ref().map(|target| target.program.clone()))
        .ok_or_else(|| anyhow!("scaffold requires --program for provider {provider}"))?;
    let inventory_path = inventory_path
        .or_else(|| default_target.as_ref().map(|target| target.output_path.clone()))
        .ok_or_else(|| anyhow!("scaffold requires --inventory-path for provider {provider}"))?;
    let manifest_path = manifest_path.unwrap_or_else(|| default_manifest_path(&provider));

    let inventory_file = workspace_root().join(&inventory_path);
    let inventory = load_inventory_file(&inventory_file)?;
    if inventory.provider != provider {
        bail!(
            "inventory {} is for provider {}, not {}",
            inventory_file.display(),
            inventory.provider,
            provider
        );
    }

    let rendered = render_manifest_scaffold(&inventory, &program)?;
    let output_path = workspace_root().join(&manifest_path);
    if output_path.exists() && !force {
        bail!(
            "manifest {} already exists, delete it or rerun with --force",
            output_path.display()
        );
    }
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&output_path, rendered).with_context(|| format!("failed to write {}", output_path.display()))?;
    println!("scaffolded {}", output_path.display());
    Ok(())
}

fn load_inventory_file(path: &Path) -> Result<CliInventory> {
    let json = fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&json).with_context(|| format!("failed to parse {}", path.display()))
}

fn render_manifest_scaffold(inventory: &CliInventory, program: &str) -> Result<String> {
    let binary_id = sanitize_identifier(program);
    let root_help_args = inventory
        .commands
        .iter()
        .find(|command| command.path.is_empty())
        .map(|command| command.help_args.clone())
        .unwrap_or_else(|| vec!["--help".to_owned()]);
    let manifest = json!({
        "provider": inventory.provider,
        "binaries": [
            {
                "id": binary_id,
                "program": program,
                "env_override": default_env_override(program),
                "version_args": ["--version"],
            }
        ],
        "capabilities": [
            {
                "id": "root_help",
                "args": root_help_args,
            }
        ],
        "commands": [
            {
                "name": format!("{}.cli.read", inventory.provider),
                "kind": "read",
                "summary": format!("Run a raw {} read command", program),
                "strategy": {
                    "kind": "raw_passthrough",
                },
                "execution": {
                    "kind": "executable",
                    "binary": binary_id,
                    "capability": "root_help",
                },
                "surface": "raw",
                "aggregate_read_supported": true,
            },
            {
                "name": format!("{}.cli.write", inventory.provider),
                "kind": "write",
                "summary": format!("Run a raw {} write command", program),
                "strategy": {
                    "kind": "raw_passthrough",
                },
                "execution": {
                    "kind": "executable",
                    "binary": binary_id,
                    "capability": "root_help",
                },
                "surface": "raw",
            }
        ]
    });
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(&manifest).context("failed to serialize scaffold manifest")?
    );
    validate_manifest_json(&rendered, inventory).context("generated scaffold manifest is invalid")?;
    Ok(rendered)
}

fn sanitize_identifier(program: &str) -> String {
    let mut identifier = String::new();
    for character in program.chars() {
        if character.is_ascii_alphanumeric() {
            identifier.push(character.to_ascii_lowercase());
        } else {
            identifier.push('_');
        }
    }

    if identifier.is_empty() {
        "cli".to_owned()
    } else {
        identifier
    }
}

fn default_env_override(program: &str) -> String {
    let upper = program
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("SWITCHBOARD_{upper}_BIN")
}

#[cfg(test)]
mod tests {
    use switchboard_core::ProviderKind;
    use switchboard_providers::inventory::{
        CliInventory, CliInventoryCommand, CliInventoryNodeKind, CliOperationKind, CliUndoSupport,
    };

    use crate::{default_env_override, render_manifest_scaffold, sanitize_identifier};

    #[test]
    fn scaffold_manifest_renders_generic_raw_tools() {
        let inventory = CliInventory {
            provider: ProviderKind::GoogleWorkspace,
            program: "gws".into(),
            commands: vec![CliInventoryCommand {
                path: Vec::new(),
                command: "gws".into(),
                summary: "Google Workspace CLI".into(),
                usage: Some("gws <command>".into()),
                help_args: vec!["--help".into()],
                node_kind: CliInventoryNodeKind::Group,
                operation_kind: CliOperationKind::Unknown,
                undo_support: CliUndoSupport::Unknown,
                subcommands: vec!["calendar".into()],
            }],
        };

        let rendered = render_manifest_scaffold(&inventory, "gws").expect("scaffold should render");
        let value: serde_json::Value = serde_json::from_str(&rendered).expect("manifest should parse");

        assert_eq!(value["provider"], "google");
        assert_eq!(value["binaries"][0]["id"], "gws");
        assert_eq!(value["binaries"][0]["env_override"], "SWITCHBOARD_GWS_BIN");
        assert_eq!(value["commands"][0]["name"], "google.cli.read");
        assert_eq!(value["commands"][1]["name"], "google.cli.write");
    }

    #[test]
    fn sanitize_identifier_normalizes_cli_program_names() {
        assert_eq!(sanitize_identifier("gh"), "gh");
        assert_eq!(sanitize_identifier("google-workspace-cli"), "google_workspace_cli");
    }

    #[test]
    fn default_env_override_normalizes_cli_program_names() {
        assert_eq!(default_env_override("gh"), "SWITCHBOARD_GH_BIN");
        assert_eq!(
            default_env_override("google-workspace-cli"),
            "SWITCHBOARD_GOOGLE_WORKSPACE_CLI_BIN"
        );
    }
}
