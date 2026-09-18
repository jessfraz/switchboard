#![cfg(unix)]

use std::{
    collections::BTreeMap,
    env, fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output},
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

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = env::temp_dir().join(format!(
            "switchboard-batch-resume-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture directory");
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
  case "$*" in
    *'"pageToken":"original-page"'*) ;;
    *) echo '{"error":{"code":400,"message":"wrong page retried"}}'; exit 1;;
  esac
  echo '{"messages":[{"id":"retained-message"}]}'
elif [ "$3" = messages ] && [ "$4" = get ]; then
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
        fs::write(self.root.join("mode"), mode).expect("set native response mode");
        let mut command = Command::new(
            env::var_os("SWITCHBOARD_BATCH_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_switchboard").into()),
        );
        command
            .arg("--config")
            .arg(self.root.join("config.toml"))
            .args(["read-batch", "--input"])
            .arg(self.root.join("input.json"))
            .arg("--checkpoint")
            .arg(self.root.join("checkpoint.json"))
            .args(["--json", "--concurrency", "1"])
            .env("SWITCHBOARD_STATE_DB", self.root.join("operations.sqlite3"))
            .env("SWITCHBOARD_GWS_BIN", self.root.join("gws"))
            .env("BATCH_TEST_MODE_FILE", self.root.join("mode"));
        if resume {
            command.arg("--resume");
        }
        command.output().expect("real Switchboard process")
    }

    fn progress(&self) -> Progress {
        let mut checkpoint: Checkpoint =
            serde_json::from_slice(&fs::read(self.root.join("checkpoint.json")).expect("durable checkpoint"))
                .expect("typed batch checkpoint");
        checkpoint.items.remove("mail").expect("saved mail progress")
    }
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
