mod client;
mod state;

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use clap::{Args, Parser, Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{json, Value};

use crate::{
    client::{AuthMode, MomenceClient, RequestBody, RequestSpec},
    state::{
        ResolvedContext, ENV_MOMENCE_ACCESS_TOKEN, ENV_MOMENCE_BASE_URL, ENV_MOMENCE_CLIENT_ID,
        ENV_MOMENCE_CLIENT_SECRET, ENV_MOMENCE_REFRESH_TOKEN,
    },
};

const AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  momence auth login-password --client-id <id> --client-secret <secret> \\\n",
    "    --username you@example.com --password 'super-secret'\n",
    "  momence member sessions list --start-after 2026-03-01T00:00:00Z\n",
    "  momence member host sessions --type fitness --sort-by startsAt\n",
    "  momence member addresses create --body '{\"address\":\"123 Main St\",\"city\":\"LA\",\"country\":\"US\",\"zipcode\":\"90001\"}'\n",
    "  momence member checkout compatible-memberships --body-file cart.json\n",
    "\n",
    "This CLI is aimed at Momence member workflows, booking Pilates classes and the surrounding account-management chaos.\n",
    "Use --body or --body-file for endpoints that accept JSON request payloads.\n",
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

    match run(cli) {
        Ok((output, compact)) => {
            println!("{}", render_json(&output, compact));
            ExitCode::SUCCESS
        }
        Err((error, compact)) => {
            eprintln!("{}", error.render(compact));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> std::result::Result<(Value, bool), (Error, bool)> {
    let compact = cli.global.compact;
    let mut context = ResolvedContext::from_global(&cli.global).map_err(|error| (error, compact))?;
    let client = MomenceClient::new(context.base_url.clone()).map_err(|error| (error, compact))?;

    let output = match cli.command {
        Commands::Auth(command) => run_auth(command.command, &client, &mut context),
        Commands::Member(command) => run_member(command.command, &client, &mut context),
    }
    .map_err(|error| (error, compact))?;

    Ok((output, compact))
}

#[derive(Debug, Parser)]
#[command(
    name = "momence",
    version,
    about = "CLI for booking Pilates classes and handling Momence member account workflows",
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
struct GlobalArgs {
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, global = true, env = ENV_MOMENCE_BASE_URL, value_name = "URL")]
    base_url: Option<String>,

    #[arg(long, global = true, env = ENV_MOMENCE_CLIENT_ID, value_name = "CLIENT_ID")]
    client_id: Option<String>,

    #[arg(long, global = true, env = ENV_MOMENCE_CLIENT_SECRET, value_name = "CLIENT_SECRET")]
    client_secret: Option<String>,

    #[arg(long, global = true, env = ENV_MOMENCE_ACCESS_TOKEN, value_name = "TOKEN")]
    access_token: Option<String>,

    #[arg(long, global = true, env = ENV_MOMENCE_REFRESH_TOKEN, value_name = "TOKEN")]
    refresh_token: Option<String>,

    #[arg(long, global = true)]
    compact: bool,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Auth(AuthCommand),
    Member(MemberCommand),
}

#[derive(Debug, Args)]
struct AuthCommand {
    #[command(subcommand)]
    command: AuthSubcommand,
}

#[derive(Debug, Subcommand)]
enum AuthSubcommand {
    #[command(name = "authorize-url")]
    AuthorizeUrl(AuthAuthorizeUrlArgs),
    #[command(name = "login-password")]
    LoginPassword(AuthLoginPasswordArgs),
    #[command(name = "exchange-code")]
    ExchangeCode(AuthExchangeCodeArgs),
    Refresh(AuthRefreshArgs),
    Profile,
    Logout,
}

#[derive(Debug, Args)]
struct AuthAuthorizeUrlArgs {
    #[arg(long, value_name = "URL")]
    redirect_uri: String,

    #[arg(long, value_enum)]
    prompt: Option<AuthPrompt>,

    #[arg(long)]
    state: Option<String>,
}

#[derive(Debug, Args)]
struct AuthLoginPasswordArgs {
    #[arg(long)]
    username: String,

    #[arg(long)]
    password: String,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Args)]
struct AuthExchangeCodeArgs {
    #[arg(long)]
    code: String,

    #[arg(long, value_name = "URL")]
    redirect_uri: String,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Args)]
struct AuthRefreshArgs {
    #[arg(long)]
    refresh_token: Option<String>,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Args)]
struct MemberCommand {
    #[command(subcommand)]
    command: MemberSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberSubcommand {
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
struct MemberEmailCommand {
    #[command(subcommand)]
    command: MemberEmailSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberEmailSubcommand {
    Update(JsonBodyArgs),
}

#[derive(Debug, Args)]
struct MemberPhoneNumberCommand {
    #[command(subcommand)]
    command: MemberPhoneNumberSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberPhoneNumberSubcommand {
    Update(JsonBodyArgs),
    Delete,
}

#[derive(Debug, Args)]
struct MemberPasswordResetEmailCommand {
    #[command(subcommand)]
    command: MemberPasswordResetEmailSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberPasswordResetEmailSubcommand {
    Request,
}

#[derive(Debug, Args)]
struct MemberAddressesCommand {
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
struct MemberBoughtMembershipsCommand {
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
struct MemberCheckoutCommand {
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
struct MemberHostCommand {
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
struct MemberHostSignableDocumentsCommand {
    #[command(subcommand)]
    command: MemberHostSignableDocumentsSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberHostSignableDocumentsSubcommand {
    List,
    Sign(JsonBodyArgs),
}

#[derive(Debug, Args)]
struct MemberSavedPaymentMethodsCommand {
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
struct MemberSessionsCommand {
    #[command(subcommand)]
    command: MemberSessionsSubcommand,
}

#[derive(Debug, Subcommand)]
enum MemberSessionsSubcommand {
    List(ListMemberSessionsArgs),
    Cancel(IdArgs),
}

#[derive(Clone, Debug, Args)]
struct JsonBodyArgs {
    #[arg(
        long,
        value_name = "JSON",
        conflicts_with = "body_file",
        help = "Inline JSON request body"
    )]
    body: Option<String>,

    #[arg(
        long = "body-file",
        value_name = "PATH",
        conflicts_with = "body",
        help = "Path to a JSON request body file, use - for stdin"
    )]
    body_file: Option<PathBuf>,
}

impl JsonBodyArgs {
    fn read(&self, schema_name: &str) -> Result<Value> {
        let contents = match (&self.body, &self.body_file) {
            (Some(body), None) => body.clone(),
            (None, Some(path)) if path.as_os_str() == "-" => std::io::read_to_string(std::io::stdin())
                .map_err(|error| Error::Io(format!("failed to read JSON body from stdin: {error}")))?,
            (None, Some(path)) => std::fs::read_to_string(path)
                .map_err(|error| Error::Io(format!("failed to read JSON body from {}: {error}", path.display())))?,
            (None, None) => {
                return Err(Error::Arguments(format!(
                    "missing request body, provide --body or --body-file for {schema_name}"
                )));
            }
            (Some(_), Some(_)) => {
                return Err(Error::Arguments("request body source is ambiguous".into()));
            }
        };

        serde_json::from_str(&contents).map_err(|error| {
            Error::Arguments(format!(
                "request body must be valid JSON matching {schema_name}: {error}"
            ))
        })
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum AuthPrompt {
    #[value(name = "login")]
    Login,
    #[value(name = "sign-up")]
    SignUp,
    #[value(name = "none")]
    None,
}

impl AuthPrompt {
    fn as_api_value(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::SignUp => "sign-up",
            Self::None => "none",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SortOrder {
    Asc,
    Desc,
}

impl SortOrder {
    fn as_api_value(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

#[derive(Clone, Debug, ValueEnum)]
enum SessionType {
    #[value(name = "private")]
    Private,
    #[value(name = "special-event")]
    SpecialEvent,
    #[value(name = "special-event-new")]
    SpecialEventNew,
    #[value(name = "retreat")]
    Retreat,
    #[value(name = "fitness")]
    Fitness,
    #[value(name = "course")]
    Course,
    #[value(name = "course-class")]
    CourseClass,
    #[value(name = "semester")]
    Semester,
    #[value(name = "recital")]
    Recital,
}

impl SessionType {
    fn as_api_value(&self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::SpecialEvent => "special-event",
            Self::SpecialEventNew => "special-event-new",
            Self::Retreat => "retreat",
            Self::Fitness => "fitness",
            Self::Course => "course",
            Self::CourseClass => "course-class",
            Self::Semester => "semester",
            Self::Recital => "recital",
        }
    }
}

#[derive(Clone, Debug, Args)]
struct PaginationArgs {
    #[arg(long, default_value_t = 0)]
    page: u32,

    #[arg(long = "page-size", default_value_t = 100)]
    page_size: u32,
}

#[derive(Clone, Debug, Args)]
struct SortArgs {
    #[arg(long = "sort-order", value_enum)]
    sort_order: Option<SortOrder>,

    #[arg(long = "sort-by")]
    sort_by: Option<String>,
}

#[derive(Clone, Debug, Args)]
struct DateWindowArgs {
    #[arg(long = "start-after")]
    start_after: Option<String>,

    #[arg(long = "start-before")]
    start_before: Option<String>,

    #[arg(long = "end-after")]
    end_after: Option<String>,

    #[arg(long = "end-before")]
    end_before: Option<String>,
}

#[derive(Debug, Args)]
struct ListAddressesArgs {
    #[command(flatten)]
    pagination: PaginationArgs,

    #[command(flatten)]
    sort: SortArgs,
}

#[derive(Debug, Args)]
struct ActiveMembershipsArgs {
    #[command(flatten)]
    pagination: PaginationArgs,

    #[arg(long)]
    include_frozen: bool,
}

#[derive(Debug, Args)]
struct ListHostLocationsArgs {
    #[command(flatten)]
    pagination: PaginationArgs,

    #[command(flatten)]
    sort: SortArgs,
}

#[derive(Debug, Args)]
struct ListHostMembershipsArgs {
    #[command(flatten)]
    pagination: PaginationArgs,

    #[command(flatten)]
    sort: SortArgs,

    #[arg(long)]
    include_disabled: bool,

    #[arg(long)]
    only_featured: bool,

    #[arg(long = "compatible-with-session-id")]
    compatible_with_session_id: Option<u64>,

    #[arg(long = "compatible-with-appointment-id")]
    compatible_with_appointment_id: Option<u64>,
}

#[derive(Debug, Args)]
struct ListHostSessionsArgs {
    #[command(flatten)]
    pagination: PaginationArgs,

    #[command(flatten)]
    sort: SortArgs,

    #[arg(long)]
    include_cancelled: bool,

    #[arg(long = "type")]
    session_types: Vec<SessionType>,

    #[arg(long = "teacher-id")]
    teacher_id: Option<u64>,

    #[arg(long = "location-id")]
    location_id: Option<u64>,

    #[command(flatten)]
    window: DateWindowArgs,
}

#[derive(Debug, Args)]
struct ListMemberSessionsArgs {
    #[command(flatten)]
    pagination: PaginationArgs,

    #[command(flatten)]
    sort: SortArgs,

    #[arg(long = "start-after")]
    start_after: Option<String>,

    #[arg(long = "end-after")]
    end_after: Option<String>,
}

#[derive(Debug, Args)]
struct IdArgs {
    #[arg(value_name = "ID")]
    id: u64,
}

#[derive(Debug, Args)]
struct UpdateByIdJsonArgs {
    #[arg(value_name = "ID")]
    id: u64,

    #[command(flatten)]
    body: JsonBodyArgs,
}

#[derive(Debug)]
enum Error {
    Arguments(String),
    Api { status_code: u16, body: Value },
    Config(String),
    Http(String),
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

fn run_auth(command: AuthSubcommand, client: &MomenceClient, context: &mut ResolvedContext) -> Result<Value> {
    match command {
        AuthSubcommand::AuthorizeUrl(args) => {
            let client_id = context.require_client_id()?;
            let mut url = client.build_url("/api/v2/auth/authorize", &[])?;
            {
                let mut pairs = url.query_pairs_mut();
                pairs.append_pair("client_id", client_id);
                pairs.append_pair("redirect_uri", &args.redirect_uri);
                pairs.append_pair("response_type", "code");
                pairs.append_pair("scope", "public-api-v2");
                if let Some(prompt) = args.prompt {
                    pairs.append_pair("prompt", prompt.as_api_value());
                }
                if let Some(state) = args.state.as_ref() {
                    pairs.append_pair("state", state);
                }
            }

            Ok(json!({
                "authorize_url": url.as_str(),
            }))
        }
        AuthSubcommand::LoginPassword(args) => {
            let (client_id, client_secret) = context.require_client_credentials()?;
            let response = client.execute(RequestSpec {
                method: Method::POST,
                path: "/api/v2/auth/token".into(),
                query: Vec::new(),
                body: RequestBody::Form(vec![
                    ("grant_type".into(), "password".into()),
                    ("username".into(), args.username),
                    ("password".into(), args.password),
                ]),
                auth: AuthMode::Basic {
                    username: client_id.to_owned(),
                    password: client_secret.to_owned(),
                },
            })?;

            if !args.no_store {
                context.store_tokens_from_response(&response)?;
            }

            Ok(response)
        }
        AuthSubcommand::ExchangeCode(args) => {
            let (client_id, client_secret) = context.require_client_credentials()?;
            let response = client.execute(RequestSpec {
                method: Method::POST,
                path: "/api/v2/auth/token".into(),
                query: Vec::new(),
                body: RequestBody::Form(vec![
                    ("grant_type".into(), "authorization_code".into()),
                    ("code".into(), args.code),
                    ("redirect_uri".into(), args.redirect_uri),
                ]),
                auth: AuthMode::Basic {
                    username: client_id.to_owned(),
                    password: client_secret.to_owned(),
                },
            })?;

            if !args.no_store {
                context.store_tokens_from_response(&response)?;
            }

            Ok(response)
        }
        AuthSubcommand::Refresh(args) => {
            let (client_id, client_secret) = context.require_client_credentials()?;
            let refresh_token = args
                .refresh_token
                .or_else(|| context.refresh_token.clone())
                .ok_or_else(|| Error::Config("missing refresh token, pass --refresh-token or login first".into()))?;

            let response = client.execute(RequestSpec {
                method: Method::POST,
                path: "/api/v2/auth/token".into(),
                query: Vec::new(),
                body: RequestBody::Form(vec![
                    ("grant_type".into(), "refresh_token".into()),
                    ("refresh_token".into(), refresh_token),
                ]),
                auth: AuthMode::Basic {
                    username: client_id.to_owned(),
                    password: client_secret.to_owned(),
                },
            })?;

            if !args.no_store {
                context.store_tokens_from_response(&response)?;
            }

            Ok(response)
        }
        AuthSubcommand::Profile => client.execute(RequestSpec {
            method: Method::GET,
            path: "/api/v2/auth/profile".into(),
            query: Vec::new(),
            body: RequestBody::None,
            auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
        }),
        AuthSubcommand::Logout => {
            let response = client.execute(RequestSpec {
                method: Method::POST,
                path: "/api/v2/auth/logout".into(),
                query: Vec::new(),
                body: RequestBody::None,
                auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
            })?;
            context.clear_tokens()?;
            Ok(response)
        }
    }
}

fn run_member(command: MemberSubcommand, client: &MomenceClient, context: &mut ResolvedContext) -> Result<Value> {
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

fn execute_bearer(
    client: &MomenceClient,
    token: String,
    method: Method,
    path: &str,
    query: Vec<(String, String)>,
    body: Option<Value>,
) -> Result<Value> {
    client.execute(RequestSpec {
        method,
        path: path.into(),
        query,
        body: match body {
            Some(body) => RequestBody::Json(body),
            None => RequestBody::None,
        },
        auth: AuthMode::Bearer(token),
    })
}

fn execute_bearer_json(
    client: &MomenceClient,
    token: String,
    method: Method,
    path: &str,
    query: Vec<(String, String)>,
    body: Value,
) -> Result<Value> {
    execute_bearer(client, token, method, path, query, Some(body))
}

fn build_pagination_query(pagination: &PaginationArgs, sort: &SortArgs) -> Vec<(String, String)> {
    let mut query = vec![
        ("page".into(), pagination.page.to_string()),
        ("pageSize".into(), pagination.page_size.to_string()),
    ];

    if let Some(sort_order) = sort.sort_order {
        query.push(("sortOrder".into(), sort_order.as_api_value().into()));
    }
    if let Some(sort_by) = sort.sort_by.as_ref() {
        query.push(("sortBy".into(), sort_by.clone()));
    }

    query
}

fn push_bool_query(query: &mut Vec<(String, String)>, key: &str, value: bool) {
    if value {
        query.push((key.into(), "true".into()));
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
    use serde_json::json;

    use super::{
        run,
        state::{MomenceState, StateStore},
        Cli,
    };

    #[test]
    fn login_password_stores_tokens_and_prints_response() {
        let capture = Arc::new(Mutex::new(String::new()));
        let server = TestServer::spawn(
            json!({
                "accessToken": "access-token",
                "access_token": "access-token",
                "accessTokenExpiresAt": "2026-03-27T00:00:00Z",
                "refreshToken": "refresh-token",
                "refresh_token": "refresh-token",
                "refreshTokenExpiresAt": "2026-04-27T00:00:00Z"
            })
            .to_string(),
            200,
            Some(capture.clone()),
        );
        let temp_dir = temp_dir("momence-login");
        let config_path = temp_dir.join("config.json");

        let output = run_command(&[
            "momence",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--base-url",
            &server.base_url(),
            "--client-id",
            "client-id",
            "--client-secret",
            "client-secret",
            "--compact",
            "auth",
            "login-password",
            "--username",
            "member@example.com",
            "--password",
            "super-secret",
        ]);

        let request = capture.lock().expect("capture lock should work").clone();
        assert!(request.starts_with("POST /api/v2/auth/token"));
        assert!(request.contains("authorization: Basic"));
        assert!(request.contains("grant_type=password"));
        assert!(request.contains("username=member%40example.com"));
        assert!(request.contains("password=super-secret"));

        let state = StateStore::new(config_path).load().expect("stored state should load");
        assert_eq!(state.access_token.as_deref(), Some("access-token"));
        assert_eq!(state.refresh_token.as_deref(), Some("refresh-token"));
        assert_eq!(output["access_token"], "access-token");
    }

    #[test]
    fn member_sessions_list_sends_bearer_token_and_query() {
        let capture = Arc::new(Mutex::new(String::new()));
        let server = TestServer::spawn(
            json!({
                "pagination": { "page": 0, "pageSize": 100, "totalCount": 1 },
                "payload": [
                    {
                        "id": 1,
                        "createdAt": "2026-03-26T00:00:00Z",
                        "roomSpotId": null,
                        "checkedIn": false,
                        "cancelledAt": null,
                        "isRecurring": false,
                        "session": {
                            "id": 10,
                            "name": "Pilates",
                            "type": "fitness",
                            "description": null,
                            "startsAt": "2026-03-30T18:00:00Z",
                            "endsAt": "2026-03-30T19:00:00Z",
                            "durationInMinutes": 60,
                            "capacity": 12,
                            "teacher": null,
                            "isRecurring": false,
                            "isInPerson": true,
                            "inPersonLocation": null,
                            "onlineStreamUrl": null,
                            "onlineStreamPassword": null,
                            "bannerImageUrl": null,
                            "hostPhotoUrl": null
                        }
                    }
                ]
            })
            .to_string(),
            200,
            Some(capture.clone()),
        );
        let temp_dir = temp_dir("momence-sessions");
        let config_path = temp_dir.join("config.json");
        let store = StateStore::new(config_path.clone());
        store
            .save(&MomenceState {
                base_url: Some(server.base_url()),
                access_token: Some("stored-access-token".into()),
                ..MomenceState::default()
            })
            .expect("state should save");

        let output = run_command(&[
            "momence",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "member",
            "sessions",
            "list",
            "--start-after",
            "2026-03-01T00:00:00Z",
        ]);

        let request = capture.lock().expect("capture lock should work").clone();
        assert!(
            request.starts_with("GET /api/v2/member/sessions?page=0&pageSize=100&startAfter=2026-03-01T00%3A00%3A00Z")
        );
        assert!(request.contains("authorization: Bearer stored-access-token"));
        assert!(output.get("payload").is_some());
    }

    #[test]
    fn cancel_booking_prints_empty_success_payload() {
        let capture = Arc::new(Mutex::new(String::new()));
        let server = TestServer::spawn(String::new(), 200, Some(capture.clone()));
        let temp_dir = temp_dir("momence-cancel");
        let config_path = temp_dir.join("config.json");
        let store = StateStore::new(config_path.clone());
        store
            .save(&MomenceState {
                base_url: Some(server.base_url()),
                access_token: Some("stored-access-token".into()),
                ..MomenceState::default()
            })
            .expect("state should save");

        let output = run_command(&[
            "momence",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "member",
            "sessions",
            "cancel",
            "77",
        ]);

        let request = capture.lock().expect("capture lock should work").clone();
        assert!(request.starts_with("DELETE /api/v2/member/sessions/77"));
        assert_eq!(output, json!({ "status": "ok", "status_code": 200 }));
    }

    fn run_command(args: &[&str]) -> serde_json::Value {
        let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
        let compact = cli.global.compact;
        let (value, _) = run(cli).unwrap_or_else(|(error, _)| panic!("{}", error.render(compact)));
        value
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
                        let lower = line.to_ascii_lowercase();
                        lower
                            .strip_prefix("content-length: ")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);

                let total_length = headers_end + 4 + content_length;
                if buffer.len() >= total_length {
                    break;
                }
            }
        }

        String::from_utf8_lossy(&buffer).replace('\r', "")
    }

    fn find_headers_end(buffer: &[u8]) -> Option<usize> {
        buffer.windows(4).position(|window| window == b"\r\n\r\n")
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("temp dir should be created");
        path
    }
}
