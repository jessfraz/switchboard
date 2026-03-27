use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Map, Value};
use switchboard_core::{
    Error, ExecutionTarget, OperationEffect, PlannedAction, Result, ToolArgumentSpec, ToolArgumentTransport,
    ToolArgumentValueKind, ToolOutput, ToolRef, ToolRefKind, ToolRequest,
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
                CliArgsSegment::Json { template } => args.push(template.render(action)?),
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
                CliArgsSegment::KeyValueOption {
                    flag,
                    key,
                    aliases,
                    repeated,
                    required,
                    boolish,
                } => {
                    let values = action_values(action, aliases, *boolish);
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
                            args.push(render_key_value_argument(key, &value, *boolish, aliases)?);
                        }
                    } else {
                        let Some(value) = values.last() else {
                            return Err(Error::InvalidArguments(format!(
                                "missing required argument {} for {}",
                                render_aliases(aliases),
                                action.tool
                            )));
                        };
                        args.push(flag.clone());
                        args.push(render_key_value_argument(key, value, *boolish, aliases)?);
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

    pub(crate) fn argument_specs(&self) -> Result<Vec<ToolArgumentSpec>> {
        let mut collector = ToolArgumentSpecCollector::default();
        for segment in &self.segments {
            segment.collect_argument_specs(&mut collector)?;
        }

        collector.finish()
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
    Json {
        template: CliJsonArgumentTemplate,
    },
    Option {
        flag: String,
        aliases: Vec<String>,
        repeated: bool,
        required: bool,
    },
    KeyValueOption {
        flag: String,
        key: String,
        aliases: Vec<String>,
        repeated: bool,
        required: bool,
        boolish: bool,
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

    pub(crate) fn json(template: CliJsonArgumentTemplate) -> Self {
        Self::Json { template }
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

    pub(crate) fn key_value_option(
        flag: impl Into<String>,
        key: impl Into<String>,
        aliases: Vec<String>,
        repeated: bool,
        required: bool,
        boolish: bool,
    ) -> Result<Self> {
        let flag = validate_flag(flag.into())?;
        let key = key.into();
        validate_non_empty("cli key/value option key", &key)?;
        Ok(Self::KeyValueOption {
            flag,
            key,
            aliases: validate_aliases("key/value option argument", aliases)?,
            repeated,
            required,
            boolish,
        })
    }

    pub(crate) fn flag(flag: impl Into<String>, aliases: Vec<String>) -> Result<Self> {
        let flag = validate_flag(flag.into())?;
        Ok(Self::Flag {
            flag,
            aliases: validate_aliases("flag argument", aliases)?,
        })
    }

    fn collect_argument_specs(&self, collector: &mut ToolArgumentSpecCollector) -> Result<()> {
        match self {
            Self::Literal(_) => Ok(()),
            Self::RequiredPositional { aliases } => collector.add_alias_argument(
                aliases,
                ToolArgumentTransport::Positional,
                ToolArgumentValueKind::String,
                true,
                false,
                None,
                None,
            ),
            Self::OptionalPositional { aliases } => collector.add_alias_argument(
                aliases,
                ToolArgumentTransport::Positional,
                ToolArgumentValueKind::String,
                false,
                false,
                None,
                None,
            ),
            Self::Json { template } => template.collect_argument_specs(collector),
            Self::Option {
                flag,
                aliases,
                repeated,
                required,
            } => collector.add_alias_argument(
                aliases,
                ToolArgumentTransport::Option,
                ToolArgumentValueKind::String,
                *required,
                *repeated,
                Some(flag.clone()),
                None,
            ),
            Self::KeyValueOption {
                flag,
                key,
                aliases,
                repeated,
                required,
                boolish,
            } => collector.add_alias_argument(
                aliases,
                ToolArgumentTransport::KeyValueOption,
                if *boolish {
                    ToolArgumentValueKind::Boolean
                } else {
                    ToolArgumentValueKind::String
                },
                *required,
                *repeated,
                Some(flag.clone()),
                Some(key.clone()),
            ),
            Self::Flag { flag, aliases } => collector.add_alias_argument(
                aliases,
                ToolArgumentTransport::Flag,
                ToolArgumentValueKind::Boolean,
                false,
                false,
                Some(flag.clone()),
                None,
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CliJsonArgumentTemplate {
    value: CliJsonArgumentValue,
}

impl CliJsonArgumentTemplate {
    pub(crate) fn new(value: CliJsonArgumentValue) -> Self {
        Self { value }
    }

    fn render(&self, action: &PlannedAction) -> Result<String> {
        serde_json::to_string(&self.value.render(action)?)
            .map_err(|error| Error::Execution(format!("failed to render CLI JSON argument: {error}")))
    }

    fn collect_argument_specs(&self, collector: &mut ToolArgumentSpecCollector) -> Result<()> {
        self.value.collect_argument_specs(collector)
    }
}

#[derive(Clone, Debug)]
pub(crate) enum CliJsonArgumentValue {
    Literal(Value),
    Argument {
        aliases: Vec<String>,
        required: bool,
        repeated: bool,
        default: Option<Value>,
    },
    Object {
        fields: Vec<CliJsonArgumentField>,
    },
    Array {
        items: Vec<CliJsonArgumentValue>,
    },
    Computed(CliComputedJsonValue),
}

impl CliJsonArgumentValue {
    fn render(&self, action: &PlannedAction) -> Result<Value> {
        match self {
            Self::Literal(value) => Ok(value.clone()),
            Self::Argument {
                aliases,
                required,
                repeated,
                default,
            } => render_json_argument_value(action, aliases, *required, *repeated, default.as_ref()),
            Self::Object { fields } => {
                let mut object = Map::new();
                for field in fields {
                    let value = field.value.render(action)?;
                    if field.omit_if_null && value.is_null() {
                        continue;
                    }
                    object.insert(field.name.clone(), value);
                }
                Ok(Value::Object(object))
            }
            Self::Array { items } => items
                .iter()
                .map(|item| item.render(action))
                .collect::<Result<Vec<_>>>()
                .map(Value::Array),
            Self::Computed(value) => value.render(action),
        }
    }

    fn collect_argument_specs(&self, collector: &mut ToolArgumentSpecCollector) -> Result<()> {
        match self {
            Self::Literal(_) => Ok(()),
            Self::Argument {
                aliases,
                required,
                repeated,
                ..
            } => collector.add_alias_argument(
                aliases,
                ToolArgumentTransport::JsonField,
                ToolArgumentValueKind::String,
                *required,
                *repeated,
                None,
                None,
            ),
            Self::Object { fields } => {
                for field in fields {
                    field.value.collect_argument_specs(collector)?;
                }
                Ok(())
            }
            Self::Array { items } => {
                for item in items {
                    item.collect_argument_specs(collector)?;
                }
                Ok(())
            }
            Self::Computed(value) => value.collect_argument_specs(collector),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CliJsonArgumentField {
    name: String,
    value: CliJsonArgumentValue,
    omit_if_null: bool,
}

impl CliJsonArgumentField {
    pub(crate) fn new(name: impl Into<String>, value: CliJsonArgumentValue, omit_if_null: bool) -> Result<Self> {
        let name = name.into();
        validate_non_empty("cli json argument field name", &name)?;
        Ok(Self {
            name,
            value,
            omit_if_null,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) enum CliComputedJsonValue {
    GmailRawMessage,
}

impl CliComputedJsonValue {
    fn render(&self, action: &PlannedAction) -> Result<Value> {
        match self {
            Self::GmailRawMessage => build_gmail_raw_message(action).map(Value::String),
        }
    }

    fn collect_argument_specs(&self, collector: &mut ToolArgumentSpecCollector) -> Result<()> {
        match self {
            Self::GmailRawMessage => {
                collector.add_spec(
                    ToolArgumentSpec::new("to", ToolArgumentTransport::JsonField, ToolArgumentValueKind::String)?
                        .with_required(true)
                        .with_repeated(true),
                )?;
                collector.add_spec(
                    ToolArgumentSpec::new("cc", ToolArgumentTransport::JsonField, ToolArgumentValueKind::String)?
                        .with_repeated(true),
                )?;
                collector.add_spec(
                    ToolArgumentSpec::new("bcc", ToolArgumentTransport::JsonField, ToolArgumentValueKind::String)?
                        .with_repeated(true),
                )?;
                collector.add_spec(ToolArgumentSpec::new(
                    "from",
                    ToolArgumentTransport::JsonField,
                    ToolArgumentValueKind::String,
                )?)?;
                collector.add_spec(ToolArgumentSpec::new(
                    "reply-to",
                    ToolArgumentTransport::JsonField,
                    ToolArgumentValueKind::String,
                )?)?;
                collector.add_spec(ToolArgumentSpec::new(
                    "subject",
                    ToolArgumentTransport::JsonField,
                    ToolArgumentValueKind::String,
                )?)?;
                collector.add_spec(ToolArgumentSpec::new(
                    "in-reply-to",
                    ToolArgumentTransport::JsonField,
                    ToolArgumentValueKind::String,
                )?)?;
                collector.add_spec(
                    ToolArgumentSpec::new(
                        "reference",
                        ToolArgumentTransport::JsonField,
                        ToolArgumentValueKind::String,
                    )?
                    .with_repeated(true),
                )?;
                collector.add_spec(ToolArgumentSpec::new(
                    "body-text",
                    ToolArgumentTransport::JsonField,
                    ToolArgumentValueKind::String,
                )?)?;
                collector.add_spec(ToolArgumentSpec::new(
                    "body-html",
                    ToolArgumentTransport::JsonField,
                    ToolArgumentValueKind::String,
                )?)?;
                Ok(())
            }
        }
    }
}

#[derive(Default)]
struct ToolArgumentSpecCollector {
    specs: Vec<ToolArgumentSpec>,
}

impl ToolArgumentSpecCollector {
    fn add_alias_argument(
        &mut self,
        aliases: &[String],
        transport: ToolArgumentTransport,
        value_kind: ToolArgumentValueKind,
        required: bool,
        repeated: bool,
        forwarded_flag: Option<String>,
        forwarded_key: Option<String>,
    ) -> Result<()> {
        let primary = aliases
            .first()
            .cloned()
            .ok_or_else(|| Error::Config("tool argument metadata requires at least one alias".into()))?;
        let spec = ToolArgumentSpec::new(primary, transport, value_kind)?
            .with_aliases(aliases.iter().skip(1).cloned().collect())?
            .with_required(required)
            .with_repeated(repeated)
            .with_forwarding(forwarded_flag, forwarded_key)?;
        self.add_spec(spec)
    }

    fn add_spec(&mut self, spec: ToolArgumentSpec) -> Result<()> {
        if let Some(existing) = self.specs.iter_mut().find(|existing| existing.name == spec.name) {
            existing.merge_with(&spec)?;
        } else {
            self.specs.push(spec);
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<ToolArgumentSpec>> {
        self.specs.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(self.specs)
    }
}

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

fn render_json_argument_value(
    action: &PlannedAction,
    aliases: &[String],
    required: bool,
    repeated: bool,
    default: Option<&Value>,
) -> Result<Value> {
    if repeated {
        let values = collect_action_values(action, aliases);
        if values.is_empty() {
            if required {
                return Err(Error::InvalidArguments(format!(
                    "missing required argument {} for {}",
                    render_aliases(aliases),
                    action.tool
                )));
            }
            return Ok(default.cloned().unwrap_or_else(|| Value::Array(Vec::new())));
        }

        return Ok(Value::Array(values.into_iter().map(Value::String).collect()));
    }

    if let Some(value) = first_action_value(action, aliases) {
        return Ok(Value::String(value));
    }
    if let Some(default) = default {
        return Ok(default.clone());
    }
    if required {
        return Err(Error::InvalidArguments(format!(
            "missing required argument {} for {}",
            render_aliases(aliases),
            action.tool
        )));
    }

    Ok(Value::Null)
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

fn collect_action_values(action: &PlannedAction, aliases: &[String]) -> Vec<String> {
    aliases
        .iter()
        .find_map(|alias| {
            let values = action.args.values(alias).map(ToOwned::to_owned).collect::<Vec<_>>();
            (!values.is_empty()).then_some(values)
        })
        .unwrap_or_default()
}

fn collect_action_values_for_name(action: &PlannedAction, name: &str) -> Vec<String> {
    action.args.values(name).map(ToOwned::to_owned).collect()
}

fn action_values(action: &PlannedAction, aliases: &[String], include_true_flags: bool) -> Vec<String> {
    if include_true_flags && aliases.iter().any(|alias| action.args.has_flag(alias)) {
        return vec!["true".to_owned()];
    }

    aliases
        .iter()
        .find_map(|alias| {
            let values = action.args.values(alias).map(ToOwned::to_owned).collect::<Vec<_>>();
            (!values.is_empty()).then_some(values)
        })
        .unwrap_or_default()
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

fn render_key_value_argument(key: &str, value: &str, boolish: bool, aliases: &[String]) -> Result<String> {
    let rendered = if boolish {
        parse_boolish(aliases, value)?.to_string()
    } else {
        value.to_owned()
    };

    Ok(format!("{key}={rendered}"))
}

fn build_gmail_raw_message(action: &PlannedAction) -> Result<String> {
    let to = collect_action_values_for_name(action, "to");
    if to.is_empty() {
        return Err(Error::InvalidArguments(format!(
            "missing required argument --to for {}",
            action.tool
        )));
    }
    for recipient in &to {
        validate_header_value("to", recipient)?;
    }

    let cc = collect_action_values_for_name(action, "cc");
    for recipient in &cc {
        validate_header_value("cc", recipient)?;
    }

    let bcc = collect_action_values_for_name(action, "bcc");
    for recipient in &bcc {
        validate_header_value("bcc", recipient)?;
    }

    if let Some(from) = action.args.value("from") {
        validate_header_value("from", from)?;
    }
    if let Some(reply_to) = action.args.value("reply-to") {
        validate_header_value("reply-to", reply_to)?;
    }
    if let Some(subject) = action.args.value("subject") {
        validate_header_value("subject", subject)?;
    }
    if let Some(in_reply_to) = action.args.value("in-reply-to") {
        validate_header_value("in-reply-to", in_reply_to)?;
    }

    let references = collect_action_values_for_name(action, "reference");
    for reference in &references {
        validate_header_value("reference", reference)?;
    }

    let content = render_gmail_body_part(action.args.value("body-text"), action.args.value("body-html"))?;
    let mut message = String::new();

    if let Some(from) = action.args.value("from") {
        message.push_str(&format!("From: {from}\r\n"));
    }
    message.push_str(&format!("To: {}\r\n", to.join(", ")));
    if !cc.is_empty() {
        message.push_str(&format!("Cc: {}\r\n", cc.join(", ")));
    }
    if !bcc.is_empty() {
        message.push_str(&format!("Bcc: {}\r\n", bcc.join(", ")));
    }
    if let Some(reply_to) = action.args.value("reply-to") {
        message.push_str(&format!("Reply-To: {reply_to}\r\n"));
    }
    if let Some(subject) = action.args.value("subject") {
        message.push_str(&format!("Subject: {subject}\r\n"));
    }
    if let Some(in_reply_to) = action.args.value("in-reply-to") {
        message.push_str(&format!("In-Reply-To: {in_reply_to}\r\n"));
    }
    if !references.is_empty() {
        message.push_str(&format!("References: {}\r\n", references.join(" ")));
    }
    message.push_str("MIME-Version: 1.0\r\n");
    message.push_str(&content);

    Ok(URL_SAFE_NO_PAD.encode(message.as_bytes()))
}

fn render_gmail_body_part(body_text: Option<&str>, body_html: Option<&str>) -> Result<String> {
    match (body_text, body_html) {
        (Some(body_text), Some(body_html)) => {
            let boundary = "switchboard-alt-boundary";
            Ok(format!(
                "Content-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\nContent-Type: text/plain; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body_text}\r\n--{boundary}\r\nContent-Type: text/html; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body_html}\r\n--{boundary}--\r\n"
            ))
        }
        (Some(body_text), None) => Ok(format!(
            "Content-Type: text/plain; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body_text}\r\n"
        )),
        (None, Some(body_html)) => Ok(format!(
            "Content-Type: text/html; charset=\"UTF-8\"\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{body_html}\r\n"
        )),
        (None, None) => Err(Error::InvalidArguments(
            "gmail draft requires either --body-text or --body-html".into(),
        )),
    }
}

fn validate_header_value(header: &str, value: &str) -> Result<()> {
    if value.contains('\r') || value.contains('\n') {
        return Err(Error::InvalidArguments(format!(
            "gmail draft argument --{header} cannot contain newlines"
        )));
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

    use serde::Deserialize;
    use serde_json::{json, Map, Value};
    use switchboard_core::{
        AuthKind, AuthSecretRefs, ExecutionMode, ExecutionTarget, PlannedAction, PlanningTarget, ProviderKind,
        ResolvedAuth, ResolvedCredentials, ResolvedNamespace, ToolArgument, ToolKind, ToolRefKind, ToolRequest,
    };

    use crate::cli::{
        command::CliResponse,
        declarative::{
            CliArgsSegment, CliArgsTemplate, CliComputedJsonValue, CliJsonArgumentField, CliJsonArgumentTemplate,
            CliJsonArgumentValue, CliJsonEffectSpec, CliJsonFieldMapping, CliJsonProjection, CliJsonProjectionConfig,
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
    fn summary_template_renders_mode_specific_verbs() {
        let template =
            CliSummaryTemplate::parse("{mode_verb:Draft delete of,Delete} event {arg:event-id} in {namespace}")
                .expect("template should parse");
        let namespace = ResolvedNamespace::new(
            "google.work",
            ProviderKind::GoogleWorkspace,
            "Google Workspace work",
            "google.work_auth",
            false,
            None,
        )
        .expect("namespace should build");
        let draft_request = ToolRequest::new(
            "google.calendar.delete",
            "google.work",
            ExecutionMode::Draft,
            vec![ToolArgument::option("event-id", "evt-123").expect("argument should build")],
        )
        .expect("request should build");
        let apply_request = ToolRequest::new(
            "google.calendar.delete",
            "google.work",
            ExecutionMode::Apply,
            vec![ToolArgument::option("event-id", "evt-123").expect("argument should build")],
        )
        .expect("request should build");

        assert_eq!(
            template
                .render(&namespace, &draft_request)
                .expect("draft summary should render"),
            "Draft delete of event evt-123 in google.work"
        );
        assert_eq!(
            template
                .render(&namespace, &apply_request)
                .expect("apply summary should render"),
            "Delete event evt-123 in google.work"
        );
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
    fn args_template_builds_key_value_options() {
        let template = CliArgsTemplate::new(vec![
            CliArgsSegment::literal("api").expect("segment should build"),
            CliArgsSegment::literal("notifications").expect("segment should build"),
            CliArgsSegment::key_value_option("-F", "all", vec!["all".into()], false, false, true)
                .expect("segment should build"),
            CliArgsSegment::key_value_option("-F", "per_page", vec!["per_page".into()], false, false, false)
                .expect("segment should build"),
        ])
        .expect("template should build");
        let action = PlannedAction::new(
            &ToolRequest::new(
                "github.notifications.list",
                "github.personal",
                ExecutionMode::Auto,
                vec![
                    ToolArgument::option("all", "true").expect("argument should build"),
                    ToolArgument::option("per_page", "50").expect("argument should build"),
                ],
            )
            .expect("request should build"),
            &planning_target(),
            ToolKind::Read,
            "List notifications",
            switchboard_core::BackendKind::Cli,
        );

        let args = template.build_args(&action).expect("args should build");
        assert_eq!(
            args,
            vec!["api", "notifications", "-F", "all=true", "-F", "per_page=50"]
        );
    }

    #[test]
    fn args_template_builds_json_segments_with_computed_values() {
        let template = CliArgsTemplate::new(vec![
            CliArgsSegment::literal("gmail").expect("segment should build"),
            CliArgsSegment::literal("users").expect("segment should build"),
            CliArgsSegment::literal("drafts").expect("segment should build"),
            CliArgsSegment::literal("create").expect("segment should build"),
            CliArgsSegment::literal("--json").expect("segment should build"),
            CliArgsSegment::json(CliJsonArgumentTemplate::new(CliJsonArgumentValue::Object {
                fields: vec![CliJsonArgumentField::new(
                    "message",
                    CliJsonArgumentValue::Object {
                        fields: vec![
                            CliJsonArgumentField::new(
                                "raw",
                                CliJsonArgumentValue::Computed(CliComputedJsonValue::GmailRawMessage),
                                false,
                            )
                            .expect("field should build"),
                            CliJsonArgumentField::new(
                                "threadId",
                                CliJsonArgumentValue::Argument {
                                    aliases: vec!["thread-id".into()],
                                    required: false,
                                    repeated: false,
                                    default: None,
                                },
                                true,
                            )
                            .expect("field should build"),
                        ],
                    },
                    false,
                )
                .expect("field should build")],
            })),
        ])
        .expect("template should build");
        let action = PlannedAction::new(
            &ToolRequest::new(
                "google.mail.draft",
                "google.work",
                ExecutionMode::Apply,
                vec![
                    ToolArgument::option("to", "dogs@example.com").expect("argument should build"),
                    ToolArgument::option("subject", "Boarding request").expect("argument should build"),
                    ToolArgument::option("body-text", "Hi there").expect("argument should build"),
                    ToolArgument::option("thread-id", "thread-123").expect("argument should build"),
                ],
            )
            .expect("request should build"),
            &google_planning_target(),
            ToolKind::Write,
            "Draft Gmail email to dogs@example.com in google.work",
            switchboard_core::BackendKind::Cli,
        );

        let args = template.build_args(&action).expect("args should build");
        let json_arg: Value = serde_json::from_str(&args[5]).expect("json segment should parse");
        assert_eq!(args[..5], ["gmail", "users", "drafts", "create", "--json"]);
        assert_eq!(json_arg["message"]["threadId"], "thread-123");
        assert!(json_arg["message"]["raw"].as_str().is_some());
    }

    #[test]
    fn json_projection_decodes_array_response_and_refs() {
        let projection = CliJsonProjection::new(CliJsonProjectionConfig {
            response_field: "repositories".into(),
            source_pointer: None,
            shape: CliJsonProjectionShape::array(vec![
                CliJsonFieldMapping::from_pointer_with_items("name", "/name", None).expect("field should build"),
                CliJsonFieldMapping::from_pointer_with_items("full_name", "/fullName", None)
                    .expect("field should build"),
                CliJsonFieldMapping::from_pointer_with_items("url", "/url", None).expect("field should build"),
            ])
            .expect("shape should build"),
            count_field: Some("count".into()),
            extra_fields: Vec::new(),
            summary_template: Some(
                CliProjectionTemplate::parse("Found {count} repositories for {namespace}")
                    .expect("template should build"),
            ),
            refs: vec![CliJsonRefsSpec::new(
                ToolRefKind::Repository,
                "full_name",
                None,
                Some("name".into()),
                Some("url".into()),
            )],
            effect: None,
            empty_stdout_json: None,
        })
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
        let fields: RepositoryProjectionFields = parse_output_fields(&output);
        assert_eq!(fields.count, 1);
        assert_eq!(fields.repositories[0].full_name, "KeepSafe/Switchboard");
        assert_eq!(output.refs.len(), 1);
        assert_eq!(output.refs[0].kind, ToolRefKind::Repository);
        assert_eq!(output.refs[0].id, "KeepSafe/Switchboard");
    }

    #[test]
    fn json_projection_supports_argument_fields_and_effect_templates() {
        let projection = CliJsonProjection::new(CliJsonProjectionConfig {
            response_field: "event".into(),
            source_pointer: None,
            shape: CliJsonProjectionShape::object(vec![
                CliJsonFieldMapping::from_pointer_with_items("event_id", "/id", None).expect("field should build"),
                CliJsonFieldMapping::from_pointer_with_items("title", "/summary", None).expect("field should build"),
                CliJsonFieldMapping::from_argument("calendar", vec!["calendar".into()], Some("primary".into()))
                    .expect("field should build"),
            ])
            .expect("shape should build"),
            count_field: None,
            extra_fields: Vec::new(),
            summary_template: Some(
                CliProjectionTemplate::parse("Created calendar event \"{field:title}\" for {namespace}")
                    .expect("template should build"),
            ),
            refs: vec![CliJsonRefsSpec::new(
                ToolRefKind::Event,
                "event_id",
                Some("calendar".into()),
                Some("title".into()),
                None,
            )],
            effect: Some(CliJsonEffectSpec::new(
                true,
                true,
                Some(
                    CliProjectionTemplate::parse("Delete calendar event \"{field:title}\" from {namespace}")
                        .expect("template should build"),
                ),
            )),
            empty_stdout_json: None,
        })
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
        let projection = CliJsonProjection::new(CliJsonProjectionConfig {
            response_field: "pull_request".into(),
            source_pointer: None,
            shape: CliJsonProjectionShape::object(vec![
                CliJsonFieldMapping::from_pointer_with_items("title", "/title", None).expect("field should build"),
                CliJsonFieldMapping::from_pointer_with_items("assignees", "/assignees", Some("/login".into()))
                    .expect("field should build"),
                CliJsonFieldMapping::from_pointer_with_items("labels", "/labels", Some("/name".into()))
                    .expect("field should build"),
            ])
            .expect("shape should build"),
            count_field: None,
            extra_fields: Vec::new(),
            summary_template: Some(
                CliProjectionTemplate::parse("Read {field:title} for {namespace}").expect("template should build"),
            ),
            refs: Vec::new(),
            effect: None,
            empty_stdout_json: None,
        })
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

        let fields: PullRequestProjectionFields = parse_output_fields(&output);
        assert_eq!(fields.pull_request.assignees, vec!["jessfraz"]);
        assert_eq!(fields.pull_request.labels, vec!["infra", "tooling"]);
    }

    #[test]
    fn json_projection_supports_extra_fields_empty_stdout_and_multiple_refs() {
        let projection = CliJsonProjection::new(CliJsonProjectionConfig {
            response_field: "message".into(),
            source_pointer: None,
            shape: CliJsonProjectionShape::object(vec![
                CliJsonFieldMapping::from_argument("gmail_message_id", vec!["message-id".into()], None)
                    .expect("field should build"),
                CliJsonFieldMapping::from_pointer_with_items("gmail_thread_id", "/thread_id", None)
                    .expect("field should build"),
                CliJsonFieldMapping::from_pointer_with_items("subject", "/subject", None).expect("field should build"),
            ])
            .expect("shape should build"),
            count_field: None,
            extra_fields: vec![
                CliJsonFieldMapping::from_argument("query", vec!["query".into()], None).expect("field should build")
            ],
            summary_template: Some(
                CliProjectionTemplate::parse("Read Gmail message \"{field:subject}\" for {namespace}")
                    .expect("template should build"),
            ),
            refs: vec![
                CliJsonRefsSpec::new(
                    ToolRefKind::Message,
                    "gmail_message_id",
                    Some("gmail_thread_id".into()),
                    Some("subject".into()),
                    None,
                ),
                CliJsonRefsSpec::new(
                    ToolRefKind::Thread,
                    "gmail_thread_id",
                    None,
                    Some("subject".into()),
                    None,
                ),
            ],
            effect: None,
            empty_stdout_json: Some(json!({
                "thread_id": "thread-123",
                "subject": "Dog hotel booking"
            })),
        })
        .expect("projection should build");
        let action = PlannedAction::new(
            &ToolRequest::new(
                "google.mail.read",
                "google.work",
                ExecutionMode::Auto,
                vec![
                    ToolArgument::option("message-id", "msg-123").expect("argument should build"),
                    ToolArgument::option("query", "from:doghotel").expect("argument should build"),
                ],
            )
            .expect("request should build"),
            &google_planning_target(),
            ToolKind::Read,
            "Read Gmail message msg-123 in google.work",
            switchboard_core::BackendKind::Cli,
        );

        let output = projection
            .decode(
                &google_execution_target(),
                &action,
                CliResponse {
                    program: PathBuf::from("gws"),
                    version: "gws 0.99.0-test".into(),
                    stdout: String::new(),
                    stderr: String::new(),
                },
            )
            .expect("projection should decode");

        let fields: MessageProjectionFields = parse_output_fields(&output);
        assert_eq!(fields.query, "from:doghotel");
        assert_eq!(fields.message.gmail_message_id, "msg-123");
        assert_eq!(fields.message.gmail_thread_id, "thread-123");
        assert_eq!(output.refs.len(), 2);
        assert_eq!(output.refs[0].kind, ToolRefKind::Message);
        assert_eq!(output.refs[0].parent_id.as_deref(), Some("thread-123"));
        assert_eq!(output.refs[1].kind, ToolRefKind::Thread);
    }

    #[test]
    fn json_projection_supports_argument_values_and_presence_fields() {
        let projection = CliJsonProjection::new(CliJsonProjectionConfig {
            response_field: "draft".into(),
            source_pointer: None,
            shape: CliJsonProjectionShape::object(vec![
                CliJsonFieldMapping::from_pointer_with_items("draft_id", "/id", None).expect("field should build"),
                CliJsonFieldMapping::from_argument_values("to", vec!["to".into()]).expect("field should build"),
                CliJsonFieldMapping::from_argument_presence("has_body_text", vec!["body-text".into()])
                    .expect("field should build"),
            ])
            .expect("shape should build"),
            count_field: None,
            extra_fields: Vec::new(),
            summary_template: None,
            refs: Vec::new(),
            effect: None,
            empty_stdout_json: None,
        })
        .expect("projection should build");
        let action = PlannedAction::new(
            &ToolRequest::new(
                "google.mail.draft",
                "google.work",
                ExecutionMode::Apply,
                vec![
                    ToolArgument::option("to", "dogs@example.com").expect("argument should build"),
                    ToolArgument::option("to", "frontdesk@example.com").expect("argument should build"),
                    ToolArgument::option("body-text", "Hi there").expect("argument should build"),
                ],
            )
            .expect("request should build"),
            &google_planning_target(),
            ToolKind::Write,
            "Draft Gmail email to dogs@example.com, frontdesk@example.com in google.work",
            switchboard_core::BackendKind::Cli,
        );

        let output = projection
            .decode(
                &google_execution_target(),
                &action,
                CliResponse {
                    program: PathBuf::from("gws"),
                    version: "gws 0.99.0-test".into(),
                    stdout: r#"{"id":"draft-123"}"#.into(),
                    stderr: String::new(),
                },
            )
            .expect("projection should decode");

        let fields: MailDraftProjectionFields = parse_output_fields(&output);
        assert_eq!(fields.draft.draft_id, "draft-123");
        assert_eq!(fields.draft.to, vec!["dogs@example.com", "frontdesk@example.com"]);
        assert!(fields.draft.has_body_text);
    }

    #[derive(Debug, Deserialize)]
    struct RepositoryProjectionFields {
        count: usize,
        repositories: Vec<RepositoryProjectionItem>,
    }

    #[derive(Debug, Deserialize)]
    struct RepositoryProjectionItem {
        full_name: String,
    }

    #[derive(Debug, Deserialize)]
    struct PullRequestProjectionFields {
        pull_request: PullRequestProjectionItem,
    }

    #[derive(Debug, Deserialize)]
    struct PullRequestProjectionItem {
        assignees: Vec<String>,
        labels: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    struct MessageProjectionFields {
        query: String,
        message: MessageProjectionItem,
    }

    #[derive(Debug, Deserialize)]
    struct MessageProjectionItem {
        gmail_message_id: String,
        gmail_thread_id: String,
    }

    #[derive(Debug, Deserialize)]
    struct MailDraftProjectionFields {
        draft: MailDraftProjectionItem,
    }

    #[derive(Debug, Deserialize)]
    struct MailDraftProjectionItem {
        draft_id: String,
        to: Vec<String>,
        has_body_text: bool,
    }

    fn parse_output_fields<T: for<'de> Deserialize<'de>>(output: &switchboard_core::ToolOutput) -> T {
        serde_json::from_value(Value::Object(
            output
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Map<String, Value>>(),
        ))
        .expect("output fields should deserialize")
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
