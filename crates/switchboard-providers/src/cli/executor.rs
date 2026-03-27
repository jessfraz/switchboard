use std::{path::PathBuf, process::Command};

use switchboard_core::{Error, Result};

use crate::process_runtime::ProcessContext;

pub(crate) trait CliExecutor: Send + Sync {
    fn execute(&self, invocation: CliInvocation) -> Result<CliOutput>;
}

pub(crate) struct CliInvocation {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub runtime: ProcessContext,
}

pub(crate) struct CliOutput {
    pub stdout: String,
    pub stderr: String,
}

pub(crate) struct ProcessCliExecutor;

impl CliExecutor for ProcessCliExecutor {
    fn execute(&self, invocation: CliInvocation) -> Result<CliOutput> {
        let mut command = Command::new(&invocation.program);
        command.args(&invocation.args);
        invocation.runtime.apply_to_command(&mut command);

        let output = command.output().map_err(|error| {
            Error::Execution(format!(
                "failed to run {} {}: {error}",
                invocation.program.display(),
                invocation.args.join(" ")
            ))
        })?;

        let stdout = String::from_utf8(output.stdout).map_err(|error| {
            Error::Execution(format!(
                "{} produced non-UTF-8 stdout: {error}",
                invocation.program.display()
            ))
        })?;
        let stderr = String::from_utf8(output.stderr).map_err(|error| {
            Error::Execution(format!(
                "{} produced non-UTF-8 stderr: {error}",
                invocation.program.display()
            ))
        })?;

        if !output.status.success() {
            let reason = if !stderr.trim().is_empty() {
                stderr.trim().to_owned()
            } else if !stdout.trim().is_empty() {
                stdout.trim().to_owned()
            } else {
                format!("process exited with status {}", output.status)
            };

            return Err(Error::Execution(format!(
                "{} {} failed: {reason}",
                invocation.program.display(),
                invocation.args.join(" ")
            )));
        }

        Ok(CliOutput { stdout, stderr })
    }
}
