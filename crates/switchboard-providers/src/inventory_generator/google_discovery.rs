use std::{collections::BTreeMap, fs, path::Path, process::Command};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use switchboard_core::{Error, Result};
use tempfile::TempDir;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sources {
    gws_version: String,
    documents: Vec<Document>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    service: String,
    file: String,
    source: String,
    sha256: String,
    bytes: u64,
}

/// Public schema snapshots are build inputs, not credentials or a user cache.
/// gws 0.22.5 reads fresh cache files before trying its live Discovery service.
pub(super) struct GoogleDiscovery {
    directory: TempDir,
    files: BTreeMap<String, String>,
}

impl GoogleDiscovery {
    pub(super) fn prepare(source: &Path) -> Result<Self> {
        let sources: Sources = serde_json::from_slice(&fs::read(source.join("sources.json")).map_err(input_error)?)
            .map_err(input_error)?;
        if sources.gws_version != "0.22.5" || sources.documents.is_empty() {
            return Err(input_error("unsupported or empty Google Discovery snapshot"));
        }
        let directory = tempfile::Builder::new()
            .prefix("switchboard-discovery-")
            .tempdir()
            .map_err(input_error)?;
        fs::create_dir(directory.path().join("cache")).map_err(input_error)?;
        let mut files = BTreeMap::new();
        for document in sources.documents {
            if document.file.contains(['/', '\\']) || !document.file.ends_with(".json") {
                return Err(input_error("invalid Discovery snapshot filename"));
            }
            let contents = fs::read(source.join(&document.file)).map_err(input_error)?;
            let digest = Sha256::digest(&contents)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            if contents.len() as u64 != document.bytes || digest != document.sha256 {
                return Err(input_error(format!("checksum mismatch for {}", document.file)));
            }
            if !document.source.starts_with("https://")
                || files.insert(document.service, document.file.clone()).is_some()
            {
                return Err(input_error("invalid or duplicate Discovery snapshot source"));
            }
            // Write instead of copying metadata: a checkout's age must not cause
            // gws to expire a pinned public document and fetch a live replacement.
            fs::write(directory.path().join("cache").join(document.file), contents).map_err(input_error)?;
        }
        Ok(Self { directory, files })
    }

    pub(super) fn configure(&self, command: &mut Command, path: &[String]) -> Result<()> {
        if let Some(service) = path.first().filter(|service| service.as_str() != "workflow") {
            let file = self
                .files
                .get(service)
                .ok_or_else(|| input_error(format!("no pinned Discovery document for {service}")))?;
            if !self.directory.path().join("cache").join(file).is_file() {
                return Err(input_error(format!("missing pinned Discovery document for {service}")));
            }
        }
        // A clean working directory prevents dotenvy from reading a checkout's
        // .env. A clean environment also excludes auth, logging, and proxy bypass.
        command.env_clear();
        for key in ["PATH", "SYSTEMROOT"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .current_dir(self.directory.path())
            .env("HOME", self.directory.path())
            .env("XDG_CONFIG_HOME", self.directory.path())
            .env("GOOGLE_WORKSPACE_CLI_CONFIG_DIR", self.directory.path());
        // reqwest honors these proxies. Port zero cannot host a listening server;
        // any unexpected live schema fetch must fail, never update the snapshot.
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env(key, "http://127.0.0.1:0");
        }
        command.env("NO_PROXY", "").env("no_proxy", "");
        Ok(())
    }
}

fn input_error(error: impl std::fmt::Display) -> Error {
    Error::Config(format!("pinned Google Discovery inputs: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned_source() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("discovery/google")
    }

    #[test]
    fn missing_or_modified_pinned_inputs_fail_closed() {
        let root = tempfile::tempdir().expect("fixture directory");
        fs::copy(pinned_source().join("sources.json"), root.path().join("sources.json")).expect("fixture sources");
        assert!(GoogleDiscovery::prepare(root.path()).is_err());
        let sources: Sources = serde_json::from_slice(&fs::read(root.path().join("sources.json")).expect("sources"))
            .expect("typed sources");
        for document in sources.documents {
            fs::write(root.path().join(document.file), b"{}").expect("modified snapshot");
        }
        assert!(GoogleDiscovery::prepare(root.path()).is_err());
    }

    #[test]
    fn unpinned_services_and_missing_cache_files_fail_before_execution() {
        let discovery = GoogleDiscovery::prepare(&pinned_source()).expect("pinned inputs");
        assert!(discovery
            .configure(&mut Command::new("unused"), &["unreviewed-service".into()])
            .is_err());
        fs::remove_file(discovery.directory.path().join("cache/drive_v3.json")).expect("remove fixture input");
        assert!(discovery
            .configure(&mut Command::new("unused"), &["drive".into()])
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn child_uses_only_pinned_schema_and_isolated_configuration() {
        let discovery = GoogleDiscovery::prepare(&pinned_source()).expect("pinned inputs");
        let root = discovery.directory.path().to_owned();
        let mut command = Command::new("/bin/sh");
        command.env("GOOGLE_WORKSPACE_CLI_TOKEN", "must-not-reach-child");
        command.env("NO_PROXY", "*");
        discovery
            .configure(&mut command, &["drive".into()])
            .expect("isolated child");
        let output = command
            .args([
                "-c",
                r#"
            set -eu
            test -z "${GOOGLE_WORKSPACE_CLI_TOKEN+x}"
            test -z "$NO_PROXY" && test -z "$no_proxy"
            test "$HTTPS_PROXY" = http://127.0.0.1:0
            test "$HOME" = "$GOOGLE_WORKSPACE_CLI_CONFIG_DIR"
            test "$(pwd -P)" = "$(cd "$HOME" && pwd -P)"
            cat "$GOOGLE_WORKSPACE_CLI_CONFIG_DIR/cache/drive_v3.json"
        "#,
            ])
            .output()
            .expect("fixture child");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(
            output.stdout,
            fs::read(pinned_source().join("drive_v3.json")).expect("pinned document")
        );
        drop(discovery);
        assert!(!root.exists(), "temporary schema copy is discarded after generation");
    }
}
