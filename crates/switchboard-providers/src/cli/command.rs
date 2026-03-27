use std::path::PathBuf;

use switchboard_core::{
    ExecutionTarget, PlannedAction, ResolvedNamespace, Result, ToolDescriptor, ToolOutput, ToolRequest,
};

use crate::cli::{
    declarative::{CliArgsTemplate, CliJsonProjection, CliSummaryTemplate},
    passthrough,
};

pub(crate) type CliSummarizeFn = fn(&ResolvedNamespace, &ToolRequest) -> Result<String>;
pub(crate) type CliBuildArgsFn = fn(&PlannedAction) -> Result<Vec<String>>;
pub(crate) type CliDecodeFn = fn(&ExecutionTarget, &PlannedAction, CliResponse) -> Result<ToolOutput>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CliBinarySpec {
    pub program: String,
    pub env_override: Option<String>,
    pub version_args: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CliCapabilityProbe {
    pub id: String,
    pub args: Vec<String>,
}

pub(crate) enum CliSummarizeStrategy {
    Handler(CliSummarizeFn),
    Template(CliSummaryTemplate),
    RawInventory { program: String, prefix: Vec<String> },
}

impl CliSummarizeStrategy {
    pub(crate) fn summarize(&self, namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
        match self {
            Self::Handler(handler) => handler(namespace, request),
            Self::Template(template) => template.render(namespace, request),
            Self::RawInventory { program, prefix } => {
                passthrough::summarize_prefixed_passthrough(namespace, request, program, prefix)
            }
        }
    }
}

pub(crate) enum CliArgsStrategy {
    Handler(CliBuildArgsFn),
    Template(CliArgsTemplate),
    RawInventory { prefix: Vec<String> },
}

impl CliArgsStrategy {
    pub(crate) fn build_args(&self, action: &PlannedAction) -> Result<Vec<String>> {
        match self {
            Self::Handler(handler) => handler(action),
            Self::Template(template) => template.build_args(action),
            Self::RawInventory { prefix } => passthrough::build_prefixed_passthrough_args(action, prefix),
        }
    }
}

pub(crate) enum CliDecodeStrategy {
    Handler(CliDecodeFn),
    JsonProjection(CliJsonProjection),
    RawInventory { program: String, prefix: Vec<String> },
}

impl CliDecodeStrategy {
    pub(crate) fn decode(
        &self,
        target: &ExecutionTarget,
        action: &PlannedAction,
        response: CliResponse,
    ) -> Result<ToolOutput> {
        match self {
            Self::Handler(handler) => handler(target, action, response),
            Self::JsonProjection(projection) => projection.decode(target, action, response),
            Self::RawInventory { program, prefix } => {
                passthrough::decode_prefixed_passthrough(target, action, response, program, prefix)
            }
        }
    }
}

pub(crate) struct CliExecutableSpec {
    pub binary: CliBinarySpec,
    pub capability: CliCapabilityProbe,
    pub args: CliArgsStrategy,
    pub decode: CliDecodeStrategy,
}

pub(crate) struct CliCommandSpec {
    pub descriptor: ToolDescriptor,
    pub summarize: CliSummarizeStrategy,
    pub executable: Option<CliExecutableSpec>,
}

impl CliCommandSpec {
    pub(crate) fn name(&self) -> &str {
        self.descriptor.name.as_str()
    }
}

pub(crate) struct CliResponse {
    pub program: PathBuf,
    pub version: String,
    pub stdout: String,
    pub stderr: String,
}
