use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{
    commands::shared::{
        push_optional_query_u64, push_ordering_query, push_window_query, OrderingArgs, WindowedQueryArgs,
    },
    execute, execute_json,
    state::validate_unique_user_id,
    Error, MindbodyClient, ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct BookingCommand {
    #[command(subcommand)]
    pub(crate) command: BookingSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum BookingSubcommand {
    List(ListBookingsArgs),
    Get(BookingIdArgs),
    Create(CreateBookingArgs),
    Cancel(CancelBookingArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ListBookingsArgs {
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
pub(crate) struct BookingIdArgs {
    #[arg(value_name = "BOOKING_ID")]
    booking_id: String,
}

#[derive(Debug, Args)]
pub(crate) struct CancelBookingArgs {
    #[arg(value_name = "BOOKING_ID")]
    booking_id: String,

    #[arg(long = "suppress-cancellation-confirmation-email")]
    suppress_cancellation_confirmation_email: bool,
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

#[derive(Debug, Args)]
pub(crate) struct CreateBookingArgs {
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

pub(crate) fn run_bookings(
    command: BookingSubcommand,
    client: &MindbodyClient,
    context: &ResolvedContext,
) -> Result<Value> {
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
