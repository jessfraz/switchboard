use serde::{Deserialize, Serialize};
use switchboard_core::{Error, ProviderKind, Result};

const GITHUB_INVENTORY_JSON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/inventories/github.json"));
const GOOGLE_INVENTORY_JSON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/inventories/google.json"));

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CliInventory {
    pub provider: ProviderKind,
    pub program: String,
    pub commands: Vec<CliInventoryCommand>,
}

impl CliInventory {
    pub fn command(&self, path: &[impl AsRef<str>]) -> Option<&CliInventoryCommand> {
        self.commands.iter().find(|command| {
            command.path.len() == path.len()
                && command
                    .path
                    .iter()
                    .zip(path.iter())
                    .all(|(left, right)| left == right.as_ref())
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CliInventoryCommand {
    pub path: Vec<String>,
    pub command: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub help_args: Vec<String>,
    pub node_kind: CliInventoryNodeKind,
    pub operation_kind: CliOperationKind,
    pub undo_support: CliUndoSupport,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subcommands: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliInventoryNodeKind {
    Group,
    Operation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliOperationKind {
    Unknown,
    Read,
    Write,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CliUndoSupport {
    Unknown,
    None,
    CandidateCommand { path: Vec<String> },
}

pub fn embedded_inventory(provider: ProviderKind) -> Result<CliInventory> {
    let json = match provider {
        ProviderKind::GitHub => GITHUB_INVENTORY_JSON,
        ProviderKind::GoogleWorkspace => GOOGLE_INVENTORY_JSON,
        _ => {
            return Err(Error::UnsupportedOperation(format!(
                "no embedded inventory for provider {provider}"
            )))
        }
    };

    serde_json::from_str(json).map_err(|error| Error::Config(format!("invalid embedded CLI inventory: {error}")))
}

#[cfg(test)]
mod tests {
    use switchboard_core::ProviderKind;

    use crate::inventory::{embedded_inventory, CliInventoryNodeKind, CliOperationKind, CliUndoSupport};

    #[test]
    fn embedded_google_inventory_contains_calendar_insert_candidate() {
        let inventory = embedded_inventory(ProviderKind::GoogleWorkspace).expect("inventory should load");
        let command = inventory
            .command(&["calendar", "events", "insert"])
            .expect("calendar insert should exist");

        assert_eq!(command.node_kind, CliInventoryNodeKind::Operation);
        assert_eq!(command.operation_kind, CliOperationKind::Write);
        assert_eq!(
            command.undo_support,
            CliUndoSupport::CandidateCommand {
                path: vec!["calendar".into(), "events".into(), "delete".into()],
            }
        );
    }

    #[test]
    fn embedded_github_inventory_contains_pr_view() {
        let inventory = embedded_inventory(ProviderKind::GitHub).expect("inventory should load");
        let command = inventory.command(&["pr", "view"]).expect("pr view should exist");

        assert_eq!(command.node_kind, CliInventoryNodeKind::Operation);
        assert_eq!(command.operation_kind, CliOperationKind::Read);
    }
}
