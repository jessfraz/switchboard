use std::path::PathBuf;

use switchboard_core::{
    ExecutionTarget, PlannedAction, ResolvedNamespace, Result, ToolDescriptor, ToolOutput, ToolRequest,
};

#[derive(Clone, Copy)]
pub(crate) struct CliBinarySpec {
    pub program: &'static str,
    pub env_override: Option<&'static str>,
    pub version_args: &'static [&'static str],
}

#[derive(Clone, Copy)]
pub(crate) struct CliCapabilityProbe {
    pub name: &'static str,
    pub args: &'static [&'static str],
}

pub(crate) struct CliCommandSpec {
    pub descriptor: ToolDescriptor,
    pub binary: &'static CliBinarySpec,
    pub capability: &'static CliCapabilityProbe,
    pub summarize: fn(&ResolvedNamespace, &ToolRequest) -> Result<String>,
    pub build_args: fn(&PlannedAction) -> Result<Vec<String>>,
    pub decode: fn(&ExecutionTarget, &PlannedAction, CliResponse) -> Result<ToolOutput>,
}

impl CliCommandSpec {
    pub(crate) fn name(&self) -> &'static str {
        self.descriptor.name
    }
}

pub(crate) struct CliResponse {
    pub program: PathBuf,
    pub version: String,
    pub stdout: String,
    pub stderr: String,
}
