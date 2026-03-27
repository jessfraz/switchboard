mod calendar;
mod gmail;

use switchboard_core::{Error, Result, ToolArguments};

use crate::cli::{CliBinarySpec, CliCapabilityProbe};
pub(crate) use crate::google::commands::{
    calendar::CALENDAR_LIST_COMMAND,
    gmail::{MAIL_READ_COMMAND, MAIL_SEARCH_COMMAND},
};

pub(crate) const GWS_BINARY: CliBinarySpec = CliBinarySpec {
    program: "gws",
    env_override: Some("SWITCHBOARD_GWS_BIN"),
    version_args: &["--version"],
};

pub(crate) const GWS_CALENDAR_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "calendar_agenda",
    args: &["calendar", "--help"],
};

pub(crate) const GWS_GMAIL_TRIAGE_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "gmail_triage",
    args: &["gmail", "+triage", "--help"],
};

pub(crate) const GWS_GMAIL_READ_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "gmail_read",
    args: &["gmail", "+read", "--help"],
};

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
