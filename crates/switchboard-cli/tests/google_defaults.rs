use std::{
    env, fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

#[derive(Deserialize)]
struct ExecutedStatus {
    status: String,
    namespace: String,
    fields: StatusFields,
}

#[derive(Deserialize)]
struct StatusFields {
    response: GoogleStatus,
}

// This is the external gws auth-status response, not a Switchboard-owned schema.
#[derive(Deserialize)]
struct GoogleStatus {
    auth_method: String,
    keyring_backend: String,
    credential_source: String,
    encrypted_credentials: PathBuf,
    encrypted_credentials_exists: bool,
    plain_credentials_exists: bool,
}

#[test]
fn real_gws_uses_isolated_file_storage_without_secret_or_environment_setup() {
    let fixture = Fixture::new();
    let config = fixture.0.join("switchboard.toml");
    fs::write(
        &config,
        r#"
[namespace.google.personal]
provider = "google"
account = "personal@example.com"

[namespace.google.work]
provider = "google"
account = "work@example.com"
"#,
    )
    .expect("config should be written");

    for namespace in ["google.personal", "google.work"] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_switchboard"));
        command
            .current_dir(&fixture.0)
            .args(["--config"])
            .arg(&config)
            .args(["google.cli.read", "--ns", namespace, "--json", "--", "auth", "status"])
            .env("HOME", &fixture.0)
            .env("XDG_CONFIG_HOME", fixture.0.join("config"))
            .env("APPDATA", fixture.0.join("appdata"))
            .env_remove("SWITCHBOARD_GWS_BIN")
            .env_remove("SWITCHBOARD_STATE_DB")
            .env_remove("SWITCHBOARD_STATE_DIR")
            .env("SWITCHBOARD_OP_BIN", fixture.0.join("op-must-not-run"))
            .env("GOOGLE_WORKSPACE_CLI_KEYRING_BACKEND", "keyring")
            .env("GOOGLE_WORKSPACE_CLI_CONFIG_DIR", fixture.0.join("ambient"))
            .env("GOOGLE_WORKSPACE_CLI_TOKEN", "ambient-token-must-not-be-used")
            .env("GOOGLE_WORKSPACE_CLI_CLIENT_ID", "ambient-client-must-not-be-used")
            .env("GOOGLE_WORKSPACE_CLI_CLIENT_SECRET", "ambient-secret-must-not-be-used")
            .env("GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE", fixture.0.join("ambient.json"));
        let output = command
            .output()
            .expect("real gws auth status should finish without authentication");
        assert!(
            output.status.success(),
            "Switchboard should run without OAuth secret configuration: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let response: ExecutedStatus = serde_json::from_slice(&output.stdout).expect("status should be JSON");
        assert_eq!(response.status, "executed");
        assert_eq!(response.namespace, namespace);
        let google = response.fields.response;
        assert_eq!(google.auth_method, "none");
        assert_eq!(google.keyring_backend, "file");
        assert_eq!(google.credential_source, "none");
        assert!(!google.encrypted_credentials_exists);
        assert!(!google.plain_credentials_exists);
        let expected = fixture.0.join(".switchboard/namespaces").join(namespace);
        assert_eq!(google.encrypted_credentials.parent(), Some(expected.as_path()));
    }
    assert!(!fixture.0.join("ambient").exists());
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let path = env::temp_dir().join(format!("switchboard-real-gws-{}-{suffix}", std::process::id()));
        fs::create_dir(&path).expect("isolated test directory should be created");
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
