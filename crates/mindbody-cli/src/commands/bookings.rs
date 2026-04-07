use std::path::PathBuf;

use clap::{Args, Subcommand, ValueEnum};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    commands::shared::{
        push_optional_query_u64, push_ordering_query, push_window_query, serialize_payload, OrderingArgs,
        WindowedQueryArgs,
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

        let payment_details = match self.reconciliation_type {
            ReconciliationType::Pass => None,
            ReconciliationType::PricingOption => {
                let payment_file = self
                    .payment_file
                    .as_deref()
                    .ok_or_else(|| Error::Arguments("pricing-option bookings require --payment-file".into()))?;
                Some(read_payment_details(payment_file)?.into_payload())
            }
        };

        serialize_payload(BookingCreateRequest {
            location_id: self.location_id,
            class_id: self.class_id,
            booking_reconciliation: BookingReconciliationPayload {
                id: self.reconciliation_id.clone(),
                r#type: self.reconciliation_type.as_api_value().to_owned(),
                pricing_option_total: match self.reconciliation_type {
                    ReconciliationType::Pass => None,
                    ReconciliationType::PricingOption => self.pricing_option_total,
                },
            },
            unique_user_id: user_id.to_owned(),
            suppress_booking_confirmation_email: self.suppress_booking_confirmation_email.then_some(true),
            suppress_purchase_receipt_email: self.suppress_purchase_receipt_email.then_some(true),
            subscriber_marketing_opt_in: self.subscriber_marketing_opt_in.then_some(true),
            user_first: self.user_first.clone(),
            user_last: self.user_last.clone(),
            user_email: self.user_email.clone(),
            user_phone: self.user_phone.clone(),
            payment_details,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BookingCreateRequest {
    location_id: u64,
    class_id: u64,
    booking_reconciliation: BookingReconciliationPayload,
    unique_user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    suppress_booking_confirmation_email: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suppress_purchase_receipt_email: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subscriber_marketing_opt_in: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_first: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_last: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_phone: Option<String>,
    payment_details: Option<PaymentDetails>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BookingReconciliationPayload {
    id: String,
    #[serde(rename = "type")]
    r#type: String,
    pricing_option_total: Option<f64>,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PaymentDetails {
    credit_card_number: String,
    credit_card_expiration_year: u16,
    credit_card_expiration_month: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    credit_card_cvv: Option<String>,
    billing_name: String,
    billing_address_line_1: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    billing_address_line_2: Option<String>,
    billing_city: String,
    billing_state: String,
    billing_postal_code: String,
}

impl PaymentDetailsInput {
    fn into_payload(self) -> PaymentDetails {
        PaymentDetails {
            credit_card_number: self.credit_card_number,
            credit_card_expiration_year: self.credit_card_expiration_year,
            credit_card_expiration_month: self.credit_card_expiration_month,
            credit_card_cvv: self.credit_card_cvv,
            billing_name: self.billing_name,
            billing_address_line_1: self.billing_address_line_1,
            billing_address_line_2: self.billing_address_line_2,
            billing_city: self.billing_city,
            billing_state: self.billing_state,
            billing_postal_code: self.billing_postal_code,
        }
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
