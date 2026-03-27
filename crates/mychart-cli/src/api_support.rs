use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
};

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{ensure_json_success, parse_key_value, Error, JsonResponse, MyChartClient, Result};

pub(crate) fn fetch_capability_summary(
    client: &MyChartClient,
    epic_client_id: Option<&str>,
) -> Result<CapabilitySummary> {
    let response = client.fetch_capability_statement(epic_client_id)?;
    ensure_json_success(&response)?;
    CapabilitySummary::from_value(response.body)
}

pub(crate) fn resolve_id_argument(args: &mut DynamicArgs) -> Result<String> {
    if let Some(id) = args.take_optional_single("id")? {
        return Ok(id);
    }
    if args.positionals.len() == 1 {
        return Ok(args.positionals.remove(0));
    }
    if args.positionals.is_empty() {
        return Err(Error::Arguments(
            "missing resource id, pass it positionally or with --id".into(),
        ));
    }
    Err(Error::Arguments(
        "too many positional arguments, only the resource id is allowed here".into(),
    ))
}

pub(crate) fn merge_bundle_pages(
    client: &MyChartClient,
    first_body: &Value,
    access_token: &str,
) -> Result<(Value, usize)> {
    if first_body.get("resourceType").and_then(Value::as_str) != Some("Bundle") {
        return Ok((first_body.clone(), 1));
    }

    let mut merged = first_body.clone();
    let mut entries = merged
        .get("entry")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut pages_fetched = 1;
    let mut next_url = bundle_next_link(first_body);

    while let Some(url) = next_url {
        let response = client.execute_bearer_json_absolute(Method::GET, &url, access_token, None)?;
        ensure_json_success(&response)?;
        if response.body.get("resourceType").and_then(Value::as_str) != Some("Bundle") {
            return Err(Error::Api {
                status_code: response.status_code,
                body: json!({
                    "message": "expected a FHIR Bundle while following next links",
                    "body": response.body,
                }),
            });
        }
        if let Some(next_entries) = response.body.get("entry").and_then(Value::as_array) {
            entries.extend(next_entries.clone());
        }
        pages_fetched += 1;
        next_url = bundle_next_link(&response.body);
    }

    if let Some(object) = merged.as_object_mut() {
        object.insert("entry".into(), Value::Array(entries));
        if let Some(links) = object.get_mut("link").and_then(Value::as_array_mut) {
            links.retain(|link| link.get("relation").and_then(Value::as_str) != Some("next"));
        }
    }

    Ok((merged, pages_fetched))
}

fn bundle_next_link(body: &Value) -> Option<String> {
    body.get("link")?
        .as_array()?
        .iter()
        .find(|link| link.get("relation").and_then(Value::as_str) == Some("next"))
        .and_then(|link| link.get("url").and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

pub(crate) fn render_api_result(
    resource: &ApiResourceCapability,
    operation: &str,
    response: &JsonResponse,
    body: Value,
    pages_fetched: usize,
) -> Value {
    json!({
        "status": "ok",
        "resource": resource.resource_type,
        "cli_name": resource.cli_name,
        "operation": operation,
        "pages_fetched": pages_fetched,
        "response": {
            "status_code": response.status_code,
            "final_url": response.final_url.as_str(),
            "content_type": response.content_type,
        },
        "body": body,
    })
}

pub(crate) fn require_capability(resource: &ApiResourceCapability, interaction: &str, operation: &str) -> Result<()> {
    if resource.supports(interaction) {
        Ok(())
    } else {
        Err(Error::Arguments(format!(
            "{} does not support {} on this patient endpoint",
            resource.resource_type, operation
        )))
    }
}

pub(crate) fn normalize_operation_name(input: &str) -> String {
    match normalize_token(input).as_str() {
        "get" | "read" => "read".into(),
        "search" | "list" => "search-type".into(),
        other => other.to_owned(),
    }
}

pub(crate) fn normalize_token(input: &str) -> String {
    input
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

pub(crate) fn normalize_query_name(name: &str) -> String {
    if name.starts_with('_') {
        return name.to_owned();
    }

    match name {
        "count" => "_count".into(),
        "include" => "_include".into(),
        "rev-include" | "revinclude" => "_revinclude".into(),
        other => other.to_owned(),
    }
}

#[derive(Debug)]
pub(crate) struct ParsedApiResourceCommand {
    pub(crate) resource: String,
    pub(crate) operation: String,
    pub(crate) args: DynamicArgs,
}

pub(crate) fn parse_api_resource_command(tokens: Vec<OsString>) -> Result<ParsedApiResourceCommand> {
    let tokens = tokens
        .into_iter()
        .map(|token| token.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err(Error::Arguments(
            "missing resource name, expected something like `mychart api appointment search --patient 123`".into(),
        ));
    }
    if tokens.len() == 1 {
        return Err(Error::Arguments(
            "missing resource operation, expected get/read or search".into(),
        ));
    }
    Ok(ParsedApiResourceCommand {
        resource: tokens[0].clone(),
        operation: tokens[1].clone(),
        args: DynamicArgs::parse(&tokens[2..])?,
    })
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DynamicArgs {
    pub(crate) options: BTreeMap<String, Vec<String>>,
    pub(crate) flags: BTreeSet<String>,
    pub(crate) positionals: Vec<String>,
}

impl DynamicArgs {
    pub(crate) fn parse(tokens: &[String]) -> Result<Self> {
        let mut parsed = Self::default();
        let mut index = 0;

        while index < tokens.len() {
            let current = &tokens[index];
            if let Some(trimmed) = current.strip_prefix("--") {
                if let Some((name, value)) = trimmed.split_once('=') {
                    parsed.push_option(name, value.to_owned())?;
                    index += 1;
                    continue;
                }

                let next = tokens.get(index + 1);
                if next.is_none() || next.is_some_and(|value| value.starts_with("--")) {
                    parsed.flags.insert(trimmed.to_owned());
                    index += 1;
                } else if let Some(next) = next {
                    parsed.push_option(trimmed, next.clone())?;
                    index += 2;
                }
            } else {
                parsed.positionals.push(current.clone());
                index += 1;
            }
        }

        Ok(parsed)
    }

    fn push_option(&mut self, name: &str, value: String) -> Result<()> {
        if name.trim().is_empty() {
            return Err(Error::Arguments("option names cannot be empty".into()));
        }
        self.options.entry(name.to_owned()).or_default().push(value);
        Ok(())
    }

    pub(crate) fn take_flag(&mut self, name: &str) -> bool {
        self.flags.remove(name)
    }

    pub(crate) fn take_optional_single(&mut self, name: &str) -> Result<Option<String>> {
        match self.options.remove(name) {
            None => Ok(None),
            Some(mut values) if values.len() == 1 => Ok(values.pop()),
            Some(_) => Err(Error::Arguments(format!(
                "--{name} may only be provided once for this operation"
            ))),
        }
    }

    pub(crate) fn into_query_pairs(mut self) -> Result<Vec<(String, String)>> {
        if !self.positionals.is_empty() {
            return Err(Error::Arguments(format!(
                "unexpected positional arguments: {}",
                self.positionals.join(", ")
            )));
        }

        let mut query = Vec::new();
        if let Some(values) = self.options.remove("query") {
            for value in values {
                let (key, value) = parse_key_value(&value).map_err(Error::Arguments)?;
                query.push((key, value));
            }
        }

        for (name, values) in self.options {
            let name = normalize_query_name(&name);
            for value in values {
                query.push((name.clone(), value));
            }
        }

        for flag in self.flags {
            query.push((normalize_query_name(&flag), "true".into()));
        }

        Ok(query)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ApiResourceCapability {
    pub(crate) resource_type: String,
    pub(crate) cli_name: String,
    interactions: Vec<String>,
    pub(crate) search_params: Vec<ApiSearchParamCapability>,
    supported_profiles: Vec<String>,
}

impl ApiResourceCapability {
    pub(crate) fn supports(&self, interaction: &str) -> bool {
        self.interactions.iter().any(|candidate| candidate == interaction)
    }

    pub(crate) fn render(&self, details: bool) -> Value {
        if !details {
            return json!({
                "resource": self.resource_type,
                "cli_name": self.cli_name,
                "interactions": self.interactions,
                "search_param_count": self.search_params.len(),
            });
        }

        json!({
            "resource": self.resource_type,
            "cli_name": self.cli_name,
            "interactions": self.interactions,
            "supported_profiles": self.supported_profiles,
            "search_params": self.search_params.iter().map(ApiSearchParamCapability::render).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ApiSearchParamCapability {
    pub(crate) name: String,
    pub(crate) parameter_type: Option<String>,
    pub(crate) documentation: Option<String>,
}

impl ApiSearchParamCapability {
    fn render(&self) -> Value {
        json!({
            "name": self.name,
            "type": self.parameter_type,
            "documentation": self.documentation,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CapabilitySummary {
    pub(crate) authorize_url: Option<String>,
    pub(crate) token_url: Option<String>,
    pub(crate) register_url: Option<String>,
    pub(crate) fhir_version: Option<String>,
    pub(crate) software_name: Option<String>,
    pub(crate) software_version: Option<String>,
    pub(crate) implementation_url: Option<String>,
    pub(crate) resources: Vec<ApiResourceCapability>,
}

impl CapabilitySummary {
    pub(crate) fn from_value(value: Value) -> Result<Self> {
        let document: CapabilityDocument = serde_json::from_value(value).map_err(|error| {
            Error::Config(format!(
                "failed to parse capability statement JSON from MyChart: {error}"
            ))
        })?;
        let rest = document
            .rest
            .into_iter()
            .find(|rest| rest.mode.as_deref() == Some("server"))
            .ok_or_else(|| Error::Config("capability statement did not include a server REST block".into()))?;
        let oauth_uris = rest.security.as_ref().and_then(|security| {
            security
                .extension
                .iter()
                .find(|extension| extension.url.ends_with("oauth-uris"))
        });

        let authorize_url = oauth_uris
            .and_then(|extension| extension.extension.iter().find(|child| child.url == "authorize"))
            .and_then(|extension| extension.value_uri.clone());
        let token_url = oauth_uris
            .and_then(|extension| extension.extension.iter().find(|child| child.url == "token"))
            .and_then(|extension| extension.value_uri.clone());
        let register_url = oauth_uris
            .and_then(|extension| extension.extension.iter().find(|child| child.url == "register"))
            .and_then(|extension| extension.value_uri.clone());

        let mut resources = rest
            .resource
            .into_iter()
            .map(|resource| ApiResourceCapability {
                cli_name: cli_resource_name(&resource.resource_type),
                resource_type: resource.resource_type,
                interactions: resource
                    .interaction
                    .into_iter()
                    .map(|interaction| interaction.code)
                    .collect(),
                search_params: resource
                    .search_param
                    .into_iter()
                    .map(|search_param| ApiSearchParamCapability {
                        name: search_param.name,
                        parameter_type: search_param.parameter_type,
                        documentation: search_param.documentation,
                    })
                    .collect(),
                supported_profiles: resource.supported_profile,
            })
            .collect::<Vec<_>>();
        resources.sort_by(|left, right| left.resource_type.cmp(&right.resource_type));

        let (software_name, software_version) = match document.software {
            Some(software) => (software.name, software.version),
            None => (None, None),
        };

        Ok(Self {
            authorize_url,
            token_url,
            register_url,
            fhir_version: document.fhir_version,
            software_name,
            software_version,
            implementation_url: document.implementation.and_then(|implementation| implementation.url),
            resources,
        })
    }

    pub(crate) fn require_authorize_url(&self) -> Result<String> {
        self.authorize_url
            .clone()
            .ok_or_else(|| Error::Config("capability statement did not advertise a SMART authorize endpoint".into()))
    }

    pub(crate) fn require_token_url(&self) -> Result<String> {
        self.token_url
            .clone()
            .ok_or_else(|| Error::Config("capability statement did not advertise a SMART token endpoint".into()))
    }

    pub(crate) fn require_register_url(&self) -> Result<String> {
        self.register_url.clone().ok_or_else(|| {
            Error::Config("capability statement did not advertise a SMART dynamic client registration endpoint".into())
        })
    }

    pub(crate) fn resolve_resource(&self, token: &str) -> Option<ApiResourceCapability> {
        let normalized = normalize_token(token);
        self.resources
            .iter()
            .find(|resource| {
                normalize_token(&resource.resource_type) == normalized
                    || normalize_token(&resource.cli_name) == normalized
            })
            .cloned()
    }
}

fn cli_resource_name(resource_type: &str) -> String {
    let mut cli_name = String::new();
    for (index, character) in resource_type.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                cli_name.push('-');
            }
            cli_name.push(character.to_ascii_lowercase());
        } else {
            cli_name.push(character);
        }
    }
    cli_name
}

#[derive(Debug, Deserialize)]
struct CapabilityDocument {
    #[serde(default)]
    rest: Vec<CapabilityRest>,
    #[serde(rename = "fhirVersion")]
    fhir_version: Option<String>,
    software: Option<CapabilitySoftware>,
    implementation: Option<CapabilityImplementation>,
}

#[derive(Debug, Deserialize)]
struct CapabilitySoftware {
    name: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CapabilityImplementation {
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CapabilityRest {
    mode: Option<String>,
    security: Option<CapabilitySecurity>,
    #[serde(default)]
    resource: Vec<CapabilityResource>,
}

#[derive(Debug, Deserialize)]
struct CapabilitySecurity {
    #[serde(default)]
    extension: Vec<CapabilityExtension>,
}

#[derive(Debug, Deserialize, Clone)]
struct CapabilityExtension {
    url: String,
    #[serde(default)]
    extension: Vec<CapabilityExtension>,
    #[serde(rename = "valueUri")]
    value_uri: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CapabilityResource {
    #[serde(rename = "type")]
    resource_type: String,
    #[serde(default)]
    interaction: Vec<CapabilityInteraction>,
    #[serde(default, rename = "searchParam")]
    search_param: Vec<CapabilitySearchParam>,
    #[serde(default, rename = "supportedProfile")]
    supported_profile: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CapabilityInteraction {
    code: String,
}

#[derive(Debug, Deserialize)]
struct CapabilitySearchParam {
    name: String,
    #[serde(rename = "type")]
    parameter_type: Option<String>,
    documentation: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OAuthTokenResponse {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) token_type: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) patient: Option<String>,
    pub(crate) expires_in: Option<u64>,
}

pub(crate) fn parse_oauth_token_response(value: &Value) -> Result<OAuthTokenResponse> {
    serde_json::from_value(value.clone()).map_err(|error| Error::Auth {
        message: "MyChart returned a token response we could not parse".into(),
        details: json!({
            "error": error.to_string(),
            "body": value,
        }),
    })
}
