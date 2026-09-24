use std::{
    io::{self, Read},
    os::{
        fd::OwnedFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) fn isolate_terminal(command: &mut Command) {
    extern "C" {
        fn setsid() -> i32;
        fn getsid(pid: i32) -> i32;
        fn getpid() -> i32;
    }
    // Closing stdin does not stop tools such as op from prompting through /dev/tty.
    // A new session removes that terminal and retains a private group for cleanup.
    // SAFETY: The pre-exec hook only calls async-signal-safe functions and reads errno.
    unsafe {
        command.pre_exec(|| {
            // Command can be reused, which registers this hook more than once.
            if getsid(0) == getpid() || setsid() != -1 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
}

pub(super) struct Capture(UnixStream);

impl Capture {
    pub(super) fn new() -> io::Result<(Self, Stdio)> {
        let (reader, writer) = UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        Ok((Self(reader), Stdio::from(OwnedFd::from(writer))))
    }

    pub(super) fn drain(&mut self, bytes: &mut Vec<u8>) -> io::Result<()> {
        let mut buffer = [0; 8192];
        // Bound each pass so continuous output cannot starve the other stream
        // or prevent the parent from checking its deadline.
        for _ in 0..64 {
            match self.0.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => bytes.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub(super) fn finish(&mut self, bytes: &mut Vec<u8>, started: Instant, timeout: Duration) -> io::Result<()> {
        loop {
            let previous_length = bytes.len();
            self.drain(bytes)?;
            if bytes.len() == previous_length {
                return Ok(());
            }
            if started.elapsed() >= timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "process output capture timed out",
                ));
            }
        }
    }
}
