use std::ffi::OsString;

use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    client::normalize_api_base_url,
    discovery::{load_catalog, slugify, DiscoveryBrand, DiscoveryMatch},
    presets::{resolve_builtin_preset, BuiltinPreset},
    state::{AccountDiscoveryState, MyChartAccountState, ResolvedContext},
    Error, Result,
};

#[derive(Debug, Args)]
pub(crate) struct ConnectCommand {
    #[command(subcommand)]
    pub(crate) command: ConnectSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConnectSubcommand {
    Search(ConnectSearchArgs),
    Add(ConnectAddArgs),
    List,
    Show(ConnectShowArgs),
    Use(ConnectUseArgs),
    #[command(external_subcommand)]
    Resolve(Vec<OsString>),
}

#[derive(Debug, Args)]
pub(crate) struct ConnectSearchArgs {
    query: String,

    #[arg(long, default_value_t = 10)]
    limit: usize,

    #[arg(long)]
    refresh: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ConnectAddArgs {
    #[arg(long)]
    pub(crate) name: String,

    #[arg(long, value_name = "URL")]
    pub(crate) base_url: String,

    #[arg(long, value_name = "URL")]
    pub(crate) portal_base_url: Option<String>,

    #[arg(long)]
    pub(crate) client_id: Option<String>,

    #[arg(long)]
    pub(crate) client_secret: Option<String>,

    #[arg(long)]
    pub(crate) clear_client_secret: bool,

    #[arg(long, value_name = "URL")]
    pub(crate) redirect_uri: Option<String>,

    #[arg(long)]
    pub(crate) no_use: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ConnectShowArgs {
    account: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ConnectUseArgs {
    account: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub(crate) struct RenderedAccount {
    pub(crate) name: String,
    pub(crate) selected: bool,
    pub(crate) base_url: Option<String>,
    pub(crate) portal_base_url: Option<String>,
    pub(crate) client_id: Option<String>,
    pub(crate) patient_id: Option<String>,
    pub(crate) authenticated: bool,
    pub(crate) portal_authenticated: bool,
    pub(crate) expires_at_epoch_seconds: Option<u64>,
    pub(crate) discovery: Option<AccountDiscoveryState>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub(crate) struct DiscoveryBrandOutput {
    pub(crate) brand_name: String,
    pub(crate) account_name: String,
    pub(crate) base_url: String,
    pub(crate) endpoint_id: String,
    pub(crate) endpoint_name: Option<String>,
    pub(crate) managing_organization_name: Option<String>,
    pub(crate) managing_organization_id: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) country: Option<String>,
    pub(crate) facility_count: usize,
    pub(crate) facilities: Vec<crate::discovery::DiscoveryFacility>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub(crate) struct DiscoveryMatchOutput {
    pub(crate) score: u32,
    pub(crate) exact: bool,
    pub(crate) brand: DiscoveryBrandOutput,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub(crate) struct ConnectAddOutput {
    pub(crate) status: String,
    pub(crate) selected_account: Option<String>,
    pub(crate) account: RenderedAccount,
    pub(crate) manual: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub(crate) struct ConnectResolveOutput {
    pub(crate) status: String,
    pub(crate) query: String,
    pub(crate) selected_account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) account: Option<RenderedAccount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) r#match: Option<DiscoveryBrandOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) matches: Vec<DiscoveryMatchOutput>,
}

pub(crate) fn run_connect(command: ConnectSubcommand, context: &mut ResolvedContext) -> Result<Value> {
    match command {
        ConnectSubcommand::Search(args) => run_search(args, context),
        ConnectSubcommand::Add(args) => render_output(run_add_output(args, context)?),
        ConnectSubcommand::List => Ok(json!({
            "status": "ok",
            "selected_account": context.active_account_name(),
            "accounts": context
                .list_accounts()
                .into_iter()
                .map(|(name, account)| render_account(name, &account, context.active_account_name()))
                .collect::<Vec<_>>(),
        })),
        ConnectSubcommand::Show(args) => {
            let (name, account) = context
                .describe_account(args.account.as_deref())
                .ok_or_else(|| Error::Config("no saved MyChart account matched that name".into()))?;
            Ok(json!({
                "status": "ok",
                "selected_account": context.active_account_name(),
                "account": render_account(name, &account, context.active_account_name()),
            }))
        }
        ConnectSubcommand::Use(args) => {
            context.set_current_account(args.account.clone())?;
            let (name, account) = context
                .describe_account(Some(&args.account))
                .ok_or_else(|| Error::Config("selected MyChart account disappeared after activation".into()))?;
            Ok(json!({
                "status": "selected",
                "selected_account": name,
                "account": render_account(name, &account, context.active_account_name()),
            }))
        }
        ConnectSubcommand::Resolve(tokens) => run_resolve(tokens, context),
    }
}

fn run_search(args: ConnectSearchArgs, context: &mut ResolvedContext) -> Result<Value> {
    let cache_path = context.discovery_cache_path()?;
    let catalog = load_catalog(&cache_path, args.refresh)?;
    let matches = catalog.search(&args.query, args.limit);
    Ok(json!({
        "status": "ok",
        "query": args.query,
        "bundle_last_updated": catalog.bundle_last_updated,
        "catalog_fetched_at_epoch_seconds": catalog.fetched_at_epoch_seconds,
        "matches": matches.iter().map(render_match).collect::<Vec<_>>(),
    }))
}

pub(crate) fn run_add_output(args: ConnectAddArgs, context: &mut ResolvedContext) -> Result<ConnectAddOutput> {
    if args.client_secret.is_some() && args.clear_client_secret {
        return Err(Error::Arguments(
            "pass either --client-secret or --clear-client-secret, not both".into(),
        ));
    }
    let name = slugify(&args.name);
    let normalized_base_url = normalize_api_base_url(&args.base_url)?;
    let mut account = context
        .describe_account(Some(&name))
        .map(|(_, account)| account)
        .unwrap_or_default();
    account.api_base_url = Some(normalized_base_url);
    account.portal_base_url = args.portal_base_url.clone().or(account.portal_base_url);
    account.client_id = args.client_id.clone().or(account.client_id);
    if args.clear_client_secret {
        account.client_secret = None;
    } else {
        account.client_secret = args.client_secret.clone().or(account.client_secret);
    }
    account.redirect_uri = args.redirect_uri.clone().or(account.redirect_uri);
    context.upsert_account(name.clone(), account.clone(), !args.no_use)?;

    Ok(ConnectAddOutput {
        status: "connected".into(),
        selected_account: if args.no_use {
            context.active_account_name().map(ToOwned::to_owned)
        } else {
            Some(name.clone())
        },
        account: render_account(name, &account, context.active_account_name()),
        manual: true,
    })
}

fn run_resolve(tokens: Vec<OsString>, context: &mut ResolvedContext) -> Result<Value> {
    render_output(run_resolve_output(tokens, context)?)
}

pub(crate) fn run_resolve_output(tokens: Vec<OsString>, context: &mut ResolvedContext) -> Result<ConnectResolveOutput> {
    let query = tokens
        .into_iter()
        .map(|token| token.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned();
    if query.is_empty() {
        return Err(Error::Arguments(
            "missing provider query, try `mychart connect search ucla` or `mychart connect ucla medical center`".into(),
        ));
    }

    if let Some(preset) = resolve_builtin_preset(&query) {
        return connect_builtin_preset(context, query, preset);
    }

    if let Some((name, account)) = context.describe_account(Some(&query)) {
        context.set_current_account(name.clone())?;
        return Ok(ConnectResolveOutput {
            status: "selected".into(),
            query,
            selected_account: Some(name.clone()),
            account: Some(render_account(name, &account, context.active_account_name())),
            r#match: None,
            matches: Vec::new(),
        });
    }

    let cache_path = context.discovery_cache_path()?;
    let catalog = load_catalog(&cache_path, false)?;
    let matches = catalog.search(&query, 10);
    if matches.is_empty() {
        return Ok(ConnectResolveOutput {
            status: "not_found".into(),
            query,
            selected_account: None,
            account: None,
            r#match: None,
            matches: Vec::new(),
        });
    }

    let Some(brand) = catalog.resolve_unique(&query) else {
        return Ok(ConnectResolveOutput {
            status: "ambiguous".into(),
            query,
            selected_account: None,
            account: None,
            r#match: None,
            matches: matches.iter().map(render_match).collect(),
        });
    };

    let account_name = allocate_account_name(context, &brand);
    let mut account = context
        .describe_account(Some(&account_name))
        .map(|(_, account)| account)
        .unwrap_or_default();
    account.api_base_url = Some(brand.fhir_base_url.clone());
    account.discovery = Some(AccountDiscoveryState {
        query: Some(query.clone()),
        brand_id: Some(brand.brand_id.clone()),
        brand_name: Some(brand.brand_name.clone()),
        endpoint_id: Some(brand.endpoint_id.clone()),
        endpoint_name: brand.endpoint_name.clone(),
        managing_organization_id: brand.managing_organization_id.clone(),
        managing_organization_name: brand.managing_organization_name.clone(),
        last_synced_at_epoch_seconds: Some(catalog.fetched_at_epoch_seconds),
    });
    context.upsert_account(account_name.clone(), account.clone(), true)?;

    Ok(ConnectResolveOutput {
        status: "connected".into(),
        query,
        selected_account: Some(account_name.clone()),
        account: Some(render_account(account_name, &account, context.active_account_name())),
        r#match: Some(render_brand(&brand)),
        matches: Vec::new(),
    })
}

fn connect_builtin_preset(
    context: &mut ResolvedContext,
    query: String,
    preset: &BuiltinPreset,
) -> Result<ConnectResolveOutput> {
    let account_name = preset.account_name.to_owned();
    let mut account = context
        .describe_account(Some(preset.account_name))
        .map(|(_, account)| account)
        .unwrap_or_default();
    let auth_identity_changed = account.api_base_url.as_deref() != Some(preset.api_base_url)
        || account.client_id.as_deref() != Some(preset.client_id)
        || account.redirect_uri.as_deref() != Some(preset.redirect_uri);
    let portal_identity_changed = account.portal_base_url.as_deref() != preset.portal_base_url;

    account.api_base_url = Some(preset.api_base_url.into());
    account.portal_base_url = preset.portal_base_url.map(str::to_owned);
    account.client_id = Some(preset.client_id.into());
    account.client_secret = None;
    account.redirect_uri = Some(preset.redirect_uri.into());

    if auth_identity_changed {
        clear_api_session_fields(&mut account);
    }
    if portal_identity_changed {
        account.cookies.clear();
    }

    context.upsert_account(account_name.clone(), account.clone(), true)?;

    Ok(ConnectResolveOutput {
        status: "connected".into(),
        query,
        selected_account: Some(account_name.clone()),
        account: Some(render_account(account_name, &account, context.active_account_name())),
        r#match: None,
        matches: Vec::new(),
    })
}

fn allocate_account_name(context: &ResolvedContext, brand: &DiscoveryBrand) -> String {
    if let Some((name, _)) = context
        .list_accounts()
        .into_iter()
        .find(|(_, account)| account.api_base_url.as_deref() == Some(brand.fhir_base_url.as_str()))
    {
        return name;
    }

    let preferred = brand.account_slug.clone();
    if context.describe_account(Some(&preferred)).is_none() {
        return preferred;
    }

    for suffix in 2.. {
        let candidate = format!("{preferred}-{suffix}");
        if context.describe_account(Some(&candidate)).is_none() {
            return candidate;
        }
    }

    preferred
}

fn render_match(candidate: &DiscoveryMatch) -> DiscoveryMatchOutput {
    DiscoveryMatchOutput {
        score: candidate.score,
        exact: candidate.exact,
        brand: render_brand(&candidate.brand),
    }
}

fn render_brand(brand: &DiscoveryBrand) -> DiscoveryBrandOutput {
    DiscoveryBrandOutput {
        brand_name: brand.brand_name.clone(),
        account_name: brand.account_slug.clone(),
        base_url: brand.fhir_base_url.clone(),
        endpoint_id: brand.endpoint_id.clone(),
        endpoint_name: brand.endpoint_name.clone(),
        managing_organization_name: brand.managing_organization_name.clone(),
        managing_organization_id: brand.managing_organization_id.clone(),
        state: brand.state.clone(),
        country: brand.country.clone(),
        facility_count: brand.facilities.len(),
        facilities: brand.facilities.iter().take(5).cloned().collect(),
    }
}

fn render_account(name: String, account: &MyChartAccountState, active_account: Option<&str>) -> RenderedAccount {
    RenderedAccount {
        selected: active_account == Some(name.as_str()),
        name,
        base_url: account.api_base_url.clone(),
        portal_base_url: account.portal_base_url.clone(),
        client_id: account.client_id.clone(),
        patient_id: account.patient_id.clone(),
        authenticated: account.access_token.is_some(),
        portal_authenticated: !account.cookies.is_empty(),
        expires_at_epoch_seconds: account.expires_at_epoch_seconds,
        discovery: account.discovery.clone(),
    }
}

fn clear_api_session_fields(account: &mut MyChartAccountState) {
    account.client_secret = None;
    account.dynamic_client = None;
    account.access_token = None;
    account.refresh_token = None;
    account.token_type = None;
    account.scope = None;
    account.patient_id = None;
    account.expires_at_epoch_seconds = None;
    account.pending_oauth_state = None;
    account.pending_code_verifier = None;
}

fn render_output<T>(output: T) -> Result<Value>
where
    T: Serialize,
{
    serde_json::to_value(output)
        .map_err(|error| Error::Config(format!("failed to serialize MyChart connect output: {error}")))
}
