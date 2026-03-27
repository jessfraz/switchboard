use switchboard_core::{Error, ResolvedNamespace, Result, ToolRequest};

use crate::cli::declarative::support::{render_aliases, validate_non_empty};

#[derive(Clone)]
pub(crate) struct CliSummaryTemplate {
    segments: Vec<CliSummarySegment>,
}

impl CliSummaryTemplate {
    pub(crate) fn parse(template: impl Into<String>) -> Result<Self> {
        let template = template.into();
        let mut segments = Vec::new();
        let mut literal = String::new();
        let mut chars = template.chars().peekable();

        while let Some(character) = chars.next() {
            if character != '{' {
                literal.push(character);
                continue;
            }

            if !literal.is_empty() {
                segments.push(CliSummarySegment::Literal(std::mem::take(&mut literal)));
            }

            let mut token = String::new();
            loop {
                let Some(next) = chars.next() else {
                    return Err(Error::Config(format!(
                        "invalid summary template {template:?}: missing closing brace"
                    )));
                };
                if next == '}' {
                    break;
                }
                token.push(next);
            }

            segments.push(parse_summary_segment(&template, token)?);
        }

        if !literal.is_empty() {
            segments.push(CliSummarySegment::Literal(literal));
        }

        Ok(Self { segments })
    }

    pub(crate) fn render(&self, namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
        let mut summary = String::new();
        for segment in &self.segments {
            match segment {
                CliSummarySegment::Literal(value) => summary.push_str(value),
                CliSummarySegment::Namespace => summary.push_str(namespace.id.as_str()),
                CliSummarySegment::ModeVerb { planned, applied } => match request.mode {
                    switchboard_core::ExecutionMode::Plan | switchboard_core::ExecutionMode::Draft => {
                        summary.push_str(planned)
                    }
                    switchboard_core::ExecutionMode::Auto | switchboard_core::ExecutionMode::Apply => {
                        summary.push_str(applied)
                    }
                },
                CliSummarySegment::Arg { aliases, repeated } => {
                    let value = render_summary_arg(request, aliases, *repeated)?;
                    summary.push_str(&value);
                }
            }
        }

        Ok(summary)
    }
}

#[derive(Clone)]
enum CliSummarySegment {
    Literal(String),
    Namespace,
    ModeVerb { planned: String, applied: String },
    Arg { aliases: Vec<String>, repeated: bool },
}

fn parse_summary_segment(template: &str, token: String) -> Result<CliSummarySegment> {
    if token == "namespace" {
        return Ok(CliSummarySegment::Namespace);
    }

    if let Some(value) = token.strip_prefix("mode_verb:") {
        let Some((planned, applied)) = value.split_once(',') else {
            return Err(Error::Config(format!(
                "invalid summary template {template:?}: placeholder {{{token}}} must provide planned and applied text"
            )));
        };
        validate_non_empty("summary template planned mode verb", planned)?;
        validate_non_empty("summary template applied mode verb", applied)?;
        return Ok(CliSummarySegment::ModeVerb {
            planned: planned.trim().to_owned(),
            applied: applied.trim().to_owned(),
        });
    }

    if let Some(value) = token.strip_prefix("arg:") {
        return Ok(CliSummarySegment::Arg {
            aliases: parse_summary_aliases(template, &token, value)?,
            repeated: false,
        });
    }

    if let Some(value) = token.strip_prefix("args:") {
        return Ok(CliSummarySegment::Arg {
            aliases: parse_summary_aliases(template, &token, value)?,
            repeated: true,
        });
    }

    Err(Error::Config(format!(
        "invalid summary template {template:?}: unsupported placeholder {{{token}}}"
    )))
}

fn parse_summary_aliases(template: &str, token: &str, value: &str) -> Result<Vec<String>> {
    let aliases = value
        .split('|')
        .map(str::trim)
        .filter(|alias| !alias.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if aliases.is_empty() {
        return Err(Error::Config(format!(
            "invalid summary template {template:?}: placeholder {{{token}}} must name at least one argument"
        )));
    }

    Ok(aliases)
}

fn render_summary_arg(request: &ToolRequest, aliases: &[String], repeated: bool) -> Result<String> {
    if repeated {
        let values = aliases
            .iter()
            .find_map(|alias| {
                let values = request.args.values(alias).map(ToOwned::to_owned).collect::<Vec<_>>();
                (!values.is_empty()).then_some(values)
            })
            .ok_or_else(|| {
                Error::InvalidArguments(format!(
                    "missing required argument {} for {}",
                    render_aliases(aliases),
                    request.tool
                ))
            })?;

        return Ok(values.join(", "));
    }

    aliases
        .iter()
        .find_map(|alias| request.args.value(alias).map(ToOwned::to_owned))
        .ok_or_else(|| {
            Error::InvalidArguments(format!(
                "missing required argument {} for {}",
                render_aliases(aliases),
                request.tool
            ))
        })
}
