use std::fmt::{self, Display};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "github")]
    GitHub,
    #[serde(rename = "google")]
    GoogleWorkspace,
    #[serde(rename = "slack")]
    Slack,
    #[serde(rename = "ramp")]
    Ramp,
    #[serde(rename = "imessage")]
    IMessage,
    #[serde(rename = "whatsapp")]
    WhatsApp,
}

impl ProviderKind {
    pub fn from_identifier(value: &str) -> Option<Self> {
        match value {
            "github" => Some(Self::GitHub),
            "google" => Some(Self::GoogleWorkspace),
            "slack" => Some(Self::Slack),
            "ramp" => Some(Self::Ramp),
            "imessage" => Some(Self::IMessage),
            "whatsapp" => Some(Self::WhatsApp),
            _ => None,
        }
    }

    pub fn from_tool_name(tool_name: &str) -> Option<Self> {
        let prefix = tool_name.split('.').next()?;
        Self::from_identifier(prefix)
    }
}

impl Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::GitHub => "github",
            Self::GoogleWorkspace => "google",
            Self::Slack => "slack",
            Self::Ramp => "ramp",
            Self::IMessage => "imessage",
            Self::WhatsApp => "whatsapp",
        };

        write!(f, "{value}")
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NamespaceId(String);

impl NamespaceId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(Error::InvalidArguments("namespace cannot be empty".into()));
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for NamespaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolName(String);

impl ToolName {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if ProviderKind::from_tool_name(&value).is_none() {
            return Err(Error::InvalidToolName(value));
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn provider(&self) -> Result<ProviderKind> {
        ProviderKind::from_tool_name(&self.0).ok_or_else(|| Error::InvalidToolName(self.0.clone()))
    }
}

impl Display for ToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OperationId(String);

impl OperationId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        crate::types::validate_non_empty("operation id", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
