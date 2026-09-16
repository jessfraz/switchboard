use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::CallError;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub backend: Backend,
    pub transcript_recipient: String,
    #[serde(default)]
    pub state_dir: PathBuf,
    pub worker_project: Option<PathBuf>,
    pub worker_command: Option<PathBuf>,
    #[serde(default)]
    pub worker_args: Vec<String>,
    #[serde(default)]
    pub livekit: LiveKitConfig,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveKitConfig {
    pub url: Option<String>,
    pub sip_trunk_id: Option<String>,
    pub stt_model: Option<String>,
    pub llm_model: Option<String>,
    pub tts_model: Option<String>,
    pub voice: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    #[default]
    Livekit,
}

impl Config {
    pub fn validate_runtime(&self) -> Result<(), CallError> {
        let endpoint = self
            .livekit
            .url
            .clone()
            .or_else(|| std::env::var("LIVEKIT_URL").ok())
            .ok_or_else(|| CallError::Configuration("livekit.url is required".into()))?;
        let url = url::Url::parse(&endpoint)
            .map_err(|_| CallError::Configuration("a secure LiveKit Cloud project URL is required".into()))?;
        if url.scheme() != "wss"
            || !url.host_str().is_some_and(|host| host.ends_with(".livekit.cloud"))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
        {
            return Err(CallError::Configuration(
                "a secure LiveKit Cloud project URL is required".into(),
            ));
        }
        let trunk = self
            .livekit
            .sip_trunk_id
            .clone()
            .or_else(|| std::env::var("LIVEKIT_SIP_TRUNK_ID").ok());
        if !trunk.is_some_and(|value| !value.trim().is_empty()) {
            return Err(CallError::Configuration("livekit.sip_trunk_id is required".into()));
        }
        Ok(())
    }

    pub fn load(explicit: Option<&Path>) -> Result<Self, CallError> {
        let path = if let Some(path) = explicit {
            path.to_path_buf()
        } else if let Some(path) = std::env::var_os("PHONE_CONFIG") {
            PathBuf::from(path)
        } else {
            let home = std::env::var_os("HOME")
                .ok_or_else(|| CallError::Configuration("HOME or PHONE_CONFIG must be set".into()))?;
            PathBuf::from(home).join(".config/phone/config.toml")
        };
        let source = std::fs::read_to_string(path)
            .map_err(|_| CallError::Configuration("could not read configuration file".into()))?;
        // Parser diagnostics can contain the input, which must never be logged.
        let mut config: Self =
            toml::from_str(&source).map_err(|_| CallError::Configuration("invalid configuration TOML".into()))?;
        if let Some(path) = std::env::var_os("PHONE_STATE_DIR") {
            config.state_dir = PathBuf::from(path);
        }
        if !config.state_dir.is_absolute() {
            return Err(CallError::Configuration("state_dir must be absolute".into()));
        }
        if config.worker_command.is_none() && !config.worker_args.is_empty() {
            return Err(CallError::Configuration("worker_args requires worker_command".into()));
        }
        Ok(config)
    }

    pub fn worker_project(&self) -> PathBuf {
        self.worker_project
            .clone()
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workers/livekit-phone"))
    }
}
