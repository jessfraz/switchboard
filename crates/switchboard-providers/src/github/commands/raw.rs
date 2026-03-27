use switchboard_core::{ExecutionTarget, PlannedAction, ResolvedNamespace, Result, ToolOutput, ToolRequest};

use crate::{
    cli::{passthrough, CliCommandSpec, CliResponse},
    github::commands::{GH_BASE_CAPABILITY, GH_BINARY},
};

pub(crate) const RAW_READ_COMMAND: CliCommandSpec = CliCommandSpec {
    descriptor: switchboard_core::ToolDescriptor {
        name: "github.cli.read",
        kind: switchboard_core::ToolKind::Read,
        summary: "Run a raw GitHub CLI read command",
        backend: switchboard_core::BackendKind::Cli,
    },
    binary: &GH_BINARY,
    capability: &GH_BASE_CAPABILITY,
    summarize: summarize_raw_read,
    build_args: build_raw_args,
    decode: decode_raw_read,
};

pub(crate) const RAW_WRITE_COMMAND: CliCommandSpec = CliCommandSpec {
    descriptor: switchboard_core::ToolDescriptor {
        name: "github.cli.write",
        kind: switchboard_core::ToolKind::Write,
        summary: "Run a raw GitHub CLI write command",
        backend: switchboard_core::BackendKind::Cli,
    },
    binary: &GH_BINARY,
    capability: &GH_BASE_CAPABILITY,
    summarize: summarize_raw_write,
    build_args: build_raw_args,
    decode: decode_raw_write,
};

fn summarize_raw_read(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    passthrough::summarize_passthrough(namespace, request, "gh")
}

fn summarize_raw_write(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    passthrough::summarize_passthrough(namespace, request, "gh")
}

fn build_raw_args(action: &PlannedAction) -> Result<Vec<String>> {
    passthrough::build_passthrough_args(action)
}

fn decode_raw_read(target: &ExecutionTarget, action: &PlannedAction, response: CliResponse) -> Result<ToolOutput> {
    passthrough::decode_passthrough(target, action, response, "gh")
}

fn decode_raw_write(target: &ExecutionTarget, action: &PlannedAction, response: CliResponse) -> Result<ToolOutput> {
    passthrough::decode_passthrough(target, action, response, "gh")
}
