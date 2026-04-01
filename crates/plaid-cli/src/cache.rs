use std::{fs, path::PathBuf};

use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension};
use serde_json::Value;

use crate::{Error, Result};

pub(crate) const DEFAULT_CACHE_DB_FILE: &str = "plaid-cache.sqlite3";

pub(crate) struct CachedItemRecord {
    pub(crate) item_id: String,
    pub(crate) institution_id: Option<String>,
    pub(crate) updated_at: String,
    pub(crate) item: Value,
}

pub(crate) struct CachedAccountRecord {
    pub(crate) account_id: String,
    pub(crate) item_id: String,
    pub(crate) updated_at: String,
    pub(crate) account: Value,
}

pub(crate) struct CachedTransactionRecord {
    pub(crate) transaction_id: String,
    pub(crate) item_id: String,
    pub(crate) account_id: Option<String>,
    pub(crate) removed: bool,
    pub(crate) updated_at: String,
    pub(crate) removed_at: Option<String>,
    pub(crate) transaction: Value,
    pub(crate) removal: Option<Value>,
}

pub(crate) struct CachedTransactionQuery<'a> {
    pub(crate) item_id: Option<&'a str>,
    pub(crate) account_id: Option<&'a str>,
    pub(crate) transaction_ids: &'a [String],
    pub(crate) include_removed: bool,
    pub(crate) limit: Option<u32>,
}

pub(crate) struct PlaidCacheStore {
    path: PathBuf,
}

impl PlaidCacheStore {
    pub(crate) fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let store = Self { path: path.into() };
        store.connect()?;
        Ok(store)
    }

    pub(crate) fn cached_cursor(&self, item_id: &str, account_scope: Option<&str>) -> Result<Option<String>> {
        let connection = self.connect()?;
        let account_scope = account_scope.unwrap_or("");

        connection
            .query_row(
                "SELECT cursor
                 FROM plaid_sync_cursors
                 WHERE item_id = ?1 AND account_scope = ?2",
                params![item_id, account_scope],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| Error::Cache(format!("failed to load Plaid sync cursor for {item_id}: {error}")))
    }

    pub(crate) fn cache_item(&self, item: &Value) -> Result<Option<String>> {
        let Some(item_id) = item.get("item_id").and_then(Value::as_str) else {
            return Ok(None);
        };
        let data_json = encode_json(item, "Plaid item")?;
        let institution_id = item.get("institution_id").and_then(Value::as_str);
        let connection = self.connect()?;

        connection
            .execute(
                "INSERT INTO plaid_items (item_id, institution_id, data_json, updated_at)
                 VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
                 ON CONFLICT(item_id) DO UPDATE SET
                   institution_id = excluded.institution_id,
                   data_json = excluded.data_json,
                   updated_at = CURRENT_TIMESTAMP",
                params![item_id, institution_id, data_json],
            )
            .map_err(|error| Error::Cache(format!("failed to upsert Plaid item {item_id}: {error}")))?;

        Ok(Some(item_id.to_owned()))
    }

    pub(crate) fn cache_accounts(&self, item_id: &str, accounts: &[Value]) -> Result<()> {
        let mut connection = self.connect()?;
        let transaction = connection
            .transaction()
            .map_err(|error| Error::Cache(format!("failed to begin Plaid account cache transaction: {error}")))?;

        for account in accounts {
            let Some(account_id) = account.get("account_id").and_then(Value::as_str) else {
                continue;
            };
            let data_json = encode_json(account, "Plaid account")?;
            transaction
                .execute(
                    "INSERT INTO plaid_accounts (account_id, item_id, data_json, updated_at)
                     VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
                     ON CONFLICT(account_id) DO UPDATE SET
                       item_id = excluded.item_id,
                       data_json = excluded.data_json,
                       updated_at = CURRENT_TIMESTAMP",
                    params![account_id, item_id, data_json],
                )
                .map_err(|error| Error::Cache(format!("failed to upsert Plaid account {account_id}: {error}")))?;
        }

        transaction
            .commit()
            .map_err(|error| Error::Cache(format!("failed to commit Plaid account cache transaction: {error}")))
    }

    pub(crate) fn cache_transactions_sync(
        &self,
        item_id: &str,
        account_scope: Option<&str>,
        next_cursor: &str,
        added: &[Value],
        modified: &[Value],
        removed: &[Value],
    ) -> Result<()> {
        let mut connection = self.connect()?;
        let transaction = connection
            .transaction()
            .map_err(|error| Error::Cache(format!("failed to begin Plaid transaction cache transaction: {error}")))?;

        for entry in added.iter().chain(modified.iter()) {
            let Some(transaction_id) = entry.get("transaction_id").and_then(Value::as_str) else {
                continue;
            };
            let account_id = entry.get("account_id").and_then(Value::as_str);
            let data_json = encode_json(entry, "Plaid transaction")?;
            transaction
                .execute(
                    "INSERT INTO plaid_transactions (transaction_id, item_id, account_id, data_json, is_removed, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 0, CURRENT_TIMESTAMP)
                     ON CONFLICT(transaction_id) DO UPDATE SET
                       item_id = excluded.item_id,
                       account_id = excluded.account_id,
                       data_json = excluded.data_json,
                       is_removed = 0,
                       updated_at = CURRENT_TIMESTAMP",
                    params![transaction_id, item_id, account_id, data_json],
                )
                .map_err(|error| {
                    Error::Cache(format!("failed to upsert Plaid transaction {transaction_id}: {error}"))
                })?;
        }

        for entry in removed {
            let Some(transaction_id) = entry.get("transaction_id").and_then(Value::as_str) else {
                continue;
            };
            let account_id = entry.get("account_id").and_then(Value::as_str);
            let data_json = encode_json(entry, "Plaid removed transaction")?;
            transaction
                .execute(
                    "INSERT INTO plaid_transactions (
                       transaction_id,
                       item_id,
                       account_id,
                       data_json,
                       is_removed,
                       removed_json,
                       removed_at,
                       updated_at
                     )
                     VALUES (?1, ?2, ?3, ?4, 1, ?4, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
                     ON CONFLICT(transaction_id) DO UPDATE SET
                       item_id = excluded.item_id,
                       account_id = COALESCE(excluded.account_id, plaid_transactions.account_id),
                       is_removed = 1,
                       removed_json = excluded.removed_json,
                       removed_at = CURRENT_TIMESTAMP,
                       updated_at = CURRENT_TIMESTAMP",
                    params![transaction_id, item_id, account_id, data_json],
                )
                .map_err(|error| {
                    Error::Cache(format!(
                        "failed to mark Plaid transaction {transaction_id} removed: {error}"
                    ))
                })?;
        }

        transaction
            .execute(
                "INSERT INTO plaid_sync_cursors (item_id, account_scope, cursor, updated_at)
                 VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
                 ON CONFLICT(item_id, account_scope) DO UPDATE SET
                   cursor = excluded.cursor,
                   updated_at = CURRENT_TIMESTAMP",
                params![item_id, account_scope.unwrap_or(""), next_cursor],
            )
            .map_err(|error| Error::Cache(format!("failed to update Plaid sync cursor for {item_id}: {error}")))?;

        transaction
            .commit()
            .map_err(|error| Error::Cache(format!("failed to commit Plaid transaction cache transaction: {error}")))
    }

    pub(crate) fn cached_items(&self, item_id: Option<&str>) -> Result<Vec<CachedItemRecord>> {
        let connection = self.connect()?;
        let mut sql = String::from(
            "SELECT item_id, institution_id, data_json, updated_at
             FROM plaid_items",
        );
        let mut params = Vec::<SqlValue>::new();
        if let Some(item_id) = item_id {
            sql.push_str(" WHERE item_id = ?");
            params.push(SqlValue::Text(item_id.to_owned()));
        }
        sql.push_str(" ORDER BY item_id ASC");

        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| Error::Cache(format!("failed to prepare Plaid cached item query: {error}")))?;
        let rows = statement
            .query_map(params_from_iter(params), |row| {
                let item_id = row.get::<_, String>(0)?;
                let institution_id = row.get::<_, Option<String>>(1)?;
                let data_json = row.get::<_, String>(2)?;
                let updated_at = row.get::<_, String>(3)?;
                Ok((item_id, institution_id, data_json, updated_at))
            })
            .map_err(|error| Error::Cache(format!("failed to query cached Plaid items: {error}")))?;

        rows.map(|row| {
            let (item_id, institution_id, data_json, updated_at) =
                row.map_err(|error| Error::Cache(format!("failed to read cached Plaid item row: {error}")))?;
            Ok(CachedItemRecord {
                item_id: item_id.clone(),
                institution_id,
                updated_at,
                item: decode_json(&data_json, "cached Plaid item", &item_id)?,
            })
        })
        .collect()
    }

    pub(crate) fn cached_accounts(
        &self,
        item_id: Option<&str>,
        account_ids: &[String],
    ) -> Result<Vec<CachedAccountRecord>> {
        let connection = self.connect()?;
        let mut sql = String::from(
            "SELECT account_id, item_id, data_json, updated_at
             FROM plaid_accounts",
        );
        let mut clauses = Vec::new();
        let mut params = Vec::<SqlValue>::new();
        if let Some(item_id) = item_id {
            clauses.push("item_id = ?".to_owned());
            params.push(SqlValue::Text(item_id.to_owned()));
        }
        if !account_ids.is_empty() {
            clauses.push(format!("account_id IN ({})", repeat_vars(account_ids.len())));
            params.extend(account_ids.iter().cloned().map(SqlValue::Text));
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY account_id ASC");

        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| Error::Cache(format!("failed to prepare cached Plaid account query: {error}")))?;
        let rows = statement
            .query_map(params_from_iter(params), |row| {
                let account_id = row.get::<_, String>(0)?;
                let item_id = row.get::<_, String>(1)?;
                let data_json = row.get::<_, String>(2)?;
                let updated_at = row.get::<_, String>(3)?;
                Ok((account_id, item_id, data_json, updated_at))
            })
            .map_err(|error| Error::Cache(format!("failed to query cached Plaid accounts: {error}")))?;

        rows.map(|row| {
            let (account_id, item_id, data_json, updated_at) =
                row.map_err(|error| Error::Cache(format!("failed to read cached Plaid account row: {error}")))?;
            Ok(CachedAccountRecord {
                account_id: account_id.clone(),
                item_id,
                updated_at,
                account: decode_json(&data_json, "cached Plaid account", &account_id)?,
            })
        })
        .collect()
    }

    pub(crate) fn cached_transactions(
        &self,
        query: CachedTransactionQuery<'_>,
    ) -> Result<Vec<CachedTransactionRecord>> {
        let connection = self.connect()?;
        let mut sql = String::from(
            "SELECT transaction_id, item_id, account_id, data_json, is_removed, updated_at
             , removed_json, removed_at
             FROM plaid_transactions",
        );
        let mut clauses = Vec::new();
        let mut params = Vec::<SqlValue>::new();
        if let Some(item_id) = query.item_id {
            clauses.push("item_id = ?".to_owned());
            params.push(SqlValue::Text(item_id.to_owned()));
        }
        if let Some(account_id) = query.account_id {
            clauses.push("account_id = ?".to_owned());
            params.push(SqlValue::Text(account_id.to_owned()));
        }
        if !query.transaction_ids.is_empty() {
            clauses.push(format!(
                "transaction_id IN ({})",
                repeat_vars(query.transaction_ids.len())
            ));
            params.extend(query.transaction_ids.iter().cloned().map(SqlValue::Text));
        }
        if !query.include_removed {
            clauses.push("is_removed = 0".to_owned());
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY transaction_id ASC");
        if let Some(limit) = query.limit {
            sql.push_str(" LIMIT ?");
            params.push(SqlValue::Integer(limit.into()));
        }

        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| Error::Cache(format!("failed to prepare cached Plaid transaction query: {error}")))?;
        let rows = statement
            .query_map(params_from_iter(params), |row| {
                let transaction_id = row.get::<_, String>(0)?;
                let item_id = row.get::<_, String>(1)?;
                let account_id = row.get::<_, Option<String>>(2)?;
                let data_json = row.get::<_, String>(3)?;
                let removed = row.get::<_, bool>(4)?;
                let updated_at = row.get::<_, String>(5)?;
                let removed_json = row.get::<_, Option<String>>(6)?;
                let removed_at = row.get::<_, Option<String>>(7)?;
                Ok((
                    transaction_id,
                    item_id,
                    account_id,
                    data_json,
                    removed,
                    updated_at,
                    removed_json,
                    removed_at,
                ))
            })
            .map_err(|error| Error::Cache(format!("failed to query cached Plaid transactions: {error}")))?;

        rows.map(|row| {
            let (transaction_id, item_id, account_id, data_json, removed, updated_at, removed_json, removed_at) =
                row.map_err(|error| Error::Cache(format!("failed to read cached Plaid transaction row: {error}")))?;
            Ok(CachedTransactionRecord {
                transaction_id: transaction_id.clone(),
                item_id,
                account_id,
                removed,
                updated_at,
                removed_at,
                transaction: decode_json(&data_json, "cached Plaid transaction", &transaction_id)?,
                removal: removed_json
                    .as_deref()
                    .map(|removed_json| decode_json(removed_json, "cached Plaid removal", &transaction_id))
                    .transpose()?,
            })
        })
        .collect()
    }

    fn connect(&self) -> Result<Connection> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                Error::Cache(format!(
                    "failed to create Plaid cache directory {}: {error}",
                    parent.display()
                ))
            })?;
        }

        let connection = Connection::open(&self.path)
            .map_err(|error| Error::Cache(format!("failed to open Plaid cache {}: {error}", self.path.display())))?;

        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA foreign_keys = ON;
                 CREATE TABLE IF NOT EXISTS plaid_items (
                   item_id TEXT PRIMARY KEY,
                   institution_id TEXT,
                   data_json TEXT NOT NULL,
                   updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                 );
                 CREATE TABLE IF NOT EXISTS plaid_accounts (
                   account_id TEXT PRIMARY KEY,
                   item_id TEXT NOT NULL,
                   data_json TEXT NOT NULL,
                   updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                 );
                 CREATE INDEX IF NOT EXISTS plaid_accounts_item_id_idx
                   ON plaid_accounts (item_id);
                 CREATE TABLE IF NOT EXISTS plaid_transactions (
                   transaction_id TEXT PRIMARY KEY,
                   item_id TEXT NOT NULL,
                   account_id TEXT,
                   data_json TEXT NOT NULL,
                   is_removed INTEGER NOT NULL DEFAULT 0,
                   removed_json TEXT,
                   removed_at TEXT,
                   updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                 );
                 CREATE INDEX IF NOT EXISTS plaid_transactions_item_id_idx
                   ON plaid_transactions (item_id);
                 CREATE INDEX IF NOT EXISTS plaid_transactions_account_id_idx
                   ON plaid_transactions (account_id);
                 CREATE TABLE IF NOT EXISTS plaid_sync_cursors (
                   item_id TEXT NOT NULL,
                   account_scope TEXT NOT NULL,
                   cursor TEXT NOT NULL,
                   updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                   PRIMARY KEY (item_id, account_scope)
                 );",
            )
            .map_err(|error| {
                Error::Cache(format!(
                    "failed to initialize Plaid cache {}: {error}",
                    self.path.display()
                ))
            })?;
        ensure_column(&connection, "plaid_transactions", "removed_json", "TEXT")?;
        ensure_column(&connection, "plaid_transactions", "removed_at", "TEXT")?;

        Ok(connection)
    }
}

fn encode_json(value: &Value, label: &str) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|error| Error::Cache(format!("failed to serialize {label} for cache: {error}")))
}

fn decode_json(data_json: &str, label: &str, identifier: &str) -> Result<Value> {
    serde_json::from_str(data_json)
        .map_err(|error| Error::Cache(format!("failed to decode {label} {identifier} from cache: {error}")))
}

fn repeat_vars(count: usize) -> String {
    std::iter::repeat("?").take(count).collect::<Vec<_>>().join(", ")
}

fn ensure_column(connection: &Connection, table: &str, column: &str, definition: &str) -> Result<()> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| Error::Cache(format!("failed to inspect Plaid cache table {table}: {error}")))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| Error::Cache(format!("failed to query Plaid cache table info for {table}: {error}")))?;
    let column_exists = columns
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| Error::Cache(format!("failed to read Plaid cache table info for {table}: {error}")))?
        .iter()
        .any(|existing| existing == column);

    if column_exists {
        return Ok(());
    }

    connection
        .execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"), [])
        .map_err(|error| {
            Error::Cache(format!(
                "failed to migrate Plaid cache table {table}, missing column {column}: {error}"
            ))
        })?;

    Ok(())
}
