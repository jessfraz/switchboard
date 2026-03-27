use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use switchboard_core::{Error, PlannedAction, Result};

use crate::cli::declarative::support::collect_action_values_for_name;

pub(super) fn build_gmail_raw_message(action: &PlannedAction) -> Result<String> {
    let to = collect_action_values_for_name(action, "to");
    if to.is_empty() {
        return Err(Error::InvalidArguments(format!(
            "missing required argument --to for {}",
            action.tool
        )));
    }
    for recipient in &to {
        validate_header_value("to", recipient)?;
    }

    let cc = collect_action_values_for_name(action, "cc");
    for recipient in &cc {
        validate_header_value("cc", recipient)?;
    }

    let bcc = collect_action_values_for_name(action, "bcc");
    for recipient in &bcc {
        validate_header_value("bcc", recipient)?;
    }

    if let Some(from) = action.args.value("from") {
        validate_header_value("from", from)?;
    }
    if let Some(reply_to) = action.args.value("reply-to") {
        validate_header_value("reply-to", reply_to)?;
    }
    if let Some(subject) = action.args.value("subject") {
        validate_header_value("subject", subject)?;
    }
    if let Some(in_reply_to) = action.args.value("in-reply-to") {
        validate_header_value("in-reply-to", in_reply_to)?;
    }

    let references = collect_action_values_for_name(action, "reference");
    for reference in &references {
        validate_header_value("reference", reference)?;
    }

    let content = render_gmail_body_part(action.args.value("body-text"), action.args.value("body-html"))?;
    let mut message = String::new();

    if let Some(from) = action.args.value("from") {
        message.push_str(&format!("From: {from}\r\n"));
    }
    message.push_str(&format!("To: {}\r\n", to.join(", ")));
    if !cc.is_empty() {
        message.push_str(&format!("Cc: {}\r\n", cc.join(", ")));
    }
    if !bcc.is_empty() {
        message.push_str(&format!("Bcc: {}\r\n", bcc.join(", ")));
    }
    if let Some(reply_to) = action.args.value("reply-to") {
        message.push_str(&format!("Reply-To: {reply_to}\r\n"));
    }
    if let Some(subject) = action.args.value("subject") {
        message.push_str(&format!("Subject: {subject}\r\n"));
    }
    if let Some(in_reply_to) = action.args.value("in-reply-to") {
        message.push_str(&format!("In-Reply-To: {in_reply_to}\r\n"));
    }
    if !references.is_empty() {
        message.push_str(&format!("References: {}\r\n", references.join(" ")));
    }
    message.push_str("MIME-Version: 1.0\r\n");
    message.push_str(&content);

    Ok(URL_SAFE_NO_PAD.encode(message.as_bytes()))
}

fn render_gmail_body_part(body_text: Option<&str>, body_html: Option<&str>) -> Result<String> {
    match (body_text, body_html) {
        (Some(body_text), Some(body_html)) => {
            let boundary = "switchboard-alt-boundary";
            Ok(format!(
                "Content-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\nContent-Type: text/plain; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body_text}\r\n--{boundary}\r\nContent-Type: text/html; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body_html}\r\n--{boundary}--\r\n"
            ))
        }
        (Some(body_text), None) => Ok(format!(
            "Content-Type: text/plain; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body_text}\r\n"
        )),
        (None, Some(body_html)) => Ok(format!(
            "Content-Type: text/html; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body_html}\r\n"
        )),
        (None, None) => Err(Error::InvalidArguments(
            "gmail draft requires either --body-text or --body-html".into(),
        )),
    }
}

fn validate_header_value(header: &str, value: &str) -> Result<()> {
    if value.contains('\r') || value.contains('\n') {
        return Err(Error::InvalidArguments(format!(
            "gmail draft argument --{header} cannot contain newlines"
        )));
    }

    Ok(())
}
