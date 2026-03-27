use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::Value;

use crate::{
    commands::shared::{
        build_pagination_query, push_bool_query, push_optional_query_string, push_optional_query_u64,
        ActiveMembershipsArgs, IdArgs, JsonBodyArgs, ListAddressesArgs, ListHostLocationsArgs, ListHostMembershipsArgs,
        ListHostSessionsArgs, ListMemberSessionsArgs, UpdateByIdJsonArgs,
    },
    execute_bearer, execute_bearer_json, MomenceClient, ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct MemberCommand {
    #[command(subcommand)]
    pub(crate) command: MemberSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum MemberSubcommand {
    Get,
    Update(JsonBodyArgs),
    Visits,
    Email(MemberEmailCommand),
    #[command(name = "phone-number")]
    PhoneNumber(MemberPhoneNumberCommand),
    #[command(name = "password-reset-email")]
    PasswordResetEmail(MemberPasswordResetEmailCommand),
    Addresses(MemberAddressesCommand),
    #[command(name = "bought-memberships")]
    BoughtMemberships(MemberBoughtMembershipsCommand),
    Checkout(MemberCheckoutCommand),
    Host(MemberHostCommand),
    #[command(name = "saved-payment-methods")]
    SavedPaymentMethods(MemberSavedPaymentMethodsCommand),
    Sessions(MemberSessionsCommand),
}

#[derive(Debug, Args)]
pub(crate) struct MemberEmailCommand {
    #[command(subcommand)]
    command: MemberEmailSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberEmailSubcommand {
    Update(JsonBodyArgs),
}

#[derive(Debug, Args)]
pub(crate) struct MemberPhoneNumberCommand {
    #[command(subcommand)]
    command: MemberPhoneNumberSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberPhoneNumberSubcommand {
    Update(JsonBodyArgs),
    Delete,
}

#[derive(Debug, Args)]
pub(crate) struct MemberPasswordResetEmailCommand {
    #[command(subcommand)]
    command: MemberPasswordResetEmailSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberPasswordResetEmailSubcommand {
    Request,
}

#[derive(Debug, Args)]
pub(crate) struct MemberAddressesCommand {
    #[command(subcommand)]
    command: MemberAddressesSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberAddressesSubcommand {
    List(ListAddressesArgs),
    Get(IdArgs),
    Create(JsonBodyArgs),
    Update(UpdateByIdJsonArgs),
    Delete(IdArgs),
}

#[derive(Debug, Args)]
pub(crate) struct MemberBoughtMembershipsCommand {
    #[command(subcommand)]
    command: MemberBoughtMembershipsSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberBoughtMembershipsSubcommand {
    Active(ActiveMembershipsArgs),
    Freeze(UpdateByIdJsonArgs),
    #[command(name = "schedule-freeze")]
    ScheduleFreeze(UpdateByIdJsonArgs),
    #[command(name = "remove-freeze")]
    RemoveFreeze(IdArgs),
    #[command(name = "schedule-unfreeze")]
    ScheduleUnfreeze(UpdateByIdJsonArgs),
    #[command(name = "remove-unfreeze")]
    RemoveUnfreeze(IdArgs),
}

#[derive(Debug, Args)]
pub(crate) struct MemberCheckoutCommand {
    #[command(subcommand)]
    command: MemberCheckoutSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberCheckoutSubcommand {
    #[command(name = "compatible-memberships")]
    CompatibleMemberships(JsonBodyArgs),
    Prices(JsonBodyArgs),
    Submit(JsonBodyArgs),
}

#[derive(Debug, Args)]
pub(crate) struct MemberHostCommand {
    #[command(subcommand)]
    command: MemberHostSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberHostSubcommand {
    Locations(ListHostLocationsArgs),
    Memberships(ListHostMembershipsArgs),
    Sessions(ListHostSessionsArgs),
    #[command(name = "signable-documents")]
    SignableDocuments(MemberHostSignableDocumentsCommand),
}

#[derive(Debug, Args)]
pub(crate) struct MemberHostSignableDocumentsCommand {
    #[command(subcommand)]
    command: MemberHostSignableDocumentsSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberHostSignableDocumentsSubcommand {
    List,
    Sign(JsonBodyArgs),
}

#[derive(Debug, Args)]
pub(crate) struct MemberSavedPaymentMethodsCommand {
    #[command(subcommand)]
    command: MemberSavedPaymentMethodsSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberSavedPaymentMethodsSubcommand {
    List,
    #[command(name = "begin-add")]
    BeginAdd(JsonBodyArgs),
    Delete(IdArgs),
}

#[derive(Debug, Args)]
pub(crate) struct MemberSessionsCommand {
    #[command(subcommand)]
    command: MemberSessionsSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberSessionsSubcommand {
    List(ListMemberSessionsArgs),
    Cancel(IdArgs),
}

pub(crate) fn run_member(
    command: MemberSubcommand,
    client: &MomenceClient,
    context: &mut ResolvedContext,
) -> Result<Value> {
    let token = context.require_access_token()?.to_owned();
    match command {
        MemberSubcommand::Get => execute_bearer(client, token, Method::GET, "/api/v2/member", Vec::new(), None),
        MemberSubcommand::Update(body) => execute_bearer_json(
            client,
            token,
            Method::PUT,
            "/api/v2/member",
            Vec::new(),
            body.read("ApiV2MemberUpdateRequestDto")?,
        ),
        MemberSubcommand::Visits => {
            execute_bearer(client, token, Method::GET, "/api/v2/member/visits", Vec::new(), None)
        }
        MemberSubcommand::Email(command) => match command.command {
            MemberEmailSubcommand::Update(body) => execute_bearer_json(
                client,
                token,
                Method::PUT,
                "/api/v2/member/email",
                Vec::new(),
                body.read("ApiV2MemberUpdateEmailRequestDto")?,
            ),
        },
        MemberSubcommand::PhoneNumber(command) => match command.command {
            MemberPhoneNumberSubcommand::Update(body) => execute_bearer_json(
                client,
                token,
                Method::PUT,
                "/api/v2/member/phone-number",
                Vec::new(),
                body.read("ApiV2MemberUpdatePhoneNumberRequestDto")?,
            ),
            MemberPhoneNumberSubcommand::Delete => execute_bearer(
                client,
                token,
                Method::DELETE,
                "/api/v2/member/phone-number",
                Vec::new(),
                None,
            ),
        },
        MemberSubcommand::PasswordResetEmail(command) => match command.command {
            MemberPasswordResetEmailSubcommand::Request => execute_bearer(
                client,
                token,
                Method::POST,
                "/api/v2/member/password-reset-email",
                Vec::new(),
                None,
            ),
        },
        MemberSubcommand::Addresses(command) => match command.command {
            MemberAddressesSubcommand::List(args) => execute_bearer(
                client,
                token,
                Method::GET,
                "/api/v2/member-addresses",
                build_pagination_query(&args.pagination, &args.sort),
                None,
            ),
            MemberAddressesSubcommand::Get(args) => execute_bearer(
                client,
                token,
                Method::GET,
                &format!("/api/v2/member-addresses/{}", args.id),
                Vec::new(),
                None,
            ),
            MemberAddressesSubcommand::Create(body) => execute_bearer_json(
                client,
                token,
                Method::POST,
                "/api/v2/member-addresses",
                Vec::new(),
                body.read("ApiV2MemberAddressRequestDto")?,
            ),
            MemberAddressesSubcommand::Update(args) => execute_bearer_json(
                client,
                token,
                Method::PUT,
                &format!("/api/v2/member-addresses/{}", args.id),
                Vec::new(),
                args.body.read("ApiV2MemberAddressRequestDto")?,
            ),
            MemberAddressesSubcommand::Delete(args) => execute_bearer(
                client,
                token,
                Method::DELETE,
                &format!("/api/v2/member-addresses/{}", args.id),
                Vec::new(),
                None,
            ),
        },
        MemberSubcommand::BoughtMemberships(command) => match command.command {
            MemberBoughtMembershipsSubcommand::Active(args) => {
                let mut query = vec![
                    ("page".into(), args.pagination.page.to_string()),
                    ("pageSize".into(), args.pagination.page_size.to_string()),
                ];
                push_bool_query(&mut query, "includeFrozen", args.include_frozen);
                execute_bearer(
                    client,
                    token,
                    Method::GET,
                    "/api/v2/member/bought-memberships/active",
                    query,
                    None,
                )
            }
            MemberBoughtMembershipsSubcommand::Freeze(args) => execute_bearer_json(
                client,
                token,
                Method::PUT,
                &format!("/api/v2/member/bought-memberships/{}/membership-freeze", args.id),
                Vec::new(),
                args.body.read("ApiV2BoughtMembershipFreezeRequestDto")?,
            ),
            MemberBoughtMembershipsSubcommand::ScheduleFreeze(args) => execute_bearer_json(
                client,
                token,
                Method::PUT,
                &format!(
                    "/api/v2/member/bought-memberships/{}/membership-schedule-freeze",
                    args.id
                ),
                Vec::new(),
                args.body.read("ApiV2BoughtMembershipScheduleFreezeRequestDto")?,
            ),
            MemberBoughtMembershipsSubcommand::RemoveFreeze(args) => execute_bearer(
                client,
                token,
                Method::DELETE,
                &format!(
                    "/api/v2/member/bought-memberships/{}/membership-schedule-freeze",
                    args.id
                ),
                Vec::new(),
                None,
            ),
            MemberBoughtMembershipsSubcommand::ScheduleUnfreeze(args) => execute_bearer_json(
                client,
                token,
                Method::PUT,
                &format!(
                    "/api/v2/member/bought-memberships/{}/membership-schedule-unfreeze",
                    args.id
                ),
                Vec::new(),
                args.body.read("ApiV2BoughtMembershipScheduleUnfreezeRequestDto")?,
            ),
            MemberBoughtMembershipsSubcommand::RemoveUnfreeze(args) => execute_bearer(
                client,
                token,
                Method::DELETE,
                &format!(
                    "/api/v2/member/bought-memberships/{}/membership-schedule-unfreeze",
                    args.id
                ),
                Vec::new(),
                None,
            ),
        },
        MemberSubcommand::Checkout(command) => match command.command {
            MemberCheckoutSubcommand::CompatibleMemberships(body) => execute_bearer_json(
                client,
                token,
                Method::POST,
                "/api/v2/member/checkout/compatible-memberships",
                Vec::new(),
                body.read("MemberCheckoutCompatibleMembershipsRequestDto")?,
            ),
            MemberCheckoutSubcommand::Prices(body) => execute_bearer_json(
                client,
                token,
                Method::POST,
                "/api/v2/member/checkout/prices",
                Vec::new(),
                body.read("MemberCheckoutPricesRequestDto")?,
            ),
            MemberCheckoutSubcommand::Submit(body) => execute_bearer_json(
                client,
                token,
                Method::POST,
                "/api/v2/member/checkout",
                Vec::new(),
                body.read("MemberCheckoutRequestDto")?,
            ),
        },
        MemberSubcommand::Host(command) => match command.command {
            MemberHostSubcommand::Locations(args) => execute_bearer(
                client,
                token,
                Method::GET,
                "/api/v2/member/host/locations",
                build_pagination_query(&args.pagination, &args.sort),
                None,
            ),
            MemberHostSubcommand::Memberships(args) => {
                let mut query = build_pagination_query(&args.pagination, &args.sort);
                push_bool_query(&mut query, "includeDisabled", args.include_disabled);
                push_bool_query(&mut query, "onlyFeatured", args.only_featured);
                push_optional_query_u64(&mut query, "compatibleWithSessionId", args.compatible_with_session_id);
                push_optional_query_u64(
                    &mut query,
                    "compatibleWithAppointmentId",
                    args.compatible_with_appointment_id,
                );

                execute_bearer(
                    client,
                    token,
                    Method::GET,
                    "/api/v2/member/host/memberships",
                    query,
                    None,
                )
            }
            MemberHostSubcommand::Sessions(args) => {
                let mut query = build_pagination_query(&args.pagination, &args.sort);
                push_bool_query(&mut query, "includeCancelled", args.include_cancelled);
                push_optional_query_u64(&mut query, "teacherId", args.teacher_id);
                push_optional_query_u64(&mut query, "locationId", args.location_id);
                push_optional_query_string(&mut query, "startAfter", args.window.start_after);
                push_optional_query_string(&mut query, "startBefore", args.window.start_before);
                push_optional_query_string(&mut query, "endAfter", args.window.end_after);
                push_optional_query_string(&mut query, "endBefore", args.window.end_before);
                for session_type in args.session_types {
                    query.push(("types".into(), session_type.as_api_value().into()));
                }

                execute_bearer(client, token, Method::GET, "/api/v2/member/host/sessions", query, None)
            }
            MemberHostSubcommand::SignableDocuments(command) => match command.command {
                MemberHostSignableDocumentsSubcommand::List => execute_bearer(
                    client,
                    token,
                    Method::GET,
                    "/api/v2/member/host/signable-documents",
                    Vec::new(),
                    None,
                ),
                MemberHostSignableDocumentsSubcommand::Sign(body) => execute_bearer_json(
                    client,
                    token,
                    Method::PUT,
                    "/api/v2/member/host/signable-documents/sign",
                    Vec::new(),
                    body.read("MemberSignDocumentRequestDto")?,
                ),
            },
        },
        MemberSubcommand::SavedPaymentMethods(command) => match command.command {
            MemberSavedPaymentMethodsSubcommand::List => execute_bearer(
                client,
                token,
                Method::GET,
                "/api/v2/member/saved-payment-methods",
                Vec::new(),
                None,
            ),
            MemberSavedPaymentMethodsSubcommand::BeginAdd(body) => execute_bearer_json(
                client,
                token,
                Method::POST,
                "/api/v2/member/saved-payment-methods",
                Vec::new(),
                body.read("ApiV2MemberManagePaymentMethodsRequestDto")?,
            ),
            MemberSavedPaymentMethodsSubcommand::Delete(args) => execute_bearer(
                client,
                token,
                Method::DELETE,
                &format!("/api/v2/member/saved-payment-methods/{}", args.id),
                Vec::new(),
                None,
            ),
        },
        MemberSubcommand::Sessions(command) => match command.command {
            MemberSessionsSubcommand::List(args) => {
                let mut query = build_pagination_query(&args.pagination, &args.sort);
                push_optional_query_string(&mut query, "startAfter", args.start_after);
                push_optional_query_string(&mut query, "endAfter", args.end_after);
                execute_bearer(client, token, Method::GET, "/api/v2/member/sessions", query, None)
            }
            MemberSessionsSubcommand::Cancel(args) => execute_bearer(
                client,
                token,
                Method::DELETE,
                &format!("/api/v2/member/sessions/{}", args.id),
                Vec::new(),
                None,
            ),
        },
    }
}
