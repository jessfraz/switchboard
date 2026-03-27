use std::path::PathBuf;

use switchboard_core::{
    Error, ExecutionTarget, PlannedAction, ResolvedNamespace, Result, ToolDescriptor, ToolOutput, ToolRequest,
};

use crate::cli::passthrough;

pub(crate) type CliSummarizeFn = fn(&ResolvedNamespace, &ToolRequest) -> Result<String>;
pub(crate) type CliBuildArgsFn = fn(&PlannedAction) -> Result<Vec<String>>;
pub(crate) type CliDecodeFn = fn(&ExecutionTarget, &PlannedAction, CliResponse) -> Result<ToolOutput>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CliBinarySpec {
    pub program: String,
    pub env_override: Option<String>,
    pub version_args: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CliCapabilityProbe {
    pub id: String,
    pub args: Vec<String>,
}

pub(crate) enum CliSummarizeStrategy {
    Handler(CliSummarizeFn),
    Template(CliSummaryTemplate),
    RawInventory { program: String, prefix: Vec<String> },
}

impl CliSummarizeStrategy {
    pub(crate) fn summarize(&self, namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
        match self {
            Self::Handler(handler) => handler(namespace, request),
            Self::Template(template) => template.render(namespace, request),
            Self::RawInventory { program, prefix } => {
                passthrough::summarize_prefixed_passthrough(namespace, request, program, prefix)
            }
        }
    }
}

pub(crate) enum CliArgsStrategy {
    Handler(CliBuildArgsFn),
    RawInventory { prefix: Vec<String> },
}

impl CliArgsStrategy {
    pub(crate) fn build_args(&self, action: &PlannedAction) -> Result<Vec<String>> {
        match self {
            Self::Handler(handler) => handler(action),
            Self::RawInventory { prefix } => passthrough::build_prefixed_passthrough_args(action, prefix),
        }
    }
}

pub(crate) enum CliDecodeStrategy {
    Handler(CliDecodeFn),
    RawInventory { program: String, prefix: Vec<String> },
}

impl CliDecodeStrategy {
    pub(crate) fn decode(
        &self,
        target: &ExecutionTarget,
        action: &PlannedAction,
        response: CliResponse,
    ) -> Result<ToolOutput> {
        match self {
            Self::Handler(handler) => handler(target, action, response),
            Self::RawInventory { program, prefix } => {
                passthrough::decode_prefixed_passthrough(target, action, response, program, prefix)
            }
        }
    }
}

pub(crate) struct CliExecutableSpec {
    pub binary: CliBinarySpec,
    pub capability: CliCapabilityProbe,
    pub args: CliArgsStrategy,
    pub decode: CliDecodeStrategy,
}

pub(crate) struct CliCommandSpec {
    pub descriptor: ToolDescriptor,
    pub summarize: CliSummarizeStrategy,
    pub executable: Option<CliExecutableSpec>,
}

impl CliCommandSpec {
    pub(crate) fn name(&self) -> &str {
        self.descriptor.name.as_str()
    }
}

pub(crate) struct CliResponse {
    pub program: PathBuf,
    pub version: String,
    pub stdout: String,
    pub stderr: String,
}

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

    fn render(&self, namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
        let mut summary = String::new();
        for segment in &self.segments {
            match segment {
                CliSummarySegment::Literal(value) => summary.push_str(value),
                CliSummarySegment::Namespace => summary.push_str(namespace.id.as_str()),
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
    Arg { aliases: Vec<String>, repeated: bool },
}

fn parse_summary_segment(template: &str, token: String) -> Result<CliSummarySegment> {
    if token == "namespace" {
        return Ok(CliSummarySegment::Namespace);
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
        .find_map(|alias| request.args.value(alias))
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            Error::InvalidArguments(format!(
                "missing required argument {} for {}",
                render_aliases(aliases),
                request.tool
            ))
        })
}

fn render_aliases(aliases: &[String]) -> String {
    aliases
        .iter()
        .map(|alias| format!("--{alias}"))
        .collect::<Vec<_>>()
        .join(" or ")
}

#[cfg(test)]
mod tests {
    use switchboard_core::{ExecutionMode, ToolArgument, ToolRequest};

    use crate::cli::command::CliSummaryTemplate;

    #[test]
    fn summary_template_renders_namespace_and_args() {
        let template = CliSummaryTemplate::parse("Draft comment for {arg:repo}#{arg:number} in {namespace}")
            .expect("template should parse");
        let request = ToolRequest::new(
            "github.pull_request.comment",
            "github.personal",
            ExecutionMode::Draft,
            vec![
                ToolArgument::option("repo", "openai/codex").expect("repo should build"),
                ToolArgument::option("number", "123").expect("number should build"),
            ],
        )
        .expect("request should build");

        let summary = template
            .render(
                &switchboard_core::ResolvedNamespace::new(
                    "github.personal",
                    switchboard_core::ProviderKind::GitHub,
                    "GitHub",
                    "github.personal_auth",
                    false,
                    None,
                )
                .expect("namespace should build"),
                &request,
            )
            .expect("summary should render");

        assert_eq!(summary, "Draft comment for openai/codex#123 in github.personal");
    }
}
