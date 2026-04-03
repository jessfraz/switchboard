use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

use switchboard_core::{Error, Result};

use crate::process_runtime::ProcessContext;

pub(crate) trait CliExecutor: Send + Sync {
    fn execute(&self, invocation: CliInvocation) -> Result<CliOutput>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CliStdioMode {
    Capture,
    Inherit,
}

pub(crate) struct CliInvocation {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub runtime: ProcessContext,
    pub stdio_mode: CliStdioMode,
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

        match invocation.stdio_mode {
            CliStdioMode::Capture => {
                let output = command.output().map_err(|error| execution_error(&invocation, error))?;

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
            CliStdioMode::Inherit => {
                command.stdin(Stdio::inherit());
                command.stdout(Stdio::inherit());
                command.stderr(Stdio::inherit());

                let status = command.status().map_err(|error| execution_error(&invocation, error))?;
                if !status.success() {
                    return Err(Error::Execution(format!(
                        "{} {} failed: process exited with status {}",
                        invocation.program.display(),
                        invocation.args.join(" "),
                        status
                    )));
                }

                Ok(CliOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        }
    }
}

fn execution_error(invocation: &CliInvocation, error: std::io::Error) -> Error {
    Error::Execution(format!(
        "failed to run {} {}: {error}",
        invocation.program.display(),
        invocation.args.join(" ")
    ))
}

#[cfg(test)]
mod tests {
    use crate::{
        cli::executor::{CliExecutor, CliInvocation, CliStdioMode, ProcessCliExecutor},
        process_runtime::ProcessContext,
        test_support::TempScript,
    };

    fn temp_script(body: &str) -> TempScript {
        TempScript::new(
            "cli-executor-test.sh",
            &format!(
                r#"#!/bin/sh
cat >> "$(dirname "$0")/env.txt" <<EOF
ARGV=$*
EOF
{body}
"#
            ),
        )
    }

    #[test]
    fn capture_mode_collects_stdout() {
        let script = temp_script("echo captured-output");
        let executor = ProcessCliExecutor;
        let output = executor
            .execute(CliInvocation {
                program: script.path().to_path_buf(),
                args: vec!["alpha".into(), "beta".into()],
                runtime: ProcessContext::new(),
                stdio_mode: CliStdioMode::Capture,
            })
            .expect("capture mode should succeed");

        assert_eq!(output.stdout, "captured-output\n");
        assert!(output.stderr.is_empty());
        assert!(script.capture_contents().contains("ARGV=alpha beta"));
    }

    #[test]
    fn inherit_mode_leaves_output_streamed() {
        let script = temp_script("echo inherited-output");
        let executor = ProcessCliExecutor;
        let output = executor
            .execute(CliInvocation {
                program: script.path().to_path_buf(),
                args: vec!["auth".into(), "login".into()],
                runtime: ProcessContext::new(),
                stdio_mode: CliStdioMode::Inherit,
            })
            .expect("inherit mode should succeed");

        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        assert!(script.capture_contents().contains("ARGV=auth login"));
    }
}
