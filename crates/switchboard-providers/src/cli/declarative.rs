use serde_json::{json, Map, Value};
use switchboard_core::{
    Error, ExecutionTarget, OperationEffect, PlannedAction, Result, ToolOutput, ToolRef, ToolRefKind, ToolRequest,
};

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
                    if flag_enabled(action, aliases)? {
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
    summary_template: Option<CliProjectionTemplate>,
    refs: Option<CliJsonRefsSpec>,
    effect: Option<CliJsonEffectSpec>,
}

impl CliJsonProjection {
    pub(crate) fn new(
        response_field: impl Into<String>,
        shape: CliJsonProjectionShape,
        count_field: Option<String>,
        summary_template: Option<CliProjectionTemplate>,
        refs: Option<CliJsonRefsSpec>,
        effect: Option<CliJsonEffectSpec>,
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
        if let Some(effect) = effect.as_ref() {
            effect.validate()?;
        }

        Ok(Self {
            response_field,
            shape,
            count_field,
            summary_template,
            refs,
            effect,
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
                let projected = Value::Object(project_object(action, &value, fields));
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
                    .map(|item| Value::Object(project_object(action, item, fields)))
                    .collect::<Vec<_>>();
                let count = projected_items.len();
                let refs = self.extract_refs(action, &projected_items)?;
                (Value::Array(projected_items), refs, Some(count))
            }
        };

        let summary = self
            .summary_template
            .as_ref()
            .map(|template| template.render(action, &projected, count))
            .transpose()?
            .unwrap_or_else(|| action.summary.clone());
        let effect = self
            .effect
            .as_ref()
            .map(|effect_spec| effect_spec.build(action, &refs, &projected, count))
            .transpose()?;
        let mut output = ToolOutput::new(action.tool.clone(), action.namespace.clone(), summary)
            .with_field("status", "ok")
            .with_field("backend", action.backend.to_string())
            .with_field("auth", target.auth.id.to_string())
            .with_field("cli_version", version)
            .with_value_field(self.response_field.clone(), projected)
            .with_refs(refs);

        if let Some((count_field, count)) = self.count_field.as_ref().zip(count) {
            output = output.with_value_field(count_field.clone(), json!(count));
        }
        if let Some(effect) = effect {
            output = output.with_effect(effect);
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
    source: CliJsonFieldSource,
}

impl CliJsonFieldMapping {
    pub(crate) fn from_pointer_with_items(
        name: impl Into<String>,
        pointer: impl Into<String>,
        item_pointer: Option<String>,
    ) -> Result<Self> {
        let name = name.into();
        let pointer = pointer.into();
        validate_non_empty("json projection field name", &name)?;
        if !pointer.is_empty() && !pointer.starts_with('/') {
            return Err(Error::Config(format!(
                "json projection pointer for field {name} must be empty or start with '/'"
            )));
        }
        if let Some(item_pointer) = item_pointer.as_ref() {
            if !item_pointer.is_empty() && !item_pointer.starts_with('/') {
                return Err(Error::Config(format!(
                    "json projection item_pointer for field {name} must be empty or start with '/'"
                )));
            }
        }

        Ok(Self {
            name,
            source: CliJsonFieldSource::Pointer { pointer, item_pointer },
        })
    }

    pub(crate) fn from_argument(
        name: impl Into<String>,
        aliases: Vec<String>,
        default: Option<String>,
    ) -> Result<Self> {
        let name = name.into();
        validate_non_empty("json projection field name", &name)?;
        Ok(Self {
            name,
            source: CliJsonFieldSource::Argument {
                aliases: validate_aliases("json projection argument source", aliases)?,
                default,
            },
        })
    }

    pub(crate) fn from_literal(name: impl Into<String>, value: Value) -> Result<Self> {
        let name = name.into();
        validate_non_empty("json projection field name", &name)?;
        Ok(Self {
            name,
            source: CliJsonFieldSource::Literal(value),
        })
    }
}

#[derive(Clone, Debug)]
enum CliJsonFieldSource {
    Pointer {
        pointer: String,
        item_pointer: Option<String>,
    },
    Argument {
        aliases: Vec<String>,
        default: Option<String>,
    },
    Literal(Value),
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

#[derive(Clone, Debug)]
pub(crate) struct CliJsonEffectSpec {
    undoable: bool,
    use_output_refs: bool,
    summary_template: Option<CliProjectionTemplate>,
}

impl CliJsonEffectSpec {
    pub(crate) fn new(undoable: bool, use_output_refs: bool, summary_template: Option<CliProjectionTemplate>) -> Self {
        Self {
            undoable,
            use_output_refs,
            summary_template,
        }
    }

    fn validate(&self) -> Result<()> {
        if !self.undoable && self.summary_template.is_some() {
            return Err(Error::Config(
                "effect summary_template is only valid for undoable effects".into(),
            ));
        }

        Ok(())
    }

    fn build(
        &self,
        action: &PlannedAction,
        output_refs: &[ToolRef],
        projection: &Value,
        count: Option<usize>,
    ) -> Result<OperationEffect> {
        let mut effect = OperationEffect::new(self.undoable);
        if self.use_output_refs {
            effect = effect.with_refs(output_refs.iter().cloned());
        }
        if let Some(summary_template) = self.summary_template.as_ref() {
            effect = effect.with_undo_summary(summary_template.render(action, projection, count)?)?;
        }
        Ok(effect)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CliProjectionTemplate {
    segments: Vec<CliProjectionSegment>,
}

impl CliProjectionTemplate {
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
                segments.push(CliProjectionSegment::Literal(std::mem::take(&mut literal)));
            }

            let mut token = String::new();
            loop {
                let Some(next) = chars.next() else {
                    return Err(Error::Config(format!(
                        "invalid projection template {template:?}: missing closing brace"
                    )));
                };
                if next == '}' {
                    break;
                }
                token.push(next);
            }

            segments.push(parse_projection_segment(&template, token)?);
        }

        if !literal.is_empty() {
            segments.push(CliProjectionSegment::Literal(literal));
        }

        Ok(Self { segments })
    }

    fn render(&self, action: &PlannedAction, value: &Value, count: Option<usize>) -> Result<String> {
        let fields = value.as_object().filter(|_| !value.is_array());
        self.render_inner(action, fields, count)
    }

    fn render_inner(
        &self,
        action: &PlannedAction,
        fields: Option<&Map<String, Value>>,
        count: Option<usize>,
    ) -> Result<String> {
        let mut rendered = String::new();
        for segment in &self.segments {
            match segment {
                CliProjectionSegment::Literal(value) => rendered.push_str(value),
                CliProjectionSegment::Namespace => rendered.push_str(action.namespace.as_str()),
                CliProjectionSegment::Count => rendered.push_str(
                    &count
                        .ok_or_else(|| {
                            Error::Execution("projection template references count for non-array output".into())
                        })?
                        .to_string(),
                ),
                CliProjectionSegment::Field { name } => {
                    let fields = fields.ok_or_else(|| {
                        Error::Execution(format!(
                            "projection template references field {name} for non-object output"
                        ))
                    })?;
                    let value = extract_field_string(fields, name).ok_or_else(|| {
                        Error::Execution(format!("projection template references missing field {name}"))
                    })?;
                    rendered.push_str(&value);
                }
            }
        }

        Ok(rendered)
    }
}

#[derive(Clone, Debug)]
enum CliProjectionSegment {
    Literal(String),
    Namespace,
    Count,
    Field { name: String },
}

fn parse_projection_segment(template: &str, token: String) -> Result<CliProjectionSegment> {
    match token.as_str() {
        "namespace" => Ok(CliProjectionSegment::Namespace),
        "count" => Ok(CliProjectionSegment::Count),
        _ => {
            if let Some(name) = token.strip_prefix("field:") {
                validate_non_empty("projection template field name", name)?;
                Ok(CliProjectionSegment::Field { name: name.to_owned() })
            } else {
                Err(Error::Config(format!(
                    "invalid projection template {template:?}: unsupported placeholder {{{token}}}"
                )))
            }
        }
    }
}

fn project_object(action: &PlannedAction, value: &Value, fields: &[CliJsonFieldMapping]) -> Map<String, Value> {
    fields
        .iter()
        .map(|field| {
            let value = match &field.source {
                CliJsonFieldSource::Pointer { pointer, item_pointer } => {
                    project_pointer_value(value, pointer, item_pointer)
                }
                CliJsonFieldSource::Argument { aliases, default } => first_action_value(action, aliases)
                    .map(Value::String)
                    .or_else(|| default.as_ref().map(|value| Value::String(value.clone())))
                    .unwrap_or(Value::Null),
                CliJsonFieldSource::Literal(value) => value.clone(),
            };
            (field.name.clone(), value)
        })
        .collect()
}

fn project_pointer_value(value: &Value, pointer: &str, item_pointer: &Option<String>) -> Value {
    let projected = if pointer.is_empty() {
        value
    } else {
        value.pointer(pointer).unwrap_or(&Value::Null)
    };

    let Some(item_pointer) = item_pointer.as_ref() else {
        return projected.clone();
    };

    match projected {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .filter_map(|item| {
                    let selected = if item_pointer.is_empty() {
                        Some(item)
                    } else {
                        item.pointer(item_pointer)
                    }?;
                    (!selected.is_null()).then_some(selected.clone())
                })
                .collect(),
        ),
        _ => Value::Array(Vec::new()),
    }
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

fn flag_enabled(action: &PlannedAction, aliases: &[String]) -> Result<bool> {
    if aliases.iter().any(|alias| action.args.has_flag(alias)) {
        return Ok(true);
    }

    match first_action_value(action, aliases) {
        Some(value) => parse_boolish(aliases, &value),
        None => Ok(false),
    }
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

fn parse_boolish(aliases: &[String], value: &str) -> Result<bool> {
    match value {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(Error::InvalidArguments(format!(
            "{} expects a boolean value, got {value:?}",
            render_aliases(aliases)
        ))),
    }
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
            CliArgsSegment, CliArgsTemplate, CliJsonEffectSpec, CliJsonFieldMapping, CliJsonProjection,
            CliJsonProjectionShape, CliJsonRefsSpec, CliProjectionTemplate, CliSummaryTemplate,
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
    fn args_template_treats_boolean_option_values_as_flags() {
        let template = CliArgsTemplate::new(vec![
            CliArgsSegment::literal("search").expect("segment should build"),
            CliArgsSegment::literal("prs").expect("segment should build"),
            CliArgsSegment::required_positional(vec!["query".into()]).expect("segment should build"),
            CliArgsSegment::flag("--draft", vec!["draft".into()]).expect("segment should build"),
            CliArgsSegment::flag("--merged", vec!["merged".into()]).expect("segment should build"),
        ])
        .expect("template should build");
        let action = PlannedAction::new(
            &ToolRequest::new(
                "github.pull_request.search",
                "github.personal",
                ExecutionMode::Auto,
                vec![
                    ToolArgument::option("query", "is:open").expect("argument should build"),
                    ToolArgument::option("draft", "true").expect("argument should build"),
                    ToolArgument::option("merged", "false").expect("argument should build"),
                ],
            )
            .expect("request should build"),
            &planning_target(),
            ToolKind::Read,
            "Search pull requests",
            switchboard_core::BackendKind::Cli,
        );

        let args = template.build_args(&action).expect("args should build");
        assert_eq!(args, vec!["search", "prs", "is:open", "--draft"]);
    }

    #[test]
    fn json_projection_decodes_array_response_and_refs() {
        let projection = CliJsonProjection::new(
            "repositories",
            CliJsonProjectionShape::array(vec![
                CliJsonFieldMapping::from_pointer_with_items("name", "/name", None).expect("field should build"),
                CliJsonFieldMapping::from_pointer_with_items("full_name", "/fullName", None)
                    .expect("field should build"),
                CliJsonFieldMapping::from_pointer_with_items("url", "/url", None).expect("field should build"),
            ])
            .expect("shape should build"),
            Some("count".into()),
            Some(
                CliProjectionTemplate::parse("Found {count} repositories for {namespace}")
                    .expect("template should build"),
            ),
            Some(CliJsonRefsSpec::new(
                ToolRefKind::Repository,
                "full_name",
                None,
                Some("name".into()),
                Some("url".into()),
            )),
            None,
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

        assert_eq!(output.summary, "Found 1 repositories for github.personal");
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

    #[test]
    fn json_projection_supports_argument_fields_and_effect_templates() {
        let projection = CliJsonProjection::new(
            "event",
            CliJsonProjectionShape::object(vec![
                CliJsonFieldMapping::from_pointer_with_items("event_id", "/id", None).expect("field should build"),
                CliJsonFieldMapping::from_pointer_with_items("title", "/summary", None).expect("field should build"),
                CliJsonFieldMapping::from_argument("calendar", vec!["calendar".into()], Some("primary".into()))
                    .expect("field should build"),
            ])
            .expect("shape should build"),
            None,
            Some(
                CliProjectionTemplate::parse("Created calendar event \"{field:title}\" for {namespace}")
                    .expect("template should build"),
            ),
            Some(CliJsonRefsSpec::new(
                ToolRefKind::Event,
                "event_id",
                Some("calendar".into()),
                Some("title".into()),
                None,
            )),
            Some(CliJsonEffectSpec::new(
                true,
                true,
                Some(
                    CliProjectionTemplate::parse("Delete calendar event \"{field:title}\" from {namespace}")
                        .expect("template should build"),
                ),
            )),
        )
        .expect("projection should build");
        let action = PlannedAction::new(
            &ToolRequest::new(
                "google.calendar.create",
                "google.work",
                ExecutionMode::Apply,
                vec![
                    ToolArgument::option("title", "Budget review").expect("argument should build"),
                    ToolArgument::option("calendar", "primary").expect("argument should build"),
                ],
            )
            .expect("request should build"),
            &google_planning_target(),
            ToolKind::Write,
            "Create calendar event \"Budget review\" for google.work",
            switchboard_core::BackendKind::Cli,
        );

        let output = projection
            .decode(
                &google_execution_target(),
                &action,
                CliResponse {
                    program: PathBuf::from("gws"),
                    version: "gws 0.99.0-test".into(),
                    stdout: r#"{"id":"event-1960budgetwork","summary":"Budget review"}"#.into(),
                    stderr: String::new(),
                },
            )
            .expect("projection should decode");

        assert_eq!(
            output.summary,
            "Created calendar event \"Budget review\" for google.work"
        );
        assert_eq!(output.refs[0].id, "event-1960budgetwork");
        assert_eq!(output.refs[0].parent_id.as_deref(), Some("primary"));
        assert_eq!(output.effect.as_ref().map(|effect| effect.undoable), Some(true));
        assert_eq!(
            output.effect.as_ref().and_then(|effect| effect.undo_summary.as_deref()),
            Some("Delete calendar event \"Budget review\" from google.work")
        );
    }

    #[test]
    fn json_projection_supports_array_field_extraction() {
        let projection = CliJsonProjection::new(
            "pull_request",
            CliJsonProjectionShape::object(vec![
                CliJsonFieldMapping::from_pointer_with_items("title", "/title", None).expect("field should build"),
                CliJsonFieldMapping::from_pointer_with_items("assignees", "/assignees", Some("/login".into()))
                    .expect("field should build"),
                CliJsonFieldMapping::from_pointer_with_items("labels", "/labels", Some("/name".into()))
                    .expect("field should build"),
            ])
            .expect("shape should build"),
            None,
            Some(CliProjectionTemplate::parse("Read {field:title} for {namespace}").expect("template should build")),
            None,
            None,
        )
        .expect("projection should build");
        let action = PlannedAction::new(
            &ToolRequest::new(
                "github.pull_request.read",
                "github.personal",
                ExecutionMode::Auto,
                vec![
                    ToolArgument::option("repo", "openai/codex").expect("argument should build"),
                    ToolArgument::option("number", "1382").expect("argument should build"),
                ],
            )
            .expect("request should build"),
            &planning_target(),
            ToolKind::Read,
            "Read GitHub pull request openai/codex#1382 in github.personal",
            switchboard_core::BackendKind::Cli,
        );

        let output = projection
            .decode(
                &execution_target(),
                &action,
                CliResponse {
                    program: PathBuf::from("gh"),
                    version: "gh version 9.9.9-test".into(),
                    stdout: r#"{"title":"Fix the thing","assignees":[{"login":"jessfraz"}],"labels":[{"name":"infra"},{"name":"tooling"}]}"#
                        .into(),
                    stderr: String::new(),
                },
            )
            .expect("projection should decode");

        assert_eq!(
            output
                .fields
                .get("pull_request")
                .and_then(|pull_request| pull_request.get("assignees"))
                .and_then(Value::as_array),
            Some(&vec![json!("jessfraz")])
        );
        assert_eq!(
            output
                .fields
                .get("pull_request")
                .and_then(|pull_request| pull_request.get("labels"))
                .and_then(Value::as_array),
            Some(&vec![json!("infra"), json!("tooling")])
        );
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

    fn google_planning_target() -> PlanningTarget {
        PlanningTarget {
            namespace: ResolvedNamespace::new(
                "google.work",
                ProviderKind::GoogleWorkspace,
                "Google Workspace work",
                "google.work_auth",
                true,
                Some(PathBuf::from("/tmp/gws-work")),
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "google.work_auth",
                ProviderKind::GoogleWorkspace,
                AuthKind::GoogleOAuth,
                "Google Workspace work",
                AuthSecretRefs::GoogleOAuth {
                    client_id: switchboard_core::SecretRef::new("google.work.client_id")
                        .expect("secret ref should build"),
                    client_secret: switchboard_core::SecretRef::new("google.work.client_secret")
                        .expect("secret ref should build"),
                    refresh_token: None,
                },
            )
            .expect("auth should build"),
        }
    }

    fn google_execution_target() -> ExecutionTarget {
        ExecutionTarget {
            namespace: ResolvedNamespace::new(
                "google.work",
                ProviderKind::GoogleWorkspace,
                "Google Workspace work",
                "google.work_auth",
                true,
                Some(PathBuf::from("/tmp/gws-work")),
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "google.work_auth",
                ProviderKind::GoogleWorkspace,
                AuthKind::GoogleOAuth,
                "Google Workspace work",
                AuthSecretRefs::GoogleOAuth {
                    client_id: switchboard_core::SecretRef::new("google.work.client_id")
                        .expect("secret ref should build"),
                    client_secret: switchboard_core::SecretRef::new("google.work.client_secret")
                        .expect("secret ref should build"),
                    refresh_token: None,
                },
            )
            .expect("auth should build"),
            credentials: ResolvedCredentials::GoogleOAuth {
                client_id: "client-id".to_owned().into(),
                client_secret: "client-secret".to_owned().into(),
                refresh_token: None,
            },
        }
    }
}
