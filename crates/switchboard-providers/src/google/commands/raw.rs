use switchboard_core::{ExecutionTarget, PlannedAction, ResolvedNamespace, Result, ToolOutput, ToolRequest};

use crate::{
    cli::{passthrough, CliCommandSpec, CliResponse},
    google::commands::{GWS_BASE_CAPABILITY, GWS_BINARY},
};

pub(crate) const RAW_READ_COMMAND: CliCommandSpec = CliCommandSpec {
    descriptor: switchboard_core::ToolDescriptor {
        name: "google.cli.read",
        kind: switchboard_core::ToolKind::Read,
        summary: "Run a raw Google Workspace CLI read command",
        backend: switchboard_core::BackendKind::Cli,
    },
    binary: &GWS_BINARY,
    capability: &GWS_BASE_CAPABILITY,
    summarize: summarize_raw_read,
    build_args: build_raw_args,
    decode: decode_raw_read,
};

pub(crate) const RAW_WRITE_COMMAND: CliCommandSpec = CliCommandSpec {
    descriptor: switchboard_core::ToolDescriptor {
        name: "google.cli.write",
        kind: switchboard_core::ToolKind::Write,
        summary: "Run a raw Google Workspace CLI write command",
        backend: switchboard_core::BackendKind::Cli,
    },
    binary: &GWS_BINARY,
    capability: &GWS_BASE_CAPABILITY,
    summarize: summarize_raw_write,
    build_args: build_raw_args,
    decode: decode_raw_write,
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
