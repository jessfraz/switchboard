use std::{
    collections::BTreeMap,
    io::{self, IsTerminal, Write},
};

use clap::Args;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    client::{AuthMode, RequestBody, RequestSpec, SchwabClient},
    commands::shared::{resolve_account_id, resolve_latest_rfc3339_window},
    Error, ResolvedContext, Result,
};

const DEFAULT_SWEEP_FUND: &str = "SWVXX";
const DEFAULT_MIN_TRADE_AMOUNT: f64 = 0.01;
const DEFAULT_PENDING_TOLERANCE: f64 = 0.01;
const MUTUAL_FUND_ASSET_TYPE: &str = "MUTUAL_FUND";
const DIRECT_PLACE_WARNING: &str =
    "Schwab previewOrder rejects mutual-fund sweep orders, so sweep places approved orders directly.";

#[derive(Clone, Debug, Args)]
pub(crate) struct SweepCommand {
    #[arg(long, help = "Account number or Schwab account hash. Defaults to all accounts.")]
    account: Option<String>,

    #[arg(long, default_value = DEFAULT_SWEEP_FUND, help = "Money market fund symbol to sweep into or out of.")]
    fund: String,

    #[arg(long, default_value_t = DEFAULT_MIN_TRADE_AMOUNT, help = "Minimum dollar/share amount required before placing a sweep.")]
    min_trade_amount: f64,

    #[arg(
        long,
        default_value_t = DEFAULT_PENDING_TOLERANCE,
        help = "Tolerance used when deciding whether an existing pending sweep already covers the needed amount."
    )]
    pending_tolerance: f64,

    #[arg(
        long,
        help = "Approve and place orders without prompting.",
        conflicts_with = "plan_only"
    )]
    yes: bool,

    #[arg(long, help = "Only build the sweep plan, never prompt or place orders.")]
    plan_only: bool,
}

#[derive(Clone, Debug)]
struct SweepSettings {
    fund_symbol: String,
    min_trade_amount: f64,
    pending_tolerance: f64,
}

#[derive(Clone, Debug)]
struct SweepSnapshot {
    accounts: Vec<AccountSnapshot>,
    orders_by_account: BTreeMap<String, Vec<OrderSnapshot>>,
}

#[derive(Clone, Debug, Deserialize)]
struct AccountListEnvelope {
    #[serde(rename = "securitiesAccount")]
    securities_account: AccountSnapshot,
}

#[derive(Clone, Debug, Deserialize)]
struct AccountGetEnvelope {
    #[serde(rename = "securitiesAccount")]
    securities_account: AccountSnapshot,
}

#[derive(Clone, Debug, Deserialize)]
struct AccountSnapshot {
    #[serde(rename = "accountNumber", deserialize_with = "deserialize_string_or_number")]
    account_number: String,
    #[serde(rename = "type")]
    account_type: Option<String>,
    #[serde(rename = "currentBalances", default)]
    current_balances: Option<CurrentBalancesSnapshot>,
    #[serde(default)]
    positions: Vec<PositionSnapshot>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct CurrentBalancesSnapshot {
    #[serde(rename = "cashBalance")]
    cash_balance: Option<f64>,
    #[serde(rename = "availableFundsNonMarginableTrade")]
    available_funds_non_marginable_trade: Option<f64>,
    #[serde(rename = "marginBalance")]
    margin_balance: Option<f64>,
    #[serde(rename = "shortBalance")]
    short_balance: Option<f64>,
    #[serde(rename = "shortMarketValue")]
    short_market_value: Option<f64>,
    #[serde(rename = "shortOptionMarketValue")]
    short_option_market_value: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PositionSnapshot {
    instrument: PositionInstrumentSnapshot,
    #[serde(rename = "longQuantity")]
    long_quantity: Option<f64>,
    #[serde(rename = "shortQuantity")]
    short_quantity: Option<f64>,
    #[serde(rename = "marketValue")]
    market_value: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PositionInstrumentSnapshot {
    symbol: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct OrderSnapshot {
    #[serde(rename = "accountNumber", deserialize_with = "deserialize_string_or_number")]
    account_number: String,
    #[serde(rename = "orderId")]
    order_id: Option<u64>,
    status: Option<String>,
    #[serde(rename = "enteredTime")]
    entered_time: Option<String>,
    quantity: Option<f64>,
    #[serde(rename = "filledQuantity")]
    filled_quantity: Option<f64>,
    #[serde(rename = "remainingQuantity")]
    remaining_quantity: Option<f64>,
    #[serde(rename = "orderLegCollection", default)]
    order_leg_collection: Vec<OrderLegSnapshot>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct OrderLegSnapshot {
    instruction: Option<String>,
    quantity: Option<f64>,
    instrument: OrderInstrumentSnapshot,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct OrderInstrumentSnapshot {
    symbol: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct SweepOutput {
    status: SweepOutputStatus,
    approval_required: bool,
    summary: String,
    fund_symbol: String,
    action_count: usize,
    blocked_count: usize,
    noop_count: usize,
    warnings: Vec<String>,
    accounts: Vec<SweepAccountPlan>,
    actions: Vec<SweepActionProposal>,
    execution: Option<SweepExecutionSummary>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum SweepOutputStatus {
    Noop,
    Blocked,
    ApprovalRequired,
    Cancelled,
    Executed,
    PartialFailure,
    ExecutionFailed,
}

#[derive(Clone, Debug, Serialize)]
struct SweepAccountPlan {
    account_number: String,
    decision: SweepDecision,
    reason: String,
    state: SweepAccountState,
    pending_sweep_orders: PendingSweepOrders,
    blocking_orders: Vec<OrderSummary>,
    warnings: Vec<String>,
    action: Option<SweepActionProposal>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SweepDecision {
    BuyFund,
    SellFund,
    SkipPendingSweep,
    BlockedByOtherOrders,
    BlockedByConflictingSweepOrders,
    BlockedByShortExposure,
    BlockedMissingBalances,
    BlockedUnsupportedNegativeCash,
    NoAction,
}

#[derive(Clone, Debug, Serialize)]
struct SweepAccountState {
    account_type: Option<String>,
    is_margin_account: bool,
    is_in_margin: bool,
    cash_balance: f64,
    sweepable_cash: f64,
    margin_balance: f64,
    debt_to_cover: f64,
    fund_shares: f64,
    fund_market_value: f64,
    short_exposure_present: bool,
    open_order_count: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
struct PendingSweepOrders {
    buy_shares: f64,
    sell_shares: f64,
}

#[derive(Clone, Debug, Serialize)]
struct OrderSummary {
    order_id: Option<u64>,
    status: Option<String>,
    entered_time: Option<String>,
    symbols: Vec<String>,
    instructions: Vec<String>,
    remaining_quantity: f64,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
enum SweepInstruction {
    Buy,
    Sell,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct SweepActionProposal {
    account_number: String,
    instruction: SweepInstruction,
    symbol: String,
    quantity: f64,
    quantity_type: &'static str,
    estimated_cash_effect: f64,
    order: SweepOrderBody,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct SweepOrderBody {
    #[serde(rename = "orderType")]
    order_type: &'static str,
    session: &'static str,
    duration: &'static str,
    #[serde(rename = "orderStrategyType")]
    order_strategy_type: &'static str,
    #[serde(rename = "orderLegCollection")]
    order_leg_collection: Vec<SweepOrderLegBody>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct SweepOrderLegBody {
    instruction: SweepInstruction,
    quantity: f64,
    instrument: SweepOrderInstrumentBody,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct SweepOrderInstrumentBody {
    symbol: String,
    #[serde(rename = "assetType")]
    asset_type: &'static str,
}

#[derive(Clone, Debug, Serialize)]
struct SweepExecutionSummary {
    status: SweepOutputStatus,
    summary: String,
    results: Vec<SweepExecutionResult>,
}

#[derive(Clone, Debug, Serialize)]
struct SweepExecutionResult {
    account_number: String,
    instruction: SweepInstruction,
    symbol: String,
    quantity: f64,
    status: SweepExecutionStatus,
    receipt: Option<Value>,
    error: Option<Value>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum SweepExecutionStatus {
    Placed,
    Failed,
}

#[derive(Clone, Debug)]
struct OrderAnalysis {
    pending_sweep_buy: f64,
    pending_sweep_sell: f64,
    blocking_orders: Vec<OrderSummary>,
    conflicting_sweep_orders: Vec<OrderSummary>,
    open_order_count: usize,
}

pub(crate) fn run_sweep(args: SweepCommand, context: &mut ResolvedContext) -> Result<Value> {
    let client = trader_client(context)?;
    let settings = SweepSettings::from_args(&args)?;
    let snapshot = fetch_sweep_snapshot(&client, context, args.account.as_deref())?;
    let mut output = build_sweep_output(snapshot, &settings);

    if output.action_count == 0 || args.plan_only {
        return to_json_value(&output);
    }

    if args.yes {
        let execution = execute_sweep_actions(&client, context, &output.actions)?;
        apply_execution_summary(&mut output, execution);
        return to_json_value(&output);
    }

    if stdin_is_interactive() {
        print_human_plan(&output)?;
        if prompt_for_approval()? {
            let execution = execute_sweep_actions(&client, context, &output.actions)?;
            apply_execution_summary(&mut output, execution);
        } else {
            output.status = SweepOutputStatus::Cancelled;
            output.approval_required = false;
            output.summary = build_summary(&output.accounts, output.status);
        }
        return to_json_value(&output);
    }

    to_json_value(&output)
}

impl SweepSettings {
    fn from_args(args: &SweepCommand) -> Result<Self> {
        if !args.min_trade_amount.is_finite() || args.min_trade_amount < 0.0 {
            return Err(Error::Arguments(
                "--min-trade-amount must be a finite value >= 0".into(),
            ));
        }
        if !args.pending_tolerance.is_finite() || args.pending_tolerance < 0.0 {
            return Err(Error::Arguments(
                "--pending-tolerance must be a finite value >= 0".into(),
            ));
        }

        let fund_symbol = args.fund.trim().to_ascii_uppercase();
        if fund_symbol.is_empty() {
            return Err(Error::Arguments("fund symbol may not be empty".into()));
        }

        Ok(Self {
            fund_symbol,
            min_trade_amount: round_currency(args.min_trade_amount),
            pending_tolerance: round_currency(args.pending_tolerance),
        })
    }
}

fn fetch_sweep_snapshot(
    client: &SchwabClient,
    context: &mut ResolvedContext,
    account: Option<&str>,
) -> Result<SweepSnapshot> {
    let accounts = fetch_accounts(client, context, account)?;
    let orders = fetch_orders(client, context, account)?;
    let mut orders_by_account: BTreeMap<String, Vec<OrderSnapshot>> = BTreeMap::new();
    for order in orders {
        orders_by_account
            .entry(order.account_number.clone())
            .or_default()
            .push(order);
    }

    Ok(SweepSnapshot {
        accounts,
        orders_by_account,
    })
}

fn fetch_accounts(
    client: &SchwabClient,
    context: &mut ResolvedContext,
    account: Option<&str>,
) -> Result<Vec<AccountSnapshot>> {
    match account {
        Some(account) => {
            let account_id = resolve_account_id(client, context, account)?;
            let value = client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path: format!("/accounts/{account_id}"),
                query: vec![("fields".into(), "positions".into())],
                headers: context.trader_headers(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })?;
            let envelope: AccountGetEnvelope = serde_json::from_value(value)
                .map_err(|error| Error::Config(format!("failed to parse Schwab account payload: {error}")))?;
            Ok(vec![envelope.securities_account])
        }
        None => {
            let value = client.execute(RequestSpec {
                method: reqwest::Method::GET,
                path: "/accounts".into(),
                query: vec![("fields".into(), "positions".into())],
                headers: context.trader_headers(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })?;
            let envelopes: Vec<AccountListEnvelope> = serde_json::from_value(value)
                .map_err(|error| Error::Config(format!("failed to parse Schwab accounts payload: {error}")))?;
            Ok(envelopes
                .into_iter()
                .map(|envelope| envelope.securities_account)
                .collect())
        }
    }
}

fn fetch_orders(
    client: &SchwabClient,
    context: &mut ResolvedContext,
    account: Option<&str>,
) -> Result<Vec<OrderSnapshot>> {
    let (from_entered_time, to_entered_time) = resolve_latest_rfc3339_window(None, None)?;
    let path = match account {
        Some(account) => {
            let account_id = resolve_account_id(client, context, account)?;
            format!("/accounts/{account_id}/orders")
        }
        None => "/orders".into(),
    };
    let value = client.execute(RequestSpec {
        method: reqwest::Method::GET,
        path,
        query: vec![
            ("fromEnteredTime".into(), from_entered_time),
            ("toEnteredTime".into(), to_entered_time),
        ],
        headers: context.trader_headers(),
        body: RequestBody::None,
        auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
    })?;

    serde_json::from_value(value)
        .map_err(|error| Error::Config(format!("failed to parse Schwab orders payload: {error}")))
}

fn build_sweep_output(snapshot: SweepSnapshot, settings: &SweepSettings) -> SweepOutput {
    let mut accounts = Vec::new();
    let mut actions = Vec::new();
    let mut warnings = Vec::new();

    for account in snapshot.accounts {
        let orders = snapshot
            .orders_by_account
            .get(&account.account_number)
            .cloned()
            .unwrap_or_default();
        let plan = build_account_plan(account, orders, settings);
        if let Some(action) = plan.action.clone() {
            actions.push(action);
        }
        warnings.extend(plan.warnings.iter().cloned());
        accounts.push(plan);
    }

    dedupe_strings(&mut warnings);
    let blocked_count = accounts
        .iter()
        .filter(|plan| {
            matches!(
                plan.decision,
                SweepDecision::BlockedByOtherOrders
                    | SweepDecision::BlockedByConflictingSweepOrders
                    | SweepDecision::BlockedByShortExposure
                    | SweepDecision::BlockedMissingBalances
                    | SweepDecision::BlockedUnsupportedNegativeCash
            )
        })
        .count();
    let noop_count = accounts.len().saturating_sub(blocked_count + actions.len());
    let status = if actions.is_empty() {
        if blocked_count > 0 {
            SweepOutputStatus::Blocked
        } else {
            SweepOutputStatus::Noop
        }
    } else {
        warnings.push(DIRECT_PLACE_WARNING.to_owned());
        dedupe_strings(&mut warnings);
        SweepOutputStatus::ApprovalRequired
    };

    SweepOutput {
        status,
        approval_required: !actions.is_empty(),
        summary: build_summary(&accounts, status),
        fund_symbol: settings.fund_symbol.clone(),
        action_count: actions.len(),
        blocked_count,
        noop_count,
        warnings,
        accounts,
        actions,
        execution: None,
    }
}

fn build_account_plan(
    account: AccountSnapshot,
    orders: Vec<OrderSnapshot>,
    settings: &SweepSettings,
) -> SweepAccountPlan {
    let mut state = derive_account_state(&account, settings);
    let order_analysis = analyze_orders(&orders, &settings.fund_symbol);
    state.open_order_count = order_analysis.open_order_count;
    let mut warnings = Vec::new();
    let account_number = account.account_number.clone();

    if state.fund_shares == 0.0 && state.fund_market_value > 0.0 {
        warnings.push(format!(
            "{} has market value for {} but zero reported shares, execution may need manual review.",
            account_number, settings.fund_symbol
        ));
    }

    if state.short_exposure_present {
        return SweepAccountPlan {
            account_number,
            decision: SweepDecision::BlockedByShortExposure,
            reason: "short exposure is present, sweep will not touch this account".into(),
            state,
            pending_sweep_orders: PendingSweepOrders::from_analysis(&order_analysis),
            blocking_orders: order_analysis.blocking_orders,
            warnings,
            action: None,
        };
    }

    if account.current_balances.is_none() {
        return SweepAccountPlan {
            account_number,
            decision: SweepDecision::BlockedMissingBalances,
            reason: "Schwab did not return current balances for this account".into(),
            state,
            pending_sweep_orders: PendingSweepOrders::from_analysis(&order_analysis),
            blocking_orders: order_analysis.blocking_orders,
            warnings,
            action: None,
        };
    }

    if !order_analysis.conflicting_sweep_orders.is_empty() {
        let reason = format!(
            "conflicting pending {} orders already exist for {}",
            settings.fund_symbol, account_number
        );
        return SweepAccountPlan {
            account_number,
            decision: SweepDecision::BlockedByConflictingSweepOrders,
            reason,
            state,
            pending_sweep_orders: PendingSweepOrders::from_analysis(&order_analysis),
            blocking_orders: order_analysis.conflicting_sweep_orders,
            warnings,
            action: None,
        };
    }

    if !order_analysis.blocking_orders.is_empty() {
        return SweepAccountPlan {
            account_number,
            decision: SweepDecision::BlockedByOtherOrders,
            reason: "non-sweep open orders are pending, cash could still move".into(),
            state,
            pending_sweep_orders: PendingSweepOrders::from_analysis(&order_analysis),
            blocking_orders: order_analysis.blocking_orders,
            warnings,
            action: None,
        };
    }

    if state.debt_to_cover >= settings.min_trade_amount {
        if order_analysis.pending_sweep_buy >= settings.min_trade_amount {
            return SweepAccountPlan {
                account_number,
                decision: SweepDecision::BlockedByConflictingSweepOrders,
                reason: format!(
                    "pending {} buy orders conflict with the needed sell sweep",
                    settings.fund_symbol
                ),
                state,
                pending_sweep_orders: PendingSweepOrders::from_analysis(&order_analysis),
                blocking_orders: Vec::new(),
                warnings,
                action: None,
            };
        }

        if !state.is_margin_account {
            return SweepAccountPlan {
                account_number,
                decision: SweepDecision::BlockedUnsupportedNegativeCash,
                reason: "cash is negative but this is not a margin account, refusing to guess".into(),
                state,
                pending_sweep_orders: PendingSweepOrders::from_analysis(&order_analysis),
                blocking_orders: Vec::new(),
                warnings,
                action: None,
            };
        }

        let additional_sell = round_currency((state.debt_to_cover - order_analysis.pending_sweep_sell).max(0.0));
        if additional_sell <= settings.pending_tolerance {
            return SweepAccountPlan {
                account_number,
                decision: SweepDecision::SkipPendingSweep,
                reason: format!(
                    "pending {} sell orders already cover the margin deficit within tolerance",
                    settings.fund_symbol
                ),
                state,
                pending_sweep_orders: PendingSweepOrders::from_analysis(&order_analysis),
                blocking_orders: Vec::new(),
                warnings,
                action: None,
            };
        }

        if state.fund_shares < settings.min_trade_amount {
            warnings.push(format!(
                "{} has negative cash but no {} shares available to sell.",
                account_number, settings.fund_symbol
            ));
            return SweepAccountPlan {
                account_number,
                decision: SweepDecision::NoAction,
                reason: format!(
                    "cash is negative but there are no {} shares to sell",
                    settings.fund_symbol
                ),
                state,
                pending_sweep_orders: PendingSweepOrders::from_analysis(&order_analysis),
                blocking_orders: Vec::new(),
                warnings,
                action: None,
            };
        }

        let quantity = round_currency(additional_sell.min(state.fund_shares));
        if quantity < additional_sell {
            warnings.push(format!(
                "{} only has {:.2} {} shares, sweep can only partially cover the margin deficit.",
                account_number, quantity, settings.fund_symbol
            ));
        }
        let action = build_action(&account_number, SweepInstruction::Sell, settings, quantity);
        return SweepAccountPlan {
            account_number,
            decision: SweepDecision::SellFund,
            reason: format!("cash is negative, sell {} to reduce margin usage", settings.fund_symbol),
            state,
            pending_sweep_orders: PendingSweepOrders::from_analysis(&order_analysis),
            blocking_orders: Vec::new(),
            warnings,
            action: Some(action),
        };
    }

    if state.sweepable_cash >= settings.min_trade_amount {
        if order_analysis.pending_sweep_sell >= settings.min_trade_amount {
            return SweepAccountPlan {
                account_number,
                decision: SweepDecision::BlockedByConflictingSweepOrders,
                reason: format!(
                    "pending {} sell orders conflict with the needed buy sweep",
                    settings.fund_symbol
                ),
                state,
                pending_sweep_orders: PendingSweepOrders::from_analysis(&order_analysis),
                blocking_orders: Vec::new(),
                warnings,
                action: None,
            };
        }

        let additional_buy = round_currency((state.sweepable_cash - order_analysis.pending_sweep_buy).max(0.0));
        if additional_buy <= settings.pending_tolerance {
            return SweepAccountPlan {
                account_number,
                decision: SweepDecision::SkipPendingSweep,
                reason: format!(
                    "pending {} buy orders already cover the sweepable cash within tolerance",
                    settings.fund_symbol
                ),
                state,
                pending_sweep_orders: PendingSweepOrders::from_analysis(&order_analysis),
                blocking_orders: Vec::new(),
                warnings,
                action: None,
            };
        }

        let action = build_action(&account_number, SweepInstruction::Buy, settings, additional_buy);
        return SweepAccountPlan {
            account_number,
            decision: SweepDecision::BuyFund,
            reason: format!(
                "excess settled cash is available, sweep it into {}",
                settings.fund_symbol
            ),
            state,
            pending_sweep_orders: PendingSweepOrders::from_analysis(&order_analysis),
            blocking_orders: Vec::new(),
            warnings,
            action: Some(action),
        };
    }

    SweepAccountPlan {
        account_number,
        decision: SweepDecision::NoAction,
        reason: "no sweepable cash or margin deficit was found".into(),
        state,
        pending_sweep_orders: PendingSweepOrders::from_analysis(&order_analysis),
        blocking_orders: Vec::new(),
        warnings,
        action: None,
    }
}

fn derive_account_state(account: &AccountSnapshot, settings: &SweepSettings) -> SweepAccountState {
    let balances = account.current_balances.clone().unwrap_or_default();
    let cash_balance = round_currency(balances.cash_balance.unwrap_or(0.0));
    let available_non_marginable = balances.available_funds_non_marginable_trade.unwrap_or(cash_balance);
    let sweepable_cash = if cash_balance > 0.0 && available_non_marginable > 0.0 {
        round_currency(cash_balance.min(available_non_marginable))
    } else {
        0.0
    };
    let margin_balance = round_currency(balances.margin_balance.unwrap_or(0.0));
    let debt_to_cover = round_currency((-cash_balance).max(-margin_balance).max(0.0));
    let short_exposure_present = has_short_exposure(account, &balances, settings.min_trade_amount);

    let fund_positions = account.positions.iter().filter(|position| {
        position
            .instrument
            .symbol
            .as_deref()
            .is_some_and(|symbol| symbol.eq_ignore_ascii_case(&settings.fund_symbol))
    });
    let fund_shares = round_currency(
        fund_positions
            .clone()
            .map(|position| position.long_quantity.unwrap_or(0.0))
            .sum(),
    );
    let fund_market_value = round_currency(
        fund_positions
            .map(|position| position.market_value.unwrap_or(0.0))
            .sum(),
    );

    SweepAccountState {
        account_type: account.account_type.clone(),
        is_margin_account: account
            .account_type
            .as_deref()
            .is_some_and(|account_type| account_type.eq_ignore_ascii_case("MARGIN")),
        is_in_margin: debt_to_cover >= settings.min_trade_amount,
        cash_balance,
        sweepable_cash,
        margin_balance,
        debt_to_cover,
        fund_shares,
        fund_market_value,
        short_exposure_present,
        open_order_count: 0,
    }
}

fn has_short_exposure(account: &AccountSnapshot, balances: &CurrentBalancesSnapshot, minimum: f64) -> bool {
    if balances.short_balance.unwrap_or(0.0).abs() >= minimum
        || balances.short_market_value.unwrap_or(0.0).abs() >= minimum
        || balances.short_option_market_value.unwrap_or(0.0).abs() >= minimum
    {
        return true;
    }

    account
        .positions
        .iter()
        .any(|position| round_currency(position.short_quantity.unwrap_or(0.0)).abs() >= minimum)
}

fn analyze_orders(orders: &[OrderSnapshot], fund_symbol: &str) -> OrderAnalysis {
    let mut analysis = OrderAnalysis {
        pending_sweep_buy: 0.0,
        pending_sweep_sell: 0.0,
        blocking_orders: Vec::new(),
        conflicting_sweep_orders: Vec::new(),
        open_order_count: 0,
    };

    for order in orders
        .iter()
        .filter(|order| !is_terminal_status(order.status.as_deref()))
    {
        analysis.open_order_count += 1;
        let summary = summarize_order(order);
        match classify_order(order, fund_symbol) {
            ClassifiedOrder::Sweep(SweepInstruction::Buy) => {
                analysis.pending_sweep_buy =
                    round_currency(analysis.pending_sweep_buy + remaining_order_quantity(order));
            }
            ClassifiedOrder::Sweep(SweepInstruction::Sell) => {
                analysis.pending_sweep_sell =
                    round_currency(analysis.pending_sweep_sell + remaining_order_quantity(order));
            }
            ClassifiedOrder::ConflictingSweep => analysis.conflicting_sweep_orders.push(summary),
            ClassifiedOrder::Blocking => analysis.blocking_orders.push(summary),
        }
    }

    analysis
}

fn classify_order(order: &OrderSnapshot, fund_symbol: &str) -> ClassifiedOrder {
    if order.order_leg_collection.is_empty() {
        return ClassifiedOrder::Blocking;
    }

    let all_fund_legs = order.order_leg_collection.iter().all(|leg| {
        leg.instrument
            .symbol
            .as_deref()
            .is_some_and(|symbol| symbol.eq_ignore_ascii_case(fund_symbol))
    });
    if !all_fund_legs {
        return ClassifiedOrder::Blocking;
    }

    let mut direction = None;
    for leg in &order.order_leg_collection {
        let leg_direction = match leg.instruction.as_deref().map(str::to_ascii_uppercase).as_deref() {
            Some("BUY") => SweepInstruction::Buy,
            Some("SELL") => SweepInstruction::Sell,
            _ => return ClassifiedOrder::Blocking,
        };
        if let Some(existing) = direction {
            if existing != leg_direction {
                return ClassifiedOrder::ConflictingSweep;
            }
        } else {
            direction = Some(leg_direction);
        }
    }

    direction.map_or(ClassifiedOrder::Blocking, ClassifiedOrder::Sweep)
}

fn build_action(
    account_number: &str,
    instruction: SweepInstruction,
    settings: &SweepSettings,
    quantity: f64,
) -> SweepActionProposal {
    let order = SweepOrderBody {
        order_type: "MARKET",
        session: "NORMAL",
        duration: "DAY",
        order_strategy_type: "SINGLE",
        order_leg_collection: vec![SweepOrderLegBody {
            instruction,
            quantity,
            instrument: SweepOrderInstrumentBody {
                symbol: settings.fund_symbol.clone(),
                asset_type: MUTUAL_FUND_ASSET_TYPE,
            },
        }],
    };
    let estimated_cash_effect = match instruction {
        SweepInstruction::Buy => round_currency(-quantity),
        SweepInstruction::Sell => round_currency(quantity),
    };

    SweepActionProposal {
        account_number: account_number.to_owned(),
        instruction,
        symbol: settings.fund_symbol.clone(),
        quantity,
        quantity_type: "SHARES",
        estimated_cash_effect,
        order,
    }
}

fn execute_sweep_actions(
    client: &SchwabClient,
    context: &mut ResolvedContext,
    actions: &[SweepActionProposal],
) -> Result<SweepExecutionSummary> {
    let mut results = Vec::new();
    let mut success_count = 0usize;

    for action in actions {
        let account_id = match resolve_account_id(client, context, &action.account_number) {
            Ok(account_id) => account_id,
            Err(error) => {
                results.push(SweepExecutionResult {
                    account_number: action.account_number.clone(),
                    instruction: action.instruction,
                    symbol: action.symbol.clone(),
                    quantity: action.quantity,
                    status: SweepExecutionStatus::Failed,
                    receipt: None,
                    error: Some(render_error_value(error)),
                });
                continue;
            }
        };

        match client.execute_response(RequestSpec {
            method: reqwest::Method::POST,
            path: format!("/accounts/{account_id}/orders"),
            query: Vec::new(),
            headers: context.trader_headers(),
            body: RequestBody::Json(
                serde_json::to_value(&action.order)
                    .map_err(|error| Error::Config(format!("failed to serialize sweep order body: {error}")))?,
            ),
            auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
        }) {
            Ok(response) => {
                success_count += 1;
                results.push(SweepExecutionResult {
                    account_number: action.account_number.clone(),
                    instruction: action.instruction,
                    symbol: action.symbol.clone(),
                    quantity: action.quantity,
                    status: SweepExecutionStatus::Placed,
                    receipt: Some(response.into_output()),
                    error: None,
                });
            }
            Err(error) => {
                results.push(SweepExecutionResult {
                    account_number: action.account_number.clone(),
                    instruction: action.instruction,
                    symbol: action.symbol.clone(),
                    quantity: action.quantity,
                    status: SweepExecutionStatus::Failed,
                    receipt: None,
                    error: Some(render_error_value(error)),
                });
            }
        }
    }

    let status = if success_count == results.len() {
        SweepOutputStatus::Executed
    } else if success_count > 0 {
        SweepOutputStatus::PartialFailure
    } else {
        SweepOutputStatus::ExecutionFailed
    };
    let summary = match status {
        SweepOutputStatus::Executed => format!("Placed {} sweep order(s).", results.len()),
        SweepOutputStatus::PartialFailure => format!(
            "Placed {} sweep order(s), {} failed.",
            success_count,
            results.len().saturating_sub(success_count)
        ),
        SweepOutputStatus::ExecutionFailed => format!("Failed to place {} sweep order(s).", results.len()),
        _ => "Sweep execution finished.".into(),
    };

    Ok(SweepExecutionSummary {
        status,
        summary,
        results,
    })
}

fn apply_execution_summary(output: &mut SweepOutput, execution: SweepExecutionSummary) {
    output.status = execution.status;
    output.approval_required = false;
    output.summary = execution.summary.clone();
    output.execution = Some(execution);
}

fn build_summary(accounts: &[SweepAccountPlan], status: SweepOutputStatus) -> String {
    let buy_count = accounts
        .iter()
        .filter(|account| account.decision == SweepDecision::BuyFund)
        .count();
    let sell_count = accounts
        .iter()
        .filter(|account| account.decision == SweepDecision::SellFund)
        .count();
    let blocked_count = accounts
        .iter()
        .filter(|account| {
            matches!(
                account.decision,
                SweepDecision::BlockedByOtherOrders
                    | SweepDecision::BlockedByConflictingSweepOrders
                    | SweepDecision::BlockedByShortExposure
                    | SweepDecision::BlockedMissingBalances
                    | SweepDecision::BlockedUnsupportedNegativeCash
            )
        })
        .count();
    let skipped_count = accounts
        .iter()
        .filter(|account| matches!(account.decision, SweepDecision::SkipPendingSweep))
        .count();

    match status {
        SweepOutputStatus::ApprovalRequired => format!(
            "Sweep plan is ready, {buy_count} buy action(s), {sell_count} sell action(s), {blocked_count} blocked account(s), {skipped_count} skipped because pending sweeps already cover them."
        ),
        SweepOutputStatus::Blocked => {
            format!("Sweep is blocked for {blocked_count} account(s), no safe actions were found.")
        }
        SweepOutputStatus::Noop => "Nothing to sweep, cash and margin are already in a boringly acceptable state.".into(),
        SweepOutputStatus::Cancelled => "Sweep cancelled, no orders were placed.".into(),
        SweepOutputStatus::Executed => "Sweep orders were placed.".into(),
        SweepOutputStatus::PartialFailure => "Some sweep orders were placed, some failed.".into(),
        SweepOutputStatus::ExecutionFailed => "Sweep execution failed, no orders were placed.".into(),
    }
}

fn print_human_plan(output: &SweepOutput) -> Result<()> {
    let mut stderr = io::stderr().lock();
    writeln!(stderr, "Sweep plan").map_err(|error| Error::Io(format!("failed to write sweep plan: {error}")))?;
    writeln!(stderr, "{}", output.summary)
        .map_err(|error| Error::Io(format!("failed to write sweep summary: {error}")))?;
    for account in &output.accounts {
        writeln!(stderr, "- Account {}: {:?}", account.account_number, account.decision)
            .map_err(|error| Error::Io(format!("failed to write sweep account heading: {error}")))?;
        writeln!(stderr, "  reason: {}", account.reason)
            .map_err(|error| Error::Io(format!("failed to write sweep reason: {error}")))?;
        writeln!(
            stderr,
            "  cash: {:.2}, sweepable cash: {:.2}, margin balance: {:.2}, debt to cover: {:.2}, {} shares: {:.2}",
            account.state.cash_balance,
            account.state.sweepable_cash,
            account.state.margin_balance,
            account.state.debt_to_cover,
            output.fund_symbol,
            account.state.fund_shares,
        )
        .map_err(|error| Error::Io(format!("failed to write sweep balances: {error}")))?;
        writeln!(
            stderr,
            "  pending sweeps: buy {:.2}, sell {:.2}, blocking orders: {}",
            account.pending_sweep_orders.buy_shares,
            account.pending_sweep_orders.sell_shares,
            account.blocking_orders.len()
        )
        .map_err(|error| Error::Io(format!("failed to write sweep pending order summary: {error}")))?;
        if let Some(action) = &account.action {
            writeln!(
                stderr,
                "  action: {:?} {:.2} {} (estimated cash effect {:+.2})",
                action.instruction, action.quantity, action.symbol, action.estimated_cash_effect
            )
            .map_err(|error| Error::Io(format!("failed to write sweep action summary: {error}")))?;
        }
        for warning in &account.warnings {
            writeln!(stderr, "  warning: {warning}")
                .map_err(|error| Error::Io(format!("failed to write sweep warning: {error}")))?;
        }
    }
    for warning in &output.warnings {
        writeln!(stderr, "warning: {warning}")
            .map_err(|error| Error::Io(format!("failed to write sweep warning: {error}")))?;
    }
    stderr
        .flush()
        .map_err(|error| Error::Io(format!("failed to flush sweep plan: {error}")))?;
    Ok(())
}

fn prompt_for_approval() -> Result<bool> {
    let mut stderr = io::stderr().lock();
    write!(stderr, "Proceed with sweep? [y/N] ")
        .map_err(|error| Error::Io(format!("failed to write sweep approval prompt: {error}")))?;
    stderr
        .flush()
        .map_err(|error| Error::Io(format!("failed to flush sweep approval prompt: {error}")))?;
    drop(stderr);

    let mut input = String::new();
    let bytes_read = io::stdin()
        .read_line(&mut input)
        .map_err(|error| Error::Io(format!("failed to read sweep approval input: {error}")))?;
    if bytes_read == 0 {
        return Ok(false);
    }

    Ok(matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn stdin_is_interactive() -> bool {
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

fn render_error_value(error: Error) -> Value {
    match error {
        Error::Arguments(message) => json!({
            "kind": "arguments",
            "message": message,
        }),
        Error::Api { status_code, body } => json!({
            "kind": "api",
            "status_code": status_code,
            "body": body,
        }),
        Error::Config(message) => json!({
            "kind": "config",
            "message": message,
        }),
        Error::Http(message) => json!({
            "kind": "http",
            "message": message,
        }),
        Error::Io(message) => json!({
            "kind": "io",
            "message": message,
        }),
    }
}

fn to_json_value<T: Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|error| Error::Config(format!("failed to serialize sweep output: {error}")))
}

fn trader_client(context: &ResolvedContext) -> Result<SchwabClient> {
    SchwabClient::new(
        context.base_url.clone(),
        format!("schwab-cli/{}", env!("CARGO_PKG_VERSION")),
    )
}

fn round_currency(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn remaining_order_quantity(order: &OrderSnapshot) -> f64 {
    let remaining = order.remaining_quantity.unwrap_or_else(|| {
        let quantity = order.quantity.unwrap_or_else(|| {
            order
                .order_leg_collection
                .iter()
                .map(|leg| leg.quantity.unwrap_or(0.0))
                .sum()
        });
        (quantity - order.filled_quantity.unwrap_or(0.0)).max(0.0)
    });
    round_currency(remaining.max(0.0))
}

fn summarize_order(order: &OrderSnapshot) -> OrderSummary {
    let mut symbols = Vec::new();
    let mut instructions = Vec::new();
    for leg in &order.order_leg_collection {
        if let Some(symbol) = leg.instrument.symbol.as_ref() {
            symbols.push(symbol.clone());
        }
        if let Some(instruction) = leg.instruction.as_ref() {
            instructions.push(instruction.clone());
        }
    }
    symbols.sort();
    symbols.dedup();
    instructions.sort();
    instructions.dedup();

    OrderSummary {
        order_id: order.order_id,
        status: order.status.clone(),
        entered_time: order.entered_time.clone(),
        symbols,
        instructions,
        remaining_quantity: remaining_order_quantity(order),
    }
}

fn is_terminal_status(status: Option<&str>) -> bool {
    status
        .map(|status| status.trim().to_ascii_uppercase())
        .is_some_and(|status| {
            matches!(
                status.as_str(),
                "FILLED" | "CANCELED" | "CANCELLED" | "EXPIRED" | "REJECTED" | "REPLACED"
            )
        })
}

fn dedupe_strings(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

fn deserialize_string_or_number<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::String(string) => Ok(string),
        Value::Number(number) => Ok(number.to_string()),
        Value::Null => Ok(String::new()),
        other => Err(serde::de::Error::custom(format!(
            "expected string or number, got {other}"
        ))),
    }
}

impl PendingSweepOrders {
    fn from_analysis(analysis: &OrderAnalysis) -> Self {
        Self {
            buy_shares: analysis.pending_sweep_buy,
            sell_shares: analysis.pending_sweep_sell,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClassifiedOrder {
    Sweep(SweepInstruction),
    ConflictingSweep,
    Blocking,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_settings() -> SweepSettings {
        SweepSettings {
            fund_symbol: DEFAULT_SWEEP_FUND.into(),
            min_trade_amount: DEFAULT_MIN_TRADE_AMOUNT,
            pending_tolerance: DEFAULT_PENDING_TOLERANCE,
        }
    }

    fn margin_account(cash_balance: f64, margin_balance: f64, fund_shares: f64) -> AccountSnapshot {
        AccountSnapshot {
            account_number: "16494905".into(),
            account_type: Some("MARGIN".into()),
            current_balances: Some(CurrentBalancesSnapshot {
                cash_balance: Some(cash_balance),
                available_funds_non_marginable_trade: Some(cash_balance.max(0.0)),
                margin_balance: Some(margin_balance),
                short_balance: Some(0.0),
                short_market_value: Some(0.0),
                short_option_market_value: Some(0.0),
            }),
            positions: if fund_shares > 0.0 {
                vec![PositionSnapshot {
                    instrument: PositionInstrumentSnapshot {
                        symbol: Some(DEFAULT_SWEEP_FUND.into()),
                    },
                    long_quantity: Some(fund_shares),
                    short_quantity: Some(0.0),
                    market_value: Some(fund_shares),
                }]
            } else {
                Vec::new()
            },
        }
    }

    fn open_order(order_id: u64, status: &str, symbol: &str, instruction: &str, quantity: f64) -> OrderSnapshot {
        OrderSnapshot {
            account_number: "16494905".into(),
            order_id: Some(order_id),
            status: Some(status.into()),
            entered_time: Some("2026-04-07T15:00:00+0000".into()),
            quantity: Some(quantity),
            filled_quantity: Some(0.0),
            remaining_quantity: Some(quantity),
            order_leg_collection: vec![OrderLegSnapshot {
                instruction: Some(instruction.into()),
                quantity: Some(quantity),
                instrument: OrderInstrumentSnapshot {
                    symbol: Some(symbol.into()),
                },
            }],
        }
    }

    #[test]
    fn scenario_one_buys_excess_cash_when_nothing_is_pending() {
        let plan = build_account_plan(margin_account(2937.72, 0.0, 0.0), Vec::new(), &test_settings());

        assert_eq!(plan.decision, SweepDecision::BuyFund);
        let action = plan.action.expect("buy action should exist");
        assert_eq!(action.instruction, SweepInstruction::Buy);
        assert_eq!(action.quantity, 2937.72);
    }

    #[test]
    fn scenario_two_skips_when_pending_buy_already_covers_sweep() {
        let plan = build_account_plan(
            margin_account(2937.72, 0.0, 0.0),
            vec![open_order(1, "WORKING", DEFAULT_SWEEP_FUND, "BUY", 2937.72)],
            &test_settings(),
        );

        assert_eq!(plan.decision, SweepDecision::SkipPendingSweep);
        assert!(plan.action.is_none());
    }

    #[test]
    fn scenario_three_buys_the_difference_when_pending_buy_is_short() {
        let plan = build_account_plan(
            margin_account(2937.72, 0.0, 0.0),
            vec![open_order(1, "WORKING", DEFAULT_SWEEP_FUND, "BUY", 2000.0)],
            &test_settings(),
        );

        assert_eq!(plan.decision, SweepDecision::BuyFund);
        let action = plan.action.expect("buy action should exist");
        assert_eq!(action.quantity, 937.72);
    }

    #[test]
    fn scenario_four_sells_money_market_to_cover_margin_debt() {
        let plan = build_account_plan(margin_account(-1500.0, -1500.0, 5000.0), Vec::new(), &test_settings());

        assert_eq!(plan.decision, SweepDecision::SellFund);
        let action = plan.action.expect("sell action should exist");
        assert_eq!(action.instruction, SweepInstruction::Sell);
        assert_eq!(action.quantity, 1500.0);
    }

    #[test]
    fn scenario_four_skips_when_pending_sell_already_covers_margin_debt() {
        let plan = build_account_plan(
            margin_account(-1500.0, -1500.0, 5000.0),
            vec![open_order(1, "QUEUED", DEFAULT_SWEEP_FUND, "SELL", 1500.0)],
            &test_settings(),
        );

        assert_eq!(plan.decision, SweepDecision::SkipPendingSweep);
        assert!(plan.action.is_none());
    }

    #[test]
    fn negative_cash_without_fund_position_is_no_action_with_warning() {
        let plan = build_account_plan(margin_account(-1500.0, -1500.0, 0.0), Vec::new(), &test_settings());

        assert_eq!(plan.decision, SweepDecision::NoAction);
        assert!(plan.warnings.iter().any(|warning| warning.contains("negative cash")));
    }

    #[test]
    fn partial_fund_position_sells_only_available_shares() {
        let plan = build_account_plan(margin_account(-1500.0, -1500.0, 400.0), Vec::new(), &test_settings());

        assert_eq!(plan.decision, SweepDecision::SellFund);
        let action = plan.action.expect("sell action should exist");
        assert_eq!(action.quantity, 400.0);
        assert!(plan.warnings.iter().any(|warning| warning.contains("partially cover")));
    }

    #[test]
    fn non_fund_open_orders_block_buy_sweeps() {
        let plan = build_account_plan(
            margin_account(2937.72, 0.0, 0.0),
            vec![open_order(2, "WORKING", "TSM", "BUY", 10.0)],
            &test_settings(),
        );

        assert_eq!(plan.decision, SweepDecision::BlockedByOtherOrders);
        assert_eq!(plan.blocking_orders.len(), 1);
    }

    #[test]
    fn conflicting_fund_orders_block_the_plan() {
        let plan = build_account_plan(
            margin_account(2937.72, 0.0, 0.0),
            vec![open_order(2, "WORKING", DEFAULT_SWEEP_FUND, "SELL", 10.0)],
            &test_settings(),
        );

        assert_eq!(plan.decision, SweepDecision::BlockedByConflictingSweepOrders);
    }

    #[test]
    fn short_exposure_blocks_the_plan() {
        let mut account = margin_account(2937.72, 0.0, 0.0);
        account.current_balances = Some(CurrentBalancesSnapshot {
            short_balance: Some(5.0),
            ..account.current_balances.clone().expect("balances should exist")
        });

        let plan = build_account_plan(account, Vec::new(), &test_settings());
        assert_eq!(plan.decision, SweepDecision::BlockedByShortExposure);
    }

    #[test]
    fn uses_conservative_non_marginable_cash_when_lower_than_cash_balance() {
        let mut account = margin_account(2937.72, 0.0, 0.0);
        account.current_balances = Some(CurrentBalancesSnapshot {
            available_funds_non_marginable_trade: Some(1000.0),
            ..account.current_balances.clone().expect("balances should exist")
        });

        let plan = build_account_plan(account, Vec::new(), &test_settings());
        let action = plan.action.expect("buy action should exist");
        assert_eq!(action.quantity, 1000.0);
    }

    #[test]
    fn terminal_orders_do_not_block_the_plan() {
        let plan = build_account_plan(
            margin_account(2937.72, 0.0, 0.0),
            vec![open_order(1, "FILLED", "TSM", "BUY", 10.0)],
            &test_settings(),
        );

        assert_eq!(plan.decision, SweepDecision::BuyFund);
    }

    #[test]
    fn missing_balances_block_the_plan() {
        let plan = build_account_plan(
            AccountSnapshot {
                account_number: "16494905".into(),
                account_type: Some("MARGIN".into()),
                current_balances: None,
                positions: Vec::new(),
            },
            Vec::new(),
            &test_settings(),
        );

        assert_eq!(plan.decision, SweepDecision::BlockedMissingBalances);
    }

    #[test]
    fn build_action_uses_mutual_fund_order_body() {
        let action = build_action("16494905", SweepInstruction::Buy, &test_settings(), 250.0);

        assert_eq!(action.order.order_type, "MARKET");
        assert_eq!(action.order.order_leg_collection.len(), 1);
        assert_eq!(
            action.order.order_leg_collection[0].instrument.asset_type,
            MUTUAL_FUND_ASSET_TYPE
        );
        assert_eq!(
            action.order.order_leg_collection[0].instrument.symbol,
            DEFAULT_SWEEP_FUND
        );
    }
}
