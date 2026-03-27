use std::{fs, path::PathBuf, process::ExitCode};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use switchboard_core::ProviderKind;
use switchboard_providers::inventory_generator::{generate_inventory, CliInventoryTarget, ProcessCliHelpRunner};

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
    let targets = resolve_targets(cli.command.provider())?;
    let runner = ProcessCliHelpRunner;

    match cli.command {
        Command::Generate { .. } => {
            for target in targets {
                let inventory = generate_inventory(&target, &runner)
                    .with_context(|| format!("failed to generate inventory for {}", target.provider))?;
                let rendered = serde_json::to_string_pretty(&inventory).context("failed to serialize inventory")?;
                let output_path = workspace_root().join(&target.output_path);
                if let Some(parent) = output_path.parent() {
                    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
                }
                fs::write(&output_path, format!("{rendered}\n"))
                    .with_context(|| format!("failed to write {}", output_path.display()))?;
                println!("updated {}", output_path.display());
            }
        }
        Command::Check { .. } => {
            for target in targets {
                let inventory = generate_inventory(&target, &runner)
                    .with_context(|| format!("failed to generate inventory for {}", target.provider))?;
                let rendered = format!(
                    "{}\n",
                    serde_json::to_string_pretty(&inventory).context("failed to serialize inventory")?
                );
                let output_path = workspace_root().join(&target.output_path);
                let existing = fs::read_to_string(&output_path)
                    .with_context(|| format!("failed to read {}", output_path.display()))?;
                if existing != rendered {
                    return Err(anyhow!(
                        "inventory file {} is stale, run `cargo run -p switchboard-providers --bin switchboard-catalog-gen -- generate{}`",
                        output_path.display(),
                        provider_hint(&target.provider)
                    ));
                }
                println!("ok {}", output_path.display());
            }
        }
    }

    Ok(())
}

impl Command {
    fn provider(&self) -> Option<&str> {
        match self {
            Self::Generate { provider } | Self::Check { provider } => provider.as_deref(),
        }
    }
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
