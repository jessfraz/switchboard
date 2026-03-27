mod calendar;
mod gmail;

use switchboard_core::{Error, Result, ToolArguments};

use crate::cli::CliCommandHandler;
pub(crate) use crate::google::commands::{
    calendar::{CALENDAR_CREATE_HANDLER, CALENDAR_DELETE_HANDLER, CALENDAR_LIST_HANDLER},
    gmail::{MAIL_DRAFT_HANDLER, MAIL_READ_HANDLER, MAIL_SEARCH_HANDLER},
};

pub(crate) const HANDLERS: &[CliCommandHandler] = &[
    CALENDAR_LIST_HANDLER,
    CALENDAR_CREATE_HANDLER,
    CALENDAR_DELETE_HANDLER,
    MAIL_SEARCH_HANDLER,
    MAIL_READ_HANDLER,
    MAIL_DRAFT_HANDLER,
];

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
