use std::{fs, path::PathBuf, process::Command};

use base64::{engine::general_purpose::STANDARD, Engine};
use messages_cli::{execute, BodyStatus, Counts, Response};
use rusqlite::{params, Connection};

struct Fixture {
    directory: tempfile::TempDir,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("fixture operation succeeds");
        let path = directory.path().join("chat.db");
        let connection = Connection::open(&path).expect("fixture operation succeeds");
        connection
            .execute_batch(include_str!("fixtures/schema.sql"))
            .expect("fixture operation succeeds");
        connection.execute_batch("INSERT INTO chat VALUES ('chat-a','Friends','iMessage'),('chat-b',NULL,'SMS'); INSERT INTO handle VALUES ('+15555550123'),('person@example.com'); INSERT INTO chat_handle_join VALUES (1,1),(1,2),(2,2);").expect("fixture operation succeeds");
        for (id, day, from, text) in [
            (1, 1, 0, "Dinner tomorrow?"),
            (2, 2, 1, "Yes, dinner"),
            (3, 2, 0, "Snow 雪"),
            (4, 3, 0, "another chat"),
        ] {
            let date: i64 = connection
                .query_row(
                    "SELECT (CAST(strftime('%s', ?) AS INTEGER)-978307200)*1000000000",
                    [format!("2026-10-{day:02}T12:00:00Z")],
                    |row| row.get(0),
                )
                .expect("fixture operation succeeds");
            connection
                .execute(
                    "INSERT INTO message (ROWID,guid,text,date,handle_id,is_from_me,is_read) VALUES (?,?,?,?,1,?,?)",
                    params![id, format!("message-{id}"), text, date, from, from],
                )
                .expect("fixture operation succeeds");
            connection
                .execute(
                    "INSERT INTO chat_message_join VALUES (?,?,?)",
                    params![if id == 4 { 2 } else { 1 }, id, date],
                )
                .expect("fixture operation succeeds");
        }
        let blob = include_bytes!("fixtures/plain.typedstream");
        connection
            .execute(
                "UPDATE message SET text=NULL,attributedBody=? WHERE ROWID=3",
                [blob.as_slice()],
            )
            .expect("fixture operation succeeds");
        let root = directory.path().join("Attachments");
        fs::create_dir(&root).expect("fixture operation succeeds");
        fs::write(root.join("image.png"), b"image bytes").expect("fixture operation succeeds");
        connection.execute("INSERT INTO attachment VALUES ('image','~/Library/Messages/Attachments/image.png','image.png','image/png',11)",[]).expect("fixture operation succeeds");
        connection
            .execute("INSERT INTO message_attachment_join VALUES (3,1)", [])
            .expect("fixture operation succeeds");
        Self { directory, path }
    }
    fn run(&self, args: &[&str]) -> messages_cli::Result<Response> {
        let mut argv = vec![
            "messages".to_owned(),
            "--db".into(),
            self.path.to_string_lossy().into_owned(),
        ];
        argv.extend(args.iter().map(|value| (*value).to_owned()));
        execute(argv)
    }
}

#[test]
fn chats_filter_participants_unread_and_page_without_offset() {
    let fixture = Fixture::new();
    let Response::Chats { chats, next_cursor, .. } = fixture
        .run(&["chats", "--limit", "1"])
        .expect("fixture operation succeeds")
    else {
        panic!("chats")
    };
    assert_eq!(chats[0].id, "chat-b");
    let cursor = next_cursor.expect("fixture operation succeeds");
    let Response::Chats { chats, next_cursor, .. } = fixture
        .run(&["chats", "--limit", "1", "--cursor", &cursor])
        .expect("fixture operation succeeds")
    else {
        panic!("chats")
    };
    assert_eq!(chats[0].id, "chat-a");
    assert_eq!(chats[0].unread_count, 2);
    assert!(next_cursor.is_none());
    let Response::Chats { chats, .. } = fixture
        .run(&[
            "chats",
            "--with",
            "+1 (555) 555-0123",
            "--with",
            "Person@Example.COM",
            "--unread",
        ])
        .expect("fixture operation succeeds")
    else {
        panic!("chats")
    };
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0].participants.len(), 2);
    assert!(fixture
        .run(&["chats", "--with", "someone-else", "--cursor", &cursor])
        .is_err());
}

#[test]
fn reads_latest_page_chronologically_with_attributed_bodies_and_attachments() {
    let fixture = Fixture::new();
    let Response::Messages {
        messages, next_cursor, ..
    } = fixture
        .run(&["read", "chat-a", "--limit", "2"])
        .expect("fixture operation succeeds")
    else {
        panic!("messages")
    };
    assert_eq!(
        messages.iter().map(|message| message.id.as_str()).collect::<Vec<_>>(),
        ["message-2", "message-3"]
    );
    assert_eq!(messages[1].body_status, BodyStatus::Decoded);
    assert_eq!(messages[1].text.as_deref(), Some("Hello from Messages 👋"));
    assert_eq!(messages[1].attachments[0].id, "image");
    assert!(messages[1].attachments[0].available_locally);
    let Response::Messages {
        messages, next_cursor, ..
    } = fixture
        .run(&[
            "read",
            "chat-a",
            "--limit",
            "2",
            "--cursor",
            &next_cursor.expect("fixture operation succeeds"),
        ])
        .expect("fixture operation succeeds")
    else {
        panic!("messages")
    };
    assert_eq!(messages[0].id, "message-1");
    assert!(next_cursor.is_none());
}

#[test]
fn search_decodes_bodies_and_validates_exact_date_bounds() {
    let fixture = Fixture::new();
    let Response::Messages { messages, .. } = fixture
        .run(&["search", "MESSAGES 👋", "--chat", "chat-a"])
        .expect("fixture operation succeeds")
    else {
        panic!("messages")
    };
    assert_eq!(messages[0].id, "message-3");
    let Response::Messages { messages, .. } = fixture
        .run(&["search", "dinner", "--since", "2026-10-02", "--until", "2026-10-03"])
        .expect("fixture operation succeeds")
    else {
        panic!("messages")
    };
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, "message-2");
    for date in [
        "2026-02-30",
        "2026-10-01T25:00:00Z",
        "2026-10-01T12:00:00",
        "2026-10-01T12:00:00.Z",
        "yesterday",
    ] {
        assert!(fixture.run(&["search", "dinner", "--since", date]).is_err(), "{date}");
    }
    assert!(fixture
        .run(&["search", "dinner", "--since", "2026-10-03", "--until", "2026-10-01"])
        .is_err());
}

#[test]
fn attachments_read_bytes_and_reject_paths_outside_the_attachment_root() {
    let fixture = Fixture::new();
    let Response::Attachment { content_base64, .. } = fixture
        .run(&["attachment", "image"])
        .expect("fixture operation succeeds")
    else {
        panic!("attachment")
    };
    assert_eq!(
        STANDARD.decode(content_base64).expect("fixture operation succeeds"),
        b"image bytes"
    );
    let outside = fixture.directory.path().join("private.txt");
    fs::write(&outside, b"secret").expect("fixture operation succeeds");
    let connection = Connection::open(&fixture.path).expect("fixture operation succeeds");
    connection
        .execute("UPDATE attachment SET filename=?", [outside.to_string_lossy()])
        .expect("fixture operation succeeds");
    assert!(fixture.run(&["attachment", "image"]).is_err());
    #[cfg(unix)]
    {
        let link = fixture.directory.path().join("Attachments/link");
        std::os::unix::fs::symlink(&outside, &link).expect("fixture operation succeeds");
        connection
            .execute("UPDATE attachment SET filename=?", [link.to_string_lossy()])
            .expect("fixture operation succeeds");
        assert!(fixture.run(&["attachment", "image"]).is_err());
    }
    connection
        .execute(
            "UPDATE attachment SET filename='~/Library/Messages/Attachments/missing'",
            [],
        )
        .expect("fixture operation succeeds");
    assert!(fixture.run(&["attachment", "image"]).is_err());
}

#[test]
fn activity_counts_unique_messages_and_pages_utc_buckets() {
    let fixture = Fixture::new();
    let Response::Activity { counts, .. } = fixture
        .run(&["activity", "--chat", "chat-a"])
        .expect("fixture operation succeeds")
    else {
        panic!("activity")
    };
    assert_eq!(
        counts,
        Counts {
            total: 3,
            sent: 1,
            received: 2
        }
    );
    let Response::Activity {
        counts,
        buckets,
        next_cursor,
    } = fixture
        .run(&["activity", "--interval", "day", "--limit", "1"])
        .expect("fixture operation succeeds")
    else {
        panic!("activity")
    };
    assert_eq!(counts.total, 4);
    assert_eq!(buckets[0].start, "2026-10-01");
    let Response::Activity { counts, buckets, .. } = fixture
        .run(&[
            "activity",
            "--interval",
            "day",
            "--limit",
            "1",
            "--cursor",
            &next_cursor.expect("fixture operation succeeds"),
        ])
        .expect("fixture operation succeeds")
    else {
        panic!("activity")
    };
    assert_eq!(counts.total, 4);
    assert_eq!(buckets[0].counts.total, 2);
}

#[test]
fn real_cli_never_modifies_database_or_unread_state_and_has_no_write_commands() {
    let fixture = Fixture::new();
    let before = fs::read(&fixture.path).expect("fixture operation succeeds");
    for args in [
        vec!["doctor"],
        vec!["chats"],
        vec!["read", "chat-a"],
        vec!["search", "dinner"],
        vec!["activity"],
        vec!["attachment", "image"],
    ] {
        fixture.run(&args).expect("fixture operation succeeds");
    }
    assert_eq!(fs::read(&fixture.path).expect("fixture operation succeeds"), before);
    let connection = Connection::open(&fixture.path).expect("fixture operation succeeds");
    let unread: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM message WHERE is_from_me=0 AND is_read=0",
            [],
            |row| row.get(0),
        )
        .expect("fixture operation succeeds");
    assert_eq!(unread, 3);
    for command in ["send", "draft", "delete", "mark-read", "sql"] {
        let output = Command::new(env!("CARGO_BIN_EXE_messages"))
            .args([command])
            .output()
            .expect("fixture operation succeeds");
        assert!(!output.status.success());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_messages"))
        .arg("--db")
        .arg(&fixture.path)
        .args(["read", "chat-a", "--json"])
        .output()
        .expect("fixture operation succeeds");
    assert!(output.status.success());
    let parsed: Response = serde_json::from_slice(&output.stdout).expect("fixture operation succeeds");
    assert!(matches!(parsed, Response::Messages { .. }));
}

#[test]
fn search_continues_through_empty_scan_pages_and_reports_unsupported_content() {
    let fixture = Fixture::new();
    let connection = Connection::open(&fixture.path).expect("fixture operation succeeds");
    connection.execute_batch("BEGIN; WITH RECURSIVE n(x) AS (SELECT 10 UNION ALL SELECT x+1 FROM n WHERE x<5010) INSERT INTO message (ROWID,guid,text,date,handle_id) SELECT x,'extra-'||x,'no match',900000000000000000+x,1 FROM n; INSERT INTO chat_message_join SELECT 1,ROWID,date FROM message WHERE ROWID>=10; COMMIT;").expect("fixture operation succeeds");
    let Response::Messages {
        messages,
        next_cursor,
        scanned_messages,
        ..
    } = fixture.run(&["search", "dinner"]).expect("fixture operation succeeds")
    else {
        panic!("messages")
    };
    assert!(messages.is_empty());
    assert_eq!(scanned_messages, 5000);
    let Response::Messages { messages, .. } = fixture
        .run(&[
            "search",
            "dinner",
            "--cursor",
            &next_cursor.expect("fixture operation succeeds"),
        ])
        .expect("fixture operation succeeds")
    else {
        panic!("messages")
    };
    assert_eq!(messages.len(), 2);
    connection
        .execute("UPDATE message SET text=NULL,attributedBody=X'FF' WHERE ROWID=4", [])
        .expect("fixture operation succeeds");
    assert!(fixture
        .run(&["search", "dinner"])
        .expect("fixture operation succeeds")
        .next_cursor()
        .is_some());
    let response = fixture.run(&["read", "chat-b"]).expect("fixture operation succeeds");
    assert!(response.is_partial());
}

#[test]
fn null_dates_remain_readable_and_paginate_after_dated_messages() {
    let fixture = Fixture::new();
    let connection = Connection::open(&fixture.path).expect("open fixture");
    connection
        .execute("UPDATE message SET date=NULL WHERE ROWID=1", [])
        .expect("clear message date");
    connection
        .execute("UPDATE chat_message_join SET message_date=NULL WHERE message_id=1", [])
        .expect("clear join date");
    let Response::Messages { next_cursor, .. } = fixture
        .run(&["read", "chat-a", "--limit", "2"])
        .expect("read dated page")
    else {
        panic!("messages")
    };
    let Response::Messages {
        messages, next_cursor, ..
    } = fixture
        .run(&[
            "read",
            "chat-a",
            "--limit",
            "2",
            "--cursor",
            &next_cursor.expect("null continuation"),
        ])
        .expect("read null page")
    else {
        panic!("messages")
    };
    assert_eq!(messages[0].id, "message-1");
    assert!(messages[0].date.is_none());
    assert!(next_cursor.is_none());
    connection
        .execute("UPDATE message SET date=NULL WHERE ROWID=4", [])
        .expect("clear last chat date");
    let Response::Chats { chats, .. } = fixture.run(&["chats"]).expect("list null dates") else {
        panic!("chats")
    };
    assert!(chats
        .iter()
        .any(|chat| chat.id == "chat-b" && chat.last_message_at.is_none()));
    let Response::Activity { counts, buckets, .. } = fixture
        .run(&["activity", "--interval", "day"])
        .expect("count undated messages")
    else {
        panic!("activity")
    };
    assert_eq!(counts.total, 4);
    assert_eq!(buckets.last().expect("undated bucket").start, "undated");
    assert_eq!(buckets.last().expect("undated bucket").counts.total, 2);
}

#[test]
fn reads_live_wal_data_and_counts_shared_messages_once() {
    let fixture = Fixture::new();
    let connection = Connection::open(&fixture.path).expect("open writer");
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .expect("enable WAL");
    connection
        .execute("UPDATE message SET text='fresh WAL content' WHERE ROWID=3", [])
        .expect("write WAL message");
    connection
        .execute(
            "INSERT INTO chat_message_join SELECT 2,ROWID,date FROM message WHERE ROWID=3",
            [],
        )
        .expect("share message");
    let Response::Messages { messages, .. } = fixture.run(&["search", "fresh WAL content"]).expect("read live WAL")
    else {
        panic!("messages")
    };
    assert_eq!(messages.len(), 2);
    let Response::Activity { counts, .. } = fixture.run(&["activity"]).expect("count unique messages") else {
        panic!("activity")
    };
    assert_eq!(counts.total, 4);
}

#[test]
fn refuses_oversize_attachments_and_internal_symlinks() {
    let fixture = Fixture::new();
    let path = fixture.directory.path().join("Attachments/image.png");
    fs::File::create(&path)
        .expect("open sparse file")
        .set_len(16 * 1024 * 1024 + 1)
        .expect("grow sparse file");
    assert!(fixture.run(&["attachment", "image"]).is_err());
    #[cfg(unix)]
    {
        fs::remove_file(&path).expect("remove oversized file");
        fs::write(fixture.directory.path().join("Attachments/target"), b"inside").expect("write target");
        std::os::unix::fs::symlink("target", &path).expect("create internal symlink");
        assert!(fixture.run(&["attachment", "image"]).is_err());
    }
}
