mod notifications;

use switchboard_core::{Error, Result, ToolArguments};

use crate::cli::CliCommandHandler;
pub(crate) use crate::github::commands::notifications::NOTIFICATIONS_HANDLER;

pub(crate) const HANDLERS: &[CliCommandHandler] = &[NOTIFICATIONS_HANDLER];

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

fn parse_bool(name: &str, value: &str) -> Result<bool> {
    match value {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(Error::InvalidArguments(format!(
            "--{name} expects a boolean value, got {value:?}"
        ))),
    }
}
