use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{params_from_iter, types::Value, Connection, OpenFlags, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    content, ActivityBucket, Attachment, BodyStatus, Chat, Command, Counts, Error, Interval, Message, MessageKind,
    Page, Participant, Response, Result,
};

const SEARCH_SCAN_LIMIT: usize = 5000;
const ATTACHMENT_LIMIT: u64 = 16 * 1024 * 1024;
const APPLE_EPOCH: i64 = 978_307_200;

pub(crate) struct Database {
    connection: Connection,
    path: PathBuf,
    attachment_root: PathBuf,
    join_dates: bool,
    nanos_per_tick: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    fingerprint: String,
    position: Position,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Position {
    Chat { date: Option<i64>, id: i64 },
    Message { date: Option<i64>, id: i64, chat: i64 },
    Activity { bucket: String },
}

impl Database {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let path = path.canonicalize().map_err(|error| Error::Access(error.to_string()))?;
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| Error::Access(error.to_string()))?;
        connection.busy_timeout(Duration::from_secs(2))?;
        // query_only is defense in depth. The connection itself has no write authority.
        connection.execute_batch("PRAGMA query_only = ON; PRAGMA trusted_schema = OFF; BEGIN DEFERRED;")?;
        for (table, columns) in [
            ("chat", &["guid", "display_name", "service_name"][..]),
            (
                "message",
                &[
                    "guid",
                    "text",
                    "attributedBody",
                    "date",
                    "handle_id",
                    "is_from_me",
                    "is_read",
                    "is_finished",
                    "item_type",
                    "associated_message_guid",
                    "associated_message_type",
                ][..],
            ),
            ("handle", &["id"][..]),
            ("chat_message_join", &["chat_id", "message_id"][..]),
            ("chat_handle_join", &["chat_id", "handle_id"][..]),
            (
                "attachment",
                &["guid", "filename", "transfer_name", "mime_type", "total_bytes"][..],
            ),
            ("message_attachment_join", &["message_id", "attachment_id"][..]),
        ] {
            let actual = table_columns(&connection, table)?;
            for column in columns {
                if !actual.iter().any(|name| name == column) {
                    return Err(Error::Schema(format!("missing {table}.{column}")));
                }
            }
        }
        let join_dates = table_columns(&connection, "chat_message_join")?
            .iter()
            .any(|name| name == "message_date");
        let latest: Option<i64> = connection
            .query_row(
                "SELECT date FROM message WHERE date IS NOT NULL ORDER BY date DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let nanos_per_tick = if latest.is_some_and(|date| date != 0 && date.unsigned_abs() < 1_000_000_000_000) {
            1_000_000_000
        } else {
            1
        };
        let attachment_root = path
            .parent()
            .ok_or_else(|| Error::Access("database has no parent directory".into()))?
            .join("Attachments");
        Ok(Self {
            connection,
            path,
            attachment_root,
            join_dates,
            nanos_per_tick,
        })
    }

    pub(crate) fn execute(&self, command: &Command) -> Result<Response> {
        match command {
            Command::Doctor => Ok(Response::Doctor {
                database: self.path.clone(),
                read_only: true,
                contacts_available: content::contacts_available(),
                history: "local_only".into(),
            }),
            Command::Chats {
                participants,
                unread,
                page,
            } => self.chats(participants, *unread, page),
            Command::Read { chat_id, unread, page } => self.messages(Some(chat_id), *unread, &[], None, page),
            Command::Search {
                query,
                chat,
                participants,
                page,
            } => {
                if query.trim().is_empty() {
                    return Err(Error::Arguments("search text cannot be empty".into()));
                }
                self.messages(chat.as_deref(), false, participants, Some(query), page)
            }
            Command::Attachment { attachment_id, .. } => self.attachment(attachment_id),
            Command::Activity { chat, interval, page } => self.activity(chat.as_deref(), *interval, page),
        }
    }

    fn fingerprint(&self, filters: impl Serialize) -> Result<String> {
        let metadata = std::fs::metadata(&self.path).map_err(|error| Error::Access(error.to_string()))?;
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            (metadata.dev(), metadata.ino())
        };
        #[cfg(not(unix))]
        let identity = (metadata.len(), 0_u64);
        let bytes = serde_json::to_vec(&(self.path.to_string_lossy(), identity, filters))
            .map_err(|error| Error::Arguments(error.to_string()))?;
        Ok(URL_SAFE_NO_PAD.encode(Sha256::digest(bytes)))
    }

    fn cursor(&self, page: &Page, fingerprint: &str) -> Result<Option<Position>> {
        let Some(encoded) = &page.cursor else {
            return Ok(None);
        };
        if encoded.len() > 4096 {
            return Err(Error::Arguments("invalid cursor".into()));
        }
        let cursor: Cursor = URL_SAFE_NO_PAD
            .decode(encoded)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| Error::Arguments("invalid cursor".into()))?;
        if cursor.version != 1 || cursor.fingerprint != fingerprint {
            return Err(Error::Arguments(
                "cursor belongs to different filters or a different database".into(),
            ));
        }
        Ok(Some(cursor.position))
    }

    fn encode_cursor(&self, fingerprint: &str, position: Position) -> Result<String> {
        let bytes = serde_json::to_vec(&Cursor {
            version: 1,
            fingerprint: fingerprint.into(),
            position,
        })
        .map_err(|error| Error::Output(error.to_string()))?;
        Ok(URL_SAFE_NO_PAD.encode(bytes))
    }

    fn date_filters(
        &self,
        page: &Page,
        column: &str,
        conditions: &mut Vec<String>,
        values: &mut Vec<Value>,
    ) -> Result<()> {
        let since = page
            .since
            .as_deref()
            .map(|date| parse_date(&self.connection, date))
            .transpose()?;
        let until = page
            .until
            .as_deref()
            .map(|date| parse_date(&self.connection, date))
            .transpose()?;
        if matches!((since, until), (Some(start), Some(end)) if start >= end) {
            return Err(Error::Arguments("--since must be earlier than --until".into()));
        }
        for (bound, operator) in [(since, ">="), (until, "<")] {
            if let Some(bound) = bound {
                let ticks =
                    bound.div_euclid(self.nanos_per_tick) + i64::from(bound.rem_euclid(self.nanos_per_tick) != 0);
                conditions.push(format!("{column} {operator} ?"));
                values.push(ticks.into());
            }
        }
        Ok(())
    }

    fn participant_filters(
        &self,
        participants: &[String],
        names: &HashMap<String, String>,
        conditions: &mut Vec<String>,
        values: &mut Vec<Value>,
    ) -> Result<()> {
        for participant in participants {
            if participant.trim().is_empty() {
                return Err(Error::Arguments("--with cannot be empty".into()));
            }
            let normalized = content::normalize_handle(participant);
            let mut handles: Vec<String> = names
                .iter()
                .filter(|(_, name)| name.to_lowercase().contains(&participant.to_lowercase()))
                .map(|(handle, _)| handle.clone())
                .collect();
            handles.push(normalized);
            handles.sort();
            handles.dedup();
            let placeholders = placeholders(handles.len());
            // Handles are normalized in SQL using the same phone punctuation rules as Contacts.
            conditions.push(format!("EXISTS (SELECT 1 FROM chat_handle_join ph JOIN handle h ON h.ROWID=ph.handle_id WHERE ph.chat_id=c.ROWID AND CASE WHEN instr(h.id,'@')>0 THEN lower(trim(h.id)) ELSE lower(replace(replace(replace(replace(replace(h.id,' ',''),'-',''),'(',''),')',''),'.','')) END IN ({placeholders}))"));
            values.extend(handles.into_iter().map(Value::Text));
        }
        Ok(())
    }

    fn timestamp_sql(&self, alias: &str) -> String {
        let divisor = 1_000_000_000 / self.nanos_per_tick;
        format!("strftime('%Y-%m-%dT%H:%M:%fZ', {alias}.date / {divisor}.0 + {APPLE_EPOCH}, 'unixepoch')")
    }

    fn chats(&self, participants: &[String], unread: bool, page: &Page) -> Result<Response> {
        let names = content::contact_names();
        let fingerprint = self.fingerprint(("chats", participants, unread, page))?;
        let mut conditions = Vec::new();
        let mut values = Vec::new();
        self.date_filters(page, "m.date", &mut conditions, &mut values)?;
        self.participant_filters(participants, &names, &mut conditions, &mut values)?;
        if unread {
            conditions.push("EXISTS (SELECT 1 FROM chat_message_join u JOIN message um ON um.ROWID=u.message_id WHERE u.chat_id=c.ROWID AND um.is_from_me=0 AND um.is_read=0 AND um.is_finished=1)".into());
        }
        if let Some(position) = self.cursor(page, &fingerprint)? {
            let Position::Chat { date, id } = position else {
                return Err(Error::Arguments("invalid chat cursor".into()));
            };
            if let Some(date) = date {
                conditions.push("((m.date,c.ROWID) < (?,?) OR m.date IS NULL)".into());
                values.extend([date.into(), id.into()]);
            } else {
                conditions.push("m.date IS NULL AND c.ROWID < ?".into());
                values.push(id.into());
            }
        }
        let limit = usize::from(page.limit.unwrap_or(20));
        values.push(
            i64::try_from(limit + 1)
                .map_err(|error| Error::Arguments(error.to_string()))?
                .into(),
        );
        let latest_order = if self.join_dates {
            "cm.message_date DESC,cm.message_id DESC"
        } else {
            "lm.date DESC,lm.ROWID DESC"
        };
        let sql = format!("SELECT c.ROWID,c.guid,c.display_name,c.service_name,m.date,{},m.text,m.attributedBody FROM chat c JOIN message m ON m.ROWID=(SELECT lm.ROWID FROM chat_message_join cm JOIN message lm ON lm.ROWID=cm.message_id WHERE cm.chat_id=c.ROWID ORDER BY {latest_order} LIMIT 1) {} ORDER BY m.date DESC,c.ROWID DESC LIMIT ?", self.timestamp_sql("m"), where_clause(&conditions));
        let mut statement = self.connection.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(values))?;
        let mut result = Vec::new();
        let mut ids = Vec::new();
        let mut positions = Vec::new();
        while let Some(row) = rows.next()? {
            let body = content::decode_body(row.get(6)?, row.get(7)?);
            ids.push(row.get::<_, i64>(0)?);
            positions.push((row.get::<_, Option<i64>>(4)?, row.get::<_, i64>(0)?));
            result.push(Chat {
                id: row.get(1)?,
                name: row.get(2)?,
                service: row.get(3)?,
                participants: Vec::new(),
                unread_count: 0,
                last_message_at: row.get(5)?,
                preview: body.text.map(|text| text.chars().take(160).collect()),
                preview_status: body.status,
            });
        }
        let has_more = result.len() > limit;
        result.truncate(limit);
        ids.truncate(limit);
        positions.truncate(limit);
        let next_cursor = if has_more {
            positions
                .last()
                .map(|(date, id)| self.encode_cursor(&fingerprint, Position::Chat { date: *date, id: *id }))
                .transpose()?
        } else {
            None
        };
        if !ids.is_empty() {
            let sql = format!("SELECT cj.chat_id,h.id FROM chat_handle_join cj JOIN handle h ON h.ROWID=cj.handle_id WHERE cj.chat_id IN ({}) ORDER BY h.id", placeholders(ids.len()));
            let mut statement = self.connection.prepare(&sql)?;
            let mut rows = statement.query(params_from_iter(&ids))?;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                if let Some(index) = ids.iter().position(|candidate| *candidate == id) {
                    result[index].participants.push(participant(row.get(1)?, &names));
                }
            }
            let sql = format!("SELECT cm.chat_id,COUNT(*) FROM chat_message_join cm JOIN message m ON m.ROWID=cm.message_id WHERE cm.chat_id IN ({}) AND m.is_from_me=0 AND m.is_read=0 AND m.is_finished=1 GROUP BY cm.chat_id", placeholders(ids.len()));
            let mut statement = self.connection.prepare(&sql)?;
            let mut rows = statement.query(params_from_iter(&ids))?;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                if let Some(index) = ids.iter().position(|candidate| *candidate == id) {
                    result[index].unread_count = count(row, 1)?;
                }
            }
        }
        let unsupported_bodies = result
            .iter()
            .filter(|chat| matches!(chat.preview_status, BodyStatus::Unsupported))
            .count() as u64;
        Ok(Response::Chats {
            chats: result,
            next_cursor,
            unsupported_bodies,
        })
    }

    fn chat_id(&self, guid: &str) -> Result<i64> {
        self.connection
            .query_row("SELECT ROWID FROM chat WHERE guid=?", [guid], |row| row.get(0))
            .optional()?
            .ok_or_else(|| Error::NotFound("chat".into()))
    }

    fn messages(
        &self,
        chat: Option<&str>,
        unread: bool,
        participants: &[String],
        query: Option<&str>,
        page: &Page,
    ) -> Result<Response> {
        let names = content::contact_names();
        let fingerprint = self.fingerprint(("messages", chat, unread, participants, query, page))?;
        let mut conditions = Vec::new();
        let mut values = Vec::new();
        let indexed_chat = chat.is_some() && self.join_dates;
        let date_column = if indexed_chat { "cm.message_date" } else { "m.date" };
        self.date_filters(page, date_column, &mut conditions, &mut values)?;
        self.participant_filters(participants, &names, &mut conditions, &mut values)?;
        if let Some(chat) = chat {
            conditions.push("cm.chat_id=?".into());
            values.push(self.chat_id(chat)?.into());
        }
        if unread {
            conditions.push("m.is_from_me=0 AND m.is_read=0 AND m.is_finished=1".into());
        }
        let base_conditions = conditions.clone();
        let base_values = values.clone();
        let mut append_null_dates = false;
        if let Some(position) = self.cursor(page, &fingerprint)? {
            let Position::Message { date, id, chat } = position else {
                return Err(Error::Arguments("invalid message cursor".into()));
            };
            if let Some(date) = date {
                if indexed_chat {
                    conditions.push("(cm.message_date,cm.message_id) < (?,?)".into());
                    values.extend([date.into(), id.into()]);
                } else {
                    conditions.push("(m.date,m.ROWID,c.ROWID) < (?,?,?)".into());
                    values.extend([date.into(), id.into(), chat.into()]);
                }
                append_null_dates = true;
            } else {
                conditions.push(format!("{date_column} IS NULL AND (m.ROWID,c.ROWID) < (?,?)"));
                values.extend([id.into(), chat.into()]);
            }
        }
        let limit = usize::from(page.limit.unwrap_or(50));
        let scan_limit = if query.is_some() { SEARCH_SCAN_LIMIT } else { limit };
        // Pin the outer loop to the index that owns this page's ordering. Otherwise
        // SQLite can sort the entire conversation before applying a small LIMIT.
        let source = if indexed_chat {
            "chat_message_join cm CROSS JOIN message m ON m.ROWID=cm.message_id CROSS JOIN chat c ON c.ROWID=cm.chat_id"
        } else {
            "message m CROSS JOIN chat_message_join cm ON cm.message_id=m.ROWID CROSS JOIN chat c ON c.ROWID=cm.chat_id"
        };
        let order = if indexed_chat {
            "cm.message_date DESC,cm.message_id DESC"
        } else {
            "m.date DESC,m.ROWID DESC,c.ROWID DESC"
        };
        let message_id = if indexed_chat { "cm.message_id" } else { "m.ROWID" };
        let select = format!("SELECT {message_id},c.ROWID,{date_column},m.guid,c.guid,{},m.text,m.attributedBody,h.id,COALESCE(m.is_from_me,0),COALESCE(m.is_read,0),COALESCE(m.is_finished,0),COALESCE(m.item_type,0),COALESCE(m.associated_message_type,0),m.associated_message_guid FROM {source} LEFT JOIN handle h ON h.ROWID=m.handle_id", self.timestamp_sql("m"));
        let sql = if append_null_dates {
            // Separate ordered ranges let SQLite merge the date index with its
            // NULL tail. An OR here makes it sort the whole conversation.
            let mut null_conditions = base_conditions;
            null_conditions.push(format!("{date_column} IS NULL"));
            values.extend(base_values);
            let order = if indexed_chat {
                "3 DESC,1 DESC"
            } else {
                "3 DESC,1 DESC,2 DESC"
            };
            format!(
                "{select} {} UNION ALL {select} {} ORDER BY {order} LIMIT ?",
                where_clause(&conditions),
                where_clause(&null_conditions)
            )
        } else {
            format!("{select} {} ORDER BY {order} LIMIT ?", where_clause(&conditions))
        };
        values.push(
            i64::try_from(scan_limit + 1)
                .map_err(|error| Error::Arguments(error.to_string()))?
                .into(),
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(values))?;
        let query = query.map(str::to_lowercase);
        let mut messages = Vec::new();
        let mut scanned_messages = 0;
        let mut unsupported_bodies = 0;
        let mut last = None;
        while scanned_messages < scan_limit as u64 && messages.len() < limit {
            let Some(row) = rows.next()? else {
                break;
            };
            scanned_messages += 1;
            last = Some(Position::Message {
                date: row.get(2)?,
                id: row.get(0)?,
                chat: row.get(1)?,
            });
            let body = content::decode_body(row.get(6)?, row.get(7)?);
            if matches!(body.status, BodyStatus::Unsupported) {
                unsupported_bodies += 1;
            }
            if query.as_ref().is_some_and(|query| {
                !body
                    .text
                    .as_ref()
                    .is_some_and(|text| text.to_lowercase().contains(query))
            }) {
                continue;
            }
            let from_me: bool = row.get(9)?;
            messages.push(Message {
                row_id: row.get(0)?,
                id: row.get(3)?,
                chat_id: row.get(4)?,
                date: row.get(5)?,
                sender: row
                    .get::<_, Option<String>>(8)?
                    .map(|handle| participant(handle, &names)),
                is_from_me: from_me,
                is_unread: !from_me && !row.get::<_, bool>(10)? && row.get::<_, bool>(11)?,
                text: body.text,
                body_status: body.status,
                kind: if row.get::<_, i64>(13)? != 0 {
                    MessageKind::Reaction
                } else if row.get::<_, i64>(12)? != 0 {
                    MessageKind::System
                } else {
                    MessageKind::Message
                },
                associated_message_id: row.get(14)?,
                attachments: Vec::new(),
            });
        }
        let has_more = rows.next()?.is_some();
        let next_cursor = if has_more {
            last.map(|position| self.encode_cursor(&fingerprint, position))
                .transpose()?
        } else {
            None
        };
        drop(rows);
        drop(statement);
        self.message_attachments(&mut messages)?;
        if query.is_none() {
            messages.reverse();
        }
        Ok(Response::Messages {
            messages,
            next_cursor,
            scanned_messages,
            unsupported_bodies,
        })
    }

    fn attachment_path(&self, filename: &str) -> Option<PathBuf> {
        if std::fs::symlink_metadata(&self.attachment_root)
            .ok()?
            .file_type()
            .is_symlink()
        {
            return None;
        }
        let path = if let Some(rest) = filename.strip_prefix("~/Library/Messages/Attachments/") {
            self.attachment_root.join(rest)
        } else {
            PathBuf::from(filename)
        };
        let relative = path.strip_prefix(&self.attachment_root).ok()?;
        if relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return None;
        }
        Some(path)
    }

    fn attachment_metadata(&self, row: &Row<'_>, offset: usize) -> rusqlite::Result<Attachment> {
        let stored: Option<String> = row.get(offset + 1)?;
        let transfer: Option<String> = row.get(offset + 2)?;
        let filename = transfer.or_else(|| {
            stored
                .as_ref()
                .and_then(|path| Path::new(path).file_name())
                .map(|name| name.to_string_lossy().into_owned())
        });
        Ok(Attachment {
            id: row.get(offset)?,
            filename,
            mime_type: row.get(offset + 3)?,
            size_bytes: count(row, offset + 4)?,
            available_locally: stored
                .as_deref()
                .and_then(|path| self.attachment_path(path))
                .is_some_and(|path| {
                    path.strip_prefix(&self.attachment_root)
                        .ok()
                        .is_some_and(|relative| content::open_attachment(&self.attachment_root, relative).is_ok())
                }),
        })
    }

    fn message_attachments(&self, messages: &mut [Message]) -> Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let ids: Vec<i64> = messages.iter().map(|message| message.row_id).collect();
        let sql = format!("SELECT ma.message_id,a.guid,a.filename,a.transfer_name,a.mime_type,a.total_bytes FROM message_attachment_join ma JOIN attachment a ON a.ROWID=ma.attachment_id WHERE ma.message_id IN ({}) ORDER BY a.ROWID", placeholders(ids.len()));
        let mut statement = self.connection.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(ids))?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let attachment = self.attachment_metadata(row, 1)?;
            // A message can be associated with multiple conversations in search results.
            for message in messages.iter_mut().filter(|message| message.row_id == id) {
                message.attachments.push(Attachment {
                    id: attachment.id.clone(),
                    filename: attachment.filename.clone(),
                    mime_type: attachment.mime_type.clone(),
                    size_bytes: attachment.size_bytes,
                    available_locally: attachment.available_locally,
                });
            }
        }
        Ok(())
    }

    fn attachment(&self, id: &str) -> Result<Response> {
        let mut statement = self
            .connection
            .prepare("SELECT guid,filename,transfer_name,mime_type,total_bytes FROM attachment WHERE guid=?")?;
        let (attachment, filename) = statement
            .query_row([id], |row| {
                Ok((self.attachment_metadata(row, 0)?, row.get::<_, Option<String>>(1)?))
            })
            .optional()?
            .ok_or_else(|| Error::NotFound("attachment".into()))?;
        let path = filename
            .as_deref()
            .and_then(|filename| self.attachment_path(filename))
            .ok_or_else(|| {
                Error::Attachment("file is unavailable locally or outside the Messages attachment directory".into())
            })?;
        let relative = path
            .strip_prefix(&self.attachment_root)
            .map_err(|error| Error::Attachment(error.to_string()))?;
        let file = content::open_attachment(&self.attachment_root, relative)
            .map_err(|error| Error::Attachment(error.to_string()))?;
        if !file
            .metadata()
            .map_err(|error| Error::Attachment(error.to_string()))?
            .is_file()
        {
            return Err(Error::Attachment("not a regular file".into()));
        }
        let mut bytes = Vec::new();
        file.take(ATTACHMENT_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| Error::Attachment(error.to_string()))?;
        if bytes.len() as u64 > ATTACHMENT_LIMIT {
            return Err(Error::Attachment("file exceeds the 16 MiB read limit".into()));
        }
        Ok(Response::Attachment {
            attachment,
            content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        })
    }

    fn activity(&self, chat: Option<&str>, interval: Option<Interval>, page: &Page) -> Result<Response> {
        let fingerprint = self.fingerprint(("activity", chat, interval, page))?;
        let mut conditions = vec!["COALESCE(m.item_type,0)=0 AND COALESCE(m.associated_message_type,0)=0".into()];
        let mut values = Vec::new();
        self.date_filters(page, "m.date", &mut conditions, &mut values)?;
        if let Some(chat) = chat {
            conditions.push(
                "EXISTS (SELECT 1 FROM chat_message_join cm WHERE cm.message_id=m.ROWID AND cm.chat_id=?)".into(),
            );
            values.push(self.chat_id(chat)?.into());
        }
        let Some(interval) = interval else {
            if page.cursor.is_some() {
                return Err(Error::Arguments("activity cursors require --interval".into()));
            }
            let sql = format!("SELECT COUNT(*),COALESCE(SUM(m.is_from_me=1),0),COALESCE(SUM(COALESCE(m.is_from_me,0)=0),0) FROM message m {}",where_clause(&conditions));
            let counts = self.connection.query_row(&sql, params_from_iter(values), counts_row)?;
            return Ok(Response::Activity {
                counts,
                buckets: Vec::new(),
                next_cursor: None,
            });
        };
        let divisor = 1_000_000_000 / self.nanos_per_tick;
        let date = format!("m.date/{divisor}.0+{APPLE_EPOCH}");
        let bucket = match interval {
            Interval::Day => format!("date({date},'unixepoch')"),
            Interval::Week => format!("date({date},'unixepoch','weekday 0','-6 days')"),
            Interval::Month => format!("date({date},'unixepoch','start of month')"),
        };
        let cursor = self.cursor(page, &fingerprint)?;
        let cursor_clause = match cursor {
            Some(Position::Activity { bucket }) => {
                values.push(bucket.into());
                "WHERE start > ?"
            }
            Some(_) => return Err(Error::Arguments("invalid activity cursor".into())),
            None => "",
        };
        let limit = usize::from(page.limit.unwrap_or(100));
        values.push(
            i64::try_from(limit + 1)
                .map_err(|error| Error::Arguments(error.to_string()))?
                .into(),
        );
        let sql = format!("WITH buckets AS (SELECT COALESCE({bucket},'undated') AS start,COUNT(*) AS total,COALESCE(SUM(m.is_from_me=1),0) AS sent,COALESCE(SUM(COALESCE(m.is_from_me,0)=0),0) AS received FROM message m {} GROUP BY start), totals AS (SELECT *,SUM(total) OVER () AS all_total,SUM(sent) OVER () AS all_sent,SUM(received) OVER () AS all_received FROM buckets) SELECT start,total,sent,received,all_total,all_sent,all_received FROM totals {cursor_clause} ORDER BY start LIMIT ?",where_clause(&conditions));
        let mut statement = self.connection.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(values))?;
        let mut buckets = Vec::new();
        let mut counts = Counts::default();
        while let Some(row) = rows.next()? {
            counts = Counts {
                total: count(row, 4)?,
                sent: count(row, 5)?,
                received: count(row, 6)?,
            };
            buckets.push(ActivityBucket {
                start: row.get(0)?,
                counts: Counts {
                    total: count(row, 1)?,
                    sent: count(row, 2)?,
                    received: count(row, 3)?,
                },
            });
        }
        let more = buckets.len() > limit;
        buckets.truncate(limit);
        let next_cursor = if more {
            buckets
                .last()
                .map(|bucket| {
                    self.encode_cursor(
                        &fingerprint,
                        Position::Activity {
                            bucket: bucket.start.clone(),
                        },
                    )
                })
                .transpose()?
        } else {
            None
        };
        Ok(Response::Activity {
            counts,
            buckets,
            next_cursor,
        })
    }
}

fn table_columns(connection: &Connection, table: &str) -> Result<Vec<String>> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = statement.query_map([], |row| row.get(1))?;
    rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Error::from)
}

fn placeholders(count: usize) -> String {
    std::iter::repeat("?").take(count).collect::<Vec<_>>().join(",")
}
fn where_clause(conditions: &[String]) -> String {
    if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    }
}
fn participant(handle: String, names: &HashMap<String, String>) -> Participant {
    let name = names.get(&content::normalize_handle(&handle)).cloned();
    Participant { handle, name }
}
fn count(row: &Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value = row.get::<_, Option<i64>>(index)?.unwrap_or(0);
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}
fn counts_row(row: &Row<'_>) -> rusqlite::Result<Counts> {
    Ok(Counts {
        total: count(row, 0)?,
        sent: count(row, 1)?,
        received: count(row, 2)?,
    })
}

fn parse_date(connection: &Connection, input: &str) -> Result<i64> {
    let invalid = || Error::Arguments(format!("invalid date {input:?}; use YYYY-MM-DD or RFC 3339"));
    if !input.is_ascii() || input.len() < 10 {
        return Err(invalid());
    }
    let number = |start, end| {
        input
            .get(start..end)
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or_else(invalid)
    };
    let year = number(0, 4)?;
    let month = number(5, 7)?;
    let day = number(8, 10)?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => return Err(invalid()),
    };
    if input.as_bytes()[4] != b'-' || input.as_bytes()[7] != b'-' || day == 0 || day > days {
        return Err(invalid());
    }
    let mut fraction = 0_i64;
    let timestamp = if input.len() == 10 {
        format!("{input}T00:00:00Z")
    } else {
        if input.len() < 20
            || input.as_bytes()[10] != b'T'
            || input.as_bytes()[13] != b':'
            || input.as_bytes()[16] != b':'
            || number(11, 13)? > 23
            || number(14, 16)? > 59
            || number(17, 19)? > 59
        {
            return Err(invalid());
        }
        let rest = &input[19..];
        let (fraction_text, zone) = if let Some(rest) = rest.strip_prefix('.') {
            let end = rest
                .find(|character: char| !character.is_ascii_digit())
                .ok_or_else(invalid)?;
            (&rest[..end], &rest[end..])
        } else {
            ("", rest)
        };
        if rest.starts_with('.') && (fraction_text.is_empty() || fraction_text.len() > 9) {
            return Err(invalid());
        }
        if !fraction_text.is_empty() {
            fraction =
                fraction_text.parse::<i64>().map_err(|_| invalid())? * 10_i64.pow(9 - fraction_text.len() as u32);
        }
        if zone != "Z"
            && !(zone.len() == 6
                && matches!(zone.as_bytes()[0], b'+' | b'-')
                && zone.as_bytes()[3] == b':'
                && zone[1..3].parse::<u32>().is_ok_and(|hours| hours <= 23)
                && zone[4..6].parse::<u32>().is_ok_and(|minutes| minutes <= 59))
        {
            return Err(invalid());
        }
        format!("{}{zone}", &input[..19])
    };
    let seconds: Option<i64> =
        connection.query_row("SELECT CAST(strftime('%s', ?) AS INTEGER)", [timestamp], |row| {
            row.get(0)
        })?;
    seconds
        .and_then(|seconds| seconds.checked_sub(APPLE_EPOCH))
        .and_then(|seconds| seconds.checked_mul(1_000_000_000))
        .and_then(|nanos| nanos.checked_add(fraction))
        .ok_or_else(invalid)
}
