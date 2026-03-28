use crate::commands::shared::normalize_match_text;

const MAX_EXCERPT_CHARS: usize = 240;
const MATCH_CONTEXT_BEFORE: usize = 72;
const MATCH_CONTEXT_AFTER: usize = 144;

pub(super) fn body_excerpt(body: &str) -> String {
    let excerpt_source = body
        .split("\n\n")
        .flat_map(str::lines)
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(body);
    let flattened = flatten_excerpt_text(excerpt_source);
    truncate_excerpt(&flattened)
}

pub(super) fn body_excerpt_for_query(body: &str, query: &str) -> String {
    let normalized_query = normalize_match_text(query);
    if normalized_query.is_empty() {
        return body_excerpt(body);
    }

    for block in excerpt_blocks(body) {
        if let Some(excerpt) = excerpt_for_query_in_text(&block, &normalized_query) {
            return excerpt;
        }
    }

    let flattened = flatten_excerpt_text(body);
    excerpt_for_query_in_text(&flattened, &normalized_query).unwrap_or_else(|| truncate_excerpt(&flattened))
}

fn excerpt_blocks(body: &str) -> Vec<String> {
    let blocks = body
        .split("\n\n")
        .flat_map(str::lines)
        .map(flatten_excerpt_text)
        .filter(|block| !block.is_empty())
        .collect::<Vec<_>>();

    if blocks.is_empty() {
        vec![flatten_excerpt_text(body)]
    } else {
        blocks
    }
}

fn flatten_excerpt_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_excerpt(text: &str) -> String {
    let characters = text.chars().collect::<Vec<_>>();
    if characters.len() <= MAX_EXCERPT_CHARS {
        return text.to_owned();
    }

    let mut excerpt = characters[..MAX_EXCERPT_CHARS].iter().collect::<String>();
    excerpt.push_str("...");
    excerpt
}

fn excerpt_for_query_in_text(text: &str, normalized_query: &str) -> Option<String> {
    let characters = text.chars().collect::<Vec<_>>();
    let searchable = SearchableText::new(&characters);
    let match_start = searchable.normalized.find(normalized_query)?;
    let match_end = match_start + normalized_query.len() - 1;
    let start_char = *searchable.normalized_to_char.get(match_start)?;
    let end_char = searchable
        .normalized_to_char
        .get(match_end)
        .map(|end| end + 1)
        .unwrap_or(start_char + 1);

    Some(window_excerpt(&characters, start_char, end_char))
}

struct SearchableText {
    normalized: String,
    normalized_to_char: Vec<usize>,
}

impl SearchableText {
    fn new(characters: &[char]) -> Self {
        let mut normalized = String::new();
        let mut normalized_to_char = Vec::new();

        for (index, character) in characters.iter().copied().enumerate() {
            if character.is_ascii_alphanumeric() {
                normalized.push(character.to_ascii_lowercase());
                normalized_to_char.push(index);
            }
        }

        Self {
            normalized,
            normalized_to_char,
        }
    }
}

fn window_excerpt(characters: &[char], start_char: usize, end_char: usize) -> String {
    if characters.len() <= MAX_EXCERPT_CHARS {
        return characters.iter().collect::<String>();
    }

    let mut window_start = start_char.saturating_sub(MATCH_CONTEXT_BEFORE);
    let mut window_end = (end_char + MATCH_CONTEXT_AFTER).min(characters.len());

    let preferred_start = sentence_boundary_before(characters, window_start, start_char);
    let preferred_end = sentence_boundary_after(characters, end_char, window_end);
    if preferred_end.saturating_sub(preferred_start) <= MAX_EXCERPT_CHARS + 48 {
        window_start = preferred_start;
        window_end = preferred_end;
    }

    if window_end.saturating_sub(window_start) > MAX_EXCERPT_CHARS {
        let match_len = end_char.saturating_sub(start_char).max(1);
        let slack = MAX_EXCERPT_CHARS.saturating_sub(match_len);
        let before = slack / 2;
        let after = slack - before;
        window_start = start_char.saturating_sub(before);
        window_end = (end_char + after).min(characters.len());
        if window_end.saturating_sub(window_start) < MAX_EXCERPT_CHARS && window_start > 0 {
            window_start = window_end.saturating_sub(MAX_EXCERPT_CHARS);
        }
    }

    let mut excerpt = characters[window_start..window_end]
        .iter()
        .collect::<String>()
        .trim()
        .to_owned();
    if window_start > 0 {
        excerpt.insert_str(0, "...");
    }
    if window_end < characters.len() {
        excerpt.push_str("...");
    }
    excerpt
}

fn sentence_boundary_before(characters: &[char], hard_start: usize, match_start: usize) -> usize {
    for index in (hard_start..match_start).rev() {
        if is_excerpt_boundary(characters[index]) {
            return skip_whitespace_forward(characters, index + 1);
        }
    }
    hard_start
}

fn sentence_boundary_after(characters: &[char], match_end: usize, hard_end: usize) -> usize {
    for index in match_end..hard_end {
        if is_excerpt_boundary(characters[index]) {
            return skip_whitespace_backward(characters, index + 1);
        }
    }
    hard_end
}

fn is_excerpt_boundary(character: char) -> bool {
    matches!(character, '.' | '!' | '?' | ':' | ';' | '\n')
}

fn skip_whitespace_forward(characters: &[char], mut index: usize) -> usize {
    while index < characters.len() && characters[index].is_whitespace() {
        index += 1;
    }
    index
}

fn skip_whitespace_backward(characters: &[char], mut index: usize) -> usize {
    while index > 0 && characters[index - 1].is_whitespace() {
        index -= 1;
    }
    index
}
