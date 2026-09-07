//! Bounded, noninteractive child-process capture.

use std::{
    io,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
#[path = "process/unix.rs"]
mod capture;
#[cfg(windows)]
#[path = "process/windows.rs"]
mod capture;

/// Capture a child with closed stdin and a finite deadline.
///
/// On Unix, a running child and descendants in its process group are terminated on
/// failure. On Windows only the direct child is terminated. Output handles held
/// by descendants cannot keep capture alive after the child exits.
pub fn output_with_timeout(command: &mut Command, timeout: Duration) -> io::Result<Output> {
    if timeout.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process timeout must be positive",
        ));
    }
    let started = Instant::now();
    let (mut stdout, child_stdout) = capture::Capture::new()?;
    let (mut stderr, child_stderr) = capture::Capture::new()?;
    command.stdin(Stdio::null()).stdout(child_stdout).stderr(child_stderr);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // A private group makes timeout cleanup include wrapper subprocesses
        // without signalling the caller's shell or other Switchboard operations.
        command.process_group(0);
    }
    let mut child = OwnedChild {
        process: command.spawn()?,
        reaped: false,
    };
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    loop {
        stdout.drain(&mut stdout_bytes)?;
        stderr.drain(&mut stderr_bytes)?;
        if let Some(status) = child.process.try_wait()? {
            // try_wait reaps the child. Its PID may now be reused, so no later
            // output-read error may signal the old process-group identifier.
            child.reaped = true;
            // Final reads collect data already written, without waiting for EOF
            // from a descendant that inherited the child's output handles.
            stdout.finish(&mut stdout_bytes, started, timeout)?;
            stderr.finish(&mut stderr_bytes, started, timeout)?;
            return Ok(Output {
                status,
                stdout: stdout_bytes,
                stderr: stderr_bytes,
            });
        }
        if started.elapsed() >= timeout {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("process timed out after {} seconds", timeout.as_secs_f64()),
            ));
        }
        thread::sleep(Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())));
    }
}

struct OwnedChild {
    process: Child,
    reaped: bool,
}

#[cfg(unix)]
fn kill_owned_process_group(child: &Child) {
    extern "C" {
        #[link_name = "kill"]
        fn signal_process(pid: i32, signal: i32) -> i32;
    }
    if let Ok(pid) = i32::try_from(child.id()) {
        if pid > 0 {
            // SAFETY: POSIX kill has no pointer arguments. process_group(0) gave
            // this child a private group with its PID. Negative PID targets only
            // that group; SIGKILL is 9 on the supported Unix platforms.
            unsafe {
                signal_process(-pid, 9);
            }
        }
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        // Child does not reap itself on drop. Cover read errors and deadlines as
        // well as successful exit, without leaving a blocked child behind.
        if !self.reaped {
            #[cfg(unix)]
            kill_owned_process_group(&self.process);
            let _ = self.process.kill();
        }
        let _ = self.process.wait();
    }
}

#[cfg(test)]
mod tests {
    use crate::process::output_with_timeout;
    use std::{
        io::ErrorKind,
        process::Command,
        time::{Duration, Instant},
    };

    #[cfg(unix)]
    #[test]
    fn captures_stdout_and_stderr_without_filling_either_pipe() {
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "i=0; while [ $i -lt 10000 ]; do printf 'stdout-line\\n'; printf 'stderr-line\\n' >&2; i=$((i + 1)); done",
        ]);
        let output = output_with_timeout(&mut command, Duration::from_secs(10)).expect("capture succeeds");
        assert!(output.status.success());
        assert_eq!(output.stdout, "stdout-line\n".repeat(10000).as_bytes());
        assert_eq!(output.stderr, "stderr-line\n".repeat(10000).as_bytes());
    }

    #[cfg(unix)]
    #[test]
    fn times_out_a_running_child_and_does_not_wait_for_inherited_output() {
        let started = Instant::now();
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 2 & wait"]);
        let error = output_with_timeout(&mut command, Duration::from_millis(30)).expect_err("deadline expires");
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn timed_out_wrapper_cannot_continue_work_in_a_descendant() {
        let marker = std::env::temp_dir().join(format!(
            "switchboard-process-descendant-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        let mut command = Command::new("sh");
        command.args(["-c", "(sleep 0.3; printf orphan > \"$1\") & wait", "sh"]);
        command.arg(&marker);
        let error = output_with_timeout(&mut command, Duration::from_millis(30)).expect_err("deadline expires");
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        std::thread::sleep(Duration::from_millis(600));
        let descendant_completed = marker.exists();
        let _ = std::fs::remove_file(&marker);
        assert!(
            !descendant_completed,
            "a timed-out wrapper's descendant must not continue its work"
        );
    }

    #[cfg(unix)]
    #[test]
    fn completed_child_does_not_wait_for_descendant_output_handles() {
        let started = Instant::now();
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 2 & printf done"]);
        let output = output_with_timeout(&mut command, Duration::from_secs(1)).expect("child succeeds");
        assert_eq!(output.stdout, b"done");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn flooding_stdout_does_not_starve_deadline_or_stderr() {
        let started = Instant::now();
        let mut command = Command::new("sh");
        command.args(["-c", "while :; do printf 'continuous output\\n'; done"]);
        let error = output_with_timeout(&mut command, Duration::from_millis(30)).expect_err("deadline expires");
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
    #[cfg(windows)]
    #[test]
    fn windows_captures_actual_stdout_and_stderr() {
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/C", "echo captured-stdout&echo captured-stderr 1>&2"]);
        let output = output_with_timeout(&mut command, Duration::from_secs(5)).expect("Windows capture succeeds");
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).expect("stdout is UTF-8").trim(),
            "captured-stdout"
        );
        assert_eq!(
            String::from_utf8(output.stderr).expect("stderr is UTF-8").trim(),
            "captured-stderr"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_terminates_the_direct_child_at_its_deadline() {
        let started = Instant::now();
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Seconds 5"]);
        let error = output_with_timeout(&mut command, Duration::from_millis(100)).expect_err("deadline expires");
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
