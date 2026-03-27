use switchboard_core::{ExecutionTarget, PlannedAction, ResolvedNamespace, Result, ToolOutput, ToolRequest};

use crate::cli::{passthrough, CliCommandHandler, CliResponse};

pub(crate) const RAW_READ_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "raw_read",
    summarize: summarize_raw_read,
    build_args: Some(build_raw_args),
    decode: Some(decode_raw_read),
};

pub(crate) const RAW_WRITE_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "raw_write",
    summarize: summarize_raw_write,
    build_args: Some(build_raw_args),
    decode: Some(decode_raw_write),
};

fn summarize_raw_read(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    passthrough::summarize_passthrough(namespace, request, "gws")
}

fn summarize_raw_write(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    passthrough::summarize_passthrough(namespace, request, "gws")
}

fn build_raw_args(action: &PlannedAction) -> Result<Vec<String>> {
    passthrough::build_passthrough_args(action)
}

fn decode_raw_read(target: &ExecutionTarget, action: &PlannedAction, response: CliResponse) -> Result<ToolOutput> {
    passthrough::decode_passthrough(target, action, response, "gws")
}

fn decode_raw_write(target: &ExecutionTarget, action: &PlannedAction, response: CliResponse) -> Result<ToolOutput> {
    passthrough::decode_passthrough(target, action, response, "gws")
}
