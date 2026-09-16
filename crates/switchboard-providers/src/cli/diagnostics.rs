use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use serde::Serialize;
use switchboard_core::{process::output_with_timeout, Error, ProviderKind, Result};

use crate::{
    cli::{
        command::CliBinarySpec,
        locator::{CliLocator, DefaultCliLocator},
        CliProviderCatalog,
    },
    inventory::embedded_inventory,
};

/// A CLI location and version check, without resolving namespace credentials.
#[derive(Clone, Debug, Serialize)]
pub struct CliBinaryDiagnostic {
    pub program: String,
    pub override_variable: Option<String>,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub issue: Option<&'static str>,
}

/// Resolve the same CLI binary used by provider commands and run only its version probe.
pub fn diagnose_provider_cli(provider: ProviderKind) -> Result<CliBinaryDiagnostic> {
    if provider == ProviderKind::Phone {
        return Ok(inspect_binary(&crate::phone::runtime::binary(), false));
    }
    let manifest = match provider {
        ProviderKind::GitHub => include_str!("../../manifests/github.json"),
        ProviderKind::GoogleWorkspace => include_str!("../../manifests/google.json"),
        ProviderKind::MyChart => include_str!("../../manifests/mychart.json"),
        ProviderKind::Schwab => include_str!("../../manifests/schwab.json"),
        _ => return Err(Error::UnsupportedOperation(format!("no CLI provider for {provider}"))),
    };
    let inventory = embedded_inventory(provider.clone())?;
    let catalog = CliProviderCatalog::from_embedded(manifest, &inventory)?;
    let binary = &catalog
        .find_command(&format!("{provider}.cli.read"))
        .and_then(|command| command.executable.as_ref())
        .ok_or_else(|| Error::Config(format!("provider {provider} has no raw CLI binary")))?
        .binary;
    Ok(inspect_binary(binary, provider == ProviderKind::GoogleWorkspace))
}

/// Inspect the 1Password binary without invoking account discovery or authentication.
pub fn diagnose_one_password_cli() -> CliBinaryDiagnostic {
    let override_value = env::var_os("SWITCHBOARD_OP_BIN");
    if override_value.as_ref().is_some_and(|value| value.is_empty()) {
        return CliBinaryDiagnostic {
            program: "op".into(),
            override_variable: Some("SWITCHBOARD_OP_BIN".into()),
            path: None,
            version: None,
            issue: Some("SWITCHBOARD_OP_BIN is empty"),
        };
    }
    // `op` accepts a bare executable name as its override, which Command resolves on PATH.
    let bare_override = override_value
        .as_ref()
        .filter(|value| Path::new(value).components().count() == 1);
    let binary = CliBinarySpec {
        program: bare_override
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "op".into()),
        env_override: bare_override.is_none().then(|| "SWITCHBOARD_OP_BIN".into()),
        version_args: vec!["--version".into()],
    };
    let mut diagnostic = inspect_binary(&binary, false);
    diagnostic.program = "op".into();
    diagnostic.override_variable = Some("SWITCHBOARD_OP_BIN".into());
    diagnostic
}

fn inspect_binary(binary: &CliBinarySpec, google: bool) -> CliBinaryDiagnostic {
    let mut diagnostic = CliBinaryDiagnostic {
        program: binary.program.clone(),
        override_variable: binary.env_override.clone(),
        path: None,
        version: None,
        issue: None,
    };
    let path = match DefaultCliLocator.resolve(binary) {
        Ok(path) => path,
        Err(_) => {
            diagnostic.issue = Some("CLI not found; check PATH and the binary override");
            return diagnostic;
        }
    };
    let mut command = Command::new(&path);
    command.args(&binary.version_args);
    if google {
        command.env("GOOGLE_WORKSPACE_CLI_KEYRING_BACKEND", "file");
    }
    diagnostic.path = Some(path);
    match output_with_timeout(&mut command, Duration::from_secs(3)) {
        Ok(output) if output.status.success() => {
            diagnostic.version = numeric_version(&String::from_utf8_lossy(&output.stdout));
            if diagnostic.version.is_none() {
                diagnostic.issue = Some("CLI version output did not contain a recognizable version");
            }
        }
        Ok(_) => diagnostic.issue = Some("CLI version check failed"),
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
            diagnostic.issue = Some("CLI version check timed out after 3 seconds");
        }
        Err(_) => diagnostic.issue = Some("CLI version check could not run"),
    }
    diagnostic
}

// Report only the version number, never arbitrary stdout or stderr from a configured executable.
fn numeric_version(output: &str) -> Option<String> {
    output.split_whitespace().find_map(|word| {
        let version = word.trim_start_matches('v').split(['-', '+']).next()?;
        let components = version.split('.').collect::<Vec<_>>();
        ((2..=4).contains(&components.len())
            && components
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())))
        .then(|| version.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use crate::cli::diagnostics::numeric_version;

    #[test]
    fn version_diagnostic_discards_arbitrary_process_output() {
        assert_eq!(
            numeric_version("gws v0.22.5-preview\nsecret-value"),
            Some("0.22.5".to_owned())
        );
        assert_eq!(numeric_version("access-token-is-sensitive"), None);
    }
}
