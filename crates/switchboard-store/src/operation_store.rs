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
               compensates_operation_id TEXT,
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
            "compensates_operation_id",
            "TEXT",
            "ALTER TABLE operations ADD COLUMN compensates_operation_id TEXT",
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
        ensure_column(
            &connection,
            "verification_json",
            "TEXT",
            "ALTER TABLE operations ADD COLUMN verification_json TEXT",
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

    fn update(
        &self,
        id: &OperationId,
        change: impl FnOnce(&mut StoredOperation) -> Result<()>,
    ) -> Result<StoredOperation> {
        let mut connection = self.connect()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| Error::Operation(format!("failed to claim operation transaction: {error}")))?;
        let mut operation = Self::get_operation(&transaction, id)?;
        change(&mut operation)?;
        let verification = operation
            .verification
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| Error::Operation(format!("failed to encode verification: {error}")))?;
        transaction
            .execute(
                "UPDATE operations SET approval_state = ?2, approval_actor = ?3, approval_note = ?4,
             status = ?5, effect_json = ?6, failure_reason = ?7, verification_json = ?8,
             updated_at = CURRENT_TIMESTAMP WHERE operation_id = ?1",
                params![
                    id.as_str(),
                    approval_state_identifier(operation.approval.state),
                    operation.approval.actor,
                    operation.approval.note,
                    operation_status_identifier(operation.status),
                    encode_effect(operation.effect.as_ref())?,
                    operation.failure_reason,
                    verification
                ],
            )
            .map_err(|error| Error::Operation(format!("failed to update operation {id}: {error}")))?;
        transaction
            .commit()
            .map_err(|error| Error::Operation(format!("failed to commit operation {id}: {error}")))?;
        Ok(operation)
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
                "SELECT operation_id, tool, namespace, auth_ref, kind, summary, backend, approval_required, approval_reason, compensates_operation_id, approval_state, approval_actor, approval_note, status, args_json, effect_json, failure_reason, verification_json
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
                   operation_id, tool, namespace, auth_ref, kind, summary, backend, approval_required, approval_reason, compensates_operation_id, approval_state, approval_actor, approval_note, status, args_json, effect_json, failure_reason
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, NULL, ?12, ?13, NULL, NULL)",
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
                    operation.compensates_operation_id.as_ref().map(OperationId::as_str),
                    approval_state_identifier(operation.approval.state),
                    operation_status_identifier(operation.status),
                    args_json,
                ],
            )
            .map_err(|error| Error::Operation(format!("failed to insert operation {}: {error}", operation.id)))?;

        Ok(operation)
    }

    fn claim_execution(&self, id: &OperationId) -> Result<StoredOperation> {
        self.update(id, |operation| operation.claim_execution())
    }

    fn mark_approved(&self, id: &OperationId, actor: &str, note: Option<&str>) -> Result<StoredOperation> {
        self.update(id, |operation| operation.approve(actor, note.map(str::to_owned)))
    }

    fn mark_rejected(&self, id: &OperationId, actor: &str, note: Option<&str>) -> Result<StoredOperation> {
        self.update(id, |operation| operation.reject(actor, note.map(str::to_owned)))
    }

    fn mark_applied(&self, id: &OperationId, output: &ToolOutput) -> Result<StoredOperation> {
        self.update(id, |operation| operation.mark_applied(output))
    }

    fn mark_failed(&self, id: &OperationId, reason: &str) -> Result<StoredOperation> {
        self.update(id, |operation| operation.mark_failed(reason))
    }

    fn mark_uncertain(&self, id: &OperationId, reason: &str) -> Result<StoredOperation> {
        self.update(id, |operation| operation.mark_uncertain(reason))
    }

    fn record_verification(
        &self,
        id: &OperationId,
        receipt: &switchboard_core::VerificationReceipt,
    ) -> Result<StoredOperation> {
        self.update(id, |operation| operation.record_verification(receipt.clone()))
    }

    fn mark_compensated(&self, id: &OperationId) -> Result<StoredOperation> {
        self.update(id, |operation| {
            operation.can_undo()?;
            operation.mark_compensated()?;
            Ok(())
        })
    }

    fn get(&self, id: &OperationId) -> Result<Option<StoredOperation>> {
        let connection = self.connect()?;
        let exists = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id = ?1)",
                params![id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| Error::Operation(format!("failed to find operation {id}: {error}")))?;
        if exists {
            Self::get_operation(&connection, id).map(Some)
        } else {
            Ok(None)
        }
    }

    fn list(&self) -> Result<Vec<StoredOperation>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT operation_id, tool, namespace, auth_ref, kind, summary, backend, approval_required, approval_reason, compensates_operation_id, approval_state, approval_actor, approval_note, status, args_json, effect_json, failure_reason, verification_json
             FROM operations ORDER BY created_at DESC, rowid DESC",
        ).map_err(|error| Error::Operation(format!("failed to query operations: {error}")))?;
        let rows = statement
            .query_map([], row_to_operation)
            .map_err(|error| Error::Operation(format!("failed to read operations: {error}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| Error::Operation(format!("failed to decode operation: {error}")))
    }
}

pub fn resolve_operation_store_path(config_path: &Path) -> PathBuf {
    operation_store_path(
        config_path,
        env::var_os("SWITCHBOARD_STATE_DB").map(PathBuf::from),
        env::var_os("SWITCHBOARD_STATE_DIR").map(PathBuf::from),
    )
}

fn operation_store_path(config_path: &Path, database: Option<PathBuf>, directory: Option<PathBuf>) -> PathBuf {
    if let Some(path) = database {
        return path;
    }

    if let Some(directory) = directory {
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
    let compensates_operation_id = row.get::<_, Option<String>>(9)?;
    let approval_state = row.get::<_, String>(10)?;
    let approval_actor = row.get::<_, Option<String>>(11)?;
    let approval_note = row.get::<_, Option<String>>(12)?;
    let status = row.get::<_, String>(13)?;
    let args_json = row.get::<_, String>(14)?;
    let effect_json = row.get::<_, Option<String>>(15)?;
    let failure_reason = row.get::<_, Option<String>>(16)?;
    let verification_json = row.get::<_, Option<String>>(17)?;

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
        compensates_operation_id: compensates_operation_id
            .map(OperationId::new)
            .transpose()
            .map_err(to_sqlite_error)?,
        approval: OperationApproval {
            state: parse_approval_state(&approval_state).map_err(to_sqlite_error)?,
            actor: approval_actor,
            note: approval_note,
        },
        status: parse_operation_status(&status).map_err(to_sqlite_error)?,
        args: serde_json::from_str(&args_json).map_err(to_sqlite_error)?,
        effect: decode_effect(effect_json.as_deref()).map_err(to_sqlite_error)?,
        failure_reason,
        verification: verification_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(to_sqlite_error)?,
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
        OperationStatus::Executing => "executing",
        OperationStatus::Uncertain => "uncertain",
        OperationStatus::Verified => "verified",
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
        "executing" => Ok(OperationStatus::Executing),
        "uncertain" => Ok(OperationStatus::Uncertain),
        "verified" => Ok(OperationStatus::Verified),
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
    if operation_columns(connection)?
        .iter()
        .any(|column| column == column_name)
    {
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
        time::{SystemTime, UNIX_EPOCH},
    };

    use switchboard_core::{
        BackendKind, ExecutionMode, NamespaceId, OperationEffect, OperationStatus, OperationStore, PlannedAction,
        PlanningTarget, ProviderKind, ResolvedAuth, ResolvedNamespace, ToolArgument, ToolKind, ToolName, ToolOutput,
        ToolRef, ToolRefKind, ToolRequest,
    };

    use super::{operation_store_path, SqliteOperationStore};

    #[test]
    fn sqlite_operation_store_persists_operations_across_reopen() {
        let path = temp_db_path("persist");
        let first = SqliteOperationStore::open(&path).expect("store should open");
        let created = first.create(&planned_action()).expect("operation should be created");
        first.claim_execution(&created.id).expect("claim should succeed");
        first
            .mark_applied(&created.id, &applied_output())
            .expect("operation should be applied");

        let reopened = SqliteOperationStore::open(&path).expect("store should reopen");
        let stored = reopened
            .get(&created.id)
            .expect("store read should succeed")
            .expect("stored operation should be persisted");

        assert_eq!(stored.status, OperationStatus::Applied);
        assert_eq!(stored.effect.as_ref().map(|effect| effect.undoable), Some(true));
    }

    #[test]
    fn state_path_defaults_under_project_dot_switchboard_for_local_config() {
        let path = operation_store_path(Path::new("/tmp/project/switchboard.toml"), None, None);
        assert_eq!(path, PathBuf::from("/tmp/project/.switchboard/operations.sqlite3"));
    }

    #[test]
    fn state_path_defaults_next_to_profile_config_for_named_config_dir() {
        let path = operation_store_path(Path::new("/tmp/home/.config/switchboard/config.toml"), None, None);
        assert_eq!(path, PathBuf::from("/tmp/home/.config/switchboard/operations.sqlite3"));
    }

    #[test]
    fn state_path_prefers_env_overrides() {
        let config = Path::new("/tmp/project/switchboard.toml");
        let directory = Some(PathBuf::from("/tmp/override-state"));
        let path = operation_store_path(config, None, directory.clone());
        assert_eq!(path, PathBuf::from("/tmp/override-state/operations.sqlite3"));
        let database = PathBuf::from("/tmp/explicit.sqlite3");
        assert_eq!(
            operation_store_path(config, Some(database.clone()), directory),
            database
        );
    }

    fn temp_db_path(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("switchboard-{label}-{stamp}/operations.sqlite3"))
    }

    #[test]
    fn malformed_operation_rows_are_errors_instead_of_empty_results() {
        let path = temp_db_path("malformed");
        let store = SqliteOperationStore::open(&path).expect("test setup should succeed");
        let operation = store.create(&planned_action()).expect("test setup should succeed");
        let connection = rusqlite::Connection::open(&path).expect("test setup should succeed");
        connection
            .execute(
                "UPDATE operations SET args_json = 'broken' WHERE operation_id = ?1",
                [&operation.id.as_str()],
            )
            .expect("test setup should succeed");
        assert!(store.get(&operation.id).is_err());
        assert!(store.list().is_err());
    }

    #[test]
    fn concurrent_claims_have_one_owner_and_uncertain_writes_cannot_retry() {
        let path = temp_db_path("claim");
        let store = SqliteOperationStore::open(&path).expect("test setup should succeed");
        let operation = store.create(&planned_action()).expect("test setup should succeed");
        let first = SqliteOperationStore::open(&path).expect("test setup should succeed");
        let second = SqliteOperationStore::open(&path).expect("test setup should succeed");
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let a = scope.spawn(|| {
                barrier.wait();
                first.claim_execution(&operation.id)
            });
            let b = scope.spawn(|| {
                barrier.wait();
                second.claim_execution(&operation.id)
            });
            [
                a.join().expect("test setup should succeed"),
                b.join().expect("test setup should succeed"),
            ]
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        store
            .mark_uncertain(&operation.id, "provider disconnected after upload")
            .expect("test setup should succeed");
        assert!(store.claim_execution(&operation.id).is_err());
        assert!(store.mark_failed(&operation.id, "pretend safe to retry").is_err());
        let mut receipt = switchboard_core::VerificationReceipt::new(
            switchboard_core::VerificationStatus::Verified,
            "readback matched",
            Vec::new(),
        );
        receipt.recovered_effect = applied_output().effect;
        let verified = store
            .record_verification(&operation.id, &receipt)
            .expect("test setup should succeed");
        assert_eq!(verified.status, OperationStatus::Verified);
        assert!(verified.can_undo().is_ok());
        assert!(store.claim_execution(&operation.id).is_err());
        assert_eq!(
            SqliteOperationStore::open(&path)
                .expect("test setup should succeed")
                .get(&operation.id)
                .expect("test setup should succeed")
                .expect("test setup should succeed")
                .verification,
            Some(receipt)
        );
        let mismatch = switchboard_core::VerificationReceipt::new(
            switchboard_core::VerificationStatus::Mismatch,
            "remote state changed",
            Vec::new(),
        );
        let observed = store
            .record_verification(&operation.id, &mismatch)
            .expect("test setup should succeed");
        assert_eq!(observed.status, OperationStatus::Applied);
        assert_eq!(observed.verification, Some(mismatch));
        assert!(store.claim_execution(&operation.id).is_err());
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
