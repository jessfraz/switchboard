use std::{
    env, fs,
    path::{Path, PathBuf},
};

use rusqlite::{params, Connection, OptionalExtension, Row};
use switchboard_core::{
    ApprovalState, BackendKind, Error, OperationApproval, OperationEffect, OperationId, OperationStatus,
    OperationStore, Result, StoredOperation, ToolKind, ToolOutput,
};

const DEFAULT_DB_FILE: &str = "operations.sqlite3";

pub struct SqliteOperationStore {
    path: PathBuf,
}

impl SqliteOperationStore {
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
                Error::Operation(format!(
                    "failed to create operation store directory {}: {error}",
                    parent.display()
                ))
            })?;
        }

        let connection = Connection::open(&self.path).map_err(|error| {
            Error::Operation(format!(
                "failed to open operation store {}: {error}",
                self.path.display()
            ))
        })?;

        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS operations (
               operation_id TEXT PRIMARY KEY,
               tool TEXT NOT NULL,
               namespace TEXT NOT NULL,
               auth_ref TEXT NOT NULL,
               kind TEXT NOT NULL,
               summary TEXT NOT NULL,
               backend TEXT NOT NULL,
               approval_required INTEGER NOT NULL,
               approval_reason TEXT,
               approval_state TEXT NOT NULL,
               approval_actor TEXT,
               approval_note TEXT,
               status TEXT NOT NULL,
               args_json TEXT NOT NULL,
               effect_json TEXT,
               failure_reason TEXT,
               created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
               updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );",
            )
            .map_err(|error| {
                Error::Operation(format!(
                    "failed to initialize operation store {}: {error}",
                    self.path.display()
                ))
            })?;

        ensure_column(
            &connection,
            "approval_reason",
            "TEXT",
            "ALTER TABLE operations ADD COLUMN approval_reason TEXT",
        )?;
        ensure_column(
            &connection,
            "approval_state",
            "TEXT NOT NULL DEFAULT 'pending'",
            "ALTER TABLE operations ADD COLUMN approval_state TEXT NOT NULL DEFAULT 'pending'",
        )?;
        ensure_column(
            &connection,
            "approval_actor",
            "TEXT",
            "ALTER TABLE operations ADD COLUMN approval_actor TEXT",
        )?;
        ensure_column(
            &connection,
            "approval_note",
            "TEXT",
            "ALTER TABLE operations ADD COLUMN approval_note TEXT",
        )?;
        connection
            .execute(
                "UPDATE operations
                 SET approval_state = 'not_required'
                 WHERE approval_required = 0 AND approval_state = 'pending'",
                [],
            )
            .map_err(|error| {
                Error::Operation(format!(
                    "failed to backfill approval state in {}: {error}",
                    self.path.display()
                ))
            })?;

        Ok(connection)
    }

    fn generate_operation_id(connection: &Connection) -> Result<OperationId> {
        let operation_id = connection
            .query_row("SELECT 'op_' || lower(hex(randomblob(16)))", [], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| Error::Operation(format!("failed to generate operation id: {error}")))?;

        OperationId::new(operation_id)
    }

    fn get_operation(connection: &Connection, id: &OperationId) -> Result<StoredOperation> {
        connection
            .query_row(
                "SELECT operation_id, tool, namespace, auth_ref, kind, summary, backend, approval_required, approval_reason, approval_state, approval_actor, approval_note, status, args_json, effect_json, failure_reason
                 FROM operations
                 WHERE operation_id = ?1",
                params![id.as_str()],
                row_to_operation,
            )
            .optional()
            .map_err(|error| Error::Operation(format!("failed to load operation {id}: {error}")))?
            .ok_or_else(|| Error::Operation(format!("unknown operation id: {id}")))
    }
}

impl OperationStore for SqliteOperationStore {
    fn create(&self, plan: &switchboard_core::PlannedAction) -> Result<StoredOperation> {
        let connection = self.connect()?;
        let operation = StoredOperation::from_plan(Self::generate_operation_id(&connection)?, plan);
        let args_json = serde_json::to_string(&operation.args)
            .map_err(|error| Error::Operation(format!("failed to encode operation arguments: {error}")))?;

        connection
            .execute(
                "INSERT INTO operations (
                   operation_id, tool, namespace, auth_ref, kind, summary, backend, approval_required, approval_reason, approval_state, approval_actor, approval_note, status, args_json, effect_json, failure_reason
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, NULL, ?11, ?12, NULL, NULL)",
                params![
                    operation.id.as_str(),
                    operation.tool.as_str(),
                    operation.namespace.as_str(),
                    operation.auth_ref.as_str(),
                    tool_kind_identifier(operation.kind),
                    &operation.summary,
                    backend_kind_identifier(operation.backend),
                    operation.approval_required,
                    operation.approval_reason.as_deref(),
                    approval_state_identifier(operation.approval.state),
                    operation_status_identifier(operation.status),
                    args_json,
                ],
            )
            .map_err(|error| Error::Operation(format!("failed to insert operation {}: {error}", operation.id)))?;

        Ok(operation)
    }

    fn mark_approved(&self, id: &OperationId, actor: &str, note: Option<&str>) -> Result<StoredOperation> {
        let connection = self.connect()?;
        let mut operation = Self::get_operation(&connection, id)?;
        operation.approve(actor, note.map(str::to_owned))?;

        connection
            .execute(
                "UPDATE operations
                 SET approval_state = ?2, approval_actor = ?3, approval_note = ?4, updated_at = CURRENT_TIMESTAMP
                 WHERE operation_id = ?1",
                params![
                    operation.id.as_str(),
                    approval_state_identifier(operation.approval.state),
                    operation.approval.actor.as_deref(),
                    operation.approval.note.as_deref(),
                ],
            )
            .map_err(|error| Error::Operation(format!("failed to mark operation {id} approved: {error}")))?;

        Ok(operation)
    }

    fn mark_rejected(&self, id: &OperationId, actor: &str, note: Option<&str>) -> Result<StoredOperation> {
        let connection = self.connect()?;
        let mut operation = Self::get_operation(&connection, id)?;
        operation.reject(actor, note.map(str::to_owned))?;

        connection
            .execute(
                "UPDATE operations
                 SET approval_state = ?2, approval_actor = ?3, approval_note = ?4, updated_at = CURRENT_TIMESTAMP
                 WHERE operation_id = ?1",
                params![
                    operation.id.as_str(),
                    approval_state_identifier(operation.approval.state),
                    operation.approval.actor.as_deref(),
                    operation.approval.note.as_deref(),
                ],
            )
            .map_err(|error| Error::Operation(format!("failed to mark operation {id} rejected: {error}")))?;

        Ok(operation)
    }

    fn mark_applied(&self, id: &OperationId, output: &ToolOutput) -> Result<StoredOperation> {
        let connection = self.connect()?;
        let mut operation = Self::get_operation(&connection, id)?;
        operation.mark_applied(output.effect.clone());
        let effect_json = encode_effect(operation.effect.as_ref())?;

        connection
            .execute(
                "UPDATE operations
                 SET status = ?2, effect_json = ?3, failure_reason = NULL, updated_at = CURRENT_TIMESTAMP
                 WHERE operation_id = ?1",
                params![
                    operation.id.as_str(),
                    operation_status_identifier(operation.status),
                    effect_json,
                ],
            )
            .map_err(|error| Error::Operation(format!("failed to mark operation {id} applied: {error}")))?;

        Ok(operation)
    }

    fn mark_failed(&self, id: &OperationId, reason: &str) -> Result<StoredOperation> {
        let connection = self.connect()?;
        let mut operation = Self::get_operation(&connection, id)?;
        operation.mark_failed(reason)?;

        connection
            .execute(
                "UPDATE operations
                 SET status = ?2, failure_reason = ?3, updated_at = CURRENT_TIMESTAMP
                 WHERE operation_id = ?1",
                params![
                    operation.id.as_str(),
                    operation_status_identifier(operation.status),
                    operation.failure_reason.as_deref(),
                ],
            )
            .map_err(|error| Error::Operation(format!("failed to mark operation {id} failed: {error}")))?;

        Ok(operation)
    }

    fn mark_compensated(&self, id: &OperationId) -> Result<StoredOperation> {
        let connection = self.connect()?;
        let mut operation = Self::get_operation(&connection, id)?;
        operation.mark_compensated();

        connection
            .execute(
                "UPDATE operations
                 SET status = ?2, updated_at = CURRENT_TIMESTAMP
                 WHERE operation_id = ?1",
                params![operation.id.as_str(), operation_status_identifier(operation.status)],
            )
            .map_err(|error| Error::Operation(format!("failed to mark operation {id} compensated: {error}")))?;

        Ok(operation)
    }

    fn get(&self, id: &OperationId) -> Option<StoredOperation> {
        let connection = self.connect().ok()?;
        Self::get_operation(&connection, id).ok()
    }

    fn list(&self) -> Vec<StoredOperation> {
        let connection = match self.connect() {
            Ok(connection) => connection,
            Err(_) => return Vec::new(),
        };
        let mut statement = match connection.prepare(
            "SELECT operation_id, tool, namespace, auth_ref, kind, summary, backend, approval_required, approval_reason, approval_state, approval_actor, approval_note, status, args_json, effect_json, failure_reason
             FROM operations
             ORDER BY created_at DESC, rowid DESC",
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        let rows = match statement.query_map([], row_to_operation) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };

        rows.filter_map(|row| row.ok()).collect()
    }
}

pub fn resolve_operation_store_path(config_path: &Path) -> PathBuf {
    if let Some(path) = env::var_os("SWITCHBOARD_STATE_DB").map(PathBuf::from) {
        return path;
    }

    if let Some(directory) = env::var_os("SWITCHBOARD_STATE_DIR").map(PathBuf::from) {
        return directory.join(DEFAULT_DB_FILE);
    }

    let parent = config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    if config_path.file_name().is_some_and(|name| name == "switchboard.toml") {
        return parent.join(".switchboard").join(DEFAULT_DB_FILE);
    }

    parent.join(DEFAULT_DB_FILE)
}

fn row_to_operation(row: &Row<'_>) -> rusqlite::Result<StoredOperation> {
    let id = row.get::<_, String>(0)?;
    let tool = row.get::<_, String>(1)?;
    let namespace = row.get::<_, String>(2)?;
    let auth_ref = row.get::<_, String>(3)?;
    let kind = row.get::<_, String>(4)?;
    let summary = row.get::<_, String>(5)?;
    let backend = row.get::<_, String>(6)?;
    let approval_required = row.get::<_, bool>(7)?;
    let approval_reason = row.get::<_, Option<String>>(8)?;
    let approval_state = row.get::<_, String>(9)?;
    let approval_actor = row.get::<_, Option<String>>(10)?;
    let approval_note = row.get::<_, Option<String>>(11)?;
    let status = row.get::<_, String>(12)?;
    let args_json = row.get::<_, String>(13)?;
    let effect_json = row.get::<_, Option<String>>(14)?;
    let failure_reason = row.get::<_, Option<String>>(15)?;

    Ok(StoredOperation {
        id: OperationId::new(id).map_err(to_sqlite_error)?,
        tool: switchboard_core::ToolName::new(tool).map_err(to_sqlite_error)?,
        namespace: switchboard_core::NamespaceId::new(namespace).map_err(to_sqlite_error)?,
        auth_ref: switchboard_core::AuthRef::new(auth_ref).map_err(to_sqlite_error)?,
        kind: parse_tool_kind(&kind).map_err(to_sqlite_error)?,
        summary,
        backend: parse_backend_kind(&backend).map_err(to_sqlite_error)?,
        approval_required,
        approval_reason,
        approval: OperationApproval {
            state: parse_approval_state(&approval_state).map_err(to_sqlite_error)?,
            actor: approval_actor,
            note: approval_note,
        },
        status: parse_operation_status(&status).map_err(to_sqlite_error)?,
        args: serde_json::from_str(&args_json).map_err(to_sqlite_error)?,
        effect: decode_effect(effect_json.as_deref()).map_err(to_sqlite_error)?,
        failure_reason,
    })
}

fn encode_effect(effect: Option<&OperationEffect>) -> Result<Option<String>> {
    effect
        .map(|effect| {
            serde_json::to_string(effect)
                .map_err(|error| Error::Operation(format!("failed to encode operation effect: {error}")))
        })
        .transpose()
}

fn decode_effect(effect_json: Option<&str>) -> Result<Option<OperationEffect>> {
    effect_json
        .map(|effect_json| {
            serde_json::from_str(effect_json)
                .map_err(|error| Error::Operation(format!("failed to decode operation effect: {error}")))
        })
        .transpose()
}

fn tool_kind_identifier(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Write => "write",
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

fn operation_status_identifier(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::Planned => "planned",
        OperationStatus::Applied => "applied",
        OperationStatus::Failed => "failed",
        OperationStatus::Compensated => "compensated",
    }
}

fn approval_state_identifier(state: ApprovalState) -> &'static str {
    match state {
        ApprovalState::NotRequired => "not_required",
        ApprovalState::Pending => "pending",
        ApprovalState::Approved => "approved",
        ApprovalState::Rejected => "rejected",
    }
}

fn parse_tool_kind(value: &str) -> Result<ToolKind> {
    match value {
        "read" => Ok(ToolKind::Read),
        "write" => Ok(ToolKind::Write),
        _ => Err(Error::Operation(format!(
            "unknown tool kind in operation store: {value}"
        ))),
    }
}

fn parse_backend_kind(value: &str) -> Result<BackendKind> {
    match value {
        "cli" => Ok(BackendKind::Cli),
        "api" => Ok(BackendKind::Api),
        "local" => Ok(BackendKind::Local),
        "bridge" => Ok(BackendKind::Bridge),
        _ => Err(Error::Operation(format!(
            "unknown backend kind in operation store: {value}"
        ))),
    }
}

fn parse_operation_status(value: &str) -> Result<OperationStatus> {
    match value {
        "planned" => Ok(OperationStatus::Planned),
        "applied" => Ok(OperationStatus::Applied),
        "failed" => Ok(OperationStatus::Failed),
        "compensated" => Ok(OperationStatus::Compensated),
        _ => Err(Error::Operation(format!(
            "unknown operation status in operation store: {value}"
        ))),
    }
}

fn parse_approval_state(value: &str) -> Result<ApprovalState> {
    match value {
        "not_required" => Ok(ApprovalState::NotRequired),
        "pending" => Ok(ApprovalState::Pending),
        "approved" => Ok(ApprovalState::Approved),
        "rejected" => Ok(ApprovalState::Rejected),
        _ => Err(Error::Operation(format!(
            "unknown approval state in operation store: {value}"
        ))),
    }
}

fn to_sqlite_error(error: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string())),
    )
}

fn ensure_column(connection: &Connection, column_name: &str, _definition: &str, statement: &str) -> Result<()> {
    if operation_columns(connection)?.iter().any(|column| column == column_name) {
        return Ok(());
    }

    connection.execute_batch(statement).map_err(|error| {
        Error::Operation(format!(
            "failed to add {column_name} column to operation store: {error}"
        ))
    })?;

    Ok(())
}

fn operation_columns(connection: &Connection) -> Result<Vec<String>> {
    let mut statement = connection
        .prepare("PRAGMA table_info(operations)")
        .map_err(|error| Error::Operation(format!("failed to inspect operation store schema: {error}")))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| Error::Operation(format!("failed to query operation store schema: {error}")))?;

    Ok(rows.filter_map(|row| row.ok()).collect())
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        sync::Mutex,
        time::{SystemTime, UNIX_EPOCH},
    };

    use switchboard_core::{
        BackendKind, ExecutionMode, NamespaceId, OperationEffect, OperationStatus, OperationStore, PlannedAction,
        PlanningTarget, ProviderKind, ResolvedAuth, ResolvedNamespace, ToolArgument, ToolKind, ToolName, ToolOutput,
        ToolRef, ToolRefKind, ToolRequest,
    };

    use super::{resolve_operation_store_path, SqliteOperationStore};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn sqlite_operation_store_persists_operations_across_reopen() {
        let path = temp_db_path("persist");
        let first = SqliteOperationStore::open(&path).expect("store should open");
        let created = first.create(&planned_action()).expect("operation should be created");
        first
            .mark_applied(&created.id, &applied_output())
            .expect("operation should be applied");

        let reopened = SqliteOperationStore::open(&path).expect("store should reopen");
        let stored = reopened.get(&created.id).expect("stored operation should be persisted");

        assert_eq!(stored.status, OperationStatus::Applied);
        assert_eq!(stored.effect.as_ref().map(|effect| effect.undoable), Some(true));
    }

    #[test]
    fn state_path_defaults_under_project_dot_switchboard_for_local_config() {
        let path = resolve_operation_store_path(Path::new("/tmp/project/switchboard.toml"));
        assert_eq!(path, PathBuf::from("/tmp/project/.switchboard/operations.sqlite3"));
    }

    #[test]
    fn state_path_defaults_next_to_profile_config_for_named_config_dir() {
        let path = resolve_operation_store_path(Path::new("/tmp/home/.config/switchboard/config.toml"));
        assert_eq!(path, PathBuf::from("/tmp/home/.config/switchboard/operations.sqlite3"));
    }

    #[test]
    fn state_path_prefers_env_overrides() {
        let _guard = match ENV_LOCK.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        std::env::set_var("SWITCHBOARD_STATE_DIR", "/tmp/override-state");
        std::env::remove_var("SWITCHBOARD_STATE_DB");

        let path = resolve_operation_store_path(Path::new("/tmp/project/switchboard.toml"));

        assert_eq!(path, PathBuf::from("/tmp/override-state/operations.sqlite3"));

        std::env::remove_var("SWITCHBOARD_STATE_DIR");
    }

    fn temp_db_path(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("switchboard-{label}-{stamp}/operations.sqlite3"))
    }

    fn planned_action() -> PlannedAction {
        let request = ToolRequest::new(
            "google.calendar.create",
            "google.personal",
            ExecutionMode::Draft,
            vec![
                ToolArgument::option("title", "Dog hotel pickup").expect("title should build"),
                ToolArgument::option("date", "2026-04-01").expect("date should build"),
            ],
        )
        .expect("request should build");
        let target = PlanningTarget {
            namespace: ResolvedNamespace::new(
                "google.personal",
                ProviderKind::GoogleWorkspace,
                "Google personal",
                "google.personal_auth",
                false,
                None,
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "google.personal_auth",
                ProviderKind::GoogleWorkspace,
                switchboard_core::AuthKind::GoogleOAuthFile,
                "me@gmail.com",
                switchboard_core::AuthSecretRefs::GoogleOAuthFile {
                    credentials: switchboard_core::SecretRef::new("google.personal_oauth")
                        .expect("secret ref should build"),
                },
            )
            .expect("auth should build"),
        };

        PlannedAction::new(
            &request,
            &target,
            ToolKind::Write,
            "Create personal calendar event",
            BackendKind::Cli,
        )
    }

    fn applied_output() -> ToolOutput {
        ToolOutput::new(
            ToolName::new("google.calendar.create").expect("tool should build"),
            NamespaceId::new("google.personal").expect("namespace should build"),
            "Created personal calendar event",
        )
        .with_effect(
            OperationEffect::new(true)
                .with_ref(
                    ToolRef::new(
                        ProviderKind::GoogleWorkspace,
                        NamespaceId::new("google.personal").expect("namespace should build"),
                        ToolRefKind::Event,
                        "evt_123",
                    )
                    .expect("tool ref should build"),
                )
                .with_undo_summary("Delete the created calendar event")
                .expect("undo summary should build"),
        )
    }
}
