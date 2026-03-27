mod calendar;
mod gmail;
mod raw;

use switchboard_core::{Error, Result, ToolArguments};

use crate::cli::{CliBinarySpec, CliCapabilityProbe};
pub(crate) use crate::google::commands::{
    calendar::{CALENDAR_CREATE_COMMAND, CALENDAR_DELETE_COMMAND, CALENDAR_LIST_COMMAND},
    gmail::{MAIL_DRAFT_COMMAND, MAIL_READ_COMMAND, MAIL_SEARCH_COMMAND},
    raw::{RAW_READ_COMMAND, RAW_WRITE_COMMAND},
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

pub(crate) const GWS_BASE_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "gws_help",
    args: &["--help"],
};

pub(crate) const GWS_CALENDAR_INSERT_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "calendar_insert",
    args: &["calendar", "+insert", "--help"],
};

pub(crate) const GWS_CALENDAR_DELETE_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "calendar_delete",
    args: &["calendar", "events", "delete", "--help"],
};

pub(crate) const GWS_GMAIL_TRIAGE_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "gmail_triage",
    args: &["gmail", "+triage", "--help"],
};

pub(crate) const GWS_GMAIL_READ_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "gmail_read",
    args: &["gmail", "+read", "--help"],
};

pub(crate) const GWS_GMAIL_DRAFT_CREATE_CAPABILITY: CliCapabilityProbe = CliCapabilityProbe {
    name: "gmail_draft_create",
    args: &["gmail", "users", "drafts", "create", "--help"],
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

pub(super) fn append_repeatable_values(args: &mut Vec<String>, arguments: &ToolArguments, name: &str) {
    for value in arguments.values(name) {
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
