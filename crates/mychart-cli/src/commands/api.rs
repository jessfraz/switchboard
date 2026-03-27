use std::ffi::OsString;

use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::{json, Value};

use crate::{
    api_client, ensure_json_success, fetch_capability_summary, merge_bundle_pages, normalize_operation_name,
    parse_api_resource_command, render_api_result, require_capability, resolve_id_argument, state::ResolvedContext,
    ApiResourceCapability, DynamicArgs, Error, Result,
};

#[derive(Debug, Args)]
pub(crate) struct ApiCommand {
    #[command(subcommand)]
    pub(crate) command: ApiSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ApiSubcommand {
    Capabilities(ApiCapabilitiesArgs),
    Resources(ApiResourcesArgs),
    #[command(external_subcommand)]
    Resource(Vec<OsString>),
}

#[derive(Debug, Args)]
pub(crate) struct ApiCapabilitiesArgs {
    #[arg(long)]
    raw: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ApiResourcesArgs {
    #[arg(long)]
    details: bool,

    #[arg(long, value_name = "OPERATION")]
    operation: Option<String>,
}

pub(crate) fn run_api(command: ApiSubcommand, context: &mut ResolvedContext) -> Result<Value> {
    match command {
        ApiSubcommand::Capabilities(args) => {
            let base_url = context.require_api_base_url()?;
            let client = api_client(&base_url)?;
            let response = client.fetch_capability_statement(context.client_id.as_deref())?;
            ensure_json_success(&response)?;
            let capability = crate::CapabilitySummary::from_value(response.body.clone())?;
            Ok(json!({
                "status": "ok",
                "base_url": base_url,
                "fhir_version": capability.fhir_version,
                "software": {
                    "name": capability.software_name,
                    "version": capability.software_version,
                },
                "implementation_url": capability.implementation_url,
                "authorize_endpoint": capability.authorize_url,
                "token_endpoint": capability.token_url,
                "resource_count": capability.resources.len(),
                "capability_statement": if args.raw { response.body } else { Value::Null },
            }))
        }
        ApiSubcommand::Resources(args) => {
            let base_url = context.require_api_base_url()?;
            let client = api_client(&base_url)?;
            let capability = fetch_capability_summary(&client, context.client_id.as_deref())?;
            let requested_interaction = args.operation.as_deref().map(normalize_operation_name);
            let resources = capability
                .resources
                .iter()
                .filter(|resource| {
                    requested_interaction
                        .as_deref()
                        .map(|interaction| resource.supports(interaction))
                        .unwrap_or(true)
                })
                .map(|resource| resource.render(args.details))
                .collect::<Vec<_>>();
            Ok(json!({
                "status": "ok",
                "base_url": base_url,
                "resource_count": resources.len(),
                "resources": resources,
            }))
        }
        ApiSubcommand::Resource(tokens) => run_api_resource(tokens, context),
    }
}

fn run_api_resource(tokens: Vec<OsString>, context: &mut ResolvedContext) -> Result<Value> {
    let parsed = parse_api_resource_command(tokens)?;
    let base_url = context.require_api_base_url()?;
    let access_token = context.require_access_token(None)?;
    let client = api_client(&base_url)?;
    let capability = fetch_capability_summary(&client, context.client_id.as_deref())?;
    let resource = capability.resolve_resource(&parsed.resource).ok_or_else(|| {
        Error::Arguments(format!(
            "unknown patient-facing resource {:?}, run `mychart api resources` to see what this endpoint actually supports",
            parsed.resource
        ))
    })?;

    match normalize_operation_name(&parsed.operation).as_str() {
        "read" => run_api_resource_get(&client, &resource, &access_token, parsed.args),
        "search-type" => run_api_resource_search(&client, &resource, &access_token, parsed.args),
        other => Err(Error::Arguments(format!(
            "unsupported resource operation {other:?}, use get/read or search"
        ))),
    }
}

fn run_api_resource_get(
    client: &crate::client::MyChartClient,
    resource: &ApiResourceCapability,
    access_token: &str,
    mut args: DynamicArgs,
) -> Result<Value> {
    require_capability(resource, "read", "get")?;
    let id = resolve_id_argument(&mut args)?;
    let query = args.into_query_pairs()?;
    let path = format!("{}/{}", resource.resource_type, id);
    let response = client.execute_bearer_json(Method::GET, &path, &query, access_token, None)?;
    ensure_json_success(&response)?;
    Ok(render_api_result(resource, "get", &response, response.body.clone(), 1))
}

fn run_api_resource_search(
    client: &crate::client::MyChartClient,
    resource: &ApiResourceCapability,
    access_token: &str,
    mut args: DynamicArgs,
) -> Result<Value> {
    require_capability(resource, "search-type", "search")?;
    let all_pages = args.take_flag("all-pages");
    let query = args.into_query_pairs()?;
    let response = client.execute_bearer_json(Method::GET, &resource.resource_type, &query, access_token, None)?;
    ensure_json_success(&response)?;
    let (body, pages_fetched) = if all_pages {
        merge_bundle_pages(client, &response.body, access_token)?
    } else {
        (response.body.clone(), 1)
    };
    Ok(render_api_result(resource, "search", &response, body, pages_fetched))
}
