mod audit_store;
mod config;
mod one_password_config;
mod operation_store;
mod policy;
mod secrets;
mod stores;

pub use crate::{
    audit_store::SqliteAuditStore,
    config::SwitchboardConfig,
    one_password_config::{OnePasswordAuthMode, OnePasswordConfig, OnePasswordProfile},
    operation_store::{resolve_operation_store_path, SqliteOperationStore},
    policy::ConfiguredPolicyEngine,
    secrets::{one_password_item_cache_expiries, one_password_session_cache_entry_count, LocalSecretResolver},
    stores::{MemoryAuditStore, MemoryOperationStore, StaticAuthStore, StaticNamespaceStore, StaticSecretStore},
};

fn open_durable_sqlite_connection(path: &std::path::Path) -> rusqlite::Result<rusqlite::Connection> {
    let connection = rusqlite::Connection::open(path)?;
    // These flags belong to the connection, so configure every writer before use.
    connection.pragma_update(None, "synchronous", "FULL")?;
    #[cfg(target_os = "macos")]
    connection.pragma_update(None, "fullfsync", true)?;
    Ok(connection)
}
