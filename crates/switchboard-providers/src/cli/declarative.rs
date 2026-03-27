use serde_json::{json, Map, Value};
use switchboard_core::{Error, ExecutionTarget, PlannedAction, Result, ToolOutput, ToolRef, ToolRefKind, ToolRequest};

use crate::cli::command::CliResponse;

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

    pub(crate) fn render(
        &self,
        namespace: &switchboard_core::ResolvedNamespace,
        request: &ToolRequest,
    ) -> Result<String> {
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

#[derive(Clone, Debug)]
pub(crate) struct CliArgsTemplate {
    segments: Vec<CliArgsSegment>,
}

impl CliArgsTemplate {
    pub(crate) fn new(segments: Vec<CliArgsSegment>) -> Result<Self> {
        if segments.is_empty() {
            return Err(Error::Config(
                "argument template must include at least one segment".into(),
            ));
        }

        Ok(Self { segments })
    }

    pub(crate) fn build_args(&self, action: &PlannedAction) -> Result<Vec<String>> {
        let mut args = Vec::new();
        for segment in &self.segments {
            match segment {
                CliArgsSegment::Literal(value) => args.push(value.clone()),
                CliArgsSegment::RequiredPositional { aliases } => {
                    args.push(required_action_value(action, aliases)?);
                }
                CliArgsSegment::OptionalPositional { aliases } => {
                    if let Some(value) = first_action_value(action, aliases) {
                        args.push(value);
                    }
                }
                CliArgsSegment::Option {
                    flag,
                    aliases,
                    repeated,
                    required,
                } => {
                    let values = aliases
                        .iter()
                        .find_map(|alias| {
                            let values = action.args.values(alias).map(ToOwned::to_owned).collect::<Vec<_>>();
                            (!values.is_empty()).then_some(values)
                        })
                        .unwrap_or_default();

                    if values.is_empty() {
                        if *required {
                            return Err(Error::InvalidArguments(format!(
                                "missing required argument {} for {}",
                                render_aliases(aliases),
                                action.tool
                            )));
                        }
                        continue;
                    }

                    if *repeated {
                        for value in values {
                            args.push(flag.clone());
                            args.push(value);
                        }
                    } else {
                        let Some(value) = values.last().cloned() else {
                            return Err(Error::InvalidArguments(format!(
                                "missing required argument {} for {}",
                                render_aliases(aliases),
                                action.tool
                            )));
                        };
                        args.push(flag.clone());
                        args.push(value);
                    }
                }
                CliArgsSegment::Flag { flag, aliases } => {
                    if aliases.iter().any(|alias| action.args.has_flag(alias)) {
                        args.push(flag.clone());
                    }
                }
            }
        }

        Ok(args)
    }
}

#[derive(Clone, Debug)]
pub(crate) enum CliArgsSegment {
    Literal(String),
    RequiredPositional {
        aliases: Vec<String>,
    },
    OptionalPositional {
        aliases: Vec<String>,
    },
    Option {
        flag: String,
        aliases: Vec<String>,
        repeated: bool,
        required: bool,
    },
    Flag {
        flag: String,
        aliases: Vec<String>,
    },
}

impl CliArgsSegment {
    pub(crate) fn literal(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_non_empty("literal argument", &value)?;
        Ok(Self::Literal(value))
    }

    pub(crate) fn required_positional(aliases: Vec<String>) -> Result<Self> {
        Ok(Self::RequiredPositional {
            aliases: validate_aliases("required positional argument", aliases)?,
        })
    }

    pub(crate) fn optional_positional(aliases: Vec<String>) -> Result<Self> {
        Ok(Self::OptionalPositional {
            aliases: validate_aliases("optional positional argument", aliases)?,
        })
    }

    pub(crate) fn option(
        flag: impl Into<String>,
        aliases: Vec<String>,
        repeated: bool,
        required: bool,
    ) -> Result<Self> {
        let flag = validate_flag(flag.into())?;
        Ok(Self::Option {
            flag,
            aliases: validate_aliases("option argument", aliases)?,
            repeated,
            required,
        })
    }

    pub(crate) fn flag(flag: impl Into<String>, aliases: Vec<String>) -> Result<Self> {
        let flag = validate_flag(flag.into())?;
        Ok(Self::Flag {
            flag,
            aliases: validate_aliases("flag argument", aliases)?,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CliJsonProjection {
    response_field: String,
    shape: CliJsonProjectionShape,
    count_field: Option<String>,
    refs: Option<CliJsonRefsSpec>,
}

impl CliJsonProjection {
    pub(crate) fn new(
        response_field: impl Into<String>,
        shape: CliJsonProjectionShape,
        count_field: Option<String>,
        refs: Option<CliJsonRefsSpec>,
    ) -> Result<Self> {
        let response_field = response_field.into();
        validate_non_empty("decode response_field", &response_field)?;

        if count_field.is_some() && !matches!(shape, CliJsonProjectionShape::Array { .. }) {
            return Err(Error::Config(
                "decode count_field is only valid for array projections".into(),
            ));
        }

        if let Some(refs) = refs.as_ref() {
            refs.validate()?;
        }

        Ok(Self {
            response_field,
            shape,
            count_field,
            refs,
        })
    }

    pub(crate) fn decode(
        &self,
        target: &ExecutionTarget,
        action: &PlannedAction,
        response: CliResponse,
    ) -> Result<ToolOutput> {
        let CliResponse {
            program,
            version,
            stdout,
            stderr,
        } = response;
        let value: Value = serde_json::from_str(&stdout).map_err(|error| {
            Error::Execution(format!(
                "{} returned invalid JSON for {}: {error}",
                program.display(),
                action.tool
            ))
        })?;

        let (projected, refs, count) = match &self.shape {
            CliJsonProjectionShape::Object { fields } => {
                let projected = Value::Object(project_object(&value, fields));
                let refs = self.extract_refs(action, &[projected.clone()])?;
                (projected, refs, None)
            }
            CliJsonProjectionShape::Array { fields } => {
                let items = value.as_array().ok_or_else(|| {
                    Error::Execution(format!(
                        "{} returned non-array JSON for {}",
                        program.display(),
                        action.tool
                    ))
                })?;
                let projected_items = items
                    .iter()
                    .map(|item| Value::Object(project_object(item, fields)))
                    .collect::<Vec<_>>();
                let count = projected_items.len();
                let refs = self.extract_refs(action, &projected_items)?;
                (Value::Array(projected_items), refs, Some(count))
            }
        };

        let mut output = ToolOutput::new(action.tool.clone(), action.namespace.clone(), action.summary.clone())
            .with_field("status", "ok")
            .with_field("backend", action.backend.to_string())
            .with_field("auth", target.auth.id.to_string())
            .with_field("cli_version", version)
            .with_value_field(self.response_field.clone(), projected)
            .with_refs(refs);

        if let Some((count_field, count)) = self.count_field.as_ref().zip(count) {
            output = output.with_value_field(count_field.clone(), json!(count));
        }

        if !stderr.trim().is_empty() {
            output = output.with_field("cli_stderr", stderr);
        }

        Ok(output)
    }

    fn extract_refs(&self, action: &PlannedAction, items: &[Value]) -> Result<Vec<ToolRef>> {
        let Some(spec) = self.refs.as_ref() else {
            return Ok(Vec::new());
        };

        let provider = action.tool.provider()?;
        let mut refs = Vec::new();
        for item in items {
            let Some(object) = item.as_object() else {
                continue;
            };
            let Some(id) = extract_field_string(object, &spec.id_field) else {
                continue;
            };
            let mut tool_ref = ToolRef::new(provider.clone(), action.namespace.clone(), spec.kind, id)?;
            if let Some(parent_id_field) = spec.parent_id_field.as_ref() {
                if let Some(parent_id) = extract_field_string(object, parent_id_field) {
                    tool_ref = tool_ref.with_parent_id(parent_id)?;
                }
            }
            if let Some(label_field) = spec.label_field.as_ref() {
                if let Some(label) = extract_field_string(object, label_field) {
                    tool_ref = tool_ref.with_label(label)?;
                }
            }
            if let Some(url_field) = spec.url_field.as_ref() {
                if let Some(url) = extract_field_string(object, url_field) {
                    tool_ref = tool_ref.with_web_url(url)?;
                }
            }
            refs.push(tool_ref);
        }

        Ok(refs)
    }
}

#[derive(Clone, Debug)]
pub(crate) enum CliJsonProjectionShape {
    Object { fields: Vec<CliJsonFieldMapping> },
    Array { fields: Vec<CliJsonFieldMapping> },
}

impl CliJsonProjectionShape {
    pub(crate) fn object(fields: Vec<CliJsonFieldMapping>) -> Result<Self> {
        if fields.is_empty() {
            return Err(Error::Config(
                "json object projection must include at least one field".into(),
            ));
        }
        Ok(Self::Object { fields })
    }

    pub(crate) fn array(fields: Vec<CliJsonFieldMapping>) -> Result<Self> {
        if fields.is_empty() {
            return Err(Error::Config(
                "json array projection must include at least one field".into(),
            ));
        }
        Ok(Self::Array { fields })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CliJsonFieldMapping {
    name: String,
    pointer: String,
}

impl CliJsonFieldMapping {
    pub(crate) fn new(name: impl Into<String>, pointer: impl Into<String>) -> Result<Self> {
        let name = name.into();
        let pointer = pointer.into();
        validate_non_empty("json projection field name", &name)?;
        if !pointer.is_empty() && !pointer.starts_with('/') {
            return Err(Error::Config(format!(
                "json projection pointer for field {name} must be empty or start with '/'"
            )));
        }

        Ok(Self { name, pointer })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CliJsonRefsSpec {
    kind: ToolRefKind,
    id_field: String,
    parent_id_field: Option<String>,
    label_field: Option<String>,
    url_field: Option<String>,
}

impl CliJsonRefsSpec {
    pub(crate) fn new(
        kind: ToolRefKind,
        id_field: impl Into<String>,
        parent_id_field: Option<String>,
        label_field: Option<String>,
        url_field: Option<String>,
    ) -> Self {
        Self {
            kind,
            id_field: id_field.into(),
            parent_id_field,
            label_field,
            url_field,
        }
    }

    fn validate(&self) -> Result<()> {
        validate_non_empty("json ref id_field", &self.id_field)?;
        if let Some(parent_id_field) = self.parent_id_field.as_ref() {
            validate_non_empty("json ref parent_id_field", parent_id_field)?;
        }
        if let Some(label_field) = self.label_field.as_ref() {
            validate_non_empty("json ref label_field", label_field)?;
        }
        if let Some(url_field) = self.url_field.as_ref() {
            validate_non_empty("json ref url_field", url_field)?;
        }

        Ok(())
    }
}

fn project_object(value: &Value, fields: &[CliJsonFieldMapping]) -> Map<String, Value> {
    fields
        .iter()
        .map(|field| {
            let value = if field.pointer.is_empty() {
                value.clone()
            } else {
                value.pointer(&field.pointer).cloned().unwrap_or(Value::Null)
            };
            (field.name.clone(), value)
        })
        .collect()
}

fn required_action_value(action: &PlannedAction, aliases: &[String]) -> Result<String> {
    first_action_value(action, aliases).ok_or_else(|| {
        Error::InvalidArguments(format!(
            "missing required argument {} for {}",
            render_aliases(aliases),
            action.tool
        ))
    })
}

fn first_action_value(action: &PlannedAction, aliases: &[String]) -> Option<String> {
    aliases
        .iter()
        .find_map(|alias| action.args.value(alias).map(ToOwned::to_owned))
}

fn render_aliases(aliases: &[String]) -> String {
    aliases
        .iter()
        .map(|alias| format!("--{alias}"))
        .collect::<Vec<_>>()
        .join(" or ")
}

fn validate_aliases(context: &str, aliases: Vec<String>) -> Result<Vec<String>> {
    let aliases = aliases
        .into_iter()
        .map(|alias| alias.trim().to_owned())
        .filter(|alias| !alias.is_empty())
        .collect::<Vec<_>>();
    if aliases.is_empty() {
        return Err(Error::Config(format!("{context} must name at least one argument")));
    }

    Ok(aliases)
}

fn validate_flag(flag: String) -> Result<String> {
    validate_non_empty("cli flag", &flag)?;
    Ok(flag)
}

fn validate_non_empty(context: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::Config(format!("{context} cannot be empty")));
    }

    Ok(())
}

fn extract_field_string(object: &Map<String, Value>, field: &str) -> Option<String> {
    match object.get(field)? {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::{json, Value};
    use switchboard_core::{
        AuthKind, AuthSecretRefs, ExecutionMode, ExecutionTarget, PlannedAction, PlanningTarget, ProviderKind,
        ResolvedAuth, ResolvedCredentials, ResolvedNamespace, ToolArgument, ToolKind, ToolRefKind, ToolRequest,
    };

    use crate::cli::{
        command::CliResponse,
        declarative::{
            CliArgsSegment, CliArgsTemplate, CliJsonFieldMapping, CliJsonProjection, CliJsonProjectionShape,
            CliJsonRefsSpec, CliSummaryTemplate,
        },
    };

    #[test]
    fn summary_template_renders_namespace_and_args() {
        let template = CliSummaryTemplate::parse("Search {arg:query} in {namespace}").expect("template should parse");
        let namespace = ResolvedNamespace::new(
            "github.personal",
            ProviderKind::GitHub,
            "GitHub personal",
            "github.personal_auth",
            true,
            None,
        )
        .expect("namespace should build");
        let request = ToolRequest::new(
            "github.pull_request.search",
            "github.personal",
            ExecutionMode::Auto,
            vec![ToolArgument::option("query", "is:open").expect("argument should build")],
        )
        .expect("request should build");

        let summary = template.render(&namespace, &request).expect("summary should render");
        assert_eq!(summary, "Search is:open in github.personal");
    }

    #[test]
    fn args_template_builds_positionals_options_and_flags() {
        let template = CliArgsTemplate::new(vec![
            CliArgsSegment::literal("search").expect("segment should build"),
            CliArgsSegment::literal("repos").expect("segment should build"),
            CliArgsSegment::required_positional(vec!["query".into()]).expect("segment should build"),
            CliArgsSegment::option("--limit", vec!["limit".into()], false, false).expect("segment should build"),
            CliArgsSegment::option("--topic", vec!["topic".into()], true, false).expect("segment should build"),
            CliArgsSegment::flag("--web", vec!["web".into()]).expect("segment should build"),
        ])
        .expect("template should build");
        let action = PlannedAction::new(
            &ToolRequest::new(
                "github.repository.search",
                "github.personal",
                ExecutionMode::Auto,
                vec![
                    ToolArgument::option("query", "switchboard").expect("argument should build"),
                    ToolArgument::option("limit", "10").expect("argument should build"),
                    ToolArgument::option("topic", "rust").expect("argument should build"),
                    ToolArgument::option("topic", "cli").expect("argument should build"),
                    ToolArgument::flag("web").expect("argument should build"),
                ],
            )
            .expect("request should build"),
            &planning_target(),
            ToolKind::Read,
            "Search repos",
            switchboard_core::BackendKind::Cli,
        );

        let args = template.build_args(&action).expect("args should build");
        assert_eq!(
            args,
            vec![
                "search",
                "repos",
                "switchboard",
                "--limit",
                "10",
                "--topic",
                "rust",
                "--topic",
                "cli",
                "--web"
            ]
        );
    }

    #[test]
    fn json_projection_decodes_array_response_and_refs() {
        let projection = CliJsonProjection::new(
            "repositories",
            CliJsonProjectionShape::array(vec![
                CliJsonFieldMapping::new("name", "/name").expect("field should build"),
                CliJsonFieldMapping::new("full_name", "/fullName").expect("field should build"),
                CliJsonFieldMapping::new("url", "/url").expect("field should build"),
            ])
            .expect("shape should build"),
            Some("count".into()),
            Some(CliJsonRefsSpec::new(
                ToolRefKind::Repository,
                "full_name",
                None,
                Some("name".into()),
                Some("url".into()),
            )),
        )
        .expect("projection should build");
        let action = PlannedAction::new(
            &ToolRequest::new(
                "github.repository.search",
                "github.personal",
                ExecutionMode::Auto,
                vec![ToolArgument::option("query", "switchboard").expect("argument should build")],
            )
            .expect("request should build"),
            &planning_target(),
            ToolKind::Read,
            "Search GitHub repositories matching switchboard",
            switchboard_core::BackendKind::Cli,
        );

        let output = projection
            .decode(
                &execution_target(),
                &action,
                CliResponse {
                    program: PathBuf::from("gh"),
                    version: "gh version 9.9.9-test".into(),
                    stdout: r#"[{"name":"Switchboard","fullName":"KeepSafe/Switchboard","url":"https://github.com/KeepSafe/Switchboard"}]"#.into(),
                    stderr: String::new(),
                },
            )
            .expect("projection should decode");

        assert_eq!(output.fields.get("count"), Some(&json!(1)));
        assert_eq!(
            output
                .fields
                .get("repositories")
                .and_then(Value::as_array)
                .and_then(|repositories| repositories.first())
                .and_then(Value::as_object)
                .and_then(|repository| repository.get("full_name"))
                .and_then(Value::as_str),
            Some("KeepSafe/Switchboard")
        );
        assert_eq!(output.refs.len(), 1);
        assert_eq!(output.refs[0].kind, ToolRefKind::Repository);
        assert_eq!(output.refs[0].id, "KeepSafe/Switchboard");
    }

    fn planning_target() -> PlanningTarget {
        PlanningTarget {
            namespace: ResolvedNamespace::new(
                "github.personal",
                ProviderKind::GitHub,
                "GitHub personal",
                "github.personal_auth",
                true,
                None,
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "github.personal_auth",
                ProviderKind::GitHub,
                AuthKind::GitHubToken,
                "GitHub personal",
                AuthSecretRefs::GitHubToken {
                    token: switchboard_core::SecretRef::new("github.personal.token").expect("secret ref should build"),
                },
            )
            .expect("auth should build"),
        }
    }

    fn execution_target() -> ExecutionTarget {
        ExecutionTarget {
            namespace: ResolvedNamespace::new(
                "github.personal",
                ProviderKind::GitHub,
                "GitHub personal",
                "github.personal_auth",
                true,
                Some(PathBuf::from("/tmp/gh-personal")),
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "github.personal_auth",
                ProviderKind::GitHub,
                AuthKind::GitHubToken,
                "GitHub personal",
                AuthSecretRefs::GitHubToken {
                    token: switchboard_core::SecretRef::new("github.personal.token").expect("secret ref should build"),
                },
            )
            .expect("auth should build"),
            credentials: ResolvedCredentials::GitHubToken {
                token: "ghp-test-token".to_owned().into(),
            },
        }
    }
}
