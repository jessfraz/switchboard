use std::path::PathBuf;
use std::process::{Command, Output};

use phone_cli::domain::CallRequest;
use phone_cli::journal::{list, read, Record};

struct Fixture {
    _temporary: tempfile::TempDir,
    config: PathBuf,
    calls: PathBuf,
    identity: age::x25519::Identity,
}

impl Fixture {
    fn new(defaults: &str) -> Self {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let directory = temporary.path().canonicalize().expect("absolute fixture path");
        let config = directory.join("config.toml");
        let calls = directory.join("calls");
        let identity = age::x25519::Identity::generate();
        // Exercise real request journaling and backend startup failure without
        // a worker, network access, or a simulated provider response.
        let missing_worker = directory.join("missing-worker");
        std::fs::write(
            &config,
            format!(
                "{defaults}\ntranscript_recipient = {:?}\nstate_dir = {:?}\nworker_command = {:?}\n[livekit]\nurl = 'wss://test.livekit.cloud'\nsip_trunk_id = 'test-trunk'\n",
                identity.to_public().to_string(), calls, missing_worker,
            ),
        )
        .expect("fixture configuration");
        Self {
            _temporary: temporary,
            config,
            calls,
            identity,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_phone"));
        command
            .env_remove("PHONE_CONFIG")
            .env_remove("PHONE_STATE_DIR")
            .env_remove("PHONE_SUPERVISOR_PID")
            .args(["--json", "--config"])
            .arg(&self.config);
        command
    }

    fn recorded_request(&self, output: &Output) -> CallRequest {
        assert!(!output.status.success(), "the missing worker must fail");
        let ids = list(&self.calls).expect("call journal directory");
        assert_eq!(
            ids.len(),
            1,
            "one authorized request should be recorded; stderr: {}",
            String::from_utf8_lossy(&output.stderr),
        );
        let records = read(&self.calls, &ids[0], &self.identity).expect("decrypt request journal");
        match &records[0].record {
            Record::Intent(request) => request.clone(),
            _ => panic!("first record must be the authorized request"),
        }
    }
}

#[test]
fn call_uses_configured_identity_and_duration_without_an_approval_flag() {
    let fixture = Fixture::new("caller_name = 'Alex'\nmax_duration_seconds = 900");
    let output = fixture
        .command()
        .args(["call", "+12125550100", "Ask for opening hours"])
        .output()
        .expect("run phone CLI");
    let request = fixture.recorded_request(&output);
    assert_eq!(request.destination, "+12125550100");
    assert_eq!(request.task, "Ask for opening hours");
    assert_eq!(request.caller_name, "Alex");
    assert_eq!(request.max_duration_seconds, 900);
}

#[test]
fn call_flags_override_configured_identity_and_duration() {
    let fixture = Fixture::new("caller_name = 'Alex'\nmax_duration_seconds = 900");
    let output = fixture
        .command()
        .args([
            "call",
            "+12125550100",
            "Ask for opening hours",
            "--caller-name",
            "Sam",
            "--max-duration-seconds",
            "3600",
        ])
        .output()
        .expect("run phone CLI");
    let request = fixture.recorded_request(&output);
    assert_eq!(request.caller_name, "Sam");
    assert_eq!(request.max_duration_seconds, 3600);
}

#[test]
fn call_keeps_the_ten_minute_default_when_duration_is_not_configured() {
    let fixture = Fixture::new("");
    let output = fixture
        .command()
        .args(["call", "+12125550100", "Ask for opening hours", "--caller-name", "Alex"])
        .output()
        .expect("run phone CLI");
    let request = fixture.recorded_request(&output);
    assert_eq!(request.caller_name, "Alex");
    assert_eq!(request.max_duration_seconds, 600);
}

#[test]
fn call_requires_a_configured_or_explicit_identity_before_creating_state() {
    let fixture = Fixture::new("");
    let output = fixture
        .command()
        .args(["call", "+12125550100", "Ask for opening hours"])
        .output()
        .expect("run phone CLI");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("caller_name"));
    assert!(!fixture.calls.exists());
}

#[test]
fn call_validates_configured_duration_before_creating_state() {
    let fixture = Fixture::new("caller_name = 'Alex'\nmax_duration_seconds = 3601");
    let output = fixture
        .command()
        .args(["call", "+12125550100", "Ask for opening hours"])
        .output()
        .expect("run phone CLI");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("between 30 and 3600"));
    assert!(!fixture.calls.exists());
}

#[test]
fn run_keeps_its_approval_requirement_and_explicit_request_values() {
    let fixture = Fixture::new("caller_name = 'Alex'\nmax_duration_seconds = 900");
    let arguments = [
        "run",
        "--destination",
        "+12125550100",
        "--task",
        "Ask for opening hours",
        "--caller-name",
        "Sam",
    ];
    let denied = fixture.command().args(arguments).output().expect("run phone CLI");
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("explicit approval"));
    assert!(!fixture.calls.exists());
    let output = fixture
        .command()
        .args(arguments)
        .arg("--approve")
        .output()
        .expect("run approved phone CLI");
    let request = fixture.recorded_request(&output);
    assert_eq!(request.caller_name, "Sam");
    assert_eq!(request.max_duration_seconds, 600);
}
