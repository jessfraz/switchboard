use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

use serde::Deserialize;

struct Fixture {
    root: PathBuf,
    config: PathBuf,
}
impl Fixture {
    fn new(account: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("test fixture should succeed")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "switchboard-dwd-test-{}-{:?}-{nonce}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir(&root).expect("test fixture should succeed");
        let root = fs::canonicalize(root).expect("fixture directory should have a canonical path");
        let config = root.join("switchboard.toml");
        fs::write(&config, format!("# Keep this comment\n[namespace.google.work]\nprovider = \"google\"\naccount = \"{account}\"\nstate_dir = \"login\"\n")).expect("test fixture should succeed");
        Self { root, config }
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_switchboard"))
            .args(["--config"])
            .arg(&self.config)
            .args(args)
            .env_remove("SWITCHBOARD_STATE_DB")
            .env_remove("SWITCHBOARD_STATE_DIR")
            .env("SWITCHBOARD_GWS_BIN", self.root.join("must-not-run"))
            .env("SWITCHBOARD_OP_BIN", self.root.join("must-not-run"))
            .output()
            .expect("test fixture should succeed")
    }

    fn configure_delegation(&self) {
        fs::write(
            &self.config,
            format!(
                r#"[auth.delegated]
provider = "google"
kind = "google_delegated"
account = "person@example.com"
[auth.delegated.delegation]
project = "example-project"
service_account = "settings@example-project.iam.gserviceaccount.com"
domain = "example.com"
subject = "person@example.com"
signer_account = "person@example.com"
signer_state_dir = "{}"
[namespace.google.work]
provider = "google"
account = "Work"
auth = "delegated"
state_dir = "login"
"#,
                self.root.join("login").display()
            ),
        )
        .expect("fixture config should be writable");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Debug, Deserialize, PartialEq)]
struct Plan {
    namespace: String,
    status: String,
    account: String,
    project: String,
    service_account: String,
    signer: String,
    scopes: Vec<String>,
    client_id: Option<String>,
}
fn success(output: Output) -> Plan {
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("test fixture should succeed")
}

#[test]
fn privileged_gmail_setting_explains_supported_auth_instead_of_planning_oauth() {
    let fixture = Fixture::new("person@example.com");
    let output = fixture.run(&[
        "google.cli.write",
        "--ns",
        "google.work",
        "--draft",
        "--json",
        "--",
        "gmail",
        "users",
        "settings",
        "sendAs",
        "create",
        "--params",
        r#"{"userId":"me"}"#,
        "--json",
        r#"{"sendAsEmail":"alias@example.com"}"#,
    ]);
    assert!(
        !output.status.success(),
        "an OAuth-only privileged setting must not produce an executable plan: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let error = String::from_utf8_lossy(&output.stdout);
    assert!(error.contains("Workspace administrator authorization"), "{error}");
    assert!(error.contains("auth login --ns google.work --setup"), "{error}");
    assert!(!error.contains("add gmail.settings.sharing"));
}

#[test]
fn alias_ensure_rejects_ordinary_oauth_and_cross_domain_addresses_before_execution() {
    let fixture = Fixture::new("person@example.com");
    let output = fixture.run(&[
        "google.mail.alias.ensure",
        "--ns",
        "google.work",
        "--email",
        "alias@example.com",
        "--approve-and-apply",
        "--json",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Workspace administrator authorization"));
    fixture.configure_delegation();
    let output = fixture.run(&[
        "google.mail.alias.ensure",
        "--ns",
        "google.work",
        "--email",
        "alias@other.example",
        "--approve-and-apply",
        "--json",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("configured Workspace subject's domain"));
}

#[test]
fn alias_ensure_draft_records_one_operation_without_contacting_google() {
    #[derive(Deserialize)]
    struct AliasPlan {
        status: String,
        operation_id: String,
        summary: String,
    }
    #[derive(Deserialize)]
    struct Operations {
        operations: Vec<SavedOperation>,
    }
    #[derive(Deserialize)]
    struct SavedOperation {
        id: switchboard_core::OperationId,
        status: switchboard_core::OperationStatus,
    }
    let fixture = Fixture::new("person@example.com");
    fixture.configure_delegation();
    let output = fixture.run(&[
        "google.mail.alias.ensure",
        "--ns",
        "google.work",
        "--email",
        "alias@example.com",
        "--draft",
        "--json",
    ]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    let plan: AliasPlan = serde_json::from_slice(&output.stdout).expect("typed alias plan");
    assert_eq!(plan.status, "planned");
    assert!(plan.summary.contains("Workspace receiving alias"));
    let output = fixture.run(&["op", "list", "--json"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    let operations: Operations = serde_json::from_slice(&output.stdout).expect("typed operation list");
    assert_eq!(operations.operations.len(), 1);
    assert_eq!(operations.operations[0].id.as_str(), plan.operation_id);
    assert_eq!(
        operations.operations[0].status,
        switchboard_core::OperationStatus::Planned
    );
}

#[test]
fn guided_plan_is_nonsecret_and_resumes_through_normal_login_without_cloud_access() {
    let fixture = Fixture::new("person@example.com");
    let before = fs::read(&fixture.config).expect("test fixture should succeed");
    let first = success(fixture.run(&[
        "auth",
        "login",
        "--ns",
        "google.work",
        "--setup",
        "--project",
        "example-project",
        "--json",
    ]));
    assert_eq!(first.status, "plan_ready");
    assert_eq!(first.account, "person@example.com");
    assert_eq!(first.signer, "person@example.com");
    assert_eq!(first.project, "example-project");
    assert!(first.client_id.is_none());
    assert_eq!(
        first.scopes,
        [
            "https://www.googleapis.com/auth/gmail.settings.basic",
            "https://www.googleapis.com/auth/gmail.settings.sharing"
        ]
    );
    let resumed = success(fixture.run(&["auth", "login", "--ns", "google.work", "--json"]));
    assert_eq!(first, resumed);
    assert_eq!(fs::read(&fixture.config).expect("test fixture should succeed"), before);
    let checkpoint = fixture.root.join("login/switchboard-gmail-settings-setup.json");
    assert!(checkpoint.is_file());
    let stored = fs::read_to_string(checkpoint).expect("test fixture should succeed");
    assert!(!stored.contains("refresh_token"));
    assert!(!stored.contains("private_key"));
}

#[test]
fn saved_plan_cannot_be_reused_after_account_or_project_changes() {
    let fixture = Fixture::new("person@example.com");
    success(fixture.run(&[
        "auth",
        "login",
        "--ns",
        "google.work",
        "--setup",
        "--project",
        "example-project",
        "--json",
    ]));
    let output = fixture.run(&[
        "auth",
        "login",
        "--ns",
        "google.work",
        "--project",
        "other-project",
        "--json",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("no longer matches"));
    let source = fs::read_to_string(&fixture.config).expect("test fixture should succeed");
    fs::write(
        &fixture.config,
        source.replace("person@example.com", "other@example.com"),
    )
    .expect("test fixture should succeed");
    let output = fixture.run(&["auth", "login", "--ns", "google.work", "--apply", "--json"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("no longer matches"));
}

#[test]
fn consumer_login_is_not_misrepresented_as_workspace_capability() {
    let fixture = Fixture::new("person@gmail.com");
    let output = fixture.run(&[
        "auth",
        "login",
        "--ns",
        "google.work",
        "--setup",
        "--project",
        "example-project",
        "--json",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("consumer Gmail is unsupported"));
    assert!(!fixture
        .root
        .join("login/switchboard-gmail-settings-setup.json")
        .exists());
}

#[test]
fn explicit_apply_cannot_skip_review_of_a_new_plan() {
    let fixture = Fixture::new("person@example.com");
    let plan = success(fixture.run(&[
        "auth",
        "login",
        "--ns",
        "google.work",
        "--setup",
        "--apply",
        "--project",
        "example-project",
        "--json",
    ]));
    assert_eq!(plan.status, "plan_ready");
}

#[test]
fn setup_does_not_replace_explicit_oauth_file_credentials() {
    let fixture = Fixture::new("person@example.com");
    let source = r#"[secret.saved]
kind = "env"
name = "SWITCHBOARD_SYNTHETIC_TEST_CREDENTIAL"
[auth.explicit]
provider = "google"
kind = "google_oauth_file"
account = "person@example.com"
credentials = "saved"
[namespace.google.work]
provider = "google"
account = "Work"
auth = "explicit"
state_dir = "login"
"#;
    fs::write(&fixture.config, source).expect("fixture config should be writable");
    let output = fixture.run(&[
        "auth",
        "login",
        "--ns",
        "google.work",
        "--setup",
        "--project",
        "example-project",
        "--json",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("namespace-managed Google login"));
    assert_eq!(
        fs::read_to_string(&fixture.config).expect("fixture config should exist"),
        source
    );
    assert!(!fixture
        .root
        .join("login/switchboard-gmail-settings-setup.json")
        .exists());
}

#[cfg(unix)]
#[test]
fn delegated_login_recovers_ordinary_session_once_without_regranting_or_exposing_export() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new("person@example.com");
    let source = format!(
        r#"[auth.delegated]
provider = "google"
kind = "google_delegated"
account = "person@example.com"
[auth.delegated.delegation]
project = "example-project"
service_account = "settings@example-project.iam.gserviceaccount.com"
domain = "example.com"
subject = "person@example.com"
signer_account = "person@example.com"
signer_state_dir = "{}"
[namespace.google.work]
provider = "google"
account = "Work"
auth = "delegated"
state_dir = "login"
"#,
        fixture.root.join("login").display()
    );
    fs::write(&fixture.config, &source).expect("fixture config should be writable");
    let gws = fixture.root.join("gws");
    fs::write(
        &gws,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$SWITCHBOARD_TEST_CALLS_FILE"
if [ "$1" = '--version' ]; then printf 'gws 0.22.5\n'; exit 0; fi
if [ "$1" = '--help' ]; then printf 'gws help\n'; exit 0; fi
case "$1 $2" in
  'auth export') printf 'raw-private-export'; printf 'raw-private-error' >&2; exit 1 ;;
  'auth login') exit 0 ;;
  'auth status') printf '%s' '{"user":"person@example.com"}'; exit 0 ;;
esac
exit 1
"#,
    )
    .expect("fixture executable should be writable");
    fs::set_permissions(&gws, fs::Permissions::from_mode(0o755)).expect("fixture executable should be usable");
    let calls = fixture.root.join("calls.txt");
    let output = Command::new(env!("CARGO_BIN_EXE_switchboard"))
        .arg("--config")
        .arg(&fixture.config)
        .args(["auth", "login", "--ns", "google.work", "--json"])
        .env_remove("SWITCHBOARD_STATE_DB")
        .env_remove("SWITCHBOARD_STATE_DIR")
        .env("SWITCHBOARD_GWS_BIN", &gws)
        .env("SWITCHBOARD_TEST_CALLS_FILE", &calls)
        .env("SWITCHBOARD_OP_BIN", fixture.root.join("must-not-run"))
        .output()
        .expect("fixture login should finish");
    assert!(!output.status.success());
    let calls = fs::read_to_string(calls).unwrap_or_else(|error| {
        panic!(
            "fixture calls missing: {error}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(
        calls.lines().filter(|line| line.starts_with("auth export")).count(),
        2,
        "{calls}: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(calls.lines().filter(|line| line.starts_with("auth login")).count(), 1);
    assert_eq!(calls.lines().filter(|line| *line == "auth status").count(), 1);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("raw-private"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("raw-private"));
    assert_eq!(
        fs::read_to_string(&fixture.config).expect("fixture config should exist"),
        source
    );
}
