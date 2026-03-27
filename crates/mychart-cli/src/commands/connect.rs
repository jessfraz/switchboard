use std::ffi::OsString;

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::{
    client::normalize_api_base_url,
    discovery::{load_catalog, slugify, DiscoveryBrand, DiscoveryMatch},
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
    name: String,

    #[arg(long, value_name = "URL")]
    base_url: String,

    #[arg(long, value_name = "URL")]
    portal_base_url: Option<String>,

    #[arg(long)]
    client_id: Option<String>,

    #[arg(long)]
    client_secret: Option<String>,

    #[arg(long, value_name = "URL")]
    redirect_uri: Option<String>,

    #[arg(long)]
    no_use: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ConnectShowArgs {
    account: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ConnectUseArgs {
    account: String,
}

pub(crate) fn run_connect(command: ConnectSubcommand, context: &mut ResolvedContext) -> Result<Value> {
    match command {
        ConnectSubcommand::Search(args) => run_search(args, context),
        ConnectSubcommand::Add(args) => run_add(args, context),
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

fn run_add(args: ConnectAddArgs, context: &mut ResolvedContext) -> Result<Value> {
    let name = slugify(&args.name);
    let normalized_base_url = normalize_api_base_url(&args.base_url)?;
    let mut account = context
        .describe_account(Some(&name))
        .map(|(_, account)| account)
        .unwrap_or_default();
    account.api_base_url = Some(normalized_base_url);
    account.portal_base_url = args.portal_base_url.clone().or(account.portal_base_url);
    account.client_id = args.client_id.clone().or(account.client_id);
    account.client_secret = args.client_secret.clone().or(account.client_secret);
    account.redirect_uri = args.redirect_uri.clone().or(account.redirect_uri);
    context.upsert_account(name.clone(), account.clone(), !args.no_use)?;

    Ok(json!({
        "status": "connected",
        "selected_account": if args.no_use { context.active_account_name() } else { Some(name.as_str()) },
        "account": render_account(name, &account, context.active_account_name()),
        "manual": true,
    }))
}

fn run_resolve(tokens: Vec<OsString>, context: &mut ResolvedContext) -> Result<Value> {
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

    if let Some((name, account)) = context.describe_account(Some(&query)) {
        context.set_current_account(name.clone())?;
        return Ok(json!({
            "status": "selected",
            "selected_account": name,
            "account": render_account(name, &account, context.active_account_name()),
        }));
    }

    let cache_path = context.discovery_cache_path()?;
    let catalog = load_catalog(&cache_path, false)?;
    let matches = catalog.search(&query, 10);
    if matches.is_empty() {
        return Ok(json!({
            "status": "not_found",
            "query": query,
            "matches": [],
        }));
    }

    let Some(brand) = catalog.resolve_unique(&query) else {
        return Ok(json!({
            "status": "ambiguous",
            "query": query,
            "matches": matches.iter().map(render_match).collect::<Vec<_>>(),
        }));
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

    Ok(json!({
        "status": "connected",
        "query": query,
        "selected_account": account_name,
        "account": render_account(account_name, &account, context.active_account_name()),
        "match": render_brand(&brand),
    }))
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

fn render_match(candidate: &DiscoveryMatch) -> Value {
    json!({
        "score": candidate.score,
        "exact": candidate.exact,
        "brand": render_brand(&candidate.brand),
    })
}

fn render_brand(brand: &DiscoveryBrand) -> Value {
    json!({
        "brand_name": brand.brand_name,
        "account_name": brand.account_slug,
        "base_url": brand.fhir_base_url,
        "endpoint_id": brand.endpoint_id,
        "endpoint_name": brand.endpoint_name,
        "managing_organization_name": brand.managing_organization_name,
        "managing_organization_id": brand.managing_organization_id,
        "state": brand.state,
        "country": brand.country,
        "facility_count": brand.facilities.len(),
        "facilities": brand.facilities.iter().take(5).collect::<Vec<_>>(),
    })
}

fn render_account(name: String, account: &MyChartAccountState, active_account: Option<&str>) -> Value {
    json!({
        "name": name,
        "selected": active_account == Some(name.as_str()),
        "base_url": account.api_base_url,
        "portal_base_url": account.portal_base_url,
        "client_id": account.client_id,
        "patient_id": account.patient_id,
        "authenticated": account.access_token.is_some(),
        "portal_authenticated": !account.cookies.is_empty(),
        "expires_at_epoch_seconds": account.expires_at_epoch_seconds,
        "discovery": account.discovery,
    })
}
