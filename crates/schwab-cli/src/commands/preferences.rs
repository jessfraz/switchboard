use clap::{Args, Subcommand};
use serde_json::Value;

use crate::{
    client::{AuthMode, RequestBody, RequestSpec, SchwabClient},
    ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct PreferenceCommand {
    #[command(subcommand)]
    pub(crate) command: PreferenceSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PreferenceSubcommand {
    Get,
}

pub(crate) fn run_preferences(command: PreferenceSubcommand, context: &ResolvedContext) -> Result<Value> {
    let client = trader_client(context)?;

    match command {
        PreferenceSubcommand::Get => client.execute(RequestSpec {
            method: reqwest::Method::GET,
            path: "/userPreference".into(),
            query: Vec::new(),
            body: RequestBody::None,
            auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
        }),
    }
}

fn trader_client(context: &ResolvedContext) -> Result<SchwabClient> {
    SchwabClient::new(
        context.base_url.clone(),
        format!("schwab-cli/{}", env!("CARGO_PKG_VERSION")),
    )
}
