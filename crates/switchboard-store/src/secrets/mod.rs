use std::path::PathBuf;

mod env_secret;
mod file_secret;
mod one_password;

use switchboard_core::{Error, ResolvedSecret, Result, SecretResolver, SecretString};

trait SecretBackend: Send + Sync {
    fn can_resolve(&self, secret: &ResolvedSecret) -> bool;
    fn resolve(&self, secret: &ResolvedSecret) -> Result<SecretString>;
}

pub struct LocalSecretResolver {
    backends: Vec<Box<dyn SecretBackend>>,
}

impl Default for LocalSecretResolver {
    fn default() -> Self {
        Self::with_one_password_session_cache(None)
    }
}

impl LocalSecretResolver {
    pub fn with_one_password_session_cache(one_password_session_cache_path: Option<PathBuf>) -> Self {
        Self {
            backends: vec![
                Box::new(env_secret::EnvSecretBackend),
                Box::new(file_secret::FileSecretBackend),
                Box::new(one_password::OnePasswordSecretBackend::new(
                    one_password_session_cache_path,
                )),
            ],
        }
    }
}

impl SecretResolver for LocalSecretResolver {
    fn resolve(&self, secret: &ResolvedSecret) -> Result<SecretString> {
        let backend = self
            .backends
            .iter()
            .find(|backend| backend.can_resolve(secret))
            .ok_or_else(|| Error::SecretResolution {
                secret_ref: secret.id.to_string(),
                reason: "no secret backend is registered for this secret source".into(),
            })?;

        backend.resolve(secret)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        path::PathBuf,
        process,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use switchboard_core::{ResolvedSecret, SecretResolver, SecretSource};

    use super::LocalSecretResolver;

    const GOOGLE_PERSONAL_OAUTH_JSON: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/secrets/google-personal-oauth.json"
    ));
    static TEMP_FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn reads_environment_values() {
        let resolver = LocalSecretResolver::default();
        let secret = ResolvedSecret::new(
            "google_workspace_cli_client_id",
            SecretSource::Env {
                name: "GOOGLE_WORKSPACE_CLI_CLIENT_ID".into(),
            },
        )
        .expect("secret should build");
        env::set_var("GOOGLE_WORKSPACE_CLI_CLIENT_ID", "client-id-from-env");

        let value = resolver.resolve(&secret).expect("env secret should resolve");

        assert_eq!(value.expose(), "client-id-from-env");
    }

    #[test]
    fn reads_file_values_and_trims_newlines() {
        let resolver = LocalSecretResolver::default();
        let fixture = TempFixtureDir::new();
        let path = fixture.write_file("google-personal-oauth.json", GOOGLE_PERSONAL_OAUTH_JSON);
        let secret =
            ResolvedSecret::new("google_personal_oauth", SecretSource::File { path }).expect("secret should build");

        let value = resolver.resolve(&secret).expect("file secret should resolve");

        assert_eq!(value.expose(), GOOGLE_PERSONAL_OAUTH_JSON.trim_end());
    }

    struct TempFixtureDir {
        path: PathBuf,
    }

    impl TempFixtureDir {
        fn new() -> Self {
            let path = env::temp_dir().join(format!(
                "switchboard-secret-test-{}-{}-{}",
                process::id(),
                TEMP_FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system time should be after unix epoch")
                    .as_nanos()
            ));
            fs::create_dir_all(&path).expect("temp fixture dir should exist");

            Self { path }
        }

        fn write_file(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.path.join(name);
            fs::write(&path, contents).expect("fixture file should be written");
            path
        }
    }

    impl Drop for TempFixtureDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
