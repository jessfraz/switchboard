use std::io::{self, Write};

use base64::{engine::general_purpose::STANDARD, Engine};

use crate::{Attachment, BodyStatus, Cli, Command, Error, Message, Response, Result};

pub(crate) fn write(cli: &Cli, response: &Response) -> Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    if cli.json {
        serde_json::to_writer(&mut output, response).map_err(|error| Error::Output(error.to_string()))?;
        return writeln!(output).map_err(output_error);
    }
    if matches!(cli.command, Command::Attachment { raw: true, .. }) {
        if let Response::Attachment { content_base64, .. } = response {
            let bytes = STANDARD
                .decode(content_base64)
                .map_err(|error| Error::Output(error.to_string()))?;
            return output.write_all(&bytes).map_err(output_error);
        }
    }
    human(&mut output, cli, response).map_err(output_error)
}

fn human(output: &mut impl Write, cli: &Cli, response: &Response) -> io::Result<()> {
    match response {
        Response::Chats {
            chats,
            unsupported_bodies,
            ..
        } => {
            if chats.is_empty() {
                writeln!(output, "No conversations found.")?;
            }
            for chat in chats {
                let label = chat
                    .name
                    .as_deref()
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        let participants = chat
                            .participants
                            .iter()
                            .map(|participant| participant.name.as_deref().unwrap_or(&participant.handle))
                            .collect::<Vec<_>>()
                            .join(", ");
                        if participants.is_empty() {
                            "Unnamed conversation".into()
                        } else {
                            participants
                        }
                    });
                write!(output, "{}  {}", clean(&chat.id), clean(&label))?;
                if chat.unread_count > 0 {
                    write!(output, "  ({} unread)", chat.unread_count)?;
                }
                writeln!(output)?;
                if let Some(date) = &chat.last_message_at {
                    write!(output, "  {}", clean(date))?;
                }
                if let Some(preview) = &chat.preview {
                    write!(output, "  {}", preview_line(preview))?;
                } else if chat.preview_status == BodyStatus::Unsupported {
                    write!(output, "  [Unsupported message content]")?;
                }
                writeln!(output)?;
            }
            unsupported(output, *unsupported_bodies)?;
        }
        Response::Messages {
            messages,
            unsupported_bodies,
            ..
        } => {
            if let Command::Read { chat_id, .. } = &cli.command {
                writeln!(output, "Conversation {}\n", clean(chat_id))?;
            }
            if messages.is_empty() {
                writeln!(output, "No messages found in this page.")?;
            }
            let show_chat = matches!(cli.command, Command::Search { .. });
            for message in messages {
                human_message(output, message, show_chat)?;
            }
            unsupported(output, *unsupported_bodies)?;
        }
        Response::Attachment { attachment, .. } => {
            human_attachment(output, attachment)?;
            writeln!(
                output,
                "Use --raw to emit the original bytes, or --json for base64 content."
            )?;
        }
        Response::Activity { counts, buckets, .. } => {
            writeln!(
                output,
                "{} messages  ·  {} sent  ·  {} received",
                counts.total, counts.sent, counts.received
            )?;
            if !buckets.is_empty() {
                writeln!(output, "\nUTC period                    Total       Sent   Received")?;
            }
            for bucket in buckets {
                writeln!(
                    output,
                    "{:<27} {:>7} {:>10} {:>10}",
                    clean(&bucket.start),
                    bucket.counts.total,
                    bucket.counts.sent,
                    bucket.counts.received
                )?;
            }
        }
        Response::Doctor {
            database,
            read_only,
            contacts_available,
            history,
        } => {
            writeln!(output, "Database: {}", clean(&database.display().to_string()))?;
            writeln!(output, "Read-only: {}", if *read_only { "yes" } else { "no" })?;
            writeln!(
                output,
                "Contacts: {}",
                if *contacts_available {
                    "available"
                } else {
                    "using phone numbers and email addresses"
                }
            )?;
            writeln!(output, "History: {}", clean(history))?;
        }
    }
    if let Some(cursor) = response.next_cursor() {
        writeln!(
            output,
            "\nMore results: repeat this command with --cursor {}",
            clean(cursor)
        )?;
    }
    Ok(())
}

fn human_message(output: &mut impl Write, message: &Message, show_chat: bool) -> io::Result<()> {
    let sender = if message.is_from_me {
        "You"
    } else {
        message
            .sender
            .as_ref()
            .map(|sender| sender.name.as_deref().unwrap_or(&sender.handle))
            .unwrap_or("Unknown sender")
    };
    write!(
        output,
        "{}  {}",
        clean(message.date.as_deref().unwrap_or("Unknown date")),
        clean(sender)
    )?;
    if show_chat {
        write!(output, "  [{}]", clean(&message.chat_id))?;
    }
    if message.is_unread {
        write!(output, "  (unread)")?;
    }
    if message.kind != crate::MessageKind::Message {
        write!(output, "  [{}]", message.kind.as_str())?;
    }
    writeln!(output)?;
    match &message.text {
        Some(text) if !text.is_empty() => {
            for line in clean(text).lines() {
                writeln!(output, "  {line}")?;
            }
        }
        _ if message.body_status == BodyStatus::Unsupported => writeln!(output, "  [Unsupported message content]")?,
        _ if message.attachments.is_empty() => writeln!(output, "  [No text]")?,
        _ => {}
    }
    for attachment in &message.attachments {
        write!(output, "  ")?;
        human_attachment(output, attachment)?;
    }
    writeln!(output)
}

fn human_attachment(output: &mut impl Write, attachment: &Attachment) -> io::Result<()> {
    writeln!(
        output,
        "Attachment {}: {} ({}, {} bytes, {})",
        clean(&attachment.id),
        clean(attachment.filename.as_deref().unwrap_or("Unnamed file")),
        clean(attachment.mime_type.as_deref().unwrap_or("unknown type")),
        attachment.size_bytes,
        if attachment.available_locally {
            "available locally"
        } else {
            "not available locally"
        },
    )
}

fn unsupported(output: &mut impl Write, count: u64) -> io::Result<()> {
    if count > 0 {
        writeln!(
            output,
            "\n{count} message bodies could not be decoded; their text is unavailable."
        )?;
    }
    Ok(())
}

fn preview_line(value: &str) -> String {
    let preview = clean(value).split_whitespace().collect::<Vec<_>>().join(" ");
    let mut characters = preview.chars();
    let mut result = characters.by_ref().take(100).collect::<String>();
    if characters.next().is_some() {
        result.push('…');
    }
    result
}

// Message text and metadata are untrusted terminal input. Preserve Unicode and
// line breaks, but never emit escape sequences, carriage returns, or bidi
// override controls that can rewrite terminal output.
fn clean(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if (character.is_control() && character != '\n')
                || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                '�'
            } else {
                character
            }
        })
        .collect()
}

fn output_error(error: io::Error) -> Error {
    Error::Output(error.to_string())
}

#[cfg(test)]
mod tests {
    use crate::presentation::clean;

    #[test]
    fn terminal_content_preserves_unicode_and_blocks_escape_sequences() {
        assert_eq!(clean("Hello 👋\n雪"), "Hello 👋\n雪");
        assert_eq!(clean("\x1b[2J\r\0\u{202e}hidden"), "�[2J���hidden");
    }
}
