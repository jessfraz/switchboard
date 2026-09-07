use std::{
    env,
    fs::{File, OpenOptions},
    io::{self, Read},
    os::windows::fs::OpenOptionsExt,
    process::{self, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static CAPTURE_ID: AtomicU64 = AtomicU64::new(0);

pub(super) struct Capture(File);

impl Capture {
    pub(super) fn new() -> io::Result<(Self, Stdio)> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "switchboard-capture-{}-{timestamp}-{}",
            process::id(),
            CAPTURE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        // Windows has no std-only nonblocking anonymous pipes. A private,
        // delete-on-close spool keeps capture bounded without blocked threads.
        const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
        const SHARE_READ_WRITE_DELETE: u32 = 7;
        const ACCESS_READ_WRITE_DELETE: u32 = 0xc001_0000;
        let writer = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .access_mode(ACCESS_READ_WRITE_DELETE)
            .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
            .share_mode(SHARE_READ_WRITE_DELETE)
            .open(&path)?;
        let reader = OpenOptions::new()
            .read(true)
            .share_mode(SHARE_READ_WRITE_DELETE)
            .open(&path)?;
        Ok((Self(reader), Stdio::from(writer)))
    }

    pub(super) fn drain(&mut self, _bytes: &mut Vec<u8>) -> io::Result<()> {
        Ok(())
    }

    pub(super) fn finish(&mut self, bytes: &mut Vec<u8>, started: Instant, timeout: Duration) -> io::Result<()> {
        // Read only the size observed when the owned child exited, even if an
        // unrelated descendant continues writing through an inherited handle.
        let length = self.0.metadata()?.len();
        let mut reader = (&mut self.0).take(length);
        let mut buffer = [0; 8192];
        loop {
            if started.elapsed() >= timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "process output capture timed out",
                ));
            }
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        Ok(())
    }
}
