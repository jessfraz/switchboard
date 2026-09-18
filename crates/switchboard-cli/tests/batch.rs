#![cfg(unix)]

use std::{
    collections::BTreeMap,
    env, fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use switchboard_core::{CoverageStatus, Failure, FailurePhase, ReadCoverage, ToolArgument, ToolName, ToolOutput};

#[derive(Serialize)]
struct Input {
    items: Vec<Item>,
}

#[derive(Serialize)]
struct Item {
    id: String,
    tool: ToolName,
    namespace: switchboard_core::NamespaceId,
    args: Vec<ToolArgument>,
}

#[derive(Debug, PartialEq, Deserialize)]
struct Page {
    requested_cursor: Option<String>,
    output: ToolOutput,
}

#[derive(Deserialize)]
struct Progress {
    pages: Vec<Page>,
    coverage: ReadCoverage,
    failure: Option<Failure>,
    finished: bool,
}

#[derive(Deserialize)]
struct Checkpoint {
    items: BTreeMap<String, Progress>,
}

#[derive(Deserialize)]
struct Report {
    status: String,
    checkpoint: PathBuf,
    resume_argv: Vec<String>,
    items: BTreeMap<String, Progress>,
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let initial = env::temp_dir().join(format!(
            "switchboard-batch-resume-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        let mut root = initial.clone();
        let mut collision = 0;
        while let Err(error) = fs::create_dir(&root) {
            assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists, "fixture directory");
            collision += 1;
            root = initial.with_extension(collision.to_string());
        }
        fs::write(root.join("config.toml"), format!(
            "[auth.test]\nprovider = 'google'\nkind = 'google_cli'\naccount = 'test@example.invalid'\n[namespace.google.test]\nprovider = 'google'\naccount = 'test@example.invalid'\nauth = 'test'\nstate_dir = {root:?}\n"
        )).expect("fixture config");
        let input = Input {
            items: vec![Item {
                id: "mail".into(),
                tool: ToolName::new("google.mail.search").expect("tool"),
                namespace: switchboard_core::NamespaceId::new("google.test").expect("namespace"),
                args: vec![
                    ToolArgument::option("query", "receipt").expect("query"),
                    ToolArgument::option("cursor", "original-page").expect("cursor"),
                ],
            }],
        };
        fs::write(root.join("input.json"), serde_json::to_vec(&input).expect("input JSON")).expect("batch input");
        // Exercise the real CLI transport with native Gmail responses. The
        // fixture never reads credentials or contacts an external provider.
        let native = root.join("gws");
        fs::write(
            &native,
            r#"#!/bin/sh
case "$*" in
  --version) echo 'gws 0.99.0-test'; exit 0;;
  *--help*) echo help; exit 0;;
esac
mode=$(cat "$BATCH_TEST_MODE_FILE")
reject() { echo '{"error":{"code":401,"message":"credential revoked"}}'; exit 1; }
if [ "$3" = getProfile ]; then
  if [ "$mode" = auth_blocked ]; then reject; fi
  echo '{"emailAddress":"test@example.invalid"}'
elif [ "$3" = messages ] && [ "$4" = list ]; then
  if [ "$mode" = paginated ]; then
    case "$*" in
      *'"pageToken":"next-page"'*) echo '{"messages":[{"id":"page-two"}]}';;
      *pageToken*) echo '{"error":{"code":400,"message":"unexpected cursor"}}'; exit 1;;
      *) echo '{"messages":[{"id":"page-one"}],"nextPageToken":"next-page"}';;
    esac
    exit 0
  fi
  case "$*" in
    *'"pageToken":"original-page"'*) ;;
    *) echo '{"error":{"code":400,"message":"wrong page retried"}}'; exit 1;;
  esac
  echo '{"messages":[{"id":"retained-message"}]}'
elif [ "$3" = messages ] && [ "$4" = get ]; then
  if [ "$mode" = paginated ]; then
    case "$*" in
      *page-two*) echo '{"id":"page-two","payload":{"headers":[]}}';;
      *) echo '{"id":"page-one","payload":{"headers":[]}}';;
    esac
    exit 0
  fi
  if [ "$mode" != complete ]; then reject; fi
  echo '{"id":"retained-message","payload":{"headers":[{"name":"Subject","value":"Receipt"}]}}'
else
  exit 3
fi
"#,
        )
        .expect("native transport fixture");
        fs::set_permissions(native, fs::Permissions::from_mode(0o700)).expect("executable native fixture");
        Self { root }
    }

    fn run(&self, mode: &str, resume: bool) -> Output {
        let mut command = self.command(mode);
        command
            .arg("--config")
            .arg(self.root.join("config.toml"))
            .args(["read-batch", "--input"])
            .arg(self.root.join("input.json"))
            .arg("--checkpoint")
            .arg(self.root.join("checkpoint.json"))
            .args(["--json", "--concurrency", "1"]);
        if resume {
            command.arg("--resume");
        }
        command.output().expect("real Switchboard process")
    }

    fn command(&self, mode: &str) -> Command {
        fs::write(self.root.join("mode"), mode).expect("set native response mode");
        let mut command = Command::new(
            env::var_os("SWITCHBOARD_BATCH_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_switchboard").into()),
        );
        command
            .env("SWITCHBOARD_STATE_DB", self.root.join("operations.sqlite3"))
            .env("SWITCHBOARD_GWS_BIN", self.root.join("gws"))
            .env("BATCH_TEST_MODE_FILE", self.root.join("mode"))
            .env_remove("SWITCHBOARD_DEADLINE_UNIX_MS");
        command
    }

    fn progress(&self) -> Progress {
        let mut checkpoint: Checkpoint =
            serde_json::from_slice(&fs::read(self.root.join("checkpoint.json")).expect("durable checkpoint"))
                .expect("typed batch checkpoint");
        checkpoint.items.remove("mail").expect("saved mail progress")
    }
}

fn report(output: &Output) -> Report {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid batch receipt: {error}; stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn stdin_input_creates_private_checkpoint_and_resumes_without_original_input() {
    let fixture = Fixture::new();
    let bytes = fs::read(fixture.root.join("input.json")).expect("typed input");
    let mut child = fixture
        .command("complete")
        .arg("--config")
        .arg(fixture.root.join("config.toml"))
        .args(["read-batch", "--input", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("real CLI");
    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(&bytes)
        .expect("send input");
    let result = child.wait_with_output().expect("real CLI result");
    let report = report(&result);
    assert!(result.status.success());
    assert_eq!(report.status, "complete");
    assert_eq!(report.checkpoint.parent(), Some(fixture.root.join("batches").as_path()));
    assert_eq!(
        fs::metadata(&report.checkpoint)
            .expect("checkpoint")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(report.checkpoint.parent().expect("parent"))
            .expect("directory")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    fs::remove_file(fixture.root.join("input.json")).expect("remove original input");
    let resumed = fixture
        .command("auth_blocked")
        .args(&report.resume_argv[1..])
        .output()
        .expect("resume CLI");
    assert!(resumed.status.success(), "finished requests must not reauthenticate");
    assert_eq!(self::report(&resumed).items["mail"].pages, report.items["mail"].pages);
}

#[test]
fn compact_queries_stop_at_page_budget_and_resume_each_saved_cursor() {
    let fixture = Fixture::new();
    let output = fixture
        .command("paginated")
        .arg("--config")
        .arg(fixture.root.join("config.toml"))
        .args([
            "read-batch",
            "--tool",
            "google.mail.search",
            "--ns",
            "google.test",
            "--args-json",
            r#"{"query":"receipt","max":1}"#,
            "--args-json",
            r#"{"query":"invoice","max":1}"#,
            "--max-pages",
            "1",
            "--concurrency",
            "1",
            "--json",
        ])
        .output()
        .expect("real CLI");
    let initial = report(&output);
    assert!(!output.status.success(), "page budget must retain partial status");
    assert_eq!(initial.status, "partial");
    assert_eq!(initial.items.len(), 2);
    for progress in initial.items.values() {
        assert_eq!(progress.pages.len(), 1);
        assert_eq!(progress.pages[0].requested_cursor, None);
        assert_eq!(progress.coverage.next_cursor.as_deref(), Some("next-page"));
        assert!(!progress.finished);
    }
    let resumed = fixture
        .command("paginated")
        .args(&initial.resume_argv[1..])
        .output()
        .expect("resume CLI");
    let finished = report(&resumed);
    assert!(resumed.status.success());
    assert_eq!(finished.status, "complete");
    for (id, progress) in &finished.items {
        assert_eq!(progress.pages.len(), 2);
        assert_eq!(progress.pages[0], initial.items[id].pages[0]);
        assert_eq!(progress.pages[1].requested_cursor.as_deref(), Some("next-page"));
        assert_eq!(progress.coverage.status, CoverageStatus::Complete);
        assert!(progress.finished);
    }
}

#[test]
fn expired_deadline_preserves_auth_preflight_failure_in_checkpoint() {
    let fixture = Fixture::new();
    let output = fixture
        .command("complete")
        .arg("--config")
        .arg(fixture.root.join("config.toml"))
        .args(["read-batch", "--input"])
        .arg(fixture.root.join("input.json"))
        .arg("--checkpoint")
        .arg(fixture.root.join("checkpoint.json"))
        .arg("--json")
        .env("SWITCHBOARD_DEADLINE_UNIX_MS", "0")
        .output()
        .expect("real CLI");
    assert!(!output.status.success());
    let progress = fixture.progress();
    assert!(progress.pages.is_empty());
    let failure = progress
        .failure
        .expect("preflight blocker must survive expired wave budget");
    assert!(matches!(
        failure.code,
        switchboard_core::FailureCode::Timeout | switchboard_core::FailureCode::AuthenticationTimeout
    ));
}

#[test]
fn projected_batch_results_keep_full_checkpoint_and_resume_projection() {
    #[derive(Deserialize)]
    struct SelectedFields {
        messages: Vec<Identity>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Identity {
        gmail_message_id: String,
    }
    #[derive(Deserialize)]
    struct SavedFields {
        messages: Vec<SavedMessage>,
    }
    #[derive(Deserialize)]
    struct SavedMessage {
        subject: String,
    }
    let fixture = Fixture::new();
    let output = fixture
        .command("complete")
        .arg("--config")
        .arg(fixture.root.join("config.toml"))
        .args(["read-batch", "--input"])
        .arg(fixture.root.join("input.json"))
        .arg("--checkpoint")
        .arg(fixture.root.join("checkpoint.json"))
        .args(["--json", "--fields", "messages.gmail_message_id"])
        .output()
        .expect("real CLI");
    let initial = report(&output);
    assert!(output.status.success());
    let visible: SelectedFields = serde_json::from_value(
        serde_json::to_value(&initial.items["mail"].pages[0].output.fields).expect("dynamic provider fields"),
    )
    .expect("projected message identity");
    assert_eq!(visible.messages[0].gmail_message_id, "retained-message");
    let persisted: SavedFields = serde_json::from_value(
        serde_json::to_value(fixture.progress().pages[0].output.fields.clone()).expect("saved provider fields"),
    )
    .expect("unprojected saved message");
    assert_eq!(persisted.messages[0].subject, "Receipt");
    let resumed = fixture
        .command("auth_blocked")
        .args(&initial.resume_argv[1..])
        .output()
        .expect("resume CLI");
    assert!(resumed.status.success());
    assert_eq!(report(&resumed).items["mail"].pages, initial.items["mail"].pages);
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn blocked_resume_preserves_partial_page_until_successful_replacement() {
    let fixture = Fixture::new();
    assert!(!fixture.run("metadata_blocked", false).status.success());
    let initial = fixture.progress();
    assert_eq!(initial.pages.len(), 1);
    assert_eq!(initial.pages[0].requested_cursor.as_deref(), Some("original-page"));
    assert_eq!(initial.pages[0].output.refs[0].id, "retained-message");
    assert_eq!(initial.coverage.status, CoverageStatus::Unknown);
    for mode in ["auth_blocked", "metadata_blocked"] {
        assert!(!fixture.run(mode, true).status.success());
        let resumed = fixture.progress();
        assert_eq!(
            resumed.pages, initial.pages,
            "{mode} must retain prior successful evidence"
        );
        assert!(!resumed.finished);
        assert_eq!(
            resumed.failure.expect("auth blocker").phase,
            FailurePhase::Authentication
        );
    }
    let completed = fixture.run("complete", true);
    assert!(
        completed.status.success(),
        "{} {}",
        String::from_utf8_lossy(&completed.stdout),
        String::from_utf8_lossy(&completed.stderr)
    );
    let resumed = fixture.progress();
    assert_eq!(resumed.pages.len(), 1);
    assert_eq!(resumed.pages[0].output.refs[0].id, "retained-message");
    assert_eq!(resumed.coverage.status, CoverageStatus::Complete);
    assert!(resumed.finished);
    assert!(resumed.failure.is_none());
}
