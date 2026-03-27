use serde_json::{json, Map, Value};
use switchboard_core::{
    Error, ExecutionTarget, OperationEffect, PlannedAction, Result, ToolOutput, ToolRef, ToolRefKind,
};

use crate::cli::{
    command::CliResponse,
    declarative::support::{extract_field_string, first_action_value, validate_aliases, validate_non_empty},
};

#[derive(Clone, Debug)]
pub(crate) struct CliJsonProjection {
    response_field: String,
    source_pointer: Option<String>,
    shape: CliJsonProjectionShape,
    count_field: Option<String>,
    extra_fields: Vec<CliJsonFieldMapping>,
    summary_template: Option<CliProjectionTemplate>,
    refs: Vec<CliJsonRefsSpec>,
    effect: Option<CliJsonEffectSpec>,
    empty_stdout_json: Option<Value>,
}

#[derive(Clone, Debug)]
pub(crate) struct CliJsonProjectionConfig {
    pub(crate) response_field: String,
    pub(crate) source_pointer: Option<String>,
    pub(crate) shape: CliJsonProjectionShape,
    pub(crate) count_field: Option<String>,
    pub(crate) extra_fields: Vec<CliJsonFieldMapping>,
    pub(crate) summary_template: Option<CliProjectionTemplate>,
    pub(crate) refs: Vec<CliJsonRefsSpec>,
    pub(crate) effect: Option<CliJsonEffectSpec>,
    pub(crate) empty_stdout_json: Option<Value>,
}

impl CliJsonProjection {
    pub(crate) fn new(config: CliJsonProjectionConfig) -> Result<Self> {
        let CliJsonProjectionConfig {
            response_field,
            source_pointer,
            shape,
            count_field,
            extra_fields,
            summary_template,
            refs,
            effect,
            empty_stdout_json,
        } = config;
        validate_non_empty("decode response_field", &response_field)?;
        if let Some(source_pointer) = source_pointer.as_ref() {
            if !source_pointer.is_empty() && !source_pointer.starts_with('/') {
                return Err(Error::Config(
                    "decode source_pointer must be empty or start with '/'".into(),
                ));
            }
        }

        if count_field.is_some() && !matches!(shape, CliJsonProjectionShape::Array { .. }) {
            return Err(Error::Config(
                "decode count_field is only valid for array projections".into(),
            ));
        }

        for refs in &refs {
            refs.validate()?;
        }
        if let Some(effect) = effect.as_ref() {
            effect.validate()?;
        }

        Ok(Self {
            response_field,
            source_pointer,
            shape,
            count_field,
            extra_fields,
            summary_template,
            refs,
            effect,
            empty_stdout_json,
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
        let value: Value = if stdout.trim().is_empty() {
            self.empty_stdout_json.clone().ok_or_else(|| {
                Error::Execution(format!(
                    "{} returned empty stdout for {}",
                    program.display(),
                    action.tool
                ))
            })?
        } else {
            serde_json::from_str(&stdout).map_err(|error| {
                Error::Execution(format!(
                    "{} returned invalid JSON for {}: {error}",
                    program.display(),
                    action.tool
                ))
            })?
        };

        let projection_source = self.projection_source(&value).ok_or_else(|| {
            Error::Execution(format!(
                "{} returned JSON missing projection source for {}",
                program.display(),
                action.tool
            ))
        })?;

        let (projected, refs, count) = match &self.shape {
            CliJsonProjectionShape::Object { fields } => {
                let projected = Value::Object(project_object(action, projection_source, fields));
                let refs = self.extract_refs(action, &[projected.clone()])?;
                (projected, refs, None)
            }
            CliJsonProjectionShape::Array { fields } => {
                let items = projection_source.as_array().ok_or_else(|| {
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
        for field in &self.extra_fields {
            output = output.with_value_field(field.name.clone(), project_field_value(action, &value, field));
        }
        if let Some(effect) = effect {
            output = output.with_effect(effect);
        }

        if !stderr.trim().is_empty() {
            output = output.with_field("cli_stderr", stderr);
        }

        Ok(output)
    }

    fn projection_source<'a>(&self, value: &'a Value) -> Option<&'a Value> {
        match self.source_pointer.as_deref() {
            Some("") | None => Some(value),
            Some(pointer) => value.pointer(pointer),
        }
    }

    fn extract_refs(&self, action: &PlannedAction, items: &[Value]) -> Result<Vec<ToolRef>> {
        if self.refs.is_empty() {
            return Ok(Vec::new());
        }

        let provider = action.tool.provider()?;
        let mut refs = Vec::new();
        for spec in &self.refs {
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
    pub(super) name: String,
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

    pub(crate) fn from_argument_values(name: impl Into<String>, aliases: Vec<String>) -> Result<Self> {
        let name = name.into();
        validate_non_empty("json projection field name", &name)?;
        Ok(Self {
            name,
            source: CliJsonFieldSource::ArgumentValues {
                aliases: validate_aliases("json projection argument values source", aliases)?,
            },
        })
    }

    pub(crate) fn from_argument_presence(name: impl Into<String>, aliases: Vec<String>) -> Result<Self> {
        let name = name.into();
        validate_non_empty("json projection field name", &name)?;
        Ok(Self {
            name,
            source: CliJsonFieldSource::ArgumentPresence {
                aliases: validate_aliases("json projection argument presence source", aliases)?,
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
    ArgumentValues {
        aliases: Vec<String>,
    },
    ArgumentPresence {
        aliases: Vec<String>,
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
        .map(|field| (field.name.clone(), project_field_value(action, value, field)))
        .collect()
}

fn project_field_value(action: &PlannedAction, value: &Value, field: &CliJsonFieldMapping) -> Value {
    match &field.source {
        CliJsonFieldSource::Pointer { pointer, item_pointer } => project_pointer_value(value, pointer, item_pointer),
        CliJsonFieldSource::Argument { aliases, default } => first_action_value(action, aliases)
            .map(Value::String)
            .or_else(|| default.as_ref().map(|value| Value::String(value.clone())))
            .unwrap_or(Value::Null),
        CliJsonFieldSource::ArgumentValues { aliases } => Value::Array(
            aliases
                .iter()
                .find_map(|alias| {
                    let values = action.args.values(alias).map(ToOwned::to_owned).collect::<Vec<_>>();
                    (!values.is_empty()).then_some(values)
                })
                .unwrap_or_default()
                .into_iter()
                .map(Value::String)
                .collect(),
        ),
        CliJsonFieldSource::ArgumentPresence { aliases } => Value::Bool(
            aliases
                .iter()
                .any(|alias| action.args.has_flag(alias) || action.args.value(alias).is_some()),
        ),
        CliJsonFieldSource::Literal(value) => value.clone(),
    }
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
