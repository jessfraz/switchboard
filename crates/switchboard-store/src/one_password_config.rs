use std::{env, time::Duration};

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

    /// An inherited or native desktop preference can prompt even with closed stdin.
    pub(crate) fn may_prompt(&self) -> bool {
        if has_external_auth() {
            return false;
        }
        if let Some(value) = env::var_os("OP_BIOMETRIC_UNLOCK_ENABLED") {
            return !value.to_str().is_some_and(|value| value.eq_ignore_ascii_case("false"));
        }
        self.desktop_integration() != Some(false)
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
            OnePasswordAuthMode::Auto if environment.graphical_session == Some(false) => Some(false),
            OnePasswordAuthMode::Auto => None,
        }
    }
}

struct AuthEnvironment {
    explicit_desktop: bool,
    external_auth: bool,
    session: bool,
    graphical_session: Option<bool>,
}

impl AuthEnvironment {
    fn current() -> Self {
        Self {
            explicit_desktop: env::var_os("OP_BIOMETRIC_UNLOCK_ENABLED").is_some(),
            external_auth: has_external_auth(),
            session: env::var_os("OP_SESSION").is_some_and(|value| !value.is_empty()),
            graphical_session: if env::var_os("CI").is_some_and(|value| {
                !value.is_empty() && value != "0" && !value.to_string_lossy().eq_ignore_ascii_case("false")
            }) {
                Some(false)
            } else {
                graphical_session()
            },
        }
    }
}

#[cfg(target_os = "macos")]
fn graphical_session() -> Option<bool> {
    #[link(name = "Security", kind = "framework")]
    extern "C" {
        fn SessionGetInfo(session: u32, session_id: *mut u32, attributes: *mut u32) -> i32;
    }
    const CALLER_SECURITY_SESSION: u32 = u32::MAX;
    const SESSION_HAS_GRAPHIC_ACCESS: u32 = 0x0010;
    let mut attributes = 0;
    // Check this process's login session, not another user's desktop or an
    // installed app. Redirected terminal IO does not imply a headless session.
    // SAFETY: AuthSession.h defines these u32 inputs/outputs; attributes points
    // to valid storage, and the optional session ID output may be null.
    let status = unsafe { SessionGetInfo(CALLER_SECURITY_SESSION, std::ptr::null_mut(), &mut attributes) };
    (status == 0).then_some(attributes & SESSION_HAS_GRAPHIC_ACCESS != 0)
}

#[cfg(target_os = "linux")]
fn graphical_session() -> Option<bool> {
    Some(
        ["DISPLAY", "WAYLAND_DISPLAY"]
            .iter()
            .any(|name| env::var_os(name).is_some_and(|value| !value.is_empty())),
    )
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn graphical_session() -> Option<bool> {
    None
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

#[cfg(test)]
mod tests {
    use crate::one_password_config::{AuthEnvironment, OnePasswordAuthMode, OnePasswordConfig};

    fn desktop() -> AuthEnvironment {
        AuthEnvironment {
            explicit_desktop: false,
            external_auth: false,
            session: false,
            graphical_session: Some(true),
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
        assert_eq!(config.desktop_integration_for(&desktop()), None);
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
    fn native_defaults_are_preserved_and_configuration_can_select_a_mode() {
        assert_eq!(OnePasswordConfig::default().desktop_integration_for(&desktop()), None);
        let config = OnePasswordConfig {
            auth_mode: OnePasswordAuthMode::Desktop,
            ..OnePasswordConfig::default()
        };
        assert_eq!(config.desktop_integration_for(&desktop()), Some(true));
        let config = OnePasswordConfig {
            auth_mode: OnePasswordAuthMode::Session,
            ..OnePasswordConfig::default()
        };
        assert_eq!(config.desktop_integration_for(&desktop()), Some(false));
    }

    #[test]
    fn auto_avoids_desktop_authentication_only_when_headless_is_known() {
        let headless = AuthEnvironment {
            graphical_session: Some(false),
            ..desktop()
        };
        let config = OnePasswordConfig::default();
        assert_eq!(config.desktop_integration_for(&headless), Some(false));
        assert_eq!(
            config.desktop_integration_for(&AuthEnvironment {
                graphical_session: None,
                ..desktop()
            }),
            None
        );
        assert_eq!(
            config.desktop_integration_for(&AuthEnvironment {
                explicit_desktop: true,
                ..headless
            }),
            None
        );
        let config = OnePasswordConfig {
            auth_mode: OnePasswordAuthMode::Desktop,
            ..config
        };
        assert_eq!(config.desktop_integration_for(&headless), Some(true));
    }
}
