#![cfg(unix)]

use std::{
    env, fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use switchboard_core::{SecretRef, SecretResolver, SecretStore};
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
