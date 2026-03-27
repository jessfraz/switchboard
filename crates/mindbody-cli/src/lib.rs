mod client;
mod state;

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result as AnyhowResult};
use clap::{Args, Parser, Subcommand, ValueEnum};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{
    client::{Credentials, MindbodyClient, RequestBody, RequestSpec},
    state::{
        validate_unique_user_id, ResolvedContext, ENV_MINDBODY_API_KEY, ENV_MINDBODY_APP_NAME, ENV_MINDBODY_BASE_URL,
        ENV_MINDBODY_CLIENT_KEY, ENV_MINDBODY_CLIENT_SECRET, ENV_MINDBODY_CONFIG, ENV_MINDBODY_USER_ID,
    },
};

const AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  mindbody locations search --search-text pilates --address '90210'\n",
    "  mindbody classes list --location-id 86784 --available-for-booking true --start-date-time 2026-03-27T00:00:00Z\n",
    "  mindbody pricing class --location-id 86784 5134512\n",
    "  mindbody bookings create --location-id 86784 --class-id 5134512 \\\n",
    "    --reconciliation-type pass --reconciliation-id 598a6916-7876-406e-9537-db6af825f9a2\n",
    "  mindbody bookings create --location-id 86784 --class-id 5134512 \\\n",
    "    --reconciliation-type pricing-option --reconciliation-id 153458 --pricing-option-total 15.75 \\\n",
    "    --user-first Joseph --user-last Smith --user-email joseph@example.com --payment-file payment.json\n",
    "\n",
    "This CLI is aimed at booking Pilates classes and the surrounding Mindbody member account chaos,\n",
    "with a switchboard-friendly command grammar instead of raw endpoint confetti.\n",
);

pub fn main_entry<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return ExitCode::FAILURE;
        }
    };
    let compact = cli.global.compact;

    match run(cli) {
        Ok((output, compact)) => {
            println!("{}", render_json(&output, compact));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", render_cli_error(&error, compact));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> AnyhowResult<(Value, bool)> {
    let compact = cli.global.compact;
    let context = ResolvedContext::from_global(&cli.global).context("failed to resolve Mindbody runtime context")?;
    let client = MindbodyClient::new(context.base_url.clone(), context.app_name.clone())
        .context("failed to build Mindbody client")?;

    let output = match cli.command {
        Commands::Account(command) => run_account(command.command, &context),
        Commands::Locations(command) => run_locations(command.command, &client, &context),
        Commands::Classes(command) => run_classes(command.command, &client, &context),
        Commands::Pricing(command) => run_pricing(command.command, &client, &context),
        Commands::Bookings(command) => run_bookings(command.command, &client, &context),
        Commands::Passes(command) => run_passes(command.command, &client, &context),
    }
    .context("Mindbody command failed")?;

    Ok((output, compact))
}

#[derive(Debug, Parser)]
#[command(
    name = "mindbody",
    version,
    about = "CLI for booking Pilates classes and handling Mindbody member account workflows",
    disable_help_subcommand = true,
    after_help = AFTER_HELP
)]
struct Cli {
    #[command(flatten)]
    global: GlobalArgs,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Args)]
pub(crate) struct GlobalArgs {
    #[arg(long, global = true, env = ENV_MINDBODY_CONFIG, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, global = true, env = ENV_MINDBODY_BASE_URL, value_name = "URL")]
    base_url: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_API_KEY, value_name = "API_KEY")]
    api_key: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_CLIENT_KEY, value_name = "CLIENT_KEY")]
    client_key: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_CLIENT_SECRET, value_name = "CLIENT_SECRET")]
    client_secret: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_USER_ID, value_name = "USER_ID")]
    user_id: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_APP_NAME, value_name = "NAME")]
    app_name: Option<String>,

    #[arg(long, global = true)]
    compact: bool,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Account(AccountCommand),
    Locations(LocationCommand),
    Classes(ClassCommand),
    Pricing(PricingCommand),
    Bookings(BookingCommand),
    Passes(PassCommand),
}

#[derive(Debug, Args)]
struct AccountCommand {
    #[command(subcommand)]
    command: AccountSubcommand,
}

#[derive(Debug, Subcommand)]
enum AccountSubcommand {
    Status,
}

#[derive(Debug, Args)]
struct LocationCommand {
    #[command(subcommand)]
    command: LocationSubcommand,
}

#[derive(Debug, Subcommand)]
enum LocationSubcommand {
    Search(SearchLocationsArgs),
    Get(LocationIdArgs),
}

#[derive(Debug, Args)]
struct ClassCommand {
    #[command(subcommand)]
    command: ClassSubcommand,
}

#[derive(Debug, Subcommand)]
enum ClassSubcommand {
    List(ListClassesArgs),
    Get(GetClassArgs),
}

#[derive(Debug, Args)]
struct PricingCommand {
    #[command(subcommand)]
    command: PricingSubcommand,
}

#[derive(Debug, Subcommand)]
enum PricingSubcommand {
    Class(ClassPricingArgs),
    Location(LocationPricingArgs),
}

#[derive(Debug, Args)]
struct BookingCommand {
    #[command(subcommand)]
    command: BookingSubcommand,
}

#[derive(Debug, Subcommand)]
enum BookingSubcommand {
    List(ListBookingsArgs),
    Get(BookingIdArgs),
    Create(CreateBookingArgs),
    Cancel(CancelBookingArgs),
}

#[derive(Debug, Args)]
struct PassCommand {
    #[command(subcommand)]
    command: PassSubcommand,
}

#[derive(Debug, Subcommand)]
enum PassSubcommand {
    List(ListPassesArgs),
    Get(PassIdArgs),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    fn as_api_value(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum ReconciliationType {
    #[value(name = "pass")]
    Pass,
    #[value(name = "pricing-option")]
    PricingOption,
}

impl ReconciliationType {
    fn as_api_value(self) -> &'static str {
        match self {
            Self::Pass => "Pass",
            Self::PricingOption => "PricingOption",
        }
    }
}

#[derive(Clone, Debug, Args)]
struct WindowedQueryArgs {
    #[arg(long = "max-results")]
    max_results: Option<u32>,

    #[arg(long)]
    offset: Option<u32>,
}

#[derive(Clone, Debug, Args)]
struct OrderingArgs {
    #[arg(long = "order-by")]
    order_by: Option<String>,

    #[arg(long, value_enum)]
    order: Option<SortDirection>,
}

#[derive(Debug, Args)]
struct SearchLocationsArgs {
    #[arg(long)]
    address: Option<String>,

    #[arg(long)]
    latitude: Option<f64>,

    #[arg(long)]
    longitude: Option<f64>,

    #[arg(long)]
    radius: Option<f64>,

    #[arg(long = "location-id")]
    location_id: Option<u64>,

    #[arg(long = "subscriber-id")]
    subscriber_id: Option<u64>,

    #[arg(long = "country-code")]
    country_code: Option<String>,

    #[arg(long = "search-text")]
    search_text: Option<String>,

    #[command(flatten)]
    window: WindowedQueryArgs,

    #[command(flatten)]
    ordering: OrderingArgs,
}

#[derive(Debug, Args)]
struct LocationIdArgs {
    #[arg(value_name = "LOCATION_ID")]
    location_id: u64,
}

#[derive(Debug, Args)]
struct ListClassesArgs {
    #[arg(long = "location-id")]
    location_id: u64,

    #[arg(long = "start-date-time")]
    start_date_time: Option<String>,

    #[arg(long = "end-date-time")]
    end_date_time: Option<String>,

    #[arg(long = "class-type-id")]
    class_type_id: Option<u64>,

    #[arg(long = "staff-last-name")]
    staff_last_name: Option<String>,

    #[arg(long = "available-for-booking", value_parser = clap::builder::BoolishValueParser::new())]
    available_for_booking: Option<bool>,

    #[arg(long = "service-category-id")]
    service_category_ids: Vec<u64>,

    #[command(flatten)]
    window: WindowedQueryArgs,

    #[command(flatten)]
    ordering: OrderingArgs,
}

#[derive(Debug, Args)]
struct GetClassArgs {
    #[arg(long = "location-id")]
    location_id: u64,

    #[arg(value_name = "CLASS_ID")]
    class_id: u64,
}

#[derive(Debug, Args)]
struct ClassPricingArgs {
    #[arg(long = "location-id")]
    location_id: u64,

    #[arg(value_name = "CLASS_ID")]
    class_id: u64,
}

#[derive(Debug, Args)]
struct LocationPricingArgs {
    #[arg(value_name = "LOCATION_ID")]
    location_id: u64,

    #[arg(long = "service-category-id")]
    service_category_ids: Vec<u64>,
}

#[derive(Debug, Args)]
struct ListBookingsArgs {
    #[arg(long = "location-id")]
    location_id: Option<u64>,

    #[arg(long = "subscriber-id")]
    subscriber_id: Option<u64>,

    #[command(flatten)]
    window: WindowedQueryArgs,

    #[command(flatten)]
    ordering: OrderingArgs,
}

#[derive(Debug, Args)]
struct BookingIdArgs {
    #[arg(value_name = "BOOKING_ID")]
    booking_id: String,
}

#[derive(Debug, Args)]
struct CancelBookingArgs {
    #[arg(value_name = "BOOKING_ID")]
    booking_id: String,

    #[arg(long = "suppress-cancellation-confirmation-email")]
    suppress_cancellation_confirmation_email: bool,
}

#[derive(Debug, Args)]
struct CreateBookingArgs {
    #[arg(long = "location-id")]
    location_id: u64,

    #[arg(long = "class-id")]
    class_id: u64,

    #[arg(long = "reconciliation-type", value_enum)]
    reconciliation_type: ReconciliationType,

    #[arg(long = "reconciliation-id")]
    reconciliation_id: String,

    #[arg(long = "pricing-option-total")]
    pricing_option_total: Option<f64>,

    #[arg(long = "suppress-booking-confirmation-email")]
    suppress_booking_confirmation_email: bool,

    #[arg(long = "suppress-purchase-receipt-email")]
    suppress_purchase_receipt_email: bool,

    #[arg(long = "subscriber-marketing-opt-in")]
    subscriber_marketing_opt_in: bool,

    #[arg(long = "user-first")]
    user_first: Option<String>,

    #[arg(long = "user-last")]
    user_last: Option<String>,

    #[arg(long = "user-email")]
    user_email: Option<String>,

    #[arg(long = "user-phone")]
    user_phone: Option<String>,

    #[arg(long = "payment-file", value_name = "PATH")]
    payment_file: Option<PathBuf>,

    #[arg(long = "idempotency-key")]
    idempotency_key: Option<String>,
}

impl CreateBookingArgs {
    fn validate(&self, user_id: &str) -> Result<()> {
        validate_unique_user_id(user_id)?;

        if let Some(idempotency_key) = self.idempotency_key.as_ref() {
            if idempotency_key.len() != 36 {
                return Err(Error::Arguments(
                    "Mindbody idempotency keys must be GUID strings with length 36".into(),
                ));
            }
        }

        match self.reconciliation_type {
            ReconciliationType::Pass => {
                if self.pricing_option_total.is_some()
                    || self.user_first.is_some()
                    || self.user_last.is_some()
                    || self.user_email.is_some()
                    || self.payment_file.is_some()
                    || self.suppress_purchase_receipt_email
                    || self.subscriber_marketing_opt_in
                {
                    return Err(Error::Arguments(
                        "pass bookings do not take pricing-option purchase fields".into(),
                    ));
                }
            }
            ReconciliationType::PricingOption => {
                if self.pricing_option_total.is_none() {
                    return Err(Error::Arguments(
                        "pricing-option bookings require --pricing-option-total".into(),
                    ));
                }
                if self.user_first.is_none() || self.user_last.is_none() || self.user_email.is_none() {
                    return Err(Error::Arguments(
                        "pricing-option bookings require --user-first, --user-last, and --user-email".into(),
                    ));
                }
                if self.payment_file.is_none() {
                    return Err(Error::Arguments(
                        "pricing-option bookings require --payment-file so card data is not shoved into argv".into(),
                    ));
                }
            }
        }

        Ok(())
    }

    fn build_body(&self, user_id: &str) -> Result<Value> {
        self.validate(user_id)?;

        let mut reconciliation = Map::new();
        reconciliation.insert("id".into(), json!(self.reconciliation_id));
        reconciliation.insert("type".into(), json!(self.reconciliation_type.as_api_value()));
        reconciliation.insert(
            "pricingOptionTotal".into(),
            match self.reconciliation_type {
                ReconciliationType::Pass => Value::Null,
                ReconciliationType::PricingOption => json!(self.pricing_option_total),
            },
        );

        let mut body = Map::new();
        body.insert("locationId".into(), json!(self.location_id));
        body.insert("classId".into(), json!(self.class_id));
        body.insert("bookingReconciliation".into(), Value::Object(reconciliation));
        body.insert("uniqueUserId".into(), json!(user_id));

        if self.suppress_booking_confirmation_email {
            body.insert("suppressBookingConfirmationEmail".into(), Value::Bool(true));
        }

        match self.reconciliation_type {
            ReconciliationType::Pass => {
                body.insert("paymentDetails".into(), Value::Null);
            }
            ReconciliationType::PricingOption => {
                if self.suppress_purchase_receipt_email {
                    body.insert("suppressPurchaseReceiptEmail".into(), Value::Bool(true));
                }
                if self.subscriber_marketing_opt_in {
                    body.insert("subscriberMarketingOptIn".into(), Value::Bool(true));
                }
                if let Some(user_first) = self.user_first.as_ref() {
                    body.insert("userFirst".into(), json!(user_first));
                }
                if let Some(user_last) = self.user_last.as_ref() {
                    body.insert("userLast".into(), json!(user_last));
                }
                if let Some(user_email) = self.user_email.as_ref() {
                    body.insert("userEmail".into(), json!(user_email));
                }
                if let Some(user_phone) = self.user_phone.as_ref() {
                    body.insert("userPhone".into(), json!(user_phone));
                }
                let payment_file = self
                    .payment_file
                    .as_deref()
                    .ok_or_else(|| Error::Arguments("pricing-option bookings require --payment-file".into()))?;
                body.insert(
                    "paymentDetails".into(),
                    read_payment_details(payment_file)?.into_value(),
                );
            }
        }

        Ok(Value::Object(body))
    }
}

#[derive(Debug, Args)]
struct ListPassesArgs {
    #[arg(long = "subscriber-id")]
    subscriber_id: Option<u64>,

    #[arg(long = "limit-to-usable", value_parser = clap::builder::BoolishValueParser::new())]
    limit_to_usable: Option<bool>,

    #[command(flatten)]
    window: WindowedQueryArgs,

    #[command(flatten)]
    ordering: OrderingArgs,
}

#[derive(Debug, Args)]
struct PassIdArgs {
    #[arg(value_name = "PASS_ID")]
    pass_id: String,
}

#[derive(Debug, Deserialize)]
struct PaymentDetailsInput {
    #[serde(alias = "creditCardNumber")]
    credit_card_number: String,
    #[serde(alias = "creditCardExpirationYear")]
    credit_card_expiration_year: u16,
    #[serde(alias = "creditCardExpirationMonth")]
    credit_card_expiration_month: u8,
    #[serde(alias = "creditCardCvv", alias = "creditCardCVV")]
    credit_card_cvv: Option<String>,
    #[serde(alias = "billingName")]
    billing_name: String,
    #[serde(alias = "billingAddressLine1")]
    billing_address_line_1: String,
    #[serde(alias = "billingAddressLine2")]
    billing_address_line_2: Option<String>,
    #[serde(alias = "billingCity")]
    billing_city: String,
    #[serde(alias = "billingState")]
    billing_state: String,
    #[serde(alias = "billingPostalCode")]
    billing_postal_code: String,
}

impl PaymentDetailsInput {
    fn into_value(self) -> Value {
        let mut value = Map::new();
        value.insert("creditCardNumber".into(), json!(self.credit_card_number));
        value.insert(
            "creditCardExpirationYear".into(),
            json!(self.credit_card_expiration_year),
        );
        value.insert(
            "creditCardExpirationMonth".into(),
            json!(self.credit_card_expiration_month),
        );
        if let Some(credit_card_cvv) = self.credit_card_cvv {
            value.insert("creditCardCvv".into(), json!(credit_card_cvv));
        }
        value.insert("billingName".into(), json!(self.billing_name));
        value.insert("billingAddressLine1".into(), json!(self.billing_address_line_1));
        if let Some(billing_address_line_2) = self.billing_address_line_2 {
            value.insert("billingAddressLine2".into(), json!(billing_address_line_2));
        }
        value.insert("billingCity".into(), json!(self.billing_city));
        value.insert("billingState".into(), json!(self.billing_state));
        value.insert("billingPostalCode".into(), json!(self.billing_postal_code));
        Value::Object(value)
    }
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("invalid arguments: {0}")]
    Arguments(String),
    #[error("Mindbody API returned HTTP {status_code}")]
    Api { status_code: u16, body: Value },
    #[error("config error: {0}")]
    Config(String),
    #[error("HTTP failure: {0}")]
    Http(String),
    #[error("I/O failure: {0}")]
    Io(String),
}

impl Error {
    fn render(&self, compact: bool) -> String {
        let value = match self {
            Self::Arguments(message) => json!({
                "status": "error",
                "kind": "arguments",
                "message": message,
            }),
            Self::Api { status_code, body } => json!({
                "status": "error",
                "kind": "api",
                "status_code": status_code,
                "body": body,
            }),
            Self::Config(message) => json!({
                "status": "error",
                "kind": "config",
                "message": message,
            }),
            Self::Http(message) => json!({
                "status": "error",
                "kind": "http",
                "message": message,
            }),
            Self::Io(message) => json!({
                "status": "error",
                "kind": "io",
                "message": message,
            }),
        };

        render_json(&value, compact)
    }
}

type Result<T> = std::result::Result<T, Error>;

fn render_cli_error(error: &anyhow::Error, compact: bool) -> String {
    if let Some(error) = error.chain().find_map(|cause| cause.downcast_ref::<Error>()) {
        return error.render(compact);
    }

    render_json(
        &json!({
            "status": "error",
            "kind": "internal",
            "message": format!("{error:#}"),
        }),
        compact,
    )
}

fn run_account(command: AccountSubcommand, context: &ResolvedContext) -> Result<Value> {
    match command {
        AccountSubcommand::Status => {
            if let Some(user_id) = context.user_id.as_deref() {
                validate_unique_user_id(user_id)?;
            }

            Ok(json!({
                "status": "ok",
                "provider": "mindbody",
                "base_url": context.base_url,
                "app_name": context.app_name,
                "user_id": context.user_id,
                "has_api_key": context.api_key.is_some(),
                "has_client_key": context.client_key.is_some(),
                "has_client_secret": context.client_secret.is_some(),
            }))
        }
    }
}

fn run_locations(command: LocationSubcommand, client: &MindbodyClient, context: &ResolvedContext) -> Result<Value> {
    match command {
        LocationSubcommand::Search(args) => {
            validate_coordinate_pair(args.latitude, args.longitude, "locations search")?;
            let mut query = Vec::new();
            push_optional_query_string(&mut query, "address", args.address);
            push_optional_query_f64(&mut query, "latitude", args.latitude);
            push_optional_query_f64(&mut query, "longitude", args.longitude);
            push_optional_query_f64(&mut query, "radius", args.radius);
            push_optional_query_u64(&mut query, "locationId", args.location_id);
            push_optional_query_u64(&mut query, "subscriberId", args.subscriber_id);
            push_optional_query_string(&mut query, "countryCode", args.country_code);
            push_optional_query_string(&mut query, "searchText", args.search_text);
            push_window_query(&mut query, &args.window);
            push_ordering_query(&mut query, &args.ordering);
            execute(client, context, Method::GET, "/locations", query)
        }
        LocationSubcommand::Get(args) => execute(
            client,
            context,
            Method::GET,
            &format!("/locations/{}", args.location_id),
            Vec::new(),
        ),
    }
}

fn run_classes(command: ClassSubcommand, client: &MindbodyClient, context: &ResolvedContext) -> Result<Value> {
    match command {
        ClassSubcommand::List(args) => {
            let mut query = Vec::new();
            push_optional_query_string(&mut query, "startDateTime", args.start_date_time);
            push_optional_query_string(&mut query, "endDateTime", args.end_date_time);
            push_optional_query_u64(&mut query, "classTypeId", args.class_type_id);
            push_optional_query_string(&mut query, "staffLastName", args.staff_last_name);
            push_optional_query_bool(&mut query, "availableForBooking", args.available_for_booking);
            push_query_csv_u64(&mut query, "serviceCategoryIds", args.service_category_ids);
            push_window_query(&mut query, &args.window);
            push_ordering_query(&mut query, &args.ordering);
            execute(
                client,
                context,
                Method::GET,
                &format!("/locations/{}/classes", args.location_id),
                query,
            )
        }
        ClassSubcommand::Get(args) => execute(
            client,
            context,
            Method::GET,
            &format!("/locations/{}/classes/{}", args.location_id, args.class_id),
            Vec::new(),
        ),
    }
}

fn run_pricing(command: PricingSubcommand, client: &MindbodyClient, context: &ResolvedContext) -> Result<Value> {
    match command {
        PricingSubcommand::Class(args) => execute(
            client,
            context,
            Method::GET,
            &format!(
                "/locations/{}/classes/{}/pricingOptions",
                args.location_id, args.class_id
            ),
            Vec::new(),
        ),
        PricingSubcommand::Location(args) => {
            let mut query = Vec::new();
            push_query_csv_u64(&mut query, "serviceCategoryIds", args.service_category_ids);
            execute(
                client,
                context,
                Method::GET,
                &format!("/locations/{}/pricingOptions", args.location_id),
                query,
            )
        }
    }
}

fn run_bookings(command: BookingSubcommand, client: &MindbodyClient, context: &ResolvedContext) -> Result<Value> {
    let user_id = context.require_user_id()?.to_owned();
    match command {
        BookingSubcommand::List(args) => {
            let mut query = Vec::new();
            push_optional_query_u64(&mut query, "locationId", args.location_id);
            push_optional_query_u64(&mut query, "subscriberId", args.subscriber_id);
            push_window_query(&mut query, &args.window);
            push_ordering_query(&mut query, &args.ordering);
            execute(
                client,
                context,
                Method::GET,
                &format!("/users/{user_id}/bookings"),
                query,
            )
        }
        BookingSubcommand::Get(args) => execute(
            client,
            context,
            Method::GET,
            &format!("/users/{user_id}/bookings/{}", args.booking_id),
            Vec::new(),
        ),
        BookingSubcommand::Create(args) => execute_json(
            client,
            context,
            Method::POST,
            "/bookings",
            Vec::new(),
            args.build_body(&user_id)?,
            args.idempotency_key,
        ),
        BookingSubcommand::Cancel(args) => {
            let mut query = Vec::new();
            if args.suppress_cancellation_confirmation_email {
                query.push(("suppressCancellationConfirmationEmail".into(), "true".into()));
            }
            execute(
                client,
                context,
                Method::DELETE,
                &format!("/users/{user_id}/bookings/{}", args.booking_id),
                query,
            )
        }
    }
}

fn run_passes(command: PassSubcommand, client: &MindbodyClient, context: &ResolvedContext) -> Result<Value> {
    let user_id = context.require_user_id()?.to_owned();
    match command {
        PassSubcommand::List(args) => {
            let mut query = Vec::new();
            push_optional_query_u64(&mut query, "subscriberId", args.subscriber_id);
            push_optional_query_bool(&mut query, "limitToUsable", args.limit_to_usable);
            push_window_query(&mut query, &args.window);
            push_ordering_query(&mut query, &args.ordering);
            execute(client, context, Method::GET, &format!("/users/{user_id}/passes"), query)
        }
        PassSubcommand::Get(args) => execute(
            client,
            context,
            Method::GET,
            &format!("/users/{user_id}/passes/{}", args.pass_id),
            Vec::new(),
        ),
    }
}

fn execute(
    client: &MindbodyClient,
    context: &ResolvedContext,
    method: Method,
    path: &str,
    query: Vec<(String, String)>,
) -> Result<Value> {
    let (api_key, client_key, client_secret) = context.require_credentials()?;
    client.execute(
        Credentials {
            api_key,
            client_key,
            client_secret,
        },
        RequestSpec {
            method,
            path: path.into(),
            query,
            body: RequestBody::None,
            idempotency_key: None,
        },
    )
}

fn execute_json(
    client: &MindbodyClient,
    context: &ResolvedContext,
    method: Method,
    path: &str,
    query: Vec<(String, String)>,
    body: Value,
    idempotency_key: Option<String>,
) -> Result<Value> {
    let (api_key, client_key, client_secret) = context.require_credentials()?;
    client.execute(
        Credentials {
            api_key,
            client_key,
            client_secret,
        },
        RequestSpec {
            method,
            path: path.into(),
            query,
            body: RequestBody::Json(body),
            idempotency_key,
        },
    )
}

fn read_payment_details(path: &std::path::Path) -> Result<PaymentDetailsInput> {
    let contents = if path.as_os_str() == "-" {
        std::io::read_to_string(std::io::stdin())
            .map_err(|error| Error::Io(format!("failed to read payment details from stdin: {error}")))?
    } else {
        std::fs::read_to_string(path).map_err(|error| {
            Error::Io(format!(
                "failed to read payment details from {}: {error}",
                path.display()
            ))
        })?
    };

    serde_json::from_str(&contents)
        .map_err(|error| Error::Arguments(format!("payment details file must be valid JSON: {error}")))
}

fn validate_coordinate_pair(latitude: Option<f64>, longitude: Option<f64>, command: &str) -> Result<()> {
    if latitude.is_some() ^ longitude.is_some() {
        return Err(Error::Arguments(format!(
            "{command} requires both --latitude and --longitude when either is provided"
        )));
    }

    Ok(())
}

fn push_window_query(query: &mut Vec<(String, String)>, window: &WindowedQueryArgs) {
    push_optional_query_u32(query, "maxResults", window.max_results);
    push_optional_query_u32(query, "offset", window.offset);
}

fn push_ordering_query(query: &mut Vec<(String, String)>, ordering: &OrderingArgs) {
    push_optional_query_string(query, "orderBy", ordering.order_by.clone());
    if let Some(order) = ordering.order {
        query.push(("order".into(), order.as_api_value().into()));
    }
}

fn push_optional_query_string(query: &mut Vec<(String, String)>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        query.push((key.into(), value));
    }
}

fn push_optional_query_u64(query: &mut Vec<(String, String)>, key: &str, value: Option<u64>) {
    if let Some(value) = value {
        query.push((key.into(), value.to_string()));
    }
}

fn push_optional_query_u32(query: &mut Vec<(String, String)>, key: &str, value: Option<u32>) {
    if let Some(value) = value {
        query.push((key.into(), value.to_string()));
    }
}

fn push_optional_query_f64(query: &mut Vec<(String, String)>, key: &str, value: Option<f64>) {
    if let Some(value) = value {
        query.push((key.into(), value.to_string()));
    }
}

fn push_optional_query_bool(query: &mut Vec<(String, String)>, key: &str, value: Option<bool>) {
    if let Some(value) = value {
        query.push((key.into(), value.to_string()));
    }
}

fn push_query_csv_u64(query: &mut Vec<(String, String)>, key: &str, values: Vec<u64>) {
    if !values.is_empty() {
        let joined = values
            .into_iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(",");
        query.push((key.into(), joined));
    }
}

fn render_json(value: &Value, compact: bool) -> String {
    let serialized = if compact {
        serde_json::to_string(value)
    } else {
        serde_json::to_string_pretty(value)
    };

    match serialized {
        Ok(serialized) => serialized,
        Err(error) => format!(
            "{{\"status\":\"error\",\"kind\":\"serialization\",\"message\":{}}}",
            serde_json::Value::String(error.to_string())
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        fs,
        io::{Read, Write},
        net::TcpListener,
        path::PathBuf,
        sync::{Arc, Mutex},
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use clap::Parser;
    use serde_json::{json, Value};

    use super::{render_cli_error, run, Cli};

    #[test]
    fn account_status_reports_resolved_context_without_leaking_secrets() {
        let temp_dir = temp_dir("mindbody-account-status");
        let config_path = temp_dir.join("config.json");
        write_config(
            &config_path,
            json!({
                "base_url": "https://mb-api.mindbodyonline.com/affiliate/api/v1",
                "app_name": "switchboard-mindbody-test",
                "user_id": "pilates.user",
                "api_key": "api-key",
                "client_key": "client-key"
            }),
        );

        let output = run_command(&[
            "mindbody",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "account",
            "status",
        ]);

        assert_eq!(output["status"], "ok");
        assert_eq!(output["provider"], "mindbody");
        assert_eq!(output["user_id"], "pilates.user");
        assert_eq!(output["has_api_key"], true);
        assert_eq!(output["has_client_key"], true);
        assert_eq!(output["has_client_secret"], false);
    }

    #[test]
    fn locations_search_sends_headers_and_query() {
        let capture = Arc::new(Mutex::new(String::new()));
        let server = TestServer::spawn(
            json!({
                "items": [
                    {
                        "id": 86784,
                        "name": "Pilates Palace"
                    }
                ],
                "offset": 0,
                "maxResults": 1,
                "totalResults": 1
            })
            .to_string(),
            200,
            Some(capture.clone()),
        );
        let temp_dir = temp_dir("mindbody-locations");
        let config_path = temp_dir.join("config.json");
        write_config(
            &config_path,
            json!({
                "base_url": server.base_url(),
                "api_key": "api-key",
                "client_key": "client-key",
                "client_secret": "client-secret"
            }),
        );

        let output = run_command(&[
            "mindbody",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "locations",
            "search",
            "--search-text",
            "pilates",
            "--country-code",
            "US",
            "--max-results",
            "5",
        ]);

        let request = capture.lock().expect("capture lock should work").clone().to_lowercase();
        assert!(request.starts_with("get /locations?"));
        assert!(request.contains("searchtext=pilates"));
        assert!(request.contains("countrycode=us"));
        assert!(request.contains("maxresults=5"));
        assert!(request.contains("api-key: api-key"));
        assert!(request.contains("authorization: basic"));
        assert_eq!(output["items"][0]["id"], 86784);
    }

    #[test]
    fn bookings_create_with_pass_builds_typed_body() {
        let capture = Arc::new(Mutex::new(String::new()));
        let server = TestServer::spawn(
            json!({
                "booking": {
                    "id": "booking-1"
                }
            })
            .to_string(),
            201,
            Some(capture.clone()),
        );
        let temp_dir = temp_dir("mindbody-booking-pass");
        let config_path = temp_dir.join("config.json");
        write_config(
            &config_path,
            json!({
                "base_url": server.base_url(),
                "api_key": "api-key",
                "client_key": "client-key",
                "client_secret": "client-secret",
                "user_id": "pilates.user"
            }),
        );

        let output = run_command(&[
            "mindbody",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "bookings",
            "create",
            "--location-id",
            "86784",
            "--class-id",
            "5134512",
            "--reconciliation-type",
            "pass",
            "--reconciliation-id",
            "598a6916-7876-406e-9537-db6af825f9a2",
            "--idempotency-key",
            "123e4567-e89b-12d3-a456-426614174000",
            "--suppress-booking-confirmation-email",
        ]);

        let request = capture.lock().expect("capture lock should work").clone();
        let request_lower = request.to_lowercase();
        assert!(request.starts_with("POST /bookings"));
        assert!(request_lower.contains("idempotency-key: 123e4567-e89b-12d3-a456-426614174000"));
        assert!(request.contains("\"uniqueUserId\":\"pilates.user\""));
        assert!(request.contains("\"type\":\"Pass\""));
        assert!(request.contains("\"paymentDetails\":null"));
        assert_eq!(output["booking"]["id"], "booking-1");
    }

    #[test]
    fn pricing_option_booking_requires_purchase_fields() {
        let error = run_command_error(&[
            "mindbody",
            "--api-key",
            "api-key",
            "--client-key",
            "client-key",
            "--client-secret",
            "client-secret",
            "--user-id",
            "pilates.user",
            "bookings",
            "create",
            "--location-id",
            "86784",
            "--class-id",
            "5134512",
            "--reconciliation-type",
            "pricing-option",
            "--reconciliation-id",
            "153458",
        ]);

        assert_eq!(error["kind"], "arguments");
        assert!(error["message"]
            .as_str()
            .expect("message should be present")
            .contains("--pricing-option-total"));
    }

    #[test]
    fn bookings_cancel_uses_user_id_and_query_flag() {
        let capture = Arc::new(Mutex::new(String::new()));
        let server = TestServer::spawn(
            json!({
                "message": "Cancellation successful.",
                "pass": {
                    "id": "d1e9b2be-655a-4419-b7dd-30aa84f7aa70"
                }
            })
            .to_string(),
            200,
            Some(capture.clone()),
        );
        let temp_dir = temp_dir("mindbody-booking-cancel");
        let config_path = temp_dir.join("config.json");
        write_config(
            &config_path,
            json!({
                "base_url": server.base_url(),
                "api_key": "api-key",
                "client_key": "client-key",
                "client_secret": "client-secret",
                "user_id": "pilates.user"
            }),
        );

        let output = run_command(&[
            "mindbody",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "bookings",
            "cancel",
            "booking-123",
            "--suppress-cancellation-confirmation-email",
        ]);

        let request = capture.lock().expect("capture lock should work").clone();
        assert!(request
            .starts_with("DELETE /users/pilates.user/bookings/booking-123?suppressCancellationConfirmationEmail=true"));
        assert_eq!(output["message"], "Cancellation successful.");
    }

    fn run_command(args: &[&str]) -> Value {
        let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
        let compact = cli.global.compact;
        let (value, _) = run(cli).unwrap_or_else(|error| panic!("{}", render_cli_error(&error, compact)));
        value
    }

    fn run_command_error(args: &[&str]) -> Value {
        let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
        let compact = cli.global.compact;
        let error = run(cli).expect_err("command should fail");
        serde_json::from_str(&render_cli_error(&error, compact)).expect("error should render as json")
    }

    fn write_config(path: &PathBuf, value: Value) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("config dir should exist");
        }
        fs::write(
            path,
            serde_json::to_vec_pretty(&value).expect("config json should serialize"),
        )
        .expect("config should write");
    }

    struct TestServer {
        address: String,
        _handle: thread::JoinHandle<()>,
    }

    impl TestServer {
        fn spawn(body: String, status_code: u16, capture: Option<Arc<Mutex<String>>>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
            let address = listener.local_addr().expect("local addr should exist");

            let handle = thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let request = read_request(&mut stream);
                    if let Some(capture) = capture {
                        if let Ok(mut guard) = capture.lock() {
                            *guard = request;
                        }
                    }

                    let status_text = match status_code {
                        200 => "OK",
                        201 => "Created",
                        400 => "Bad Request",
                        401 => "Unauthorized",
                        _ => "OK",
                    };
                    let response = format!(
                        "HTTP/1.1 {status_code} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
            });

            Self {
                address: format!("http://{address}"),
                _handle: handle,
            }
        }

        fn base_url(&self) -> String {
            self.address.clone()
        }
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut temp = [0_u8; 4096];
        loop {
            let bytes_read = stream.read(&mut temp).expect("request should read");
            if bytes_read == 0 {
                break;
            }
            buffer.extend_from_slice(&temp[..bytes_read]);

            if let Some(headers_end) = find_headers_end(&buffer) {
                let headers = String::from_utf8_lossy(&buffer[..headers_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let mut parts = line.splitn(2, ':');
                        let name = parts.next()?.trim();
                        if !name.eq_ignore_ascii_case("content-length") {
                            return None;
                        }
                        parts.next()?.trim().parse::<usize>().ok()
                    })
                    .unwrap_or(0);
                let total_length = headers_end + 4 + content_length;
                if buffer.len() >= total_length {
                    break;
                }
            }
        }

        String::from_utf8_lossy(&buffer).into_owned()
    }

    fn find_headers_end(buffer: &[u8]) -> Option<usize> {
        buffer.windows(4).position(|window| window == b"\r\n\r\n")
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("current time should be after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{nanos:x}"));
        fs::create_dir_all(&path).expect("temp dir should create");
        path
    }
}
