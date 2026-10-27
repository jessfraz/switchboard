mod content;
mod database;
mod presentation;

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyStatus {
    Plain,
    Decoded,
    Empty,
    Unsupported,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Participant {
    pub handle: String,
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Chat {
    pub id: String,
    pub name: Option<String>,
    pub service: Option<String>,
    pub participants: Vec<Participant>,
    pub unread_count: u64,
    pub last_message_at: Option<String>,
    pub preview: Option<String>,
    pub preview_status: BodyStatus,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub filename: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub available_locally: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub chat_id: String,
    pub date: Option<String>,
    pub sender: Option<Participant>,
    pub is_from_me: bool,
    pub is_unread: bool,
    pub text: Option<String>,
    pub body_status: BodyStatus,
    pub kind: MessageKind,
    pub associated_message_id: Option<String>,
    pub attachments: Vec<Attachment>,
    #[serde(skip)]
    pub(crate) row_id: i64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Message,
    Reaction,
    System,
}

impl MessageKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Reaction => "reaction",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Counts {
    pub total: u64,
    pub sent: u64,
    pub received: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ActivityBucket {
    pub start: String,
    #[serde(flatten)]
    pub counts: Counts,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Chats {
        chats: Vec<Chat>,
        next_cursor: Option<String>,
        unsupported_bodies: u64,
    },
    Messages {
        messages: Vec<Message>,
        next_cursor: Option<String>,
        scanned_messages: u64,
        unsupported_bodies: u64,
    },
    Attachment {
        attachment: Attachment,
        content_base64: String,
    },
    Activity {
        counts: Counts,
        buckets: Vec<ActivityBucket>,
        next_cursor: Option<String>,
    },
    Doctor {
        database: std::path::PathBuf,
        read_only: bool,
        contacts_available: bool,
        history: String,
    },
}

impl Response {
    pub fn next_cursor(&self) -> Option<&str> {
        match self {
            Self::Chats { next_cursor, .. }
            | Self::Messages { next_cursor, .. }
            | Self::Activity { next_cursor, .. } => next_cursor.as_deref(),
            _ => None,
        }
    }

    pub fn is_partial(&self) -> bool {
        match self {
            Self::Chats { unsupported_bodies, .. } | Self::Messages { unsupported_bodies, .. } => {
                *unsupported_bodies > 0
            }
            _ => false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Arguments(String),
    #[error("cannot read Messages database: {0}. Check access in System Settings > Privacy & Security > Full Disk Access for the application running this command")]
    Access(String),
    #[error("unsupported Messages database: {0}")]
    Schema(String),
    #[error("Messages query failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("{0} was not found in the local Messages database")]
    NotFound(String),
    #[error("cannot read attachment: {0}")]
    Attachment(String),
    #[error("output failed: {0}")]
    Output(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Parser)]
#[command(
    name = "messages",
    version,
    about = "Read local macOS Messages without modifying them",
    after_help = "Examples:\n  messages chats --unread\n  messages chats --with '+15555550123'\n  messages read 'iMessage;-;example'\n  messages search dinner --since 2026-10-01\n  messages attachment ATTACHMENT_ID --raw > attachment.png\n  messages activity --interval month --json\n\nOnly locally available history and attachments are covered. Reads preserve unread state.\nContact names are used only when Contacts access is already authorized."
)]
struct Cli {
    #[arg(long, global = true, env = "MESSAGES_DB", value_name = "PATH")]
    db: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List conversations by most recent activity.
    Chats {
        /// Participant phone number, email address, or resolved contact name. Repeat to require all.
        #[arg(long = "with")]
        participants: Vec<String>,
        /// Include only conversations with incoming unread messages.
        #[arg(long)]
        unread: bool,
        #[command(flatten)]
        page: Page,
    },
    /// Read the latest page of a conversation in chronological order.
    Read {
        chat_id: String,
        #[arg(long)]
        unread: bool,
        #[command(flatten)]
        page: Page,
    },
    /// Search decoded message text, newest first (Unicode case-insensitive substring).
    Search {
        query: String,
        #[arg(long)]
        chat: Option<String>,
        #[arg(long = "with")]
        participants: Vec<String>,
        #[command(flatten)]
        page: Page,
    },
    /// Read a locally available attachment, including images (maximum 16 MiB).
    Attachment {
        attachment_id: String,
        /// Emit original bytes on stdout. Redirect to a file to save them.
        #[arg(long, conflicts_with = "json")]
        raw: bool,
    },
    /// Count sent and received messages, optionally grouped into UTC time buckets.
    Activity {
        #[arg(long)]
        chat: Option<String>,
        #[arg(long, value_enum)]
        interval: Option<Interval>,
        #[command(flatten)]
        page: Page,
    },
    /// Check read access and the supported database schema.
    Doctor,
}

#[derive(Debug, Args, Serialize)]
struct Page {
    /// Include activity at or after this UTC date or RFC 3339 timestamp.
    #[arg(long)]
    since: Option<String>,
    /// Include activity before this UTC date or RFC 3339 timestamp.
    #[arg(long)]
    until: Option<String>,
    /// Page size (1..500). Defaults to 20 chats, 50 messages, or 100 buckets.
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=500))]
    #[serde(skip)]
    limit: Option<u16>,
    /// Opaque continuation from the same command, database, and filters.
    #[arg(long)]
    #[serde(skip)]
    cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum Interval {
    Day,
    Week,
    Month,
}

/// Execute a read-only command. Arguments include the program name, `messages`.
pub fn execute<I, T>(args: I) -> Result<Response>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::try_parse_from(args).map_err(|error| Error::Arguments(error.to_string()))?;
    run(&cli)
}

fn run(cli: &Cli) -> Result<Response> {
    let path = match &cli.db {
        Some(path) => path.clone(),
        None => std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library/Messages/chat.db"))
            .ok_or_else(|| Error::Arguments("HOME is not set; supply --db PATH".into()))?,
    };
    let db = database::Database::open(&path)?;
    db.execute(&cli.command)
}

pub fn main_entry<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return if code == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
    };
    match run(&cli).and_then(|response| presentation::write(&cli, &response)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if cli.json {
                #[derive(Serialize)]
                struct Failure<'a> {
                    status: &'a str,
                    message: String,
                }
                let failure = Failure {
                    status: "error",
                    message: error.to_string(),
                };
                match serde_json::to_string(&failure) {
                    Ok(json) => eprintln!("{json}"),
                    Err(_) => eprintln!("Messages failed to encode its error"),
                }
            } else {
                eprintln!("{error}");
            }
            ExitCode::FAILURE
        }
    }
}
