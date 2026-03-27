use std::env;

use switchboard_core::{Error, ResolvedSecret, Result, SecretRef, SecretSource, SecretString};

use crate::secrets::SecretBackend;

pub(super) struct EnvSecretBackend;

impl SecretBackend for EnvSecretBackend {
    fn can_resolve(&self, secret: &ResolvedSecret) -> bool {
        matches!(secret.source, SecretSource::Env { .. })
    }

    fn resolve(&self, secret: &ResolvedSecret) -> Result<SecretString> {
        let SecretSource::Env { name } = &secret.source else {
            return Err(Error::SecretResolution {
                secret_ref: secret.id.to_string(),
                reason: "env backend received a non-env secret".into(),
            });
        };
        let value = env::var(name).map_err(|error| Error::SecretResolution {
            secret_ref: secret.id.to_string(),
            reason: format!("failed to read environment variable {name}: {error}"),
        })?;

        normalize_secret(&secret.id, value)
    }
}

pub(crate) fn normalize_secret(secret_ref: &SecretRef, value: String) -> Result<SecretString> {
    let trimmed = value.trim_end_matches(['\n', '\r']).to_owned();
    if trimmed.is_empty() {
        return Err(Error::SecretResolution {
            secret_ref: secret_ref.to_string(),
            reason: "resolved to an empty value".into(),
        });
    }

    Ok(SecretString::from(trimmed))
}
