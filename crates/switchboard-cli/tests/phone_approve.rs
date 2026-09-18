use std::{
    env, fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use switchboard_core::{ApprovalState, AuditOutcome, AuditStore, OperationId, OperationStatus, OperationStore};
use switchboard_store::{SqliteAuditStore, SqliteOperationStore};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[derive(Deserialize)]
struct CallOutput {
    operation_id: OperationId,
    fields: CallFields,
}

#[derive(Deserialize)]
struct CallFields {
    response: CallResult,
}

#[derive(Deserialize)]
struct CallResult {
    call_id: String,
    status: String,
}

struct Fixture {
    root: PathBuf,
    config: PathBuf,
    database: PathBuf,
    namespace: PathBuf,
}

impl Fixture {
    fn new(policy: &str) -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "switchboard-phone-approve-{}-{suffix}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let namespace = root.join("phone");
        fs::create_dir_all(&namespace).expect("isolated namespace");
        let root = root.canonicalize().expect("absolute fixture directory");
        let namespace = root.join("phone");
        let config = root.join("switchboard.toml");
        let database = root.join("operations.sqlite3");
        fs::write(
            &config,
            format!(
                "[policy]\nwrite = {policy:?}\n[auth.phone_test]\nprovider = 'phone'\nkind = 'phone_cli'\naccount = 'test'\n[namespace.phone.test]\nprovider = 'phone'\naccount = 'test'\nauth = 'phone_test'\nstate_dir = {namespace:?}\n",
            ),
        )
        .expect("Switchboard fixture configuration");
        // This synthetic recipient encodes the X25519 base point. No private
        // key or network is needed to test real encrypted journal creation.
        fs::write(
            namespace.join("config.toml"),
            format!(
                "transcript_recipient = 'age1pyqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq8r66x'\nstate_dir = {:?}\nworker_command = {:?}\n[livekit]\nurl = 'wss://test.livekit.cloud'\nsip_trunk_id = 'test-trunk'\n",
                namespace.join("calls"), root.join("missing-worker"),
            ),
        )
        .expect("phone fixture configuration");
        Self {
            root,
            config,
            database,
            namespace,
        }
    }

    fn command(&self) -> Command {
        let binary = PathBuf::from(env!("CARGO_BIN_EXE_switchboard"));
        let phone = binary.with_file_name(format!("phone{}", env::consts::EXE_SUFFIX));
        assert!(
            phone.is_file(),
            "build the real phone CLI with cargo build -p phone-cli first"
        );
        let mut command = Command::new(binary);
        command
            .args(["--config"])
            .arg(&self.config)
            .env("SWITCHBOARD_STATE_DB", &self.database)
            .env("SWITCHBOARD_PHONE_BIN", phone)
            .env("USER", "phone-test-user");
        command
    }

    fn call(&self, flags: &[&str]) -> Output {
        self.command()
            .args([
                "phone.call.run",
                "--ns",
                "phone.test",
                "--destination",
                "+12125550100",
                "--caller-name",
                "Alex",
                "--task",
                "Ask for opening hours",
                "--max-duration-seconds",
                "30",
                "--json",
            ])
            .args(flags)
            .output()
            .expect("run the real Switchboard CLI")
    }

    fn operations(&self) -> SqliteOperationStore {
        SqliteOperationStore::open(&self.database).expect("open operation store")
    }

    fn audit(&self) -> SqliteAuditStore {
        SqliteAuditStore::open(&self.database).expect("open audit store")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn approve_persists_and_audits_one_exact_phone_attempt_without_redialing() {
    let fixture = Fixture::new("require_approval");
    let output = fixture.call(&["--approve-and-apply"]);
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let response: CallOutput = serde_json::from_slice(&output.stdout).expect("call response");
    assert_eq!(response.fields.response.call_id, response.operation_id.to_string());
    // The actual phone CLI attempted to start a nonexistent executable. The
    // operation is consumed even though the call itself could not connect.
    assert_eq!(response.fields.response.status, "failed");
    let operations = fixture.operations().list().expect("operation store read");
    assert_eq!(operations.len(), 1);
    let operation = &operations[0];
    assert_eq!(operation.id, response.operation_id);
    assert_eq!(operation.status, OperationStatus::Applied);
    assert_eq!(operation.approval.state, ApprovalState::Approved);
    assert_eq!(operation.approval.actor.as_deref(), Some("phone-test-user"));
    assert!(fixture.namespace.join("calls").join(operation.id.as_str()).is_dir());
    let audit = fixture.audit().list();
    assert_eq!(
        audit.iter().map(|event| &event.outcome).collect::<Vec<_>>(),
        vec![&AuditOutcome::Executed, &AuditOutcome::Approved, &AuditOutcome::Planned]
    );
    assert!(audit
        .iter()
        .all(|event| event.operation_id.as_ref() == Some(&operation.id)));
    let retry = fixture
        .command()
        .args(["op", "apply", operation.id.as_str(), "--json"])
        .output()
        .expect("retry consumed operation");
    assert!(!retry.status.success());
    assert_eq!(fixture.operations().list().expect("operation store read").len(), 1);
    assert_eq!(fixture.audit().list().len(), 3);
}

#[test]
fn ordinary_and_apply_phone_requests_still_wait_for_separate_approval() {
    for flags in [vec![], vec!["--apply"]] {
        let fixture = Fixture::new("require_approval");
        let output = fixture.call(&flags);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        let operations = fixture.operations().list().expect("operation store read");
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].status, OperationStatus::Planned);
        assert_eq!(operations[0].approval.state, ApprovalState::Pending);
        assert!(!fixture.namespace.join("calls").exists());
    }
}

#[test]
fn approve_does_not_override_denied_write_policy() {
    let fixture = Fixture::new("deny");
    let output = fixture.call(&["--approve-and-apply"]);
    assert!(!output.status.success());
    assert!(fixture.operations().list().expect("operation store read").is_empty());
    let events = fixture.audit().list();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome, AuditOutcome::Blocked);
    assert!(!fixture.namespace.join("calls").exists());
}

#[test]
fn approve_rejects_conflicting_modes_and_multiple_namespaces_before_planning() {
    for flags in [
        vec!["--approve-and-apply", "--draft"],
        vec!["--plan", "--approve-and-apply"],
        vec!["--approve-and-apply", "--apply"],
        vec!["--approve-and-apply", "--dry-run"],
        vec!["--approve-and-apply", "--ns", "phone.test"],
    ] {
        let fixture = Fixture::new("require_approval");
        let output = fixture.call(&flags);
        assert!(!output.status.success());
        assert!(fixture.operations().list().expect("operation store read").is_empty());
        assert!(fixture.audit().list().is_empty());
        assert!(!fixture.namespace.join("calls").exists());
    }
}

#[test]
fn approve_rejects_read_tools_before_dispatch() {
    let fixture = Fixture::new("require_approval");
    let output = fixture
        .command()
        .args(["phone.doctor", "--ns", "phone.test", "--approve-and-apply", "--json"])
        .output()
        .expect("read shortcut");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("write tool"));
    assert!(fixture.audit().list().is_empty());
}

#[test]
fn approve_failure_reports_the_persisted_operation_id_without_retrying() {
    let fixture = Fixture::new("require_approval");
    let output = fixture
        .command()
        .env("SWITCHBOARD_PHONE_BIN", fixture.root.join("missing-phone"))
        .args([
            "phone.call.run",
            "--ns",
            "phone.test",
            "--destination",
            "+12125550100",
            "--caller-name",
            "Alex",
            "--task",
            "Ask for opening hours",
            "--approve-and-apply",
            "--json",
        ])
        .output()
        .expect("run with missing phone executable");
    assert!(!output.status.success());
    let operations = fixture.operations().list().expect("operation store read");
    assert_eq!(operations.len(), 1);
    let operation = &operations[0];
    assert_eq!(operation.status, OperationStatus::Failed);
    assert_eq!(operation.approval.state, ApprovalState::Approved);
    assert!(String::from_utf8_lossy(&output.stdout).contains(operation.id.as_str()));
    let audit = fixture.audit().list();
    assert_eq!(
        audit.iter().map(|event| &event.outcome).collect::<Vec<_>>(),
        vec![&AuditOutcome::Failed, &AuditOutcome::Approved, &AuditOutcome::Planned]
    );
    assert!(!fixture.namespace.join("calls").exists());
}

#[test]
fn approve_also_applies_writes_whose_policy_does_not_require_approval() {
    let fixture = Fixture::new("allow");
    let mut config = fs::read_to_string(&fixture.config).expect("fixture configuration");
    config.push_str("\n[auth.github_test]\nprovider = 'github'\nkind = 'gh_cli'\naccount = 'test'\n[namespace.github.test]\nprovider = 'github'\naccount = 'test'\nauth = 'github_test'\n");
    fs::write(&fixture.config, config).expect("add isolated GitHub namespace");
    let output = fixture
        .command()
        .env("SWITCHBOARD_GH_BIN", fixture.root.join("missing-gh"))
        .args([
            "github.pull_request.comment",
            "--ns",
            "github.test",
            "--repo",
            "example/project",
            "--number",
            "42",
            "--body",
            "Check the regression test",
            "--approve-and-apply",
            "--json",
        ])
        .output()
        .expect("run without a GitHub executable");
    assert!(!output.status.success());
    let operations = fixture.operations().list().expect("operation store read");
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].approval.state, ApprovalState::NotRequired);
    assert_eq!(operations[0].status, OperationStatus::Planned);
    assert!(String::from_utf8_lossy(&output.stdout).contains(operations[0].id.as_str()));
    let audit = fixture.audit().list();
    assert_eq!(
        audit.iter().map(|event| &event.outcome).collect::<Vec<_>>(),
        vec![&AuditOutcome::Planned]
    );
}

#[test]
fn approve_reports_credential_resolution_cause_with_the_operation_id() {
    let fixture = Fixture::new("require_approval");
    let mut config = fs::read_to_string(&fixture.config)
        .expect("fixture configuration")
        .replace(
            "kind = 'phone_cli'",
            "kind = 'phone_cli'\napi_key = 'missing_phone_key'",
        );
    config.push_str("\n[secret.missing_phone_key]\nkind = 'env'\nname = 'SWITCHBOARD_TEST_MISSING_PHONE_KEY'\n");
    fs::write(&fixture.config, config).expect("add missing credential reference");
    let output = fixture
        .command()
        .env_remove("SWITCHBOARD_TEST_MISSING_PHONE_KEY")
        .args([
            "phone.call.run",
            "--ns",
            "phone.test",
            "--destination",
            "+12125550100",
            "--caller-name",
            "Alex",
            "--task",
            "Ask for opening hours",
            "--approve-and-apply",
            "--json",
        ])
        .output()
        .expect("run without required credential");
    assert!(!output.status.success());
    let operations = fixture.operations().list().expect("operation store read");
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].status, OperationStatus::Planned);
    assert_eq!(operations[0].approval.state, ApprovalState::Approved);
    let error = String::from_utf8_lossy(&output.stdout);
    assert!(error.contains(operations[0].id.as_str()));
    assert!(
        error.contains("missing_phone_key"),
        "credential cause was lost: {error}"
    );
    assert!(!fixture.namespace.join("calls").exists());
}
