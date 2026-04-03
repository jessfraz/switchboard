use switchboard_core::{ExecutionTarget, PlannedAction, Result, ToolOutput};

use crate::{
    cli::{
        command::{CliExecutableSpec, CliResponse},
        executor::{CliExecutor, CliInvocation, ProcessCliExecutor},
        locator::{CliLocator, DefaultCliLocator},
        probe::{CliProbe, DefaultCliProbe},
    },
    process_runtime::ProcessContext,
};

pub(crate) trait CliRuntimeMaterializer: Send + Sync {
    fn prepare(&self, target: &ExecutionTarget) -> Result<ProcessContext>;
}

pub(crate) struct CliProviderBackend {
    locator: Box<dyn CliLocator>,
    probe: Box<dyn CliProbe>,
    executor: Box<dyn CliExecutor>,
    materializer: Box<dyn CliRuntimeMaterializer>,
}

impl CliProviderBackend {
    pub(crate) fn new(materializer: Box<dyn CliRuntimeMaterializer>) -> Self {
        Self {
            locator: Box::new(DefaultCliLocator),
            probe: Box::new(DefaultCliProbe::default()),
            executor: Box::new(ProcessCliExecutor),
            materializer,
        }
    }

    pub(crate) fn execute(
        &self,
        target: &ExecutionTarget,
        action: &PlannedAction,
        spec: &CliExecutableSpec,
    ) -> Result<ToolOutput> {
        let program = self.locator.resolve(&spec.binary)?;
        let version = self
            .probe
            .inspect(&spec.binary, &program, &spec.capability, self.executor.as_ref())?;
        let args = spec.args.build_args(action)?;
        let stdio_mode = spec.args.stdio_mode(action)?;
        let runtime = self.materializer.prepare(target)?;
        let output = self.executor.execute(CliInvocation {
            program: program.clone(),
            args,
            runtime,
            stdio_mode,
        })?;

        spec.decode.decode(
            target,
            action,
            CliResponse {
                program,
                version,
                stdout: output.stdout,
                stderr: output.stderr,
            },
        )
    }
}
