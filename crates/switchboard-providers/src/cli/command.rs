use std::path::PathBuf;

use switchboard_core::{
    ExecutionTarget, PlannedAction, ResolvedNamespace, Result, ToolDescriptor, ToolOutput, ToolRequest,
};

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

pub(crate) struct CliExecutableSpec {
    pub binary: CliBinarySpec,
    pub capability: CliCapabilityProbe,
    pub build_args: fn(&PlannedAction) -> Result<Vec<String>>,
    pub decode: fn(&ExecutionTarget, &PlannedAction, CliResponse) -> Result<ToolOutput>,
}

pub(crate) struct CliCommandSpec {
    pub descriptor: ToolDescriptor,
    pub summarize: fn(&ResolvedNamespace, &ToolRequest) -> Result<String>,
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
