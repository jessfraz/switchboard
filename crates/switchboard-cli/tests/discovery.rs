use std::{
    collections::BTreeMap,
    env, fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
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
    fn command(&self) -> Command {
        let mut command = Command::new(
            env::var_os("SWITCHBOARD_DISCOVERY_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_switchboard").into()),
        );
        command
            .arg("--config")
            .arg(&self.config)
            // A directory cannot be opened as the operation database. Discovery
            // must never reach it or any provider/auth executable.
            .env("SWITCHBOARD_STATE_DB", &self.root)
            .env("SWITCHBOARD_OP_BIN", self.root.join("missing-op"))
            .env("SWITCHBOARD_GWS_BIN", self.root.join("missing-gws"))
            .env("SWITCHBOARD_GH_BIN", self.root.join("missing-gh"));
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command()
            .args(args)
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
fn messages_discovery_describes_local_reads_without_opening_the_database() {
    let fixture = Fixture::new();
    let output = fixture.run(&["messages.chats.read", "--help", "--json", "--full"]);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let detail: Detail = serde_json::from_slice(&output.stdout).expect("Messages CLI discovery");
    assert_eq!(detail.tool.name, "messages.chats.read");
    assert!(detail
        .tool
        .arguments
        .iter()
        .any(|argument| argument.name == "chat-id" && argument.required));
    let pagination = detail.tool.pagination.expect("Messages pagination");
    assert_eq!(pagination.limit_argument, "limit");
    assert_eq!(pagination.cursor_argument, "cursor");
    assert_eq!((pagination.default_limit, pagination.max_limit), (50, 500));
    assert!(detail.tool.raw_fallback.is_none());
    assert!(fs::read_dir(&fixture.root).expect("fixture directory").next().is_none());
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

#[test]
fn namespace_list_never_opens_runtime_state_or_resolves_credentials() {
    let fixture = Fixture::new();
    fixture.configure();
    let output = fixture.run(&["ns", "list", "--json"]);
    assert!(
        output.status.success(),
        "namespace discovery must not initialize runtime state: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    #[derive(Deserialize)]
    struct Namespaces {
        schema_version: u32,
        namespaces: Vec<Namespace>,
    }
    #[derive(Deserialize)]
    struct Namespace {
        id: String,
        provider: ProviderKind,
        account_label: String,
        auth_ref: String,
        default_read: bool,
    }
    let result: Namespaces = serde_json::from_slice(&output.stdout).expect("namespace JSON");
    assert_eq!(result.schema_version, 1);
    assert_eq!(result.namespaces.len(), 1);
    let namespace = &result.namespaces[0];
    assert_eq!(namespace.id, "github.test");
    assert_eq!(namespace.provider, ProviderKind::GitHub);
    assert_eq!(namespace.account_label, "test");
    assert_eq!(namespace.auth_ref, "test");
    assert!(!namespace.default_read);
    let absent_database = fixture.root.join("new-state/operations.sqlite3");
    let second = fixture
        .command()
        .args(["ns", "list", "--json"])
        .env("SWITCHBOARD_STATE_DB", &absent_database)
        .output()
        .expect("discovery with absent state");
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stdout));
    assert!(!absent_database.exists());
    assert_eq!(fs::read_dir(&fixture.root).expect("fixture contents").count(), 1);
}

#[derive(Debug, Deserialize)]
struct NamespaceList {
    schema_version: u32,
    namespaces: Vec<NamespaceRow>,
    config: ConfigProvenance,
}
#[derive(Debug, Deserialize)]
struct NamespaceRow {
    id: String,
    provider: ProviderKind,
    account_label: String,
    auth_ref: String,
    auth_kind: String,
    auth_account_label: String,
    default_read: bool,
    state_dir: Option<PathBuf>,
    auth_scope_profile: Option<String>,
}
#[derive(Debug, Deserialize)]
struct ConfigProvenance {
    path: PathBuf,
    selected_by: String,
}
#[derive(Debug, Deserialize)]
struct NamespaceShow {
    namespace: NamespaceDetail,
    config: ConfigProvenance,
}
#[derive(Debug, Deserialize)]
struct NamespaceDetail {
    #[serde(flatten)]
    identity: NamespaceRow,
    auth: AuthMetadata,
    write_policy: String,
    transport: TransportMetadata,
}
#[derive(Debug, Deserialize)]
struct AuthMetadata {
    scope_profile: String,
    credential_source_kinds: Vec<String>,
    one_password: Option<PasswordMetadata>,
}
#[derive(Debug, Deserialize)]
struct PasswordMetadata {
    configured_auth_mode: String,
    timeout_seconds: u64,
    service_account_profiles: Vec<String>,
    unprofiled_sources: bool,
}
#[derive(Debug, Deserialize)]
struct TransportMetadata {
    backend: String,
    cli: Option<CliTransport>,
}
#[derive(Debug, Deserialize)]
struct CliTransport {
    program: String,
    override_variable: Option<String>,
}

fn decode<T: serde::de::DeserializeOwned>(output: Output) -> T {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("typed CLI JSON")
}

#[test]
fn namespace_filters_use_exact_auth_identity_and_preserve_legacy_fields() {
    let fixture = Fixture::new();
    fs::write(
        &fixture.config,
        r#"
[namespace.google.zulu]
provider = "google"
account = "Work display label"
auth = "shared"
default_read = true
state_dir = "isolated"
auth_scope_profile = "workspace_admin"
[namespace.google.alpha]
provider = "google"
account = "Personal display label"
auth = "shared"
[auth.shared]
provider = "google"
kind = "google_cli"
account = "exact@example.com"
[namespace.messages.local]
provider = "messages"
account = "Local history"
default_read = true
"#,
    )
    .expect("fixture config");
    let all: NamespaceList = decode(fixture.run(&["ns", "list", "--json"]));
    assert_eq!(all.schema_version, 1);
    assert_eq!(all.config.path, fixture.config);
    assert_eq!(all.config.selected_by, "flag");
    assert_eq!(
        all.namespaces.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["google.alpha", "google.zulu", "messages.local"]
    );
    let selected: NamespaceList = decode(fixture.run(&[
        "ns",
        "list",
        "--provider",
        "google",
        "--account",
        "exact@example.com",
        "--default-read",
        "--json",
    ]));
    assert_eq!(selected.namespaces.len(), 1);
    let row = &selected.namespaces[0];
    assert_eq!(row.id, "google.zulu");
    assert_eq!(row.provider, ProviderKind::GoogleWorkspace);
    assert_eq!(row.account_label, "Work display label");
    assert_eq!(row.auth_account_label, "exact@example.com");
    assert_eq!(row.auth_ref, "shared");
    assert_eq!(row.auth_kind, "google_cli");
    assert!(row.default_read);
    assert_eq!(row.auth_scope_profile.as_deref(), Some("workspace_admin"));
    assert_eq!(row.state_dir.as_ref(), Some(&fixture.root.join("isolated")));
    for account in [
        "Work display label",
        "exact",
        "EXACT@example.com",
        "missing@example.com",
    ] {
        let empty: NamespaceList = decode(fixture.run(&["ns", "list", "--account", account, "--json"]));
        assert!(empty.namespaces.is_empty());
    }
    let full: NamespaceList = decode(fixture.run(&["ns", "list", "--full", "--json"]));
    assert_eq!(full.namespaces.len(), all.namespaces.len());
    assert!(all.namespaces[0].auth_scope_profile.is_none());
    assert!(all.namespaces[0].state_dir.is_some());
    assert!(!fixture
        .run(&["ns", "list", "--provider", "goog", "--json"])
        .status
        .success());
    assert!(!fixture.run(&["ns", "show", "google.zu", "--json"]).status.success());
    let shown: NamespaceShow = decode(fixture.run(&["ns", "show", "google.zulu", "--json"]));
    assert_eq!(shown.namespace.identity.id, row.id);
    assert_eq!(shown.namespace.auth.scope_profile, "workspace_admin");
    assert_eq!(shown.namespace.write_policy, "require_approval");
    assert_eq!(shown.namespace.transport.backend, "cli");
    let cli = shown.namespace.transport.cli.expect("CLI transport");
    assert_eq!(cli.program, "gws");
    assert_eq!(cli.override_variable.as_deref(), Some("SWITCHBOARD_GWS_BIN"));
    assert!(shown.namespace.auth.one_password.is_none());
    assert_eq!(shown.config.path, fixture.config);
    let local: NamespaceShow = decode(fixture.run(&["ns", "show", "messages.local", "--full", "--json"]));
    assert_eq!(local.namespace.transport.backend, "local");
    assert!(local.namespace.transport.cli.is_none());
    assert_eq!(local.namespace.auth.scope_profile, "standard");
    assert!(local.namespace.auth.credential_source_kinds.is_empty());
    assert_eq!(fs::read_dir(&fixture.root).expect("fixture contents").count(), 1);
}

#[test]
fn namespace_errors_never_echo_configuration_source_or_credential_locators() {
    let fixture = Fixture::new();
    for source in [
        "canary-secret-value = [malformed TOML",
        "[namespace.google.test]\nprovider = 'google'\naccount = 'test'\nunknown_key = 'canary-secret-value'",
        "[namespace.google.test]\nprovider = 'canary-secret-value'\naccount = 'test'",
        "[namespace.google.test]\nprovider = 'google'\naccount = 'test'\nauth = 'canary-secret-value'",
    ] {
        fs::write(&fixture.config, source).expect("invalid fixture config");
        for command in [vec!["ns", "list"], vec!["ns", "show", "google.test"]] {
            for flags in [vec![], vec!["--full"], vec!["--json"], vec!["--full", "--json"]] {
                let mut args = command.clone();
                args.extend(flags);
                let output = fixture.run(&args);
                assert!(!output.status.success());
                let combined = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(!combined.contains("canary-secret-value"), "{combined}");
                assert!(combined.contains("could not be read or validated"), "{combined}");
            }
        }
    }
    assert_eq!(fs::read_dir(&fixture.root).expect("fixture contents").count(), 1);
}

#[cfg(unix)]
#[test]
fn namespace_show_is_allowlisted_and_never_launches_clis_or_reads_credentials() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let forbidden = fixture.root.join("forbidden-cli");
    let marker = fixture.root.join("subprocess-ran");
    fs::write(
        &forbidden,
        format!("#!/bin/sh\nprintf called > '{}'\nexit 91\n", marker.display()),
    )
    .expect("forbidden executable");
    fs::set_permissions(&forbidden, fs::Permissions::from_mode(0o755)).expect("executable permissions");
    let secret_file = fixture.root.join("canary-credential-file");
    // A FIFO would block if read. No discovery command may open it.
    let created = Command::new("/usr/bin/mkfifo")
        .arg(&secret_file)
        .status()
        .expect("mkfifo");
    assert!(created.success());
    let source = format!(
        r#"
[one_password]
auth_mode = "session"
timeout_seconds = 17
[one_password.profiles.scoped]
token_file = {secret_file:?}
[secret.client_id]
kind = "env"
name = "CANARY_ENV_LOCATOR"
[secret.client_secret]
kind = "onepassword_item"
account = "canary-password-account"
item = "canary-item-locator"
vault = "canary-vault-locator"
field = "canary-field-locator"
auth_profile = "scoped"
[secret.refresh]
kind = "file"
path = {secret_file:?}
[auth.test]
provider = "google"
kind = "google_oauth"
account = "test@example.com"
client_id = "client_id"
client_secret = "client_secret"
refresh_token = "refresh"
[namespace.google.test]
provider = "google"
account = "Display label"
auth = "test"
[policy]
write = "deny"
"#
    );
    fs::write(&fixture.config, &source).expect("fixture config");
    for args in [
        vec!["ns", "list", "--json"],
        vec!["ns", "show", "google.test", "--json"],
        vec!["ns", "show", "google.test", "--full", "--json"],
        vec!["ns", "show", "google.test", "--full"],
    ] {
        let mut command = fixture.command();
        command.args(&args).env("CANARY_ENV_LOCATOR", "canary-env-value");
        for variable in [
            "SWITCHBOARD_OP_BIN",
            "SWITCHBOARD_GWS_BIN",
            "SWITCHBOARD_GH_BIN",
            "SWITCHBOARD_MYCHART_BIN",
            "SWITCHBOARD_SCHWAB_BIN",
            "SWITCHBOARD_PHONE_BIN",
        ] {
            command.env(variable, &forbidden);
        }
        let output = switchboard_core::process::output_with_timeout(&mut command, std::time::Duration::from_secs(5))
            .expect("discovery must not open credential FIFOs");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!text.contains("canary-"), "{text}");
        assert!(!text.contains("CANARY_ENV_LOCATOR"), "{text}");
        if args.contains(&"show") && args.contains(&"--json") {
            let shown: NamespaceShow = serde_json::from_slice(&output.stdout).expect("show metadata");
            assert_eq!(shown.namespace.write_policy, "deny");
            assert_eq!(
                shown.namespace.auth.credential_source_kinds,
                ["env", "file", "onepassword_item"]
            );
            let password = shown.namespace.auth.one_password.expect("1Password metadata");
            assert_eq!(password.configured_auth_mode, "session");
            assert_eq!(password.timeout_seconds, 17);
            assert_eq!(password.service_account_profiles, ["scoped"]);
            assert!(!password.unprofiled_sources);
        }
        assert!(!marker.exists());
    }
    assert_eq!(fs::read_to_string(&fixture.config).expect("unchanged config"), source);
    assert_eq!(fs::read_dir(&fixture.root).expect("fixture contents").count(), 3);
}

#[test]
fn namespace_config_provenance_matches_resolution_precedence_without_fallback_on_error() {
    let fixture = Fixture::new();
    let cwd = fixture.root.join("cwd");
    fs::create_dir(&cwd).expect("cwd");
    let paths = [
        fixture.config.clone(),
        fixture.root.join("env.toml"),
        cwd.join("switchboard.toml"),
        fixture.root.join("appdata/switchboard/config.toml"),
        fixture.root.join("xdg/switchboard/config.toml"),
        fixture.root.join("home/.config/switchboard/config.toml"),
    ];
    for path in &paths {
        fs::create_dir_all(path.parent().expect("parent")).expect("config directory");
        fs::write(
            path,
            "[namespace.google.test]\nprovider = 'google'\naccount = 'test@example.com'",
        )
        .expect("fixture config");
    }
    let invoke = |flag: bool, environment: bool| {
        let mut command = Command::new(
            env::var_os("SWITCHBOARD_DISCOVERY_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_switchboard").into()),
        );
        command
            .current_dir(&cwd)
            .env_remove("SWITCHBOARD_CONFIG")
            .env("APPDATA", fixture.root.join("appdata"))
            .env("XDG_CONFIG_HOME", fixture.root.join("xdg"))
            .env("HOME", fixture.root.join("home"))
            .env("SWITCHBOARD_STATE_DB", &fixture.root);
        if flag {
            command.arg("--config").arg(&paths[0]);
        }
        if environment {
            command.env("SWITCHBOARD_CONFIG", &paths[1]);
        }
        command
            .args(["ns", "list", "--json"])
            .output()
            .expect("discovery command")
    };
    for (index, origin) in ["flag", "environment", "cwd", "appdata", "xdg", "home"]
        .iter()
        .enumerate()
    {
        let listed: NamespaceList = decode(invoke(index == 0, index <= 1));
        assert_eq!(&listed.config.selected_by, origin);
        // A cwd-selected path may be relative, but it must refer to that same file.
        let selected = if listed.config.path.is_absolute() {
            listed.config.path
        } else {
            cwd.join(listed.config.path)
        };
        assert_eq!(selected, paths[index]);
        if index <= 1 {
            fs::write(&paths[index], "canary-invalid = [").expect("invalid config");
            let failed = invoke(index == 0, index <= 1);
            assert!(!failed.status.success(), "invalid selected config must not fall back");
            assert!(!String::from_utf8_lossy(&failed.stdout).contains("canary-invalid"));
        }
        fs::remove_file(&paths[index]).expect("remove higher-priority config");
        if index <= 1 {
            assert!(
                !invoke(index == 0, true).status.success(),
                "missing explicitly selected config must not fall back"
            );
        }
    }
    assert!(!invoke(false, false).status.success());
}
