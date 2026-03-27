use switchboard_core::{Error, ResolvedNamespace, Result, ToolRequest};

use crate::cli::CliCommandHandler;

pub(crate) const MAIL_SEND_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "mail_send",
    summarize: summarize_mail_send,
    build_args: None,
    decode: None,
};

pub(crate) const DRIVE_SEARCH_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "drive_search",
    summarize: summarize_drive_search,
    build_args: None,
    decode: None,
};

fn summarize_mail_send(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let to = request
        .args
        .value("to")
        .ok_or_else(|| Error::InvalidArguments(format!("missing required argument --to for {}", request.tool)))?;
    Ok(format!("Draft email to {to} from {}", namespace.id))
}

fn summarize_drive_search(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let query = request
        .args
        .value("query")
        .ok_or_else(|| Error::InvalidArguments(format!("missing required argument --query for {}", request.tool)))?;
    Ok(format!("Search Google Drive in {} for {query:?}", namespace.id))
}
