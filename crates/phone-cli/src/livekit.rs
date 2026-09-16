//! The LiveKit SDK lives in a supervised worker. Its wire format is private to
//! this adapter; callers only see the backend-neutral calling contract.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::domain::{ActiveCall, AuthorizedCall, CallBackend, CallEvent, CallRequest};
use crate::error::CallError;

const MAX_EVENT_BYTES: u64 = 1_048_576;

pub struct LiveKitBackend {
    command: std::path::PathBuf,
    arguments: Vec<std::ffi::OsString>,
    settings: Vec<(&'static str, String)>,
}

impl LiveKitBackend {
    pub fn new(config: &Config) -> Self {
        let settings = [
            ("LIVEKIT_URL", &config.livekit.url),
            ("LIVEKIT_SIP_TRUNK_ID", &config.livekit.sip_trunk_id),
            ("LIVEKIT_PHONE_STT_MODEL", &config.livekit.stt_model),
            ("LIVEKIT_PHONE_LLM_MODEL", &config.livekit.llm_model),
            ("LIVEKIT_PHONE_TTS_MODEL", &config.livekit.tts_model),
            ("LIVEKIT_PHONE_VOICE", &config.livekit.voice),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.as_ref().map(|value| (name, value.clone())))
        .collect();
        if let Some(command) = &config.worker_command {
            Self {
                command: command.clone(),
                arguments: config.worker_args.iter().map(Into::into).collect(),
                settings,
            }
        } else {
            Self {
                command: "uv".into(),
                arguments: vec![
                    "run".into(),
                    "--project".into(),
                    config.worker_project().into_os_string(),
                    "--frozen".into(),
                    "--no-sync".into(),
                    "livekit-phone-worker".into(),
                ],
                settings,
            }
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkerCommand<'a> {
    Start {
        protocol_version: u32,
        #[serde(flatten)]
        request: &'a CallRequest,
    },
    Cancel {
        protocol_version: u32,
    },
}

#[derive(Deserialize)]
struct WorkerEvent {
    protocol_version: u32,
    #[serde(flatten)]
    event: CallEvent,
}

impl CallBackend for LiveKitBackend {
    fn start(&self, call: &AuthorizedCall) -> Result<Box<dyn ActiveCall>, CallError> {
        let mut command = Command::new(&self.command);
        command.args(&self.arguments).env_clear();
        // Do not pass transcript identities, other provider credentials, or the
        // caller's whole environment into a model-connected process.
        for name in [
            "PATH",
            "HOME",
            "TMPDIR",
            "TMP",
            "TEMP",
            "SYSTEMROOT",
            "UV_CACHE_DIR",
            "SSL_CERT_FILE",
            "LIVEKIT_URL",
            "LIVEKIT_SIP_TRUNK_ID",
            "LIVEKIT_PHONE_STT_MODEL",
            "LIVEKIT_PHONE_LLM_MODEL",
            "LIVEKIT_PHONE_TTS_MODEL",
            "LIVEKIT_PHONE_VOICE",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        for (name, value) in &self.settings {
            command.env(name, value);
        }
        for (generic, backend) in [
            ("PHONE_API_KEY", "LIVEKIT_API_KEY"),
            ("PHONE_API_SECRET", "LIVEKIT_API_SECRET"),
        ] {
            if let Some(value) = std::env::var_os(generic).or_else(|| std::env::var_os(backend)) {
                command.env(backend, value);
            }
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|_| CallError::BackendStart)?;
        let mut stdin = child.stdin.take().ok_or(CallError::BackendStart)?;
        let stdout = child.stdout.take().ok_or(CallError::BackendStart)?;
        let (sender, receiver) = mpsc::sync_channel(16);
        let (commands, command_receiver) = mpsc::sync_channel::<Vec<u8>>(2);
        // A worker that never reads stdin must not defeat the caller's deadline.
        // The supervisor remains responsive while a pipe write is blocked.
        std::thread::spawn(move || {
            for line in command_receiver {
                if stdin.write_all(&line).and_then(|()| stdin.flush()).is_err() {
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                let result = match (&mut reader).take(MAX_EVENT_BYTES + 1).read_until(b'\n', &mut line) {
                    Ok(0) => Err(CallError::BackendExited),
                    Ok(length) if length > MAX_EVENT_BYTES as usize => {
                        Err(CallError::Protocol("event exceeds size limit"))
                    }
                    Ok(_) => decode_event(&line),
                    Err(_) => Err(CallError::Protocol("could not read event")),
                };
                let failed = result.is_err();
                if sender.send(result).is_err() || failed {
                    break;
                }
            }
        });
        let mut process = WorkerProcess {
            child,
            commands,
            receiver,
            cancelled: false,
        };
        process.send(&WorkerCommand::Start {
            protocol_version: 1,
            request: call.request(),
        })?;
        Ok(Box::new(process))
    }
}

fn decode_event(line: &[u8]) -> Result<CallEvent, CallError> {
    let envelope: WorkerEvent = serde_json::from_slice(line).map_err(|_| CallError::Protocol("malformed event"))?;
    if envelope.protocol_version != 1 {
        return Err(CallError::Protocol("unsupported protocol version"));
    }
    Ok(envelope.event)
}

struct WorkerProcess {
    child: Child,
    commands: SyncSender<Vec<u8>>,
    receiver: Receiver<Result<CallEvent, CallError>>,
    cancelled: bool,
}

impl WorkerProcess {
    fn send(&mut self, command: &WorkerCommand<'_>) -> Result<(), CallError> {
        let mut line = serde_json::to_vec(command).map_err(|_| CallError::Protocol("could not encode command"))?;
        line.push(b'\n');
        self.commands.try_send(line).map_err(|_| CallError::Cancellation)
    }
}

impl ActiveCall for WorkerProcess {
    fn next_event(&mut self, timeout: Duration) -> Result<Option<CallEvent>, CallError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(event) => event.map(Some),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(CallError::BackendExited),
        }
    }

    fn cancel(&mut self) -> Result<(), CallError> {
        if !self.cancelled {
            self.send(&WorkerCommand::Cancel { protocol_version: 1 })?;
            self.cancelled = true;
        }
        Ok(())
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Ok(pid) = i32::try_from(self.child.id()) {
            // The child is the leader of its own group. Kill its worker and
            // launcher together so abandoning a call cannot leave local audio
            // or credential-bearing descendants alive. This cannot establish
            // whether the remote telephone call has actually ended.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::domain::{CallBackend, CallEvent, CallId, CallRequest, TerminationReason};
    use crate::livekit::LiveKitBackend;

    #[cfg(unix)]
    #[test]
    fn real_child_protocol_keeps_request_off_argv_and_confirms_cancellation() {
        let script = r#"
read start
case "$start" in *'"type":"start"'*'"task":"private task"'*) ;; *) exit 12 ;; esac
printf '%s\n' '{"protocol_version":1,"type":"ready"}'
read cancel
case "$cancel" in *'"type":"cancel"'*) ;; *) exit 13 ;; esac
printf '%s\n' '{"protocol_version":1,"type":"completed","reason":"cancelled","remote_hangup_confirmed":true}'
"#;
        let backend = LiveKitBackend {
            command: "/bin/sh".into(),
            arguments: vec!["-c".into(), script.into()],
            settings: Vec::new(),
        };
        let authorized = CallRequest {
            call_id: CallId::new(),
            destination: "+12125550100".into(),
            task: "private task".into(),
            caller_name: "Test".into(),
            max_duration_seconds: 30,
        }
        .authorize(true)
        .expect("test fixture should succeed");
        let mut call = backend.start(&authorized).expect("test fixture should succeed");
        assert!(matches!(
            call.next_event(Duration::from_secs(2))
                .expect("test fixture should succeed"),
            Some(CallEvent::Ready)
        ));
        call.cancel().expect("test fixture should succeed");
        assert!(
            matches!(call.next_event(Duration::from_secs(2)).expect("test fixture should succeed"), Some(CallEvent::Completed(outcome)) if outcome.reason == TerminationReason::Cancelled && outcome.remote_hangup_confirmed)
        );
    }

    #[test]
    fn malformed_or_new_protocol_is_not_treated_as_call_success() {
        assert!(crate::livekit::decode_event(b"not json").is_err());
        assert!(crate::livekit::decode_event(br#"{"protocol_version":2,"type":"ready"}"#).is_err());
    }
}
