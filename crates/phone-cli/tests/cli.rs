#![cfg(unix)]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use phone_cli::domain::{CallId, CallRequest};
use phone_cli::journal::{read, Record};

struct Fixture {
    _temporary: tempfile::TempDir,
    config: PathBuf,
    root: PathBuf,
    identity: age::x25519::Identity,
}

impl Fixture {
    fn new(script: &str) -> Self {
        let temporary = tempfile::tempdir().expect("test fixture should succeed");
        let directory = temporary.path().canonicalize().expect("test fixture should succeed");
        let root = directory.join("calls");
        let script_path = directory.join("worker.sh");
        std::fs::write(&script_path, script).expect("test fixture should succeed");
        let identity = age::x25519::Identity::generate();
        let config = directory.join("config.toml");
        std::fs::write(
            &config,
            format!(
                "transcript_recipient = {:?}\nstate_dir = {:?}\nworker_command = '/bin/sh'\nworker_args = [{:?}]\n[livekit]\nurl = 'wss://test.livekit.cloud'\nsip_trunk_id = 'test-trunk'\n",
                identity.to_public().to_string(), root, script_path
            ),
        ).expect("test fixture should succeed");
        Self {
            _temporary: temporary,
            config,
            root,
            identity,
        }
    }

    fn run(&self, id: &CallId, approve: bool) -> Output {
        let request = CallRequest {
            call_id: id.clone(),
            destination: "+12125550100".into(),
            task: "Private opening hours request".into(),
            caller_name: "Test Caller".into(),
            max_duration_seconds: 30,
        };
        let mut command = Command::new(env!("CARGO_BIN_EXE_phone"));
        command
            .env_remove("PHONE_CONFIG")
            .env_remove("PHONE_STATE_DIR")
            .env_remove("PHONE_SUPERVISOR_PID")
            .args(["--json", "--config"])
            .arg(&self.config)
            .args(["run", "--request-stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if approve {
            command.arg("--approve");
        }
        let mut child = command.spawn().expect("test fixture should succeed");
        let bytes = serde_json::to_vec(&request).expect("test fixture should succeed");
        let mut stdin = child.stdin.take().expect("test fixture should succeed");
        let _ = stdin.write_all(&bytes);
        drop(stdin);
        child.wait_with_output().expect("test fixture should succeed")
    }
}

#[test]
fn completed_call_keeps_sensitive_text_encrypted_and_refuses_duplicate_id() {
    let fixture = Fixture::new(
        r#"
read request
printf '%s\n' '{"protocol_version":1,"type":"ready"}'
printf '%s\n' '{"protocol_version":1,"type":"dialing"}'
printf '%s\n' '{"protocol_version":1,"type":"connected"}'
printf '%s\n' '{"protocol_version":1,"type":"transcript","speaker":"recipient","text":"Private business answer","timestamp_ms":123,"interrupted":false}'
printf '%s\n' '{"protocol_version":1,"type":"completed","reason":"completed","remote_hangup_confirmed":true,"summary":"Private answer summary"}'
"#,
    );
    let id = CallId::new();
    let output = fixture.run(&id, true);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let text = String::from_utf8(output.stdout).expect("test fixture should succeed");
    assert!(!text.contains("Private"));
    assert!(!text.contains("12125550100"));
    assert!(text.contains("\"remote_hangup_confirmed\":true"));
    let records = read(&fixture.root, &id, &fixture.identity).expect("test fixture should succeed");
    assert!(matches!(
        records.first().map(|record| &record.record),
        Some(Record::Intent(_))
    ));
    assert!(
        matches!(records.last().map(|record| &record.record), Some(Record::Outcome(outcome)) if outcome.remote_hangup_confirmed)
    );
    let second = fixture.run(&id, true);
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("refusing to dial again"));
}

#[test]
fn malformed_worker_output_preserves_unknown_remote_state_without_echoing_it() {
    let fixture = Fixture::new("read request\nprintf '%s\\n' 'secret credential malformed event'\n");
    let id = CallId::new();
    let output = fixture.run(&id, true);
    assert!(!output.status.success());
    let text = String::from_utf8(output.stdout).expect("test fixture should succeed");
    assert!(text.contains("\"status\":\"failed\""));
    assert!(text.contains("\"remote_hangup_confirmed\":false"));
    assert!(!text.contains("credential"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("credential"));
    assert!(read(&fixture.root, &id, &fixture.identity).is_ok());
}

#[test]
fn missing_approval_does_not_create_call_state() {
    let fixture = Fixture::new("exit 98\n");
    let output = fixture.run(&CallId::new(), false);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("explicit approval"));
    assert!(!fixture.root.exists());
}

#[test]
fn approval_requested_mid_call_cancels_and_retains_that_reason() {
    let fixture = Fixture::new(
        r#"
read request
printf '%s\n' '{"protocol_version":1,"type":"ready"}'
printf '%s\n' '{"protocol_version":1,"type":"approval_required","reason":"A purchase is required"}'
read cancel
case "$cancel" in *'"type":"cancel"'*) ;; *) exit 19 ;; esac
printf '%s\n' '{"protocol_version":1,"type":"completed","reason":"cancelled","remote_hangup_confirmed":true}'
"#,
    );
    let output = fixture.run(&CallId::new(), true);
    assert!(!output.status.success());
    let text = String::from_utf8(output.stdout).expect("test fixture should succeed");
    assert!(text.contains("\"status\":\"approval_required\""));
    assert!(text.contains("\"remote_hangup_confirmed\":true"));
}

#[test]
fn losing_the_supervisor_cancels_the_real_cli_worker() {
    use std::time::{Duration, Instant};

    let fixture = Fixture::new("");
    let directory = fixture.config.parent().expect("fixture parent");
    let ready = directory.join("ready");
    let phone_pid = directory.join("phone.pid");
    let script = format!(
        r#"
read request
printf '%s\n' '{{"protocol_version":1,"type":"ready"}}'
printf '%s\n' '{{"protocol_version":1,"type":"dialing"}}'
printf '%s\n' '{{"protocol_version":1,"type":"connected"}}'
printf ready > '{}'
read cancel
case "$cancel" in *'"type":"cancel"'*) ;; *) exit 19 ;; esac
printf '%s\n' '{{"protocol_version":1,"type":"completed","reason":"cancelled","remote_hangup_confirmed":true}}'
"#,
        ready.display()
    );
    std::fs::write(directory.join("worker.sh"), script).expect("worker fixture");
    let id = CallId::new();
    let request = CallRequest {
        call_id: id.clone(),
        destination: "+12125550100".into(),
        task: "Test call".into(),
        caller_name: "Test".into(),
        max_duration_seconds: 30,
    };
    let input = directory.join("request.json");
    std::fs::write(&input, serde_json::to_vec(&request).expect("request JSON")).expect("synthetic request fixture");
    let mut supervisor = Command::new("/bin/sh")
        .args([
            "-c",
            r#"PHONE_SUPERVISOR_PID=$$ "$1" --config "$2" --json run --approve --request-stdin < "$3" &
printf '%s' "$!" > "$4"
wait"#,
            "phone-test-supervisor",
        ])
        .arg(env!("CARGO_BIN_EXE_phone"))
        .arg(&fixture.config)
        .arg(input)
        .arg(&phone_pid)
        .env_remove("PHONE_CONFIG")
        .env_remove("PHONE_STATE_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("supervisor fixture");
    let ready_deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < ready_deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    supervisor.kill().expect("kill supervisor");
    supervisor.wait().expect("reap supervisor");
    let was_ready = ready.exists();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut confirmed = false;
    while Instant::now() < deadline {
        if let Ok(records) = read(&fixture.root, &id, &fixture.identity) {
            if matches!(records.last().map(|record| &record.record), Some(Record::Outcome(outcome)) if outcome.reason == phone_cli::domain::TerminationReason::Cancelled && outcome.remote_hangup_confirmed)
            {
                confirmed = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !confirmed {
        if let Ok(pid) = std::fs::read_to_string(phone_pid).unwrap_or_default().parse::<i32>() {
            // Test cleanup only: the CLI's handler cancels and reaps its worker.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }
    assert!(was_ready, "worker must connect before terminating its supervisor");
    assert!(
        confirmed,
        "supervisor death must cause confirmed cancellation within five seconds"
    );
}
