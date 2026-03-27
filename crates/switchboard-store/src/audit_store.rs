use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{params, Connection, OptionalExtension, Row};
use switchboard_core::{
    AuditEvent, AuditEventId, AuditOutcome, AuditStore, AuthRef, BackendKind, Error, NamespaceId, OperationId, Result,
    StoredAuditEvent, ToolName,
};

pub struct SqliteAuditStore {
    path: PathBuf,
}

impl SqliteAuditStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let store = Self { path: path.into() };
        store.connect()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn connect(&self) -> Result<Connection> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                Error::Audit(format!(
                    "failed to create audit store directory {}: {error}",
                    parent.display()
                ))
            })?;
        }

        let connection = Connection::open(&self.path)
            .map_err(|error| Error::Audit(format!("failed to open audit store {}: {error}", self.path.display())))?;

        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS audit_events (
               event_id TEXT PRIMARY KEY,
               tool TEXT NOT NULL,
               namespace TEXT NOT NULL,
               auth_ref TEXT NOT NULL,
               summary TEXT NOT NULL,
               backend TEXT NOT NULL,
               approval_required INTEGER NOT NULL,
               outcome TEXT NOT NULL,
               operation_id TEXT,
               compensates_operation_id TEXT,
               recorded_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );",
            )
            .map_err(|error| {
                Error::Audit(format!(
                    "failed to initialize audit store {}: {error}",
                    self.path.display()
                ))
            })?;

        Ok(connection)
    }

    fn generate_event_id(connection: &Connection) -> Result<AuditEventId> {
        let event_id = connection
            .query_row("SELECT 'audit_' || lower(hex(randomblob(16)))", [], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| Error::Audit(format!("failed to generate audit event id: {error}")))?;

        AuditEventId::new(event_id)
    }

    fn get_event(connection: &Connection, id: &AuditEventId) -> Result<StoredAuditEvent> {
        connection
            .query_row(
                "SELECT event_id, tool, namespace, auth_ref, summary, backend, approval_required, outcome, operation_id, compensates_operation_id, recorded_at
                 FROM audit_events
                 WHERE event_id = ?1",
                params![id.as_str()],
                row_to_audit_event,
            )
            .optional()
            .map_err(|error| Error::Audit(format!("failed to load audit event {id}: {error}")))?
            .ok_or_else(|| Error::UnknownAuditEvent(id.clone()))
    }
}

impl AuditStore for SqliteAuditStore {
    fn record(&self, event: &AuditEvent) -> Result<()> {
        let connection = self.connect()?;
        let event_id = Self::generate_event_id(&connection)?;
        connection
            .execute(
                "INSERT INTO audit_events (
                   event_id, tool, namespace, auth_ref, summary, backend, approval_required, outcome, operation_id, compensates_operation_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    event_id.as_str(),
                    event.tool.as_str(),
                    event.namespace.as_str(),
                    event.auth_ref.as_str(),
                    &event.summary,
                    backend_kind_identifier(event.backend),
                    event.approval_required,
                    audit_outcome_identifier(&event.outcome),
                    event.operation_id.as_ref().map(OperationId::as_str),
                    event.compensates_operation_id.as_ref().map(OperationId::as_str),
                ],
            )
            .map_err(|error| Error::Audit(format!("failed to insert audit event {event_id}: {error}")))?;

        Ok(())
    }

    fn get(&self, id: &AuditEventId) -> Option<StoredAuditEvent> {
        let connection = self.connect().ok()?;
        Self::get_event(&connection, id).ok()
    }

    fn list(&self) -> Vec<StoredAuditEvent> {
        let connection = match self.connect() {
            Ok(connection) => connection,
            Err(_) => return Vec::new(),
        };
        let mut statement = match connection.prepare(
            "SELECT event_id, tool, namespace, auth_ref, summary, backend, approval_required, outcome, operation_id, compensates_operation_id, recorded_at
             FROM audit_events
             ORDER BY recorded_at DESC, rowid DESC",
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        let rows = match statement.query_map([], row_to_audit_event) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };

        rows.filter_map(|row| row.ok()).collect()
    }
}

fn row_to_audit_event(row: &Row<'_>) -> rusqlite::Result<StoredAuditEvent> {
    let id = row.get::<_, String>(0)?;
    let tool = row.get::<_, String>(1)?;
    let namespace = row.get::<_, String>(2)?;
    let auth_ref = row.get::<_, String>(3)?;
    let summary = row.get::<_, String>(4)?;
    let backend = row.get::<_, String>(5)?;
    let approval_required = row.get::<_, bool>(6)?;
    let outcome = row.get::<_, String>(7)?;
    let operation_id = row.get::<_, Option<String>>(8)?;
    let compensates_operation_id = row.get::<_, Option<String>>(9)?;
    let recorded_at = row.get::<_, String>(10)?;

    Ok(StoredAuditEvent {
        id: AuditEventId::new(id).map_err(to_sqlite_error)?,
        tool: ToolName::new(tool).map_err(to_sqlite_error)?,
        namespace: NamespaceId::new(namespace).map_err(to_sqlite_error)?,
        auth_ref: AuthRef::new(auth_ref).map_err(to_sqlite_error)?,
        summary,
        backend: parse_backend_kind(&backend).map_err(to_sqlite_error)?,
        approval_required,
        outcome: parse_audit_outcome(&outcome).map_err(to_sqlite_error)?,
        operation_id: operation_id
            .map(OperationId::new)
            .transpose()
            .map_err(to_sqlite_error)?,
        compensates_operation_id: compensates_operation_id
            .map(OperationId::new)
            .transpose()
            .map_err(to_sqlite_error)?,
        recorded_at,
    })
}

fn audit_outcome_identifier(outcome: &AuditOutcome) -> &'static str {
    match outcome {
        AuditOutcome::Planned => "planned",
        AuditOutcome::Approved => "approved",
        AuditOutcome::Rejected => "rejected",
        AuditOutcome::Executed => "executed",
        AuditOutcome::Failed => "failed",
        AuditOutcome::Compensated => "compensated",
        AuditOutcome::Blocked => "blocked",
    }
}

fn parse_audit_outcome(value: &str) -> Result<AuditOutcome> {
    match value {
        "planned" => Ok(AuditOutcome::Planned),
        "approved" => Ok(AuditOutcome::Approved),
        "rejected" => Ok(AuditOutcome::Rejected),
        "executed" => Ok(AuditOutcome::Executed),
        "failed" => Ok(AuditOutcome::Failed),
        "compensated" => Ok(AuditOutcome::Compensated),
        "blocked" => Ok(AuditOutcome::Blocked),
        _ => Err(Error::Audit(format!("unknown audit outcome in audit store: {value}"))),
    }
}

fn backend_kind_identifier(backend: BackendKind) -> &'static str {
    match backend {
        BackendKind::Cli => "cli",
        BackendKind::Api => "api",
        BackendKind::Local => "local",
        BackendKind::Bridge => "bridge",
    }
}

fn parse_backend_kind(value: &str) -> Result<BackendKind> {
    match value {
        "cli" => Ok(BackendKind::Cli),
        "api" => Ok(BackendKind::Api),
        "local" => Ok(BackendKind::Local),
        "bridge" => Ok(BackendKind::Bridge),
        _ => Err(Error::Audit(format!("unknown backend kind in audit store: {value}"))),
    }
}

fn to_sqlite_error(error: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())),
    )
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use switchboard_core::{AuditEvent, AuditOutcome, AuditStore, AuthRef, BackendKind, NamespaceId, ToolName};

    use super::SqliteAuditStore;

    #[test]
    fn sqlite_audit_store_persists_and_lists_events() {
        let path = unique_test_path("audit-store");
        let store = SqliteAuditStore::open(&path).expect("audit store should open");
        let event = AuditEvent {
            tool: ToolName::new("google.calendar.create").expect("tool should build"),
            namespace: NamespaceId::new("google.work").expect("namespace should build"),
            auth_ref: AuthRef::new("google.work").expect("auth ref should build"),
            summary: "Create calendar event".into(),
            backend: BackendKind::Cli,
            approval_required: true,
            outcome: AuditOutcome::Planned,
            operation_id: None,
            compensates_operation_id: None,
        };

        store.record(&event).expect("audit event should record");

        let events = store.list();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].tool.to_string(), "google.calendar.create");
        assert_eq!(events[0].outcome, AuditOutcome::Planned);
        assert!(!events[0].recorded_at.is_empty());

        let fetched = store.get(&events[0].id).expect("stored event should be retrievable");
        assert_eq!(fetched.id, events[0].id);
    }

    fn unique_test_path(prefix: &str) -> PathBuf {
        Path::new(&std::env::temp_dir()).join(format!(
            "{prefix}-{}-{}.sqlite3",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ))
    }
}
