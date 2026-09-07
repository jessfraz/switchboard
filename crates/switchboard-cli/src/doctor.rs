use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Result};
use serde::Serialize;
use switchboard_core::{AuthKind, AuthStore, NamespaceStore, ProviderKind, SecretSource, SecretStore};
use switchboard_providers::{diagnose_one_password_cli, diagnose_provider_cli, CliBinaryDiagnostic};
use switchboard_store::{
    one_password_item_cache_expiries, one_password_session_cache_entry_count, resolve_operation_store_path,
    SwitchboardConfig,
};

use crate::{args::DoctorCommand, output::render_json, resolve_config_path};

#[derive(Debug, Serialize)]
struct DoctorReport {
    status: &'static str,
    config: PathDiagnostic,
    state_dir: PathDiagnostic,
    operation_store: PathDiagnostic,
    session_cache: CacheDiagnostic,
    item_cache: CacheDiagnostic,
    one_password: Option<OnePasswordDiagnostic>,
    namespaces: Vec<NamespaceDiagnostic>,
    issues: Vec<String>,
}

#[derive(Debug, Serialize)]
struct OnePasswordDiagnostic {
    auth_mode: &'static str,
    desktop_integration_default: Option<bool>,
    timeout_seconds: u64,
    cli: Option<CliBinaryDiagnostic>,
}

#[derive(Debug, Serialize)]
struct NamespaceDiagnostic {
    namespace: String,
    provider: ProviderKind,
    auth_mode: String,
    secret_source_kinds: BTreeSet<&'static str>,
    state_dir: Option<PathDiagnostic>,
    google_storage_backend: Option<&'static str>,
    saved_auth_files: Vec<PathDiagnostic>,
    cli: Option<CliBinaryDiagnostic>,
}

#[derive(Debug, Serialize)]
struct PathDiagnostic {
    path: PathBuf,
    status: PathStatus,
    unix_mode: Option<String>,
    readonly: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PathStatus {
    Missing,
    File,
    Directory,
    Other,
    Unreadable,
}

#[derive(Debug, Serialize)]
struct CacheDiagnostic {
    file: PathDiagnostic,
    status: &'static str,
    entries: Option<usize>,
    expired_entries: Option<usize>,
    next_expiry_epoch_seconds: Option<u64>,
}

pub(crate) fn run(config_path: Option<&Path>, arguments: DoctorCommand) -> Result<String> {
    let config_path = resolve_config_path(config_path)?;
    let report = inspect(&config_path, arguments.namespace.as_deref())?;
    if arguments.json {
        render_json(&report, true)
    } else {
        Ok(report.render_human())
    }
}

fn inspect(config_path: &Path, namespace_filter: Option<&str>) -> Result<DoctorReport> {
    let operation_store = resolve_operation_store_path(config_path);
    let state_dir = operation_store.parent().unwrap_or_else(|| Path::new("."));
    let mut report = DoctorReport {
        status: "ok",
        config: inspect_path(config_path),
        state_dir: inspect_path(state_dir),
        operation_store: inspect_path(&operation_store),
        session_cache: inspect_session_cache(&state_dir.join("onepassword-sessions.json")),
        item_cache: inspect_item_cache(&state_dir.join("onepassword-items.json")),
        one_password: None,
        namespaces: Vec::new(),
        issues: Vec::new(),
    };
    let config = match SwitchboardConfig::from_file(config_path) {
        Ok(config) => config,
        Err(_) => {
            // TOML errors can include source lines containing credentials. Keep diagnostics source-free.
            report
                .issues
                .push("Configuration could not be read or validated; check the config path and TOML fields.".into());
            report.status = "issues_found";
            return Ok(report);
        }
    };
    report.one_password = Some(OnePasswordDiagnostic {
        auth_mode: config.one_password.auth_mode.as_str(),
        desktop_integration_default: config.one_password.desktop_integration(),
        timeout_seconds: config.one_password.timeout_seconds,
        cli: None,
    });
    let (namespaces, auth, secrets) = config.into_stores();
    let selected = namespaces
        .list()
        .into_iter()
        .filter(|namespace| namespace_filter.map_or(true, |filter| namespace.id.as_str() == filter))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(anyhow!("the requested namespace is not configured"));
    }
    let mut provider_diagnostics = BTreeMap::new();
    let mut needs_one_password = false;
    for namespace in selected {
        let auth = auth
            .get(&namespace.auth_ref)
            .ok_or_else(|| anyhow!("namespace auth is missing"))?;
        let source_kinds: BTreeSet<_> = auth
            .secret_refs()
            .into_iter()
            .filter_map(|reference| secrets.get(reference))
            .map(|secret| match secret.source {
                SecretSource::Env { .. } => "env",
                SecretSource::File { .. } => "file",
                SecretSource::OnePasswordItem { .. } => "onepassword_item",
            })
            .collect();
        needs_one_password |= source_kinds.contains("onepassword_item");
        let google = namespace.provider == ProviderKind::GoogleWorkspace;
        let saved_auth_files = if google {
            namespace
                .state_dir
                .as_deref()
                .map(google_auth_files)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if google && auth.kind == AuthKind::GoogleCli && !saved_auth_files.iter().any(is_saved_credential) {
            report.issues.push(format!(
                "{}: no saved Google credentials found; run switchboard google.cli.write --ns {} -- auth login after configuring the OAuth client.",
                namespace.id, namespace.id
            ));
        }
        let diagnostic = provider_diagnostics
            .entry(namespace.provider.clone())
            .or_insert_with(|| diagnose_provider_cli(namespace.provider.clone()));
        let cli = match diagnostic {
            Ok(cli) => {
                if let Some(issue) = cli.issue {
                    report.issues.push(format!("{}: {issue}", namespace.id));
                }
                Some(cli.clone())
            }
            Err(_) => {
                report
                    .issues
                    .push(format!("{}: provider CLI is unavailable", namespace.id));
                None
            }
        };
        report.namespaces.push(NamespaceDiagnostic {
            namespace: namespace.id.to_string(),
            provider: namespace.provider,
            auth_mode: auth.kind.to_string(),
            secret_source_kinds: source_kinds,
            state_dir: namespace.state_dir.as_deref().map(inspect_path),
            google_storage_backend: google.then_some("file"),
            saved_auth_files,
            cli,
        });
    }
    if needs_one_password {
        if let Some(one_password) = &mut report.one_password {
            let cli = diagnose_one_password_cli();
            if let Some(issue) = cli.issue {
                report.issues.push(format!("1Password: {issue}"));
            }
            one_password.cli = Some(cli);
        }
    }
    for cache in [&report.session_cache, &report.item_cache] {
        if !matches!(cache.status, "ok" | "missing") {
            report
                .issues
                .push(format!("{}: {}", cache.file.path.display(), cache.status));
        }
    }
    if !report.issues.is_empty() {
        report.status = "issues_found";
    }
    Ok(report)
}

fn inspect_path(path: &Path) -> PathDiagnostic {
    let mut diagnostic = PathDiagnostic {
        path: path.to_path_buf(),
        status: PathStatus::Missing,
        unix_mode: None,
        readonly: None,
    };
    match fs::metadata(path) {
        Ok(metadata) => {
            diagnostic.status = if metadata.is_file() {
                PathStatus::File
            } else if metadata.is_dir() {
                PathStatus::Directory
            } else {
                PathStatus::Other
            };
            diagnostic.readonly = Some(metadata.permissions().readonly());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                diagnostic.unix_mode = Some(format!("{:04o}", metadata.permissions().mode() & 0o777));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => diagnostic.status = PathStatus::Unreadable,
    }
    diagnostic
}

fn google_auth_files(directory: &Path) -> Vec<PathDiagnostic> {
    // These are the filenames used by the pinned gws 0.22.5. Do not decrypt or read them.
    [
        "credentials.enc",
        "credentials.json",
        ".encryption_key",
        "client_secret.json",
        "token_cache.json",
    ]
    .map(|name| inspect_path(&directory.join(name)))
    .into_iter()
    .collect()
}

fn is_saved_credential(file: &PathDiagnostic) -> bool {
    file.status == PathStatus::File
        && file
            .path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("credentials."))
}

fn empty_cache_diagnostic(path: &Path) -> CacheDiagnostic {
    let file = inspect_path(path);
    let status = match file.status {
        PathStatus::Missing => "missing",
        PathStatus::File => "ok",
        _ => "unreadable",
    };
    CacheDiagnostic {
        file,
        status,
        entries: None,
        expired_entries: None,
        next_expiry_epoch_seconds: None,
    }
}

fn read_cache_metadata<T>(diagnostic: &mut CacheDiagnostic, parse: impl FnOnce(&str) -> Option<T>) -> Option<T> {
    if diagnostic.status != "ok" {
        return None;
    }
    let contents = match fs::read_to_string(&diagnostic.file.path) {
        Ok(contents) => contents,
        Err(_) => {
            diagnostic.status = "unreadable";
            return None;
        }
    };
    match parse(&contents) {
        Some(metadata) => Some(metadata),
        None => {
            diagnostic.status = "invalid_json_or_format";
            None
        }
    }
}

fn inspect_session_cache(path: &Path) -> CacheDiagnostic {
    let mut diagnostic = empty_cache_diagnostic(path);
    diagnostic.entries = read_cache_metadata(&mut diagnostic, one_password_session_cache_entry_count);
    diagnostic
}

fn inspect_item_cache(path: &Path) -> CacheDiagnostic {
    let mut diagnostic = empty_cache_diagnostic(path);
    if let Some(expiries) = read_cache_metadata(&mut diagnostic, one_password_item_cache_expiries) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        diagnostic.entries = Some(expiries.len());
        diagnostic.expired_entries = Some(expiries.iter().filter(|expiry| **expiry <= now).count());
        diagnostic.next_expiry_epoch_seconds = expiries.into_iter().filter(|expiry| *expiry > now).min();
    }
    diagnostic
}

impl DoctorReport {
    fn render_human(&self) -> String {
        let mut output = format!("Switchboard doctor: {}\n", self.status);
        for (label, path) in [
            ("Config", &self.config),
            ("State directory", &self.state_dir),
            ("Operation store", &self.operation_store),
        ] {
            output.push_str(&format!("{label}: {}\n", path.render_human()));
        }
        if let Some(one_password) = &self.one_password {
            output.push_str(&format!(
                "1Password: auth={}, timeout={}s",
                one_password.auth_mode, one_password.timeout_seconds
            ));
            if let Some(enabled) = one_password.desktop_integration_default {
                output.push_str(&format!(", desktop integration default={enabled}"));
            } else {
                output.push_str(", authentication follows the existing CLI environment");
            }
            output.push('\n');
            if let Some(cli) = &one_password.cli {
                output.push_str(&format!("  CLI: {}", cli.program));
                if let Some(path) = &cli.path {
                    output.push_str(&format!(" at {}", path.display()));
                }
                if let Some(version) = &cli.version {
                    output.push_str(&format!(", version {version}"));
                }
                output.push('\n');
            }
        }
        for (label, cache) in [
            ("1Password session cache", &self.session_cache),
            ("1Password item cache", &self.item_cache),
        ] {
            output.push_str(&format!("{label}: {} ({})", cache.file.render_human(), cache.status));
            if let Some(entries) = cache.entries {
                output.push_str(&format!(", {entries} entries"));
            }
            if let Some(expired) = cache.expired_entries {
                output.push_str(&format!(", {expired} expired"));
            }
            output.push('\n');
        }
        for namespace in &self.namespaces {
            output.push_str(&format!(
                "\n{}: provider={}, auth={}\n",
                namespace.namespace, namespace.provider, namespace.auth_mode
            ));
            if !namespace.secret_source_kinds.is_empty() {
                output.push_str(&format!(
                    "  Secret sources: {}\n",
                    namespace
                        .secret_source_kinds
                        .iter()
                        .copied()
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if let Some(directory) = &namespace.state_dir {
                output.push_str(&format!("  State directory: {}\n", directory.render_human()));
            }
            if let Some(backend) = namespace.google_storage_backend {
                output.push_str(&format!("  Google credential backend: {backend} (enforced)\n"));
            }
            if let Some(cli) = &namespace.cli {
                output.push_str(&format!("  CLI: {}", cli.program));
                if let Some(path) = &cli.path {
                    output.push_str(&format!(" at {}", path.display()));
                }
                if let Some(version) = &cli.version {
                    output.push_str(&format!(", version {version}"));
                }
                output.push('\n');
            }
            for file in &namespace.saved_auth_files {
                output.push_str(&format!("  Saved auth: {}\n", file.render_human()));
            }
        }
        for issue in &self.issues {
            output.push_str(&format!("\nIssue: {issue}\n"));
        }
        output.push_str("\nSaved-file presence does not verify credential validity. No secrets resolved or authentication attempted.\n");
        output
    }
}

impl PathDiagnostic {
    fn render_human(&self) -> String {
        let status = match self.status {
            PathStatus::Missing => "missing",
            PathStatus::File => "file",
            PathStatus::Directory => "directory",
            PathStatus::Other => "other",
            PathStatus::Unreadable => "unreadable",
        };
        let mode = self
            .unix_mode
            .as_ref()
            .map(|mode| format!(", mode {mode}"))
            .unwrap_or_default();
        format!("{} ({status}{mode})", self.path.display())
    }
}

#[cfg(test)]
mod tests {
    use std::{env, ffi::OsString, fs, path::Path};

    use clap::Parser;

    use crate::{
        args::Cli,
        doctor::{inspect, inspect_item_cache, inspect_path, inspect_session_cache, PathStatus},
        run,
        test_support::{lock_env, TempScript},
    };

    struct EnvGuard {
        name: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(name: &'static str, value: Option<&Path>) -> Self {
            let previous = env::var_os(name);
            match value {
                Some(value) => env::set_var(name, value),
                None => env::remove_var(name),
            }
            Self { name, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => env::set_var(self.name, value),
                None => env::remove_var(self.name),
            }
        }
    }

    #[test]
    fn doctor_inspects_saved_state_without_authenticating_resolving_secrets_or_opening_database() {
        let _lock = lock_env();
        let script = TempScript::new(
            "gws",
            r#"#!/bin/sh
if [ "$*" != "--version" ] || [ "$GOOGLE_WORKSPACE_CLI_KEYRING_BACKEND" != file ]; then
    exit 1
fi
printf 'gws 0.22.5\nSENSITIVE_PROCESS_OUTPUT\n'
"#,
        );
        let root = script.path().parent().expect("fixture has a directory");
        let config_path = root.join("switchboard.toml");
        let state = root.join(".switchboard");
        let google = root.join("google");
        fs::create_dir_all(&state).expect("cache directory should exist");
        fs::create_dir_all(&google).expect("Google directory should exist");
        fs::write(google.join("credentials.enc"), "SENSITIVE_CREDENTIALS").expect("credentials should exist");
        let session_bytes = r#"{"sessions":{"private-account":{"kind":"token","token":"SENSITIVE_SESSION"}}}"#;
        fs::write(state.join("onepassword-sessions.json"), session_bytes).expect("session cache should exist");
        fs::write(
            &config_path,
            r#"
[secret.client_id]
kind = "env"
name = "SWITCHBOARD_DOCTOR_UNSET_SECRET"
[secret.client_secret]
kind = "file"
path = "missing-secret.txt"
[auth.google_personal]
provider = "google"
kind = "google_oauth"
account = "private-account"
client_id = "client_id"
client_secret = "client_secret"
[namespace.google.personal]
provider = "google"
account = "private-account"
auth = "google_personal"
state_dir = "google"
"#,
        )
        .expect("config should exist");
        let _binary = EnvGuard::set("SWITCHBOARD_GWS_BIN", Some(script.path()));
        let _db = EnvGuard::set("SWITCHBOARD_STATE_DB", None);
        let _state = EnvGuard::set("SWITCHBOARD_STATE_DIR", None);
        let report = inspect(&config_path, Some("google.personal")).expect("doctor should inspect the config");
        assert_eq!(report.status, "ok");
        assert_eq!(report.session_cache.entries, Some(1));
        assert!(report
            .one_password
            .as_ref()
            .is_some_and(|settings| settings.cli.is_none()));
        assert_eq!(report.namespaces.len(), 1);
        assert_eq!(report.namespaces[0].auth_mode, "google_oauth");
        assert_eq!(
            report.namespaces[0].secret_source_kinds,
            ["env", "file"].into_iter().collect()
        );
        assert_eq!(
            report.namespaces[0].cli.as_ref().and_then(|cli| cli.version.as_deref()),
            Some("0.22.5")
        );
        for json in [false, true] {
            let mut argv = vec![
                OsString::from("switchboard"),
                OsString::from("--config"),
                config_path.clone().into_os_string(),
                OsString::from("doctor"),
                OsString::from("--ns"),
                OsString::from("google.personal"),
            ];
            if json {
                argv.push(OsString::from("--json"));
            }
            let output =
                run(Cli::try_parse_from(argv).expect("doctor arguments should parse")).expect("doctor should run");
            assert!(!output.contains("SENSITIVE_"));
            assert!(!output.contains("private-account"));
        }
        assert!(!state.join("operations.sqlite3").exists());
        assert_eq!(fs::read_dir(&state).expect("cache directory should exist").count(), 1);
        assert_eq!(
            fs::read_to_string(state.join("onepassword-sessions.json")).expect("cache should be readable"),
            session_bytes
        );
        assert_eq!(
            fs::read_to_string(google.join("credentials.enc")).expect("credentials should be untouched"),
            "SENSITIVE_CREDENTIALS"
        );
    }

    #[test]
    fn doctor_reports_fresh_setup_without_creating_state() {
        let _lock = lock_env();
        let script = TempScript::new("gws", "#!/bin/sh\necho 'gws 0.22.5'\n");
        let root = script.path().parent().expect("fixture has a directory");
        let config_path = root.join("switchboard.toml");
        fs::write(
            &config_path,
            r#"
[namespace.google.personal]
provider = "google"
account = "personal"
"#,
        )
        .expect("config should exist");
        let _binary = EnvGuard::set("SWITCHBOARD_GWS_BIN", Some(script.path()));
        let _db = EnvGuard::set("SWITCHBOARD_STATE_DB", None);
        let _state = EnvGuard::set("SWITCHBOARD_STATE_DIR", None);
        let report = inspect(&config_path, None).expect("doctor should inspect a fresh config");
        assert_eq!(report.status, "issues_found");
        assert_eq!(report.session_cache.status, "missing");
        assert_eq!(report.namespaces[0].auth_mode, "google_cli");
        assert_eq!(report.namespaces[0].google_storage_backend, Some("file"));
        assert!(report.issues.iter().any(|issue| issue.contains("auth login")));
        assert!(!root.join(".switchboard").exists());
        assert!(inspect(&config_path, Some("google.unknown")).is_err());
    }

    #[test]
    fn malformed_config_diagnostics_never_include_source_lines() {
        let _lock = lock_env();
        let fixture = TempScript::new("config.toml", "SENSITIVE_CONFIG_VALUE = [malformed TOML");
        let _db = EnvGuard::set("SWITCHBOARD_STATE_DB", None);
        let _state = EnvGuard::set("SWITCHBOARD_STATE_DIR", None);
        let report = inspect(fixture.path(), None).expect("invalid config should yield diagnostics");
        assert_eq!(report.status, "issues_found");
        assert!(!report.render_human().contains("SENSITIVE_CONFIG_VALUE"));
        assert!(!serde_json::to_string(&report)
            .expect("diagnostics should serialize")
            .contains("SENSITIVE_CONFIG_VALUE"));
    }

    #[test]
    fn cache_diagnostics_count_expired_entries_without_pruning_or_exposing_values() {
        let fixture = TempScript::new(
            "cache.json",
            r#"{"items":{"sensitive-item-name":{"expires_at_epoch_seconds":1,"fields":{"secret":"SENSITIVE_ITEM"}},"another":{"expires_at_epoch_seconds":18446744073709551615,"fields":{}}}}"#,
        );
        let before = fs::read(fixture.path()).expect("cache should exist");
        let diagnostic = inspect_item_cache(fixture.path());
        assert_eq!(diagnostic.status, "ok");
        assert_eq!(diagnostic.entries, Some(2));
        assert_eq!(diagnostic.expired_entries, Some(1));
        assert_eq!(diagnostic.next_expiry_epoch_seconds, Some(u64::MAX));
        let serialized = serde_json::to_string(&diagnostic).expect("diagnostics should serialize");
        assert!(!serialized.contains("SENSITIVE_ITEM"));
        assert!(!serialized.contains("sensitive-item-name"));
        assert_eq!(fs::read(fixture.path()).expect("cache should exist"), before);
        fs::write(fixture.path(), r#"{"sessions":{"secret":"SENSITIVE_BROKEN_JSON""#).expect("fixture should update");
        assert_eq!(inspect_session_cache(fixture.path()).status, "invalid_json_or_format");
    }

    #[test]
    fn cache_diagnostics_reject_formats_the_secret_backend_cannot_load() {
        let fixture = TempScript::new("cache.json", "{}");
        assert_eq!(inspect_session_cache(fixture.path()).entries, Some(0));
        assert_eq!(inspect_item_cache(fixture.path()).entries, Some(0));
        for contents in [
            r#"{"sessions":{"account":{"kind":"typo","token":"SENSITIVE_TOKEN"}}}"#,
            r#"{"sessions":{"account":{"kind":"token","token":123}}}"#,
        ] {
            fs::write(fixture.path(), contents).expect("fixture should update");
            let diagnostic = inspect_session_cache(fixture.path());
            assert_eq!(diagnostic.status, "invalid_json_or_format");
            assert!(!serde_json::to_string(&diagnostic)
                .expect("report should serialize")
                .contains("SENSITIVE_TOKEN"));
        }
        for contents in [
            r#"{"items":{"item":{"expires_at_epoch_seconds":1,"fields":{"secret":123}}}}"#,
            r#"{"items":{"item":{"expires_at_epoch_seconds":1,"fields":[]}}}"#,
        ] {
            fs::write(fixture.path(), contents).expect("fixture should update");
            assert_eq!(inspect_item_cache(fixture.path()).status, "invalid_json_or_format");
        }
    }

    #[test]
    fn doctor_probes_required_one_password_binary_without_authentication() {
        let _lock = lock_env();
        let gws = TempScript::new("gws", "#!/bin/sh\necho 'gws 0.22.5'\n");
        let op = TempScript::new(
            "op",
            r#"#!/bin/sh
if [ "$*" != "--version" ]; then exit 73; fi
printf '%s' "$*" > "$(dirname "$0")/env.txt"
printf '2.32.0\nSENSITIVE_OP_OUTPUT\n'
"#,
        );
        let root = gws.path().parent().expect("fixture has a directory");
        let config_path = root.join("switchboard.toml");
        fs::write(
            &config_path,
            r#"
[secret.credentials]
kind = "onepassword_item"
account = "private-account"
item = "private-item"
field = "private-field"
[auth.google_personal]
provider = "google"
account = "personal"
kind = "google_oauth_file"
credentials = "credentials"
[namespace.google.personal]
provider = "google"
account = "personal"
auth = "google_personal"
"#,
        )
        .expect("config should exist");
        let _gws = EnvGuard::set("SWITCHBOARD_GWS_BIN", Some(gws.path()));
        let _db = EnvGuard::set("SWITCHBOARD_STATE_DB", None);
        let _state = EnvGuard::set("SWITCHBOARD_STATE_DIR", None);
        {
            let _op = EnvGuard::set("SWITCHBOARD_OP_BIN", Some(&root.join("missing-op")));
            let report = inspect(&config_path, None).expect("doctor should inspect the config");
            assert_eq!(report.status, "issues_found");
            assert!(report
                .issues
                .iter()
                .any(|issue| issue.contains("1Password: CLI not found")));
        }
        let _op = EnvGuard::set("SWITCHBOARD_OP_BIN", Some(op.path()));
        let report = inspect(&config_path, None).expect("doctor should inspect the config");
        assert_eq!(report.status, "ok");
        let diagnostic = report
            .one_password
            .as_ref()
            .and_then(|settings| settings.cli.as_ref())
            .expect("op diagnostic should exist");
        assert_eq!(diagnostic.path.as_deref(), Some(op.path()));
        assert_eq!(diagnostic.version.as_deref(), Some("2.32.0"));
        assert_eq!(op.capture_contents(), "--version");
        assert!(!report.render_human().contains("SENSITIVE_OP_OUTPUT"));
        assert!(!root.join(".switchboard").exists());
        let op_directory = op.path().parent().expect("fixture has a directory");
        let existing_path = env::var_os("PATH").unwrap_or_default();
        let search_path =
            env::join_paths(std::iter::once(op_directory.to_path_buf()).chain(env::split_paths(&existing_path)))
                .expect("PATH should be valid");
        let _path = EnvGuard::set("PATH", Some(Path::new(&search_path)));
        let _bare_override = EnvGuard::set("SWITCHBOARD_OP_BIN", Some(Path::new("op")));
        let report = inspect(&config_path, None).expect("bare op override should resolve on PATH");
        assert_eq!(
            report
                .one_password
                .and_then(|settings| settings.cli)
                .and_then(|cli| cli.path)
                .as_deref(),
            Some(op.path())
        );
        let _empty_override = EnvGuard::set("SWITCHBOARD_OP_BIN", Some(Path::new("")));
        let report = inspect(&config_path, None).expect("empty override should be diagnosed");
        assert_eq!(report.status, "issues_found");
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.contains("SWITCHBOARD_OP_BIN is empty")));
    }

    #[test]
    fn path_diagnostics_report_missing_and_existing_paths_without_opening_contents() {
        let fixture = TempScript::new("sensitive-file", "SENSITIVE_FILE");
        let root = fixture.path().parent().expect("fixture has a directory");
        assert_eq!(inspect_path(root).status, PathStatus::Directory);
        assert_eq!(inspect_path(&root.join("missing")).status, PathStatus::Missing);
        assert_eq!(inspect_path(fixture.path()).status, PathStatus::File);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o400)).expect("permissions should update");
            let diagnostic = inspect_path(fixture.path());
            assert_eq!(diagnostic.unix_mode.as_deref(), Some("0400"));
            assert_eq!(diagnostic.readonly, Some(true));
        }
    }
}
