use std::fs;

use switchboard_core::{Error, ResolvedSecret, Result, SecretSource, SecretString};

use crate::secrets::{env_secret::normalize_secret, SecretBackend};

pub(super) struct FileSecretBackend;

impl SecretBackend for FileSecretBackend {
    fn can_resolve(&self, secret: &ResolvedSecret) -> bool {
        matches!(secret.source, SecretSource::File { .. })
    }

    fn resolve(&self, secret: &ResolvedSecret) -> Result<SecretString> {
        let SecretSource::File { path } = &secret.source else {
            return Err(Error::SecretResolution {
                secret_ref: secret.id.to_string(),
                reason: "file backend received a non-file secret".into(),
            });
        };
        let value = fs::read_to_string(path).map_err(|error| Error::SecretResolution {
            secret_ref: secret.id.to_string(),
            reason: format!("failed to read {}: {error}", path.display()),
        })?;

        normalize_secret(&secret.id, value)
    }
}
