use std::{env, path::Path, time::Duration};

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OnePasswordAuthMode {
    #[default]
    Auto,
    Desktop,
    Session,
    ServiceAccount,
}

impl OnePasswordAuthMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Desktop => "desktop",
            Self::Session => "session",
            Self::ServiceAccount => "service_account",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OnePasswordConfig {
    pub auth_mode: OnePasswordAuthMode,
    pub timeout_seconds: u64,
}

impl Default for OnePasswordConfig {
    fn default() -> Self {
        Self {
            auth_mode: OnePasswordAuthMode::Auto,
            timeout_seconds: 60,
        }
    }
}

impl OnePasswordConfig {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_seconds)
    }

    /// The default app-integration setting. Explicit 1Password environment choices win.
    pub fn desktop_integration(&self) -> Option<bool> {
        self.desktop_integration_for(&AuthEnvironment::current())
    }

    fn desktop_integration_for(&self, environment: &AuthEnvironment) -> Option<bool> {
        if environment.explicit_desktop {
            return None;
        }
        if environment.external_auth || environment.session {
            return Some(false);
        }
        match self.auth_mode {
            OnePasswordAuthMode::Desktop => Some(true),
            OnePasswordAuthMode::Session | OnePasswordAuthMode::ServiceAccount => Some(false),
            OnePasswordAuthMode::Auto if environment.desktop_available => Some(true),
            OnePasswordAuthMode::Auto => None,
        }
    }
}

struct AuthEnvironment {
    explicit_desktop: bool,
    external_auth: bool,
    session: bool,
    desktop_available: bool,
}

impl AuthEnvironment {
    fn current() -> Self {
        Self {
            explicit_desktop: env::var_os("OP_BIOMETRIC_UNLOCK_ENABLED").is_some(),
            external_auth: has_external_auth(),
            session: env::var_os("OP_SESSION").is_some_and(|value| !value.is_empty()),
            desktop_available: desktop_available(),
        }
    }
}

pub(crate) fn has_environment_session() -> bool {
    env::vars_os().any(|(name, value)| {
        let name = name.to_string_lossy();
        (name == "OP_SESSION" || name.starts_with("OP_SESSION_")) && !value.is_empty()
    })
}

pub(crate) fn has_external_auth() -> bool {
    ["OP_SERVICE_ACCOUNT_TOKEN", "OP_CONNECT_HOST", "OP_CONNECT_TOKEN"]
        .iter()
        .any(|name| env::var_os(name).is_some_and(|value| !value.is_empty()))
}

fn desktop_available() -> bool {
    if ["SSH_CONNECTION", "SSH_CLIENT", "CI"]
        .iter()
        .any(|name| env::var_os(name).is_some())
    {
        return false;
    }
    if cfg!(target_os = "macos") {
        return Path::new("/Applications/1Password.app").is_dir()
            || env::var_os("HOME").is_some_and(|home| Path::new(&home).join("Applications/1Password.app").is_dir());
    }
    // On other platforms the CLI's own integration setting remains authoritative.
    false
}

#[cfg(test)]
mod tests {
    use crate::one_password_config::{AuthEnvironment, OnePasswordAuthMode, OnePasswordConfig};

    fn desktop() -> AuthEnvironment {
        AuthEnvironment {
            explicit_desktop: false,
            external_auth: false,
            session: false,
            desktop_available: true,
        }
    }

    #[test]
    fn configuration_defaults_and_explicit_modes_are_validated() {
        let default = crate::SwitchboardConfig::from_toml_str(
            "[namespace.google.personal]\nprovider = \"google\"\naccount = \"personal@example.com\"",
        )
        .expect("default config loads");
        assert_eq!(default.one_password.auth_mode, OnePasswordAuthMode::Auto);
        assert_eq!(default.one_password.timeout_seconds, 60);
        let explicit =
            crate::SwitchboardConfig::from_toml_str("[one_password]\nauth_mode = \"session\"\ntimeout_seconds = 15\n[namespace.google.personal]\nprovider = \"google\"\naccount = \"personal@example.com\"")
                .expect("session config loads");
        assert_eq!(explicit.one_password.auth_mode, OnePasswordAuthMode::Session);
        assert_eq!(explicit.one_password.timeout_seconds, 15);
        let error = crate::SwitchboardConfig::from_toml_str("[one_password]\ntimeout_seconds = 0")
            .expect_err("zero timeout is rejected");
        assert!(error.to_string().contains("must be greater than zero"));
        assert!(crate::SwitchboardConfig::from_toml_str("[one_password]\nauth_mode = \"typo\"").is_err());
    }

    #[test]
    fn desktop_auto_default_does_not_override_explicit_authentication() {
        let config = OnePasswordConfig::default();
        assert_eq!(config.desktop_integration_for(&desktop()), Some(true));
        assert_eq!(
            config.desktop_integration_for(&AuthEnvironment {
                session: true,
                ..desktop()
            }),
            Some(false)
        );
        assert_eq!(
            config.desktop_integration_for(&AuthEnvironment {
                external_auth: true,
                ..desktop()
            }),
            Some(false)
        );
        assert_eq!(
            config.desktop_integration_for(&AuthEnvironment {
                explicit_desktop: true,
                ..desktop()
            }),
            None
        );
    }

    #[test]
    fn servers_keep_native_defaults_and_configuration_can_select_a_mode() {
        let server = AuthEnvironment {
            desktop_available: false,
            ..desktop()
        };
        assert_eq!(OnePasswordConfig::default().desktop_integration_for(&server), None);
        let config = OnePasswordConfig {
            auth_mode: OnePasswordAuthMode::Desktop,
            ..OnePasswordConfig::default()
        };
        assert_eq!(config.desktop_integration_for(&server), Some(true));
        let config = OnePasswordConfig {
            auth_mode: OnePasswordAuthMode::Session,
            ..OnePasswordConfig::default()
        };
        assert_eq!(config.desktop_integration_for(&desktop()), Some(false));
    }
}
