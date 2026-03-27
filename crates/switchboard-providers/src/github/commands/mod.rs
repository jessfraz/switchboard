mod issues;
mod notifications;
mod pull_requests;
mod raw;

use switchboard_core::{Error, Result, ToolArguments};

use crate::cli::{CliBinarySpec, CliCapabilityProbe};
pub(crate) use crate::github::commands::{
    issues::ISSUE_READ_COMMAND,
    notifications::NOTIFICATIONS_COMMAND,
    pull_requests::{PULL_REQUEST_READ_COMMAND, PULL_REQUEST_SEARCH_COMMAND},
    raw::{RAW_READ_COMMAND, RAW_WRITE_COMMAND},
};

pub(crate) const GH_BINARY: CliBinarySpec = CliBinarySpec {
    program: "gh",
    env_override: Some("SWITCHBOARD_GH_BIN"),
    version_args: &["--version"],
};

pub(crate) const GH_API_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "gh_api",
    args: &["api", "--help"],
};

pub(crate) const GH_BASE_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "gh_help",
    args: &["--help"],
};

pub(crate) const GH_PR_SEARCH_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "gh_search_prs",
    args: &["search", "prs", "--help"],
};

pub(crate) const GH_PR_VIEW_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "gh_pr_view",
    args: &["pr", "view", "--help"],
};

pub(crate) const GH_ISSUE_VIEW_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "gh_issue_view",
    args: &["issue", "view", "--help"],
};

pub(super) fn append_query_bool(args: &mut Vec<String>, arguments: &ToolArguments, name: &str) -> Result<()> {
    if arguments.has_flag(name) {
        args.push("-F".to_owned());
        args.push(format!("{name}=true"));
        return Ok(());
    }

    if let Some(value) = arguments.value(name) {
        let value = parse_bool(name, value)?;
        args.push("-F".to_owned());
        args.push(format!("{name}={value}"));
    }

    Ok(())
}

pub(super) fn append_query_value(args: &mut Vec<String>, arguments: &ToolArguments, name: &str) {
    if let Some(value) = arguments.value(name) {
        args.push("-F".to_owned());
        args.push(format!("{name}={value}"));
    }
}

pub(super) fn append_optional_flag(args: &mut Vec<String>, arguments: &ToolArguments, name: &str) -> Result<()> {
    if flag_enabled(arguments, name)? {
        args.push(format!("--{name}"));
    }

    Ok(())
}

pub(super) fn append_optional_value(args: &mut Vec<String>, arguments: &ToolArguments, name: &str) {
    if let Some(value) = arguments.value(name) {
        args.push(format!("--{name}"));
        args.push(value.to_owned());
    }
}

pub(super) fn flag_enabled(arguments: &ToolArguments, name: &str) -> Result<bool> {
    if arguments.has_flag(name) {
        return Ok(true);
    }

    match arguments.value(name) {
        Some(value) => parse_bool(name, value),
        None => Ok(false),
    }
}

fn parse_bool(name: &str, value: &str) -> Result<bool> {
    match value {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(Error::InvalidArguments(format!(
            "--{name} expects a boolean value, got {value:?}"
        ))),
    }
}
