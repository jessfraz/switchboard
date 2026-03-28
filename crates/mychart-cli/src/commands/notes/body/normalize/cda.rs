use quick_xml::{escape::unescape, events::Event, name::QName, Reader};

use super::{collapse_plain_text, push_text_separator};
use crate::commands::notes::body::normalize::rtf::replace_embedded_base64_rtf_payloads;

#[derive(Debug, Default)]
struct CdaSectionText {
    title: String,
    text: String,
}

pub(super) fn extract_cda_section_text(input: &str) -> Option<String> {
    let mut reader = Reader::from_str(input);
    reader.config_mut().trim_text(false);

    let mut buffer = Vec::new();
    let mut tag_stack = Vec::<String>::new();
    let mut document_title = String::new();
    let mut sections = Vec::<CdaSectionText>::new();
    let mut inside_section_text = 0usize;
    let mut capture_document_title = false;
    let mut capture_section_title = false;

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                let name = xml_name_string(event.name());
                let parent = tag_stack.last().cloned();

                if name == "section" {
                    sections.push(CdaSectionText::default());
                } else if name == "title" && parent.as_deref() == Some("ClinicalDocument") {
                    capture_document_title = true;
                } else if name == "title" && parent.as_deref() == Some("section") {
                    capture_section_title = true;
                } else if name == "text" && has_open_section(&tag_stack) {
                    inside_section_text += 1;
                } else if inside_section_text > 0 {
                    push_cda_tag_start(&mut sections, &name);
                }

                tag_stack.push(name);
            }
            Ok(Event::Empty(event)) => {
                let name = xml_name_string(event.name());
                if inside_section_text > 0 {
                    push_cda_tag_start(&mut sections, &name);
                    push_cda_tag_end(&mut sections, &name);
                }
            }
            Ok(Event::End(event)) => {
                let name = xml_name_string(event.name());

                if name == "title" {
                    capture_document_title = false;
                    capture_section_title = false;
                } else if name == "text" && inside_section_text > 0 {
                    inside_section_text -= 1;
                    push_cda_text_separator(&mut sections, '\n');
                } else if inside_section_text > 0 {
                    push_cda_tag_end(&mut sections, &name);
                }

                tag_stack.pop();
            }
            Ok(Event::Text(event)) => {
                if let Some(text) = decode_xml_text(event.as_ref()) {
                    push_cda_text_fragment(
                        &mut sections,
                        &mut document_title,
                        capture_document_title,
                        capture_section_title,
                        inside_section_text > 0,
                        &text,
                    );
                }
            }
            Ok(Event::CData(event)) => {
                if let Some(text) = decode_cdata_text(event.as_ref()) {
                    push_cda_text_fragment(
                        &mut sections,
                        &mut document_title,
                        capture_document_title,
                        capture_section_title,
                        inside_section_text > 0,
                        &text,
                    );
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }

        buffer.clear();
    }

    let document_title = collapse_plain_text(&document_title);
    let rendered_sections = sections
        .into_iter()
        .filter_map(|section| {
            let title = collapse_plain_text(&section.title);
            let text = collapse_plain_text(&replace_embedded_base64_rtf_payloads(&section.text));
            if title.is_empty() && text.is_empty() {
                return None;
            }
            if title.is_empty() {
                return Some(text);
            }
            if text.is_empty() {
                return Some(title);
            }
            Some(format!("{title}\n{text}"))
        })
        .collect::<Vec<_>>();

    if rendered_sections.is_empty() {
        return None;
    }

    let mut output = String::new();
    if !document_title.is_empty() {
        output.push_str(&document_title);
        output.push_str("\n\n");
    }
    output.push_str(&rendered_sections.join("\n\n"));
    Some(output.trim().to_owned())
}

fn has_open_section(tag_stack: &[String]) -> bool {
    tag_stack.iter().rev().any(|name| name == "section")
}

fn xml_name_string(name: QName<'_>) -> String {
    let local = name
        .as_ref()
        .rsplit(|byte| *byte == b':')
        .next()
        .unwrap_or(name.as_ref());
    String::from_utf8_lossy(local).into_owned()
}

fn decode_xml_text(bytes: &[u8]) -> Option<String> {
    let decoded = std::str::from_utf8(bytes).ok()?;
    let unescaped = unescape(decoded).ok()?;
    Some(unescaped.into_owned())
}

fn decode_cdata_text(bytes: &[u8]) -> Option<String> {
    Some(String::from_utf8_lossy(bytes).into_owned())
}

fn push_cda_text_fragment(
    sections: &mut [CdaSectionText],
    document_title: &mut String,
    capture_document_title: bool,
    capture_section_title: bool,
    inside_section_text: bool,
    text: &str,
) {
    if text.trim().is_empty() {
        return;
    }

    if capture_document_title {
        append_text_fragment(document_title, text);
        return;
    }

    if capture_section_title {
        if let Some(section) = sections.last_mut() {
            append_text_fragment(&mut section.title, text);
        }
        return;
    }

    if inside_section_text {
        if let Some(section) = sections.last_mut() {
            section.text.push_str(text);
        }
    }
}

fn append_text_fragment(target: &mut String, text: &str) {
    if target.is_empty() {
        target.push_str(text);
        return;
    }

    let needs_space = !target.ends_with([' ', '\n', '\t'])
        && !text.starts_with([' ', '\n', '\t'])
        && target
            .chars()
            .last()
            .is_some_and(|character| character.is_ascii_alphanumeric())
        && text
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric());
    if needs_space {
        target.push(' ');
    }
    target.push_str(text);
}

fn push_cda_text_separator(sections: &mut [CdaSectionText], separator: char) {
    if let Some(section) = sections.last_mut() {
        push_text_separator(&mut section.text, separator);
    }
}

fn push_cda_tag_start(sections: &mut [CdaSectionText], name: &str) {
    match name {
        "paragraph" | "list" | "table" | "tbody" | "thead" | "tfoot" | "tr" | "caption" | "content" | "br" => {
            push_cda_text_separator(sections, '\n')
        }
        "item" | "td" | "th" => push_cda_text_separator(sections, ' '),
        _ => {}
    }
}

fn push_cda_tag_end(sections: &mut [CdaSectionText], name: &str) {
    match name {
        "paragraph" | "item" | "tr" | "table" | "list" | "caption" | "content" => {
            push_cda_text_separator(sections, '\n')
        }
        "td" | "th" => push_cda_text_separator(sections, ' '),
        _ => {}
    }
}
