mod cda;
mod rtf;

pub(super) fn normalize_attachment_body_text(content_type: Option<&str>, body_text: String) -> String {
    let trimmed = body_text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if looks_like_cda_document(trimmed) {
        if let Some(extracted) = cda::extract_cda_section_text(trimmed) {
            if !extracted.is_empty() {
                return extracted;
            }
        }
    }

    if is_rtf_content_type(content_type) || rtf::looks_like_rtf(trimmed) {
        let stripped = rtf::strip_rtf_to_text(trimmed);
        if !stripped.is_empty() {
            return stripped;
        }
    }

    if is_markup_content_type(content_type) || looks_like_markup(trimmed) {
        let stripped = strip_markup_to_text(trimmed);
        if !stripped.is_empty() {
            return stripped;
        }
    }

    collapse_plain_text(&rtf::replace_embedded_base64_rtf_payloads(trimmed))
}

pub(super) fn is_textual_content_type(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return true;
    };
    let normalized = content_type.to_ascii_lowercase();
    normalized.starts_with("text/")
        || normalized.contains("json")
        || normalized.contains("xml")
        || normalized.contains("html")
        || normalized.contains("rtf")
}

pub(super) fn collapse_plain_text(input: &str) -> String {
    let collapsed = input
        .lines()
        .map(|line| {
            line.split_whitespace()
                .filter(|token| !token.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    insert_soft_boundaries_in_text(&collapsed)
}

pub(super) fn push_text_separator(output: &mut String, separator: char) {
    if output.is_empty() {
        return;
    }

    if output.ends_with([' ', '\n', '\t']) {
        if separator == '\n' && !output.ends_with('\n') {
            output.push('\n');
        }
        return;
    }

    output.push(separator);
}

fn is_markup_content_type(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return false;
    };
    let normalized = content_type.to_ascii_lowercase();
    normalized.contains("xml") || normalized.contains("html")
}

fn is_rtf_content_type(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return false;
    };
    content_type.to_ascii_lowercase().contains("rtf")
}

fn looks_like_markup(body_text: &str) -> bool {
    let trimmed = body_text.trim_start();
    trimmed.starts_with('<') && trimmed.contains('>')
}

fn looks_like_cda_document(body_text: &str) -> bool {
    let trimmed = body_text.trim_start();
    trimmed.starts_with("<ClinicalDocument") || trimmed.contains("urn:hl7-org:v3")
}

fn strip_markup_to_text(input: &str) -> String {
    let mut output = String::new();
    let mut entity = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    let mut in_entity = false;

    for character in input.chars() {
        if in_tag {
            if character == '>' {
                push_block_break_for_tag(&mut output, &tag);
                tag.clear();
                in_tag = false;
            } else {
                tag.push(character);
            }
            continue;
        }

        if in_entity {
            if character == ';' {
                output.push_str(&decode_markup_entity(&entity));
                entity.clear();
                in_entity = false;
            } else if entity.len() < 16 {
                entity.push(character);
            } else {
                output.push('&');
                output.push_str(&entity);
                output.push(character);
                entity.clear();
                in_entity = false;
            }
            continue;
        }

        match character {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '&' => {
                in_entity = true;
                entity.clear();
            }
            _ => output.push(character),
        }
    }

    if in_entity {
        output.push('&');
        output.push_str(&entity);
    }

    collapse_plain_text(&rtf::replace_embedded_base64_rtf_payloads(&output))
}

fn push_block_break_for_tag(output: &mut String, raw_tag: &str) {
    let trimmed = raw_tag.trim();
    let is_closing = trimmed.starts_with('/');
    let is_self_closing = trimmed.ends_with('/');
    let normalized = trimmed
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_ascii_lowercase();

    if matches!(
        normalized.as_str(),
        "br" | "div"
            | "p"
            | "li"
            | "tr"
            | "td"
            | "th"
            | "section"
            | "title"
            | "text"
            | "paragraph"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
    ) && (is_closing || is_self_closing || normalized == "br")
    {
        push_text_separator(output, '\n');
        return;
    }

    if matches!(
        normalized.as_str(),
        "given"
            | "family"
            | "prefix"
            | "suffix"
            | "streetaddressline"
            | "city"
            | "district"
            | "county"
            | "state"
            | "postalcode"
            | "country"
    ) && (is_closing || is_self_closing)
    {
        push_text_separator(output, ' ');
    }
}

fn decode_markup_entity(entity: &str) -> String {
    match entity {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" => "'".into(),
        "nbsp" => " ".into(),
        _ if entity.starts_with("#x") => u32::from_str_radix(&entity[2..], 16)
            .ok()
            .and_then(char::from_u32)
            .map(|character| character.to_string())
            .unwrap_or_else(|| format!("&{entity};")),
        _ if entity.starts_with('#') => entity[1..]
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(|character| character.to_string())
            .unwrap_or_else(|| format!("&{entity};")),
        _ => format!("&{entity};"),
    }
}

fn insert_soft_boundaries_in_text(text: &str) -> String {
    let characters = text.chars().collect::<Vec<_>>();
    let mut normalized = String::new();

    for (index, character) in characters.iter().copied().enumerate() {
        if should_insert_soft_boundary(&characters, index)
            && !normalized.ends_with([' ', '\n', '\t', '/', '-', '(', '['])
        {
            normalized.push(' ');
        }
        normalized.push(character);
    }

    normalized
}

fn should_insert_soft_boundary(characters: &[char], index: usize) -> bool {
    if index == 0 {
        return false;
    }

    let previous = characters[index - 1];
    let current = characters[index];
    let next = characters.get(index + 1).copied();

    if previous.is_ascii_whitespace() || current.is_ascii_whitespace() {
        return false;
    }

    if matches!(previous, '.' | '!' | '?') && current.is_ascii_uppercase() {
        return true;
    }

    if is_soft_break_punctuation(previous) && current.is_ascii_alphanumeric() {
        return true;
    }

    if previous.is_ascii_lowercase()
        && current.is_ascii_uppercase()
        && contiguous_run_len(characters, index - 1, |character| character.is_ascii_alphabetic()) >= 3
    {
        return true;
    }

    if previous.is_ascii_uppercase()
        && current.is_ascii_uppercase()
        && next.is_some_and(|next| next.is_ascii_lowercase())
        && contiguous_run_len(characters, index - 1, |character| character.is_ascii_uppercase()) >= 2
    {
        return true;
    }

    if previous.is_ascii_alphabetic() && current.is_ascii_digit() {
        return true;
    }

    if previous.is_ascii_digit() && current.is_ascii_alphabetic() {
        return true;
    }

    false
}

fn contiguous_run_len<F>(characters: &[char], end_index: usize, predicate: F) -> usize
where
    F: Fn(char) -> bool,
{
    let mut index = end_index;
    let mut count = 0;

    loop {
        if !predicate(characters[index]) {
            break;
        }
        count += 1;
        if index == 0 {
            break;
        }
        index -= 1;
    }

    count
}

fn is_soft_break_punctuation(character: char) -> bool {
    matches!(character, ')' | ']' | '}' | ':' | ';')
}
