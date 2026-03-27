mod issues;
mod notifications;
mod planned;
mod pull_requests;
mod raw;

use switchboard_core::{Error, Result, ToolArguments};

use crate::cli::CliCommandHandler;
pub(crate) use crate::github::commands::{
    issues::ISSUE_READ_HANDLER,
    notifications::NOTIFICATIONS_HANDLER,
    planned::{ISSUE_COMMENT_HANDLER, PULL_REQUEST_COMMENT_HANDLER, REPOSITORY_SEARCH_HANDLER},
    pull_requests::{PULL_REQUEST_READ_HANDLER, PULL_REQUEST_SEARCH_HANDLER},
    raw::{RAW_READ_HANDLER, RAW_WRITE_HANDLER},
};

pub(crate) const HANDLERS: &[CliCommandHandler] = &[
    NOTIFICATIONS_HANDLER,
    PULL_REQUEST_SEARCH_HANDLER,
    PULL_REQUEST_READ_HANDLER,
    PULL_REQUEST_COMMENT_HANDLER,
    ISSUE_READ_HANDLER,
    ISSUE_COMMENT_HANDLER,
    REPOSITORY_SEARCH_HANDLER,
    RAW_READ_HANDLER,
    RAW_WRITE_HANDLER,
];

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
