use super::{collapse_plain_text, push_text_separator};

#[derive(Clone, Copy)]
struct RtfGroupState {
    ignorable: bool,
    uc_skip: usize,
}

pub(super) fn looks_like_rtf(body_text: &str) -> bool {
    body_text.trim_start().starts_with("{\\rtf")
}

pub(super) fn replace_embedded_base64_rtf_payloads(input: &str) -> String {
    let characters = input.chars().collect::<Vec<_>>();
    let mut output = String::new();
    let mut index = 0;

    while index < characters.len() {
        if let Some((next_index, replacement)) = decode_embedded_base64_rtf_payload(&characters, index) {
            let replacement = replacement.trim();
            if !replacement.is_empty() {
                if !output.is_empty() && !output.ends_with([' ', '\n', '\t']) {
                    output.push(' ');
                }
                output.push_str(replacement);
                if next_index < characters.len()
                    && !output.ends_with([' ', '\n', '\t'])
                    && !characters[next_index].is_ascii_whitespace()
                {
                    output.push(' ');
                }
            }
            index = next_index;
            continue;
        }

        output.push(characters[index]);
        index += 1;
    }

    output
}

pub(super) fn strip_rtf_to_text(input: &str) -> String {
    let mut states = vec![RtfGroupState {
        ignorable: false,
        uc_skip: 1,
    }];
    let mut pending_ignorable_group = false;
    let mut fallback_skip = 0usize;
    let mut output = String::new();
    let characters = input.chars().collect::<Vec<_>>();
    let mut index = 0usize;

    while index < characters.len() {
        match characters[index] {
            '{' => {
                let mut next_state = *states.last().unwrap_or(&RtfGroupState {
                    ignorable: false,
                    uc_skip: 1,
                });
                if pending_ignorable_group {
                    next_state.ignorable = true;
                    pending_ignorable_group = false;
                }
                states.push(next_state);
                index += 1;
            }
            '}' => {
                if states.len() > 1 {
                    states.pop();
                }
                pending_ignorable_group = false;
                fallback_skip = 0;
                index += 1;
            }
            '\\' => {
                let current = *states.last().unwrap_or(&RtfGroupState {
                    ignorable: false,
                    uc_skip: 1,
                });
                index += 1;
                if index >= characters.len() {
                    break;
                }

                match characters[index] {
                    '\\' | '{' | '}' if !current.ignorable => {
                        output.push(characters[index]);
                        index += 1;
                    }
                    '~' if !current.ignorable => {
                        output.push(' ');
                        index += 1;
                    }
                    '_' if !current.ignorable => {
                        output.push('-');
                        index += 1;
                    }
                    '-' if !current.ignorable => {
                        output.push('-');
                        index += 1;
                    }
                    '*' => {
                        pending_ignorable_group = true;
                        index += 1;
                    }
                    '\'' => {
                        if index + 2 < characters.len() {
                            let hex = [characters[index + 1], characters[index + 2]]
                                .iter()
                                .collect::<String>();
                            if !current.ignorable {
                                if let Ok(value) = u8::from_str_radix(&hex, 16) {
                                    output.push(value as char);
                                }
                            }
                            index += 3;
                        } else {
                            break;
                        }
                    }
                    character if character.is_ascii_alphabetic() => {
                        let word_start = index;
                        index += 1;
                        while index < characters.len() && characters[index].is_ascii_alphabetic() {
                            index += 1;
                        }
                        let word = characters[word_start..index].iter().collect::<String>();

                        let mut sign = 1i32;
                        if index < characters.len() && characters[index] == '-' {
                            sign = -1;
                            index += 1;
                        }

                        let number_start = index;
                        while index < characters.len() && characters[index].is_ascii_digit() {
                            index += 1;
                        }
                        let argument = if number_start < index {
                            characters[number_start..index]
                                .iter()
                                .collect::<String>()
                                .parse::<i32>()
                                .ok()
                                .map(|value| value * sign)
                        } else {
                            None
                        };

                        if index < characters.len() && characters[index] == ' ' {
                            index += 1;
                        }

                        handle_rtf_control_word(
                            &word,
                            argument,
                            current,
                            &mut states,
                            &mut pending_ignorable_group,
                            &mut fallback_skip,
                            &mut output,
                        );
                    }
                    _ => {
                        index += 1;
                    }
                }
            }
            character => {
                let current = *states.last().unwrap_or(&RtfGroupState {
                    ignorable: false,
                    uc_skip: 1,
                });
                if fallback_skip > 0 {
                    fallback_skip -= 1;
                } else if !current.ignorable {
                    output.push(character);
                }
                index += 1;
            }
        }
    }

    collapse_plain_text(&output)
}

fn decode_embedded_base64_rtf_payload(characters: &[char], start_index: usize) -> Option<(usize, String)> {
    if !characters
        .get(start_index)
        .is_some_and(|character| matches!(character, 'e' | 'E'))
    {
        return None;
    }

    let mut collapsed = String::new();
    let mut end_index = start_index;
    let mut saw_padding = false;

    while end_index < characters.len() {
        let character = characters[end_index];
        if character.is_ascii_whitespace() {
            end_index += 1;
            continue;
        }

        if saw_padding {
            if character == '=' {
                collapsed.push(character);
                end_index += 1;
                continue;
            }
            break;
        }

        if is_base64_character(character) {
            collapsed.push(character);
            if character == '=' {
                saw_padding = true;
            }
            end_index += 1;
            continue;
        }

        break;
    }

    if collapsed.len() < 128 || !collapsed.starts_with("e1xydGY") {
        return None;
    }

    let decoded = super::super::decode_base64(&collapsed).ok()?;
    let decoded_text = String::from_utf8_lossy(&decoded);
    if !decoded_text.starts_with("{\\rtf") {
        return None;
    }

    let stripped = strip_rtf_to_text(&decoded_text);
    if stripped.is_empty() {
        return None;
    }

    Some((end_index, stripped))
}

fn is_base64_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '+' | '/' | '-' | '_' | '=')
}

fn handle_rtf_control_word(
    word: &str,
    argument: Option<i32>,
    current: RtfGroupState,
    states: &mut [RtfGroupState],
    pending_ignorable_group: &mut bool,
    fallback_skip: &mut usize,
    output: &mut String,
) {
    if *pending_ignorable_group {
        if let Some(state) = states.last_mut() {
            state.ignorable = true;
        }
        *pending_ignorable_group = false;
    }

    let current = states.last().copied().unwrap_or(current);
    match word {
        "par" | "line" => {
            if !current.ignorable {
                push_text_separator(output, '\n');
            }
        }
        "tab" => {
            if !current.ignorable {
                push_text_separator(output, ' ');
            }
        }
        "emdash" | "endash" => {
            if !current.ignorable {
                output.push('-');
            }
        }
        "uc" => {
            if let Some(value) = argument {
                if let Some(state) = states.last_mut() {
                    state.uc_skip = value.max(0) as usize;
                }
            }
        }
        "u" => {
            if !current.ignorable {
                if let Some(value) = argument {
                    let codepoint = if value < 0 {
                        (value + 65_536) as u32
                    } else {
                        value as u32
                    };
                    if let Some(character) = char::from_u32(codepoint) {
                        output.push(character);
                    }
                }
            }
            *fallback_skip = current.uc_skip;
        }
        "fonttbl" | "colortbl" | "stylesheet" | "info" | "pict" | "object" | "fldinst" | "xmlopen" | "xmlattrname"
        | "xmlattrvalue" | "datastore" => {
            if let Some(state) = states.last_mut() {
                state.ignorable = true;
            }
        }
        _ => {}
    }
}
