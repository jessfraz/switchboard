#![cfg(unix)]

use std::{
    env, fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use switchboard_core::{Error, SecretRef, SecretResolver, SecretStore};
use switchboard_store::{LocalSecretResolver, SwitchboardConfig};

static ENV_LOCK: Mutex<()> = Mutex::new(());
static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = env::temp_dir().join(format!(
            "switchboard-profile-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("profile fixture succeeds")
                .as_nanos(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("profile fixture succeeds");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("profile fixture succeeds");
        let script = root.join("op");
        fs::write(
            &script,
            r#"#!/bin/sh
set -eu
test "$1" = item && test "$2" = get || exit 41
test "$4" = --vault && test "$5" = vault-id || exit 42
test "$6" = --fields && test "$7" = label=token || exit 43
test "$8" = --reveal || exit 44
test "$OP_BIOMETRIC_UNLOCK_ENABLED" = false || exit 45
test -z "${OP_CONNECT_HOST+x}" && test -z "${OP_CONNECT_TOKEN+x}" || exit 46
test -z "${OP_SESSION+x}" && test -z "${OP_SESSION_fixture+x}" || exit 47
test -z "${OP_ACCOUNT+x}" || exit 48
printf '%s\n' lookup >> "$(dirname "$0")/lookups"
directory="$(dirname "$0")"
if test -f "$directory/delay"; then sleep "$(cat "$directory/delay")"; fi
if test -f "$directory/error"; then
  cat "$directory/error" >&2
  printf '\n%s\n' "$OP_SERVICE_ACCOUNT_TOKEN" >&2
  exit 1
fi
case "$OP_SERVICE_ACCOUNT_TOKEN" in
  personal-bootstrap) printf '%s\n' personal-provider ;;
  work-bootstrap) printf '%s\n' work-provider ;;
  rotated-bootstrap) printf '%s\n' rotated-provider ;;
  *) printf '%s\n' "$OP_SERVICE_ACCOUNT_TOKEN" >&2; exit 51 ;;
esac
"#,
        )
        .expect("profile fixture succeeds");
        fs::set_permissions(script, fs::Permissions::from_mode(0o700)).expect("profile fixture succeeds");
        Self { root }
    }

    fn token(&self, name: &str, value: &str) {
        let path = self.root.join(format!("{name}.token"));
        fs::write(&path, value).expect("profile fixture succeeds");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("profile fixture succeeds");
    }

    fn config(&self) -> SwitchboardConfig {
        let config_path = self.root.join("config.toml");
        fs::write(
            &config_path,
            r#"
[one_password.profiles.personal]
token_file = "personal.token"
[namespace.github.fixture]
provider = "github"
account = "fixture"
auth = "fixture"
[auth.fixture]
provider = "github"
account = "fixture"
kind = "gh_cli"
[one_password.profiles.work]
token_file = "work.token"
[secret.personal]
kind = "onepassword_item"
account = "same.1password.com"
vault = "vault-id"
item = "item-id"
field = "token"
auth_profile = "personal"
[secret.work]
kind = "onepassword_item"
account = "same.1password.com"
vault = "vault-id"
item = "item-id"
field = "token"
auth_profile = "work"
[secret.personal_other]
kind = "onepassword_item"
account = "same.1password.com"
vault = "vault-id"
item = "other-item"
field = "token"
auth_profile = "personal"
[secret.other_account]
kind = "onepassword_item"
account = "other.1password.com"
vault = "vault-id"
item = "item-id"
field = "token"
auth_profile = "work"
"#,
        )
        .expect("profile fixture succeeds");
        SwitchboardConfig::from_file(config_path).expect("profile config is supported")
    }

    fn resolver(&self, config: &SwitchboardConfig) -> LocalSecretResolver {
        LocalSecretResolver::with_one_password_config(
            Some(self.root.join("onepassword-sessions.json")),
            config.one_password.clone(),
        )
    }

    fn environment(&self) -> Vec<EnvGuard> {
        vec![
            EnvGuard::new("SWITCHBOARD_OP_BIN", self.root.join("op").as_os_str()),
            EnvGuard::new("OP_SERVICE_ACCOUNT_TOKEN", "ambient-bootstrap"),
            EnvGuard::new("OP_CONNECT_HOST", "https://ambient.invalid"),
            EnvGuard::new("OP_CONNECT_TOKEN", "ambient-connect"),
            EnvGuard::new("OP_SESSION", "ambient-session"),
            EnvGuard::new("OP_SESSION_fixture", "ambient-account-session"),
            EnvGuard::new("OP_ACCOUNT", "ambient-account"),
            EnvGuard::new("OP_BIOMETRIC_UNLOCK_ENABLED", "true"),
        ]
    }

    fn worker(&self, secret: &str) -> Child {
        self.worker_with_timeout(secret, 30)
    }

    fn worker_with_timeout(&self, secret: &str, seconds: u64) -> Child {
        Command::new(env::current_exe().expect("profile test executable"))
            .args(["--exact", "profile_worker", "--nocapture"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.root)
            .env("PROFILE_FIXTURE", &self.root)
            .env("PROFILE_SECRET", secret)
            .env("PROFILE_TIMEOUT", seconds.to_string())
            .env("SWITCHBOARD_OP_BIN", self.root.join("op"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("profile fixture worker starts")
    }

    fn lookup(&self, secret: &str) -> Lookup {
        worker_result(self.worker(secret).wait_with_output().expect("profile worker finishes"))
    }

    fn lookups(&self) -> usize {
        fs::read_to_string(self.root.join("lookups"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    fn quota(&self, scope: &str, delay: &str) {
        fs::write(self.root.join("error"), format!("Error: [ERROR] (429) Too Many Requests: You've reached the maximum number of this type of requests this {scope} is allowed to make. Please retry in {delay} or try other requests.")).expect("quota fixture succeeds");
    }
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Lookup {
    Value(String),
    Limited(u64),
    TimedOut(u64),
    Failed(String),
}

fn worker_result(output: Output) -> Lookup {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8(output.stdout).expect("fixture stdout");
    let encoded = stdout
        .lines()
        .find_map(|line| line.strip_prefix("LOOKUP "))
        .expect("worker result");
    serde_json::from_str(encoded).expect("typed worker result")
}

#[test]
fn profile_worker() {
    let Some(root) = env::var_os("PROFILE_FIXTURE").map(PathBuf::from) else {
        return;
    };
    let mut config = SwitchboardConfig::from_file(root.join("config.toml")).expect("worker config");
    config.one_password.timeout_seconds = env::var("PROFILE_TIMEOUT")
        .expect("worker timeout")
        .parse()
        .expect("numeric worker timeout");
    let resolver = LocalSecretResolver::with_one_password_config(
        Some(root.join("onepassword-sessions.json")),
        config.one_password.clone(),
    );
    let (_, _, secrets) = config.into_stores();
    let name = env::var("PROFILE_SECRET").expect("worker secret name");
    let secret = secrets
        .get(&SecretRef::new(name).expect("worker secret ref"))
        .expect("worker secret");
    let result = match resolver.resolve(&secret) {
        Ok(value) => Lookup::Value(value.expose().into()),
        Err(Error::RateLimited {
            retry_after_seconds, ..
        }) => Lookup::Limited(retry_after_seconds),
        Err(Error::AuthenticationTimeout { seconds }) => Lookup::TimedOut(seconds),
        Err(error) => Lookup::Failed(error.to_string()),
    };
    println!(
        "LOOKUP {}",
        serde_json::to_string(&result).expect("worker result serialization")
    );
}

fn process_fixture() -> Fixture {
    let fixture = Fixture::new();
    fixture.token("personal", "personal-bootstrap");
    fixture.token("work", "work-bootstrap");
    fixture.config();
    fixture
}

fn assert_limited(result: Lookup, maximum: u64) {
    assert!(
        matches!(result, Lookup::Limited(seconds) if seconds > 0 && seconds <= maximum + 1),
        "{result:?}"
    );
}

#[test]
fn concurrent_processes_fetch_a_cold_profile_field_once() {
    let fixture = process_fixture();
    fs::write(fixture.root.join("delay"), "1").expect("delayed lookup fixture");
    let first = fixture.worker("personal");
    let second = fixture.worker("personal");
    for worker in [first, second] {
        assert_eq!(
            worker_result(worker.wait_with_output().expect("worker finishes")),
            Lookup::Value("personal-provider".into())
        );
    }
    assert_eq!(fixture.lookups(), 1);
}

#[test]
fn process_waiting_for_a_profile_lock_obeys_its_timeout() {
    let fixture = process_fixture();
    fs::write(fixture.root.join("delay"), "3").expect("delayed lookup fixture");
    let first = fixture.worker("personal");
    let started = Instant::now();
    while fixture.lookups() == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "first worker starts its lookup"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let waiting = Instant::now();
    let second = fixture.worker_with_timeout("personal", 1);
    assert_eq!(
        worker_result(second.wait_with_output().expect("waiting worker finishes")),
        Lookup::TimedOut(1)
    );
    assert!(waiting.elapsed() < Duration::from_secs(3), "lock wait stays bounded");
    assert_eq!(fixture.lookups(), 1);
    assert_eq!(
        worker_result(first.wait_with_output().expect("first worker finishes")),
        Lookup::Value("personal-provider".into())
    );
    assert_eq!(fixture.lookups(), 1);
}

#[test]
fn service_quota_is_shared_across_items_but_not_profiles_or_generations() {
    let fixture = process_fixture();
    fixture.quota("service account", "59 minutes");
    assert_limited(fixture.lookup("personal"), 3540);
    assert_limited(fixture.lookup("personal_other"), 3540);
    assert_eq!(fixture.lookups(), 1);
    fs::remove_file(fixture.root.join("error")).expect("remove quota fixture");
    assert_eq!(fixture.lookup("work"), Lookup::Value("work-provider".into()));
    fixture.token("personal", "rotated-bootstrap");
    assert_eq!(fixture.lookup("personal"), Lookup::Value("rotated-provider".into()));
    assert_eq!(fixture.lookups(), 3);
    let metadata = fs::read_to_string(fixture.root.join("onepassword-limits.json")).expect("cooldown metadata");
    for forbidden in ["bootstrap", "provider", "item-id", "Too Many Requests"] {
        assert!(!metadata.contains(forbidden));
    }
}

#[test]
fn daily_quota_is_shared_only_by_the_exact_configured_account() {
    let fixture = process_fixture();
    fixture.quota("1Password account", "23 hours and 59 minutes");
    assert_limited(fixture.lookup("personal"), 86340);
    assert_limited(fixture.lookup("work"), 86340);
    assert_eq!(fixture.lookups(), 1);
    fs::remove_file(fixture.root.join("error")).expect("remove quota fixture");
    assert_eq!(fixture.lookup("other_account"), Lookup::Value("work-provider".into()));
    assert_eq!(fixture.lookups(), 2);
}

#[test]
fn valid_cached_field_stays_usable_during_quota_cooldown() {
    let fixture = process_fixture();
    assert_eq!(fixture.lookup("personal"), Lookup::Value("personal-provider".into()));
    fixture.quota("service account", "59 minutes");
    assert_limited(fixture.lookup("personal_other"), 3540);
    assert_eq!(fixture.lookup("personal"), Lookup::Value("personal-provider".into()));
    assert_eq!(fixture.lookups(), 2);
    fs::remove_file(fixture.root.join("personal.token")).expect("remove bootstrap fixture");
    assert!(matches!(fixture.lookup("personal"), Lookup::Failed(_)));
    assert_eq!(fixture.lookups(), 2);
}

#[test]
fn expired_quota_allows_a_new_lookup() {
    let fixture = process_fixture();
    fixture.quota("service account", "1 second");
    assert_limited(fixture.lookup("personal"), 1);
    // Persisted deadlines round upward to avoid a retry before the full delay.
    std::thread::sleep(Duration::from_secs(2));
    fs::remove_file(fixture.root.join("error")).expect("remove quota fixture");
    assert_eq!(fixture.lookup("personal"), Lookup::Value("personal-provider".into()));
    assert_eq!(fixture.lookups(), 2);
}

#[test]
fn unsupported_quota_duration_is_safe_without_inventing_a_deadline() {
    for duration in [
        "soon",
        "999999 hours",
        "0 seconds",
        "2 minutes and 80 seconds",
        "1 hour and",
        "-1 minutes",
    ] {
        let fixture = process_fixture();
        fixture.quota("service account", duration);
        let Lookup::Failed(reason) = fixture.lookup("personal") else {
            panic!("unknown quota delay must remain explicit");
        };
        assert!(reason.contains("quota limit"), "{reason}");
        assert!(!reason.contains("personal-bootstrap"));
        assert!(!fixture.root.join("onepassword-limits.json").exists());
        assert_eq!(fixture.lookups(), 1);
    }
}

#[test]
fn memory_only_resolver_also_respects_quota() {
    let _lock = ENV_LOCK.lock().expect("profile fixture succeeds");
    let fixture = process_fixture();
    let _environment = fixture.environment();
    fixture.quota("service account", "59 minutes");
    let config = fixture.config();
    let resolver = LocalSecretResolver::with_one_password_config(None, config.one_password.clone());
    let (_, _, secrets) = config.into_stores();
    for name in ["personal", "personal_other"] {
        let secret = secrets
            .get(&SecretRef::new(name).expect("fixture secret ref"))
            .expect("fixture secret");
        assert!(matches!(resolver.resolve(&secret), Err(Error::RateLimited { .. })));
    }
    assert_eq!(fixture.lookups(), 1);
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct EnvGuard {
    name: &'static str,
    previous: Option<std::ffi::OsString>,
}
impl EnvGuard {
    fn new(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = env::var_os(name);
        env::set_var(name, value);
        Self { name, previous }
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => env::set_var(self.name, value),
            None => env::remove_var(self.name),
        }
    }
}

#[test]
fn concurrent_profiles_select_tokens_and_do_not_share_cached_credentials() {
    let _lock = ENV_LOCK.lock().expect("profile fixture succeeds");
    let fixture = Fixture::new();
    let _environment = fixture.environment();
    fixture.token("personal", "personal-bootstrap");
    fixture.token("work", "work-bootstrap");
    let config = fixture.config();
    let resolver = fixture.resolver(&config);
    let (_, _, secrets) = config.into_stores();
    let personal = secrets
        .get(&SecretRef::new("personal").expect("profile fixture succeeds"))
        .expect("profile fixture succeeds");
    let work = secrets
        .get(&SecretRef::new("work").expect("profile fixture succeeds"))
        .expect("profile fixture succeeds");
    std::thread::scope(|scope| {
        let first = scope.spawn(|| resolver.resolve(&personal).expect("profile fixture succeeds"));
        let second = scope.spawn(|| resolver.resolve(&work).expect("profile fixture succeeds"));
        assert_eq!(
            first.join().expect("profile fixture succeeds").expose(),
            "personal-provider"
        );
        assert_eq!(
            second.join().expect("profile fixture succeeds").expose(),
            "work-provider"
        );
    });
    let reopened = fixture.resolver(&fixture.config());
    assert_eq!(
        reopened.resolve(&personal).expect("profile fixture succeeds").expose(),
        "personal-provider"
    );
    assert_eq!(
        reopened.resolve(&work).expect("profile fixture succeeds").expose(),
        "work-provider"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("lookups"))
            .expect("profile fixture succeeds")
            .lines()
            .count(),
        2
    );
    let cache = fs::read_to_string(fixture.root.join("onepassword-items.json")).expect("profile fixture succeeds");
    assert!(
        !cache.contains("bootstrap"),
        "bootstrap credentials must never be persisted in the provider cache"
    );
    assert!(!fixture.root.join("onepassword-sessions.json").exists());
}

#[test]
fn rotation_and_bootstrap_file_security_are_checked_before_memory_or_disk_cache() {
    let _lock = ENV_LOCK.lock().expect("profile fixture succeeds");
    let fixture = Fixture::new();
    let _environment = fixture.environment();
    fixture.token("personal", "personal-bootstrap");
    let config = fixture.config();
    let resolver = fixture.resolver(&config);
    let (_, _, secrets) = config.into_stores();
    let secret = secrets
        .get(&SecretRef::new("personal").expect("profile fixture succeeds"))
        .expect("profile fixture succeeds");
    assert_eq!(
        resolver.resolve(&secret).expect("profile fixture succeeds").expose(),
        "personal-provider"
    );
    fixture.token("personal", "rotated-bootstrap");
    assert_eq!(
        resolver.resolve(&secret).expect("profile fixture succeeds").expose(),
        "rotated-provider"
    );
    let path = fixture.root.join("personal.token");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("profile fixture succeeds");
    assert!(resolver.resolve(&secret).is_err());
    assert!(fixture.resolver(&fixture.config()).resolve(&secret).is_err());
    fs::remove_file(&path).expect("profile fixture succeeds");
    assert!(resolver.resolve(&secret).is_err());
    fixture.token("other", "rotated-bootstrap");
    symlink(fixture.root.join("other.token"), &path).expect("profile fixture succeeds");
    assert!(resolver.resolve(&secret).is_err());
    fs::remove_file(&path).expect("profile fixture succeeds");
    fixture.token("personal", "rotated-bootstrap");
    fs::set_permissions(&fixture.root, fs::Permissions::from_mode(0o755)).expect("profile fixture succeeds");
    assert!(resolver.resolve(&secret).is_err());
    assert_eq!(
        fs::read_to_string(fixture.root.join("lookups"))
            .expect("profile fixture succeeds")
            .lines()
            .count(),
        2
    );
}

#[test]
fn rejected_profile_has_no_desktop_fallback_and_does_not_echo_bootstrap_token() {
    let _lock = ENV_LOCK.lock().expect("profile fixture succeeds");
    let fixture = Fixture::new();
    let _environment = fixture.environment();
    fixture.token("personal", "rejected-bootstrap");
    let config = fixture.config();
    let resolver = fixture.resolver(&config);
    let (_, _, secrets) = config.into_stores();
    let secret = secrets
        .get(&SecretRef::new("personal").expect("profile fixture succeeds"))
        .expect("profile fixture succeeds");
    let error = resolver
        .resolve(&secret)
        .expect_err("profile lookup is rejected")
        .to_string();
    assert!(error.contains("no desktop fallback"));
    assert!(!error.contains("rejected-bootstrap"));
    assert_eq!(
        fs::read_to_string(fixture.root.join("lookups"))
            .expect("profile fixture succeeds")
            .lines()
            .count(),
        1
    );
    assert!(!fixture.root.join("onepassword-items.json").exists());
}
