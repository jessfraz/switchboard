use serde::{de::DeserializeOwned, Serialize};
use switchboard_core::{Error, ExecutionTarget, Result, ToolArguments};

use crate::{cli::CliStdioMode, github::GitHubAdapter};

impl GitHubAdapter {
    pub(super) fn read_json<T: DeserializeOwned>(&self, target: &ExecutionTarget, argv: Vec<String>) -> Result<T> {
        let spec = self
            .catalog
            .find_command("github.cli.read")
            .and_then(|command| command.executable.as_ref())
            .ok_or_else(|| Error::NotImplemented("GitHub raw reads are unavailable".into()))?;
        let response = self.backend.execute_raw(target, spec, argv, CliStdioMode::Capture)?;
        serde_json::from_str(&response.stdout)
            .map_err(|error| Error::Execution(format!("GitHub returned an invalid response: {error}")))
    }
}

pub(super) fn encode(value: &impl Serialize) -> Result<serde_json::Value> {
    serde_json::to_value(value).map_err(|error| Error::Execution(format!("cannot encode GitHub result: {error}")))
}

pub(super) fn repository(args: &ToolArguments) -> Result<String> {
    let value = args
        .value("repo")
        .ok_or_else(|| Error::InvalidArguments("missing --repo owner/name".into()))?;
    let parts = value.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || matches!(*part, "." | "..")
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
    {
        return Err(Error::InvalidArguments("--repo must be owner/name".into()));
    }
    Ok(value.to_owned())
}

pub(super) fn bounded(args: &ToolArguments, name: &str, default: u64, min: u64, max: u64) -> Result<u64> {
    let parsed = args
        .value(name)
        .map(str::parse::<u64>)
        .transpose()
        .map_err(|_| Error::InvalidArguments(format!("--{name} must be an integer between {min} and {max}")))?
        .unwrap_or(default);
    if !(min..=max).contains(&parsed) {
        return Err(Error::InvalidArguments(format!(
            "--{name} must be between {min} and {max}"
        )));
    }
    Ok(parsed)
}
