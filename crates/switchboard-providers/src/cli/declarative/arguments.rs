use serde_json::{Map, Value};
use switchboard_core::{
    Error, PlannedAction, Result, ToolArgumentSpec, ToolArgumentTransport, ToolArgumentValueKind,
};

use crate::cli::declarative::{
    gmail::build_gmail_raw_message,
    support::{
        ToolArgumentSpecCollector, ToolArgumentSpecSeed, action_values, collect_action_values, first_action_value,
        flag_enabled, render_aliases, render_key_value_argument, required_action_value, validate_aliases,
        validate_flag, validate_non_empty,
    },
};

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
                ToolArgumentSpecSeed::new(ToolArgumentTransport::Positional, ToolArgumentValueKind::String)
                    .with_required(true),
            ),
            Self::OptionalPositional { aliases } => collector.add_alias_argument(
                aliases,
                ToolArgumentSpecSeed::new(ToolArgumentTransport::Positional, ToolArgumentValueKind::String),
            ),
            Self::Json { template } => template.collect_argument_specs(collector),
            Self::Option {
                flag,
                aliases,
                repeated,
                required,
            } => collector.add_alias_argument(
                aliases,
                ToolArgumentSpecSeed::new(ToolArgumentTransport::Option, ToolArgumentValueKind::String)
                    .with_required(*required)
                    .with_repeated(*repeated)
                    .with_forwarding(Some(flag.clone()), None),
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
                ToolArgumentSpecSeed::new(
                    ToolArgumentTransport::KeyValueOption,
                    if *boolish {
                        ToolArgumentValueKind::Boolean
                    } else {
                        ToolArgumentValueKind::String
                    },
                )
                .with_required(*required)
                .with_repeated(*repeated)
                .with_forwarding(Some(flag.clone()), Some(key.clone())),
            ),
            Self::Flag { flag, aliases } => collector.add_alias_argument(
                aliases,
                ToolArgumentSpecSeed::new(ToolArgumentTransport::Flag, ToolArgumentValueKind::Boolean)
                    .with_forwarding(Some(flag.clone()), None),
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
                ToolArgumentSpecSeed::new(ToolArgumentTransport::JsonField, ToolArgumentValueKind::String)
                    .with_required(*required)
                    .with_repeated(*repeated),
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
