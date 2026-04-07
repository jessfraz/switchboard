use clap::{Args, Subcommand};
use serde_json::Value;

use crate::{
    client::{AuthMode, RequestBody, RequestSpec, SchwabClient},
    commands::shared::{optional_bool_query, optional_query},
    ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct MarketCommand {
    #[command(subcommand)]
    pub(crate) command: MarketSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum MarketSubcommand {
    Quote(MarketQuoteArgs),
    Quotes(MarketQuotesArgs),
    #[command(name = "price-history")]
    PriceHistory(MarketPriceHistoryArgs),
}

#[derive(Debug, Args)]
pub(crate) struct MarketQuoteArgs {
    symbol: String,

    #[arg(long = "field", value_name = "FIELD", value_delimiter = ',')]
    fields: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct MarketQuotesArgs {
    #[arg(long = "symbol", value_name = "SYMBOL", required = true, value_delimiter = ',')]
    symbols: Vec<String>,

    #[arg(long = "field", value_name = "FIELD", value_delimiter = ',')]
    fields: Vec<String>,

    #[arg(long)]
    indicative: bool,
}

#[derive(Debug, Args)]
pub(crate) struct MarketPriceHistoryArgs {
    symbol: String,

    #[arg(long)]
    period_type: Option<String>,

    #[arg(long)]
    period: Option<u32>,

    #[arg(long)]
    frequency_type: Option<String>,

    #[arg(long)]
    frequency: Option<u32>,

    #[arg(long)]
    start_date: Option<i64>,

    #[arg(long)]
    end_date: Option<i64>,

    #[arg(long)]
    need_extended_hours_data: bool,

    #[arg(long)]
    need_previous_close: bool,
}

pub(crate) fn run_market(command: MarketSubcommand, context: &ResolvedContext) -> Result<Value> {
    let client = market_client(context)?;

    match command {
        MarketSubcommand::Quote(args) => {
            let mut query = Vec::new();
            optional_query(&mut query, "fields", join_csv(args.fields));

            client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path: format!("/{}/quotes", args.symbol),
                query,
                headers: Vec::new(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
        }
        MarketSubcommand::Quotes(args) => {
            let mut query = Vec::new();
            optional_query(&mut query, "symbols", Some(args.symbols.join(",")));
            optional_query(&mut query, "fields", join_csv(args.fields));
            optional_bool_query(&mut query, "indicative", args.indicative);

            client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path: "/quotes".into(),
                query,
                headers: Vec::new(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
        }
        MarketSubcommand::PriceHistory(args) => {
            let mut query = vec![("symbol".into(), args.symbol)];
            optional_query(&mut query, "periodType", args.period_type);
            optional_query(&mut query, "period", args.period.map(|value| value.to_string()));
            optional_query(&mut query, "frequencyType", args.frequency_type);
            optional_query(&mut query, "frequency", args.frequency.map(|value| value.to_string()));
            optional_query(&mut query, "startDate", args.start_date.map(|value| value.to_string()));
            optional_query(&mut query, "endDate", args.end_date.map(|value| value.to_string()));
            optional_bool_query(&mut query, "needExtendedHoursData", args.need_extended_hours_data);
            optional_bool_query(&mut query, "needPreviousClose", args.need_previous_close);

            client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path: "/pricehistory".into(),
                query,
                headers: Vec::new(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
        }
    }
}

fn market_client(context: &ResolvedContext) -> Result<SchwabClient> {
    SchwabClient::new(
        context.market_data_base_url.clone(),
        format!("schwab-cli/{}", env!("CARGO_PKG_VERSION")),
    )
}

fn join_csv(values: Vec<String>) -> Option<String> {
    if values.is_empty() {
        None
    } else {
        Some(values.join(","))
    }
}
