use std::{
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use switchboard_core::{process::output_with_timeout, Error, Result};

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
                let started = Instant::now();
                let output = output_with_timeout(&mut command, Duration::from_secs(60))
                    .map_err(|error| execution_error(&invocation, error, started.elapsed()))?;

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
                    if let Some(error) = provider_failure(&stdout).or_else(|| provider_failure(&stderr)) {
                        return Err(error);
                    }
                    let reason = if !stderr.trim().is_empty() {
                        stderr.trim().to_owned()
                    } else if !stdout.trim().is_empty() {
                        stdout.trim().to_owned()
                    } else {
                        format!("process exited with status {}", output.status)
                    };

                    return Err(Error::ProviderFailed {
                        program: invocation.program.display().to_string(),
                        exit_code: output.status.code(),
                        reason,
                    });
                }

                Ok(CliOutput { stdout, stderr })
            }
            CliStdioMode::Inherit => {
                command.stdin(Stdio::inherit());
                command.stdout(Stdio::inherit());
                command.stderr(Stdio::inherit());

                let started = Instant::now();
                let status = command
                    .status()
                    .map_err(|error| execution_error(&invocation, error, started.elapsed()))?;
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

fn execution_error(invocation: &CliInvocation, error: std::io::Error, elapsed: Duration) -> Error {
    if matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
    ) {
        return Error::Launch(format!("{}: {error}", invocation.program.display()));
    }
    if error.kind() == std::io::ErrorKind::TimedOut {
        return Error::TimedOut {
            program: invocation.program.display().to_string(),
            seconds: elapsed.as_secs(),
        };
    }
    Error::Execution(format!(
        "failed to run {} {}: {error}",
        invocation.program.display(),
        invocation.args.join(" ")
    ))
}

fn provider_failure(text: &str) -> Option<Error> {
    #[derive(serde::Deserialize)]
    struct OAuthError {
        error: String,
        #[serde(default)]
        error_description: String,
    }
    if let Ok(error) = serde_json::from_str::<OAuthError>(text) {
        return match error.error.as_str() {
            "interaction_required" | "consent_required" => Some(Error::BrowserConsentRequired {
                reason: error.error_description,
            }),
            "invalid_grant" | "invalid_token" => Some(Error::AuthenticationRejected {
                reason: error.error_description,
            }),
            _ => None,
        };
    }
    #[derive(serde::Deserialize)]
    struct GitHubError {
        status: String,
        message: String,
    }
    if let Ok(error) = serde_json::from_str::<GitHubError>(text) {
        if error.status == "401" {
            return Some(Error::AuthenticationRejected { reason: error.message });
        }
    }
    #[derive(serde::Deserialize)]
    struct Envelope {
        error: ApiError,
    }
    #[derive(serde::Deserialize)]
    struct ApiError {
        code: u16,
        #[serde(default)]
        message: String,
        retry_after_seconds: Option<u64>,
        #[serde(default)]
        details: Vec<Detail>,
    }
    #[derive(serde::Deserialize)]
    struct Detail {
        #[serde(rename = "@type")]
        kind: String,
        #[serde(rename = "retryDelay")]
        retry_delay: Option<String>,
    }
    let error = serde_json::from_str::<Envelope>(text).ok()?.error;
    if error.code == 401 {
        return Some(Error::AuthenticationRejected { reason: error.message });
    }
    if !matches!(error.code, 429 | 503) {
        return None;
    }
    let retry = error.retry_after_seconds.or_else(|| {
        error.details.iter().find_map(|detail| {
            if detail.kind != "type.googleapis.com/google.rpc.RetryInfo" {
                return None;
            }
            let seconds = detail.retry_delay.as_deref()?.strip_suffix('s')?.parse::<f64>().ok()?;
            (seconds.is_finite() && (0.0..=3600.0).contains(&seconds)).then_some(seconds.ceil() as u64)
        })
    })?;
    Some(Error::RateLimited {
        retry_after_seconds: retry,
        reason: error.message,
    })
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
    fn structured_oauth_failures_distinguish_consent_and_revocation() {
        for (code, consent) in [("consent_required", true), ("invalid_grant", false)] {
            let script = temp_script(&format!(
                r#"printf '%s\n' '{{"error":"{code}","error_description":"fixture blocker"}}' >&2
exit 1"#
            ));
            let result = ProcessCliExecutor.execute(CliInvocation {
                program: script.path().to_path_buf(),
                args: vec!["identity".into()],
                runtime: ProcessContext::new(),
                stdio_mode: CliStdioMode::Capture,
            });
            if consent {
                assert!(matches!(
                    result,
                    Err(switchboard_core::Error::BrowserConsentRequired { .. })
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(switchboard_core::Error::AuthenticationRejected { .. })
                ));
            }
        }
    }

    #[test]
    fn native_github_401_is_authentication_but_403_is_not() {
        for (status, message, authentication) in [
            ("401", "Bad credentials", true),
            ("403", "Resource not accessible by integration", false),
            ("403", "API rate limit exceeded", false),
        ] {
            let script = temp_script(&format!(
                r#"printf '%s\n' '{{"message":"{message}","documentation_url":"https://docs.github.com/rest","status":"{status}"}}'
printf '%s\n' 'gh: {message} (HTTP {status})' >&2
exit 1"#
            ));
            let result = ProcessCliExecutor.execute(CliInvocation {
                program: script.path().to_path_buf(),
                args: vec!["api".into(), "user".into()],
                runtime: ProcessContext::new(),
                stdio_mode: CliStdioMode::Capture,
            });
            if authentication {
                assert!(matches!(
                    result,
                    Err(switchboard_core::Error::AuthenticationRejected { .. })
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(switchboard_core::Error::ProviderFailed { exit_code: Some(1), .. })
                ));
            }
        }
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
