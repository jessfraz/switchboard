mod excerpt;
mod materialize;
mod normalize;

use serde_json::Value;

pub(super) fn aggregate_note_body_text(content: &[Value]) -> String {
    content
        .iter()
        .filter_map(|entry| entry.get("body_text").and_then(Value::as_str))
        .filter(|body| !body.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn decode_base64(input: &str) -> std::result::Result<Vec<u8>, String> {
    let mut buffer = 0u32;
    let mut bits = 0u32;
    let mut output = Vec::new();

    for character in input.chars().filter(|character| !character.is_ascii_whitespace()) {
        if character == '=' {
            break;
        }

        let value = match character {
            'A'..='Z' => (character as u8) - b'A',
            'a'..='z' => (character as u8) - b'a' + 26,
            '0'..='9' => (character as u8) - b'0' + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            _ => {
                return Err(format!(
                    "attachment body included invalid base64 character {character:?}"
                ))
            }
        } as u32;

        buffer = (buffer << 6) | value;
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
        }
    }

    Ok(output)
}

pub(super) fn body_excerpt_for_query(body: &str, query: &str) -> String {
    excerpt::body_excerpt_for_query(body, query)
}

pub(super) fn hydrate_note_content(session: &crate::commands::shared::PatientSession, resource: &Value) -> Vec<Value> {
    materialize::hydrate_note_content(session, resource)
}
