use std::time::Instant;

use switchboard_core::{ExecutionTarget, ExecutionTimings, PlannedAction, Result, ToolOutput};

use crate::{
    cli::{
        command::{CliExecutableSpec, CliResponse},
        executor::{CliExecutor, CliInvocation, CliStdioMode, ProcessCliExecutor},
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
        let args = spec.args.build_args(action)?;
        let stdio_mode = spec.args.stdio_mode(action)?;
        let response = self.execute_raw(target, spec, args, stdio_mode)?;

        spec.decode.decode(target, action, response)
    }

    pub(crate) fn execute_raw(
        &self,
        target: &ExecutionTarget,
        spec: &CliExecutableSpec,
        args: Vec<String>,
        stdio_mode: CliStdioMode,
    ) -> Result<CliResponse> {
        let started = Instant::now();
        let program = self.locator.resolve(&spec.binary)?;
        let locate_us = ExecutionTimings::elapsed_us(started);
        let started = Instant::now();
        let version = self
            .probe
            .inspect(&spec.binary, &program, &spec.capability, self.executor.as_ref())?;
        let probe_us = ExecutionTimings::elapsed_us(started);
        let started = Instant::now();
        let runtime = self.materializer.prepare(target)?;
        let materialize_us = ExecutionTimings::elapsed_us(started);
        let started = Instant::now();
        let output = self.executor.execute(CliInvocation {
            program: program.clone(),
            args,
            runtime,
            stdio_mode,
        })?;

        let provider_us = ExecutionTimings::elapsed_us(started);
        Ok(CliResponse {
            timings: ExecutionTimings {
                locate_us: Some(locate_us),
                probe_us: Some(probe_us),
                materialize_us: Some(materialize_us),
                provider_us: Some(provider_us),
                ..ExecutionTimings::default()
            },
            program,
            version,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}
