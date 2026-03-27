use serde_json::{Map, Value};
use switchboard_core::{Error, PlannedAction, Result, ToolArgumentSpec, ToolArgumentTransport, ToolArgumentValueKind};

pub(super) fn required_action_value(action: &PlannedAction, aliases: &[String]) -> Result<String> {
    first_action_value(action, aliases).ok_or_else(|| {
        Error::InvalidArguments(format!(
            "missing required argument {} for {}",
            render_aliases(aliases),
            action.tool
        ))
    })
}

pub(super) fn first_action_value(action: &PlannedAction, aliases: &[String]) -> Option<String> {
    aliases
        .iter()
        .find_map(|alias| action.args.value(alias).map(ToOwned::to_owned))
}

pub(super) fn collect_action_values(action: &PlannedAction, aliases: &[String]) -> Vec<String> {
    aliases
        .iter()
        .find_map(|alias| {
            let values = action.args.values(alias).map(ToOwned::to_owned).collect::<Vec<_>>();
            (!values.is_empty()).then_some(values)
        })
        .unwrap_or_default()
}

pub(super) fn collect_action_values_for_name(action: &PlannedAction, name: &str) -> Vec<String> {
    action.args.values(name).map(ToOwned::to_owned).collect()
}

pub(super) fn action_values(action: &PlannedAction, aliases: &[String], include_true_flags: bool) -> Vec<String> {
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

pub(super) fn flag_enabled(action: &PlannedAction, aliases: &[String]) -> Result<bool> {
    if aliases.iter().any(|alias| action.args.has_flag(alias)) {
        return Ok(true);
    }

    match first_action_value(action, aliases) {
        Some(value) => parse_boolish(aliases, &value),
        None => Ok(false),
    }
}

pub(super) fn render_aliases(aliases: &[String]) -> String {
    aliases
        .iter()
        .map(|alias| format!("--{alias}"))
        .collect::<Vec<_>>()
        .join(" or ")
}

pub(super) fn validate_aliases(context: &str, aliases: Vec<String>) -> Result<Vec<String>> {
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

pub(super) fn validate_flag(flag: String) -> Result<String> {
    validate_non_empty("cli flag", &flag)?;
    Ok(flag)
}

pub(super) fn validate_non_empty(context: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::Config(format!("{context} cannot be empty")));
    }

    Ok(())
}

pub(super) fn parse_boolish(aliases: &[String], value: &str) -> Result<bool> {
    match value {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(Error::InvalidArguments(format!(
            "{} expects a boolean value, got {value:?}",
            render_aliases(aliases)
        ))),
    }
}

pub(super) fn render_key_value_argument(key: &str, value: &str, boolish: bool, aliases: &[String]) -> Result<String> {
    let rendered = if boolish {
        parse_boolish(aliases, value)?.to_string()
    } else {
        value.to_owned()
    };

    Ok(format!("{key}={rendered}"))
}

pub(super) fn extract_field_string(object: &Map<String, Value>, field: &str) -> Option<String> {
    match object.get(field)? {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

#[derive(Default)]
pub(super) struct ToolArgumentSpecCollector {
    specs: Vec<ToolArgumentSpec>,
}

pub(super) struct ToolArgumentSpecSeed {
    transport: ToolArgumentTransport,
    value_kind: ToolArgumentValueKind,
    required: bool,
    repeated: bool,
    forwarded_flag: Option<String>,
    forwarded_key: Option<String>,
}

impl ToolArgumentSpecSeed {
    pub(super) fn new(transport: ToolArgumentTransport, value_kind: ToolArgumentValueKind) -> Self {
        Self {
            transport,
            value_kind,
            required: false,
            repeated: false,
            forwarded_flag: None,
            forwarded_key: None,
        }
    }

    pub(super) fn with_required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    pub(super) fn with_repeated(mut self, repeated: bool) -> Self {
        self.repeated = repeated;
        self
    }

    pub(super) fn with_forwarding(mut self, forwarded_flag: Option<String>, forwarded_key: Option<String>) -> Self {
        self.forwarded_flag = forwarded_flag;
        self.forwarded_key = forwarded_key;
        self
    }
}

impl ToolArgumentSpecCollector {
    pub(super) fn add_alias_argument(&mut self, aliases: &[String], seed: ToolArgumentSpecSeed) -> Result<()> {
        let primary = aliases
            .first()
            .cloned()
            .ok_or_else(|| Error::Config("tool argument metadata requires at least one alias".into()))?;
        let spec = ToolArgumentSpec::new(primary, seed.transport, seed.value_kind)?
            .with_aliases(aliases.iter().skip(1).cloned().collect())?
            .with_required(seed.required)
            .with_repeated(seed.repeated)
            .with_forwarding(seed.forwarded_flag, seed.forwarded_key)?;
        self.add_spec(spec)
    }

    pub(super) fn add_spec(&mut self, spec: ToolArgumentSpec) -> Result<()> {
        if let Some(existing) = self.specs.iter_mut().find(|existing| existing.name == spec.name) {
            existing.merge_with(&spec)?;
        } else {
            self.specs.push(spec);
        }
        Ok(())
    }

    pub(super) fn finish(mut self) -> Result<Vec<ToolArgumentSpec>> {
        self.specs.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(self.specs)
    }
}
