use clap::{Args, Subcommand};
use serde_json::Value;

use crate::{
    client::{AuthMode, RequestBody, RequestSpec, SchwabClient},
    commands::shared::{optional_bool_query, optional_query},
    ResolvedContext, Result,
};

#[derive(Clone, Debug, Args)]
pub(crate) struct MarketCommand {
    #[command(subcommand)]
    pub(crate) command: MarketSubcommand,
}

#[derive(Clone, Debug, Subcommand)]
pub(crate) enum MarketSubcommand {
    Quote(MarketQuoteArgs),
    Quotes(MarketQuotesArgs),
    Chain(MarketChainArgs),
    #[command(name = "expiration-chain")]
    ExpirationChain(MarketExpirationChainArgs),
    #[command(name = "price-history")]
    PriceHistory(MarketPriceHistoryArgs),
    Movers(MarketMoversArgs),
    Markets(MarketMarketsArgs),
    Market(MarketMarketArgs),
    Instruments(MarketInstrumentsArgs),
    Instrument(MarketInstrumentArgs),
}

#[derive(Clone, Debug, Args)]
pub(crate) struct MarketQuoteArgs {
    symbol: String,

    #[arg(long = "field", value_name = "FIELD", value_delimiter = ',')]
    fields: Vec<String>,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct MarketQuotesArgs {
    #[arg(long = "symbol", value_name = "SYMBOL", required = true, value_delimiter = ',')]
    symbols: Vec<String>,

    #[arg(long = "field", value_name = "FIELD", value_delimiter = ',')]
    fields: Vec<String>,

    #[arg(long)]
    indicative: bool,
}

#[derive(Clone, Debug, Args)]
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

#[derive(Clone, Debug, Args)]
pub(crate) struct MarketChainArgs {
    symbol: String,

    #[arg(long)]
    contract_type: Option<String>,

    #[arg(long)]
    strike_count: Option<u32>,

    #[arg(long)]
    include_underlying_quote: bool,

    #[arg(long)]
    strategy: Option<String>,

    #[arg(long)]
    interval: Option<f64>,

    #[arg(long)]
    strike: Option<f64>,

    #[arg(long)]
    range: Option<String>,

    #[arg(long)]
    from_date: Option<String>,

    #[arg(long)]
    to_date: Option<String>,

    #[arg(long)]
    volatility: Option<f64>,

    #[arg(long)]
    underlying_price: Option<f64>,

    #[arg(long)]
    interest_rate: Option<f64>,

    #[arg(long)]
    days_to_expiration: Option<u32>,

    #[arg(long)]
    exp_month: Option<String>,

    #[arg(long)]
    option_type: Option<String>,

    #[arg(long)]
    entitlement: Option<String>,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct MarketExpirationChainArgs {
    symbol: String,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct MarketMoversArgs {
    symbol: String,

    #[arg(long)]
    sort: Option<String>,

    #[arg(long)]
    frequency: Option<u32>,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct MarketMarketsArgs {
    #[arg(long = "market", value_name = "MARKET", required = true, value_delimiter = ',')]
    markets: Vec<String>,

    #[arg(long)]
    date: Option<String>,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct MarketMarketArgs {
    market: String,

    #[arg(long)]
    date: Option<String>,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct MarketInstrumentsArgs {
    symbol: String,

    #[arg(long)]
    projection: String,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct MarketInstrumentArgs {
    cusip: String,
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
                headers: context.market_headers(),
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
                headers: context.market_headers(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
        }
        MarketSubcommand::Chain(args) => {
            let mut query = vec![("symbol".into(), args.symbol)];
            optional_query(&mut query, "contractType", args.contract_type);
            optional_query(
                &mut query,
                "strikeCount",
                args.strike_count.map(|value| value.to_string()),
            );
            optional_bool_query(&mut query, "includeUnderlyingQuote", args.include_underlying_quote);
            optional_query(&mut query, "strategy", args.strategy);
            optional_query(&mut query, "interval", args.interval.map(|value| value.to_string()));
            optional_query(&mut query, "strike", args.strike.map(|value| value.to_string()));
            optional_query(&mut query, "range", args.range);
            optional_query(&mut query, "fromDate", args.from_date);
            optional_query(&mut query, "toDate", args.to_date);
            optional_query(&mut query, "volatility", args.volatility.map(|value| value.to_string()));
            optional_query(
                &mut query,
                "underlyingPrice",
                args.underlying_price.map(|value| value.to_string()),
            );
            optional_query(
                &mut query,
                "interestRate",
                args.interest_rate.map(|value| value.to_string()),
            );
            optional_query(
                &mut query,
                "daysToExpiration",
                args.days_to_expiration.map(|value| value.to_string()),
            );
            optional_query(&mut query, "expMonth", args.exp_month);
            optional_query(&mut query, "optionType", args.option_type);
            optional_query(&mut query, "entitlement", args.entitlement);

            client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path: "/chains".into(),
                query,
                headers: context.market_headers(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
        }
        MarketSubcommand::ExpirationChain(args) => client.execute(RequestSpec {
            method: reqwest::Method::GET,
            path: "/expirationchain".into(),
            query: vec![("symbol".into(), args.symbol)],
            headers: context.market_headers(),
            body: RequestBody::None,
            auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
        }),
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
                headers: context.market_headers(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
        }
        MarketSubcommand::Movers(args) => {
            let mut query = Vec::new();
            optional_query(&mut query, "sort", args.sort);
            optional_query(&mut query, "frequency", args.frequency.map(|value| value.to_string()));

            client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path: format!("/movers/{}", args.symbol),
                query,
                headers: context.market_headers(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
        }
        MarketSubcommand::Markets(args) => {
            let mut query = Vec::new();
            optional_query(&mut query, "markets", join_csv(args.markets));
            optional_query(&mut query, "date", args.date);

            client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path: "/markets".into(),
                query,
                headers: context.market_headers(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
        }
        MarketSubcommand::Market(args) => {
            let mut query = Vec::new();
            optional_query(&mut query, "date", args.date);

            client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path: format!("/markets/{}", args.market),
                query,
                headers: context.market_headers(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })
        }
        MarketSubcommand::Instruments(args) => client.execute(RequestSpec {
            method: reqwest::Method::GET,
            path: "/instruments".into(),
            query: vec![("symbol".into(), args.symbol), ("projection".into(), args.projection)],
            headers: context.market_headers(),
            body: RequestBody::None,
            auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
        }),
        MarketSubcommand::Instrument(args) => client.execute(RequestSpec {
            method: reqwest::Method::GET,
            path: format!("/instruments/{}", args.cusip),
            query: Vec::new(),
            headers: context.market_headers(),
            body: RequestBody::None,
            auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
        }),
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
