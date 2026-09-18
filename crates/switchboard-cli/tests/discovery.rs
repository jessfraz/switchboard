use serde::Deserialize;
use std::{
    collections::BTreeMap,
    env, fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use switchboard_core::{ProviderKind, ToolArgumentSpec, ToolExecutionSupport};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[derive(Deserialize)]
struct Detail {
    tool: DescribedTool,
}
#[derive(Deserialize)]
struct DescribedTool {
    name: String,
    arguments: Vec<ToolArgumentSpec>,
    examples: Vec<String>,
    scope_guidance: Vec<String>,
    pagination: Option<Pagination>,
    raw_fallback: Option<Fallback>,
    output_schema: Schema,
}
#[derive(Deserialize)]
struct Schema {
    #[serde(default)]
    properties: BTreeMap<String, Schema>,
    #[serde(default, rename = "enum")]
    values: Vec<String>,
}
#[derive(Deserialize)]
struct Pagination {
    limit_argument: String,
    cursor_argument: String,
    default_limit: u32,
    max_limit: u32,
}
#[derive(Deserialize)]
struct Fallback {
    argv: Vec<String>,
}
#[derive(Deserialize)]
struct Failed {
    failure: FailureContext,
}
#[derive(Deserialize)]
struct FailureContext {
    namespace: Option<String>,
}
#[derive(Deserialize)]
struct Listed {
    tools: Vec<ListedTool>,
}
#[derive(Deserialize)]
struct ListedTool {
    name: String,
    provider: ProviderKind,
    execution_support: ToolExecutionSupport,
}

struct Fixture {
    root: PathBuf,
    config: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = env::temp_dir().join(format!(
            "switchboard-discovery-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after Unix epoch")
                .as_nanos(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("fixture operation should succeed");
        let config = root.join("config.toml");
        Self { root, config }
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(
            env::var_os("SWITCHBOARD_DISCOVERY_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_switchboard").into()),
        )
        .arg("--config")
        .arg(&self.config)
        .args(args)
        // A directory cannot be opened as the operation database. Discovery
        // must never reach it or any provider/auth executable.
        .env("SWITCHBOARD_STATE_DB", &self.root)
        .env("SWITCHBOARD_OP_BIN", self.root.join("missing-op"))
        .env("SWITCHBOARD_GWS_BIN", self.root.join("missing-gws"))
        .env("SWITCHBOARD_GH_BIN", self.root.join("missing-gh"))
        .output()
        .expect("fixture operation should succeed")
    }
    fn configure(&self) {
        fs::write(&self.config, "[secret.token]\nkind = 'file'\npath = '/nonexistent/discovery-test-token'\n[auth.test]\nprovider = 'github'\nkind = 'github_token'\naccount = 'test'\ntoken = 'token'\n[namespace.github.test]\nprovider = 'github'\naccount = 'test'\nauth = 'test'\n").expect("fixture operation should succeed");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn external_help_works_without_namespace_config_auth_or_database() {
    let fixture = Fixture::new();
    let output = fixture.run(&["google.mail.search", "--help", "--json", "--full"]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let detail: Detail = serde_json::from_slice(&output.stdout).expect("Switchboard should emit the documented JSON");
    assert_eq!(detail.tool.name, "google.mail.search");
    assert!(detail
        .tool
        .arguments
        .iter()
        .any(|argument| argument.name == "query" && argument.required));
    assert!(!detail.tool.examples.is_empty());
    assert!(!detail.tool.scope_guidance.is_empty());
    let status = detail
        .tool
        .output_schema
        .properties
        .get("status")
        .expect("execution status schema");
    assert!(status.values.iter().any(|value| value == "executed"));
    assert!(status.values.iter().any(|value| value == "partial"));
    let pagination = detail.tool.pagination.expect("Gmail help should describe pagination");
    assert_eq!(pagination.limit_argument, "max");
    assert_eq!(pagination.cursor_argument, "cursor");
    assert_eq!((pagination.default_limit, pagination.max_limit), (20, 500));
    let fallback = detail
        .tool
        .raw_fallback
        .expect("Gmail help should describe native fallback");
    assert_eq!(fallback.argv[1], "google.cli.read");
    assert!(fs::read_dir(&fixture.root)
        .expect("fixture operation should succeed")
        .next()
        .is_none());
}

#[test]
fn namespace_and_executable_filters_work_without_resolving_secrets() {
    let fixture = Fixture::new();
    fixture.configure();
    let output = fixture.run(&[
        "tools",
        "list",
        "--ns",
        "github.test",
        "--provider",
        "github",
        "--executable",
        "--search",
        "pull_request",
        "--json",
    ]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let listed: Listed = serde_json::from_slice(&output.stdout).expect("Switchboard should emit the documented JSON");
    assert!(!listed.tools.is_empty());
    for tool in listed.tools {
        assert_eq!(tool.provider, ProviderKind::GitHub);
        assert_eq!(tool.execution_support, ToolExecutionSupport::Executable);
        assert!(tool.name.contains("pull_request"));
    }
    assert_eq!(
        fs::read_dir(&fixture.root)
            .expect("fixture operation should succeed")
            .count(),
        1
    );
    let invalid = fixture.run(&["tools", "list", "--ns", "github.missing", "--json"]);
    assert!(!invalid.status.success());
    assert_eq!(
        fs::read_dir(&fixture.root)
            .expect("fixture operation should succeed")
            .count(),
        1
    );
}

#[test]
fn raw_delimiter_keeps_native_help_out_of_switchboard_discovery() {
    let fixture = Fixture::new();
    let output = fixture.run(&["google.cli.read", "--", "--help", "--json"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--ns"));
    assert!(
        output.stdout.is_empty(),
        "native --json must not change Switchboard's error stream"
    );
    let output = fixture.run(&["github.cli.read", "--ns=github.test", "--json", "--", "--help"]);
    assert!(!output.status.success());
    let failed: Failed = serde_json::from_slice(&output.stdout).expect("typed failure envelope");
    assert_eq!(failed.failure.namespace.as_deref(), Some("github.test"));
    let output = fixture.run(&["google.cli.read", "--json", "--", "--ns=google.test"]);
    assert!(!output.status.success());
    let failed: Failed = serde_json::from_slice(&output.stdout).expect("typed failure envelope");
    assert_eq!(
        failed.failure.namespace, None,
        "native --ns must not identify Switchboard's account"
    );
}

#[test]
fn discovery_defaults_to_bounded_actionable_results_with_opt_in_full_schema() {
    #[derive(Deserialize)]
    struct Matches {
        total: usize,
        omitted: usize,
        tools: Vec<Match>,
    }
    #[derive(Deserialize)]
    struct Match {
        name: String,
        example: String,
    }
    #[derive(Deserialize)]
    struct Brief {
        tool: BriefTool,
    }
    #[derive(Deserialize)]
    struct BriefTool {
        arguments: Vec<ToolArgumentSpec>,
        examples: Vec<String>,
        output_schema: Option<Schema>,
    }
    let fixture = Fixture::new();
    let result = fixture.run(&[
        "tools",
        "list",
        "--provider",
        "google",
        "--search",
        "mail",
        "--executable",
        "--limit",
        "3",
        "--json",
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let matches: Matches = serde_json::from_slice(&result.stdout).expect("decode short catalog");
    assert_eq!(matches.tools.len(), 3);
    assert_eq!(matches.total, matches.tools.len() + matches.omitted);
    assert!(matches.omitted > 0);
    assert!(matches
        .tools
        .iter()
        .all(|tool| !tool.name.is_empty() && tool.example.starts_with("switchboard ")));
    assert!(
        result.stdout.len() < 2500,
        "bounded discovery should fit a small context budget"
    );
    let brief = fixture.run(&["google.mail.read", "--help", "--json"]);
    assert!(brief.status.success());
    let brief: Brief = serde_json::from_slice(&brief.stdout).expect("decode short help");
    assert!(!brief.tool.arguments.is_empty());
    assert!(!brief.tool.examples.is_empty());
    assert!(brief.tool.output_schema.is_none());
    let full = fixture.run(&["google.mail.read", "--help", "--full", "--json"]);
    assert!(full.status.success());
    let full: Brief = serde_json::from_slice(&full.stdout).expect("decode full help");
    assert!(full.tool.output_schema.is_some());
    assert!(fs::read_dir(&fixture.root)
        .expect("read fixture directory")
        .next()
        .is_none());
}
