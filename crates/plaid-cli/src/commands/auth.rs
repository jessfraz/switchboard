use std::{
    process::Command,
    thread,
    time::{Duration, Instant},
};

use clap::{Args, Subcommand};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    commands::{
        link::{
            apply_default_auth_login_products, build_link_token_create_request, LinkTokenCreateArgs,
            LinkTokenCreateResponse, LinkTokenHostedLink,
        },
        shared::{credentials, redact_secret, require_response_string, serialize_payload, AccessTokenRequest},
    },
    state::PendingLinkSessionState,
    Error, PlaidClient, ResolvedContext, Result, PLAID_GITHUB_PAGES_COMPLETION_REDIRECT_URI,
};

#[derive(Debug, Args)]
pub(crate) struct AuthCommand {
    #[command(subcommand)]
    pub(crate) command: AuthSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AuthSubcommand {
    Login(Box<AuthLoginArgs>),
    Finish(AuthFinishArgs),
    Status,
    #[command(name = "exchange-public-token")]
    ExchangePublicToken(AuthExchangePublicTokenArgs),
    #[command(name = "import-access-token")]
    ImportAccessToken(AuthImportAccessTokenArgs),
    #[command(name = "invalidate-access-token")]
    InvalidateAccessToken(AuthInvalidateAccessTokenArgs),
    Logout,
}

#[derive(Debug, Args)]
pub(crate) struct AuthLoginArgs {
    #[command(flatten)]
    link: LinkTokenCreateArgs,

    #[arg(long, default_value_t = 300)]
    timeout_seconds: u64,

    #[arg(long, default_value_t = 2000)]
    poll_interval_ms: u64,

    #[arg(long)]
    no_open: bool,

    #[arg(long, value_name = "URL")]
    completion_redirect_uri: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AuthFinishArgs {
    #[arg(long)]
    link_token: Option<String>,

    #[arg(long, default_value_t = 300)]
    timeout_seconds: u64,

    #[arg(long, default_value_t = 2000)]
    poll_interval_ms: u64,
}

#[derive(Debug, Args)]
pub(crate) struct AuthExchangePublicTokenArgs {
    #[arg(long)]
    public_token: String,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ExchangePublicTokenRequest {
    pub(crate) public_token: String,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ExchangePublicTokenResponse {
    pub(crate) access_token: String,
    pub(crate) item_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) request_id: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AuthImportAccessTokenArgs {
    #[arg(long)]
    access_token: String,

    #[arg(long)]
    item_id: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AuthInvalidateAccessTokenArgs {
    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct LinkTokenGetRequest {
    link_token: String,
}

#[derive(Debug, Deserialize)]
struct LinkTokenGetResponse {
    link_token: String,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    link_sessions: Vec<LinkTokenSession>,
}

#[derive(Debug, Deserialize)]
struct LinkTokenSession {
    #[serde(default)]
    link_session_id: Option<String>,
    #[serde(default)]
    finished_at: Option<String>,
    #[serde(default)]
    results: Option<LinkTokenSessionResults>,
    #[serde(default)]
    on_exit: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct LinkTokenSessionResults {
    #[serde(default)]
    item_add_results: Vec<LinkTokenSessionItemAddResult>,
}

#[derive(Debug, Deserialize)]
struct LinkTokenSessionItemAddResult {
    #[serde(default)]
    public_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct AuthHostedLinkPendingOutput {
    status: &'static str,
    link_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hosted_link_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    redirect_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_redirect_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expiration: Option<String>,
    browser_open_attempted: bool,
    opened_browser: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    browser_open_error: Option<String>,
    next_step: String,
}

#[derive(Debug, Serialize)]
struct AuthHostedLinkCompletedOutput {
    status: &'static str,
    authenticated: bool,
    stored: bool,
    link_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    link_session_id: Option<String>,
    access_token: String,
    item_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct AuthHostedLinkExitedOutput {
    status: &'static str,
    authenticated: bool,
    link_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    link_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    on_exit: Option<Value>,
    next_step: String,
}

#[derive(Debug)]
struct BrowserLaunch {
    attempted: bool,
    opened: bool,
    error: Option<String>,
}

#[derive(Debug)]
enum HostedLinkSessionStatus {
    Pending,
    Completed {
        link_session_id: Option<String>,
        request_id: Option<String>,
        public_token: String,
    },
    Exited {
        link_session_id: Option<String>,
        request_id: Option<String>,
        on_exit: Option<Value>,
    },
}

pub(crate) fn run_auth(command: AuthSubcommand, client: &PlaidClient, context: &mut ResolvedContext) -> Result<Value> {
    match command {
        AuthSubcommand::Login(args) => run_login(*args, client, context),
        AuthSubcommand::Finish(args) => run_finish(args, client, context),
        AuthSubcommand::Status => auth_status(client, context),
        AuthSubcommand::ExchangePublicToken(args) => {
            let response = exchange_public_token(client, context, args.public_token, args.no_store)?;
            serialize_payload(response)
        }
        AuthSubcommand::ImportAccessToken(args) => {
            let item_id = args.item_id.clone();
            context.store_access_token(args.access_token.clone(), item_id.clone())?;
            Ok(json!({
                "status": "ok",
                "stored": true,
                "item_id": item_id,
                "access_token": redact_secret(&args.access_token),
            }))
        }
        AuthSubcommand::InvalidateAccessToken(args) => {
            let credentials = credentials(context)?;
            let response = client.post(
                credentials,
                "/item/access_token/invalidate",
                serialize_payload(AccessTokenRequest {
                    access_token: context.require_access_token()?.to_owned(),
                })?,
            )?;

            if !args.no_store {
                let access_token = require_response_string(&response, "new_access_token")?;
                context.store_access_token(access_token, context.item_id.clone())?;
            }

            Ok(response)
        }
        AuthSubcommand::Logout => {
            context.clear_auth_state()?;
            Ok(json!({
                "status": "logged_out",
                "environment": context.environment,
                "base_url": context.base_url,
                "plaid_version": context.plaid_version,
            }))
        }
    }
}

fn run_login(args: AuthLoginArgs, client: &PlaidClient, context: &mut ResolvedContext) -> Result<Value> {
    let completion_redirect_uri = args
        .completion_redirect_uri
        .unwrap_or_else(|| PLAID_GITHUB_PAGES_COMPLETION_REDIRECT_URI.to_owned());
    let request = build_link_token_create_request(
        apply_default_auth_login_products(args.link),
        context,
        Some(LinkTokenHostedLink {
            completion_redirect_uri: completion_redirect_uri.clone(),
        }),
    )?;
    let redirect_uri = request.redirect_uri.clone();
    let response: LinkTokenCreateResponse = decode_response(
        client.post(credentials(context)?, "/link/token/create", serialize_payload(request)?)?,
        "Plaid link token create response",
    )?;
    let hosted_link_url = response
        .hosted_link_url
        .clone()
        .ok_or_else(|| Error::Config("Plaid link token create response was missing hosted_link_url".into()))?;
    context.store_pending_link_session(
        response.link_token.clone(),
        Some(hosted_link_url.clone()),
        redirect_uri,
        Some(completion_redirect_uri),
        response.expiration.clone(),
    )?;

    if args.no_open {
        return serialize_payload(render_pending_output(
            context.pending_link_session.as_ref(),
            &response.link_token,
            BrowserLaunch {
                attempted: false,
                opened: false,
                error: None,
            },
            "Open the hosted_link_url in a browser. When Plaid finishes, run `plaid auth finish` to complete token exchange."
                .into(),
        ));
    }

    let browser_launch = launch_browser(&hosted_link_url);
    if !browser_launch.opened {
        return serialize_payload(render_pending_output(
            context.pending_link_session.as_ref(),
            &response.link_token,
            browser_launch,
            "Open the hosted_link_url manually, then run `plaid auth finish` to complete token exchange.".into(),
        ));
    }

    eprintln!("Waiting for Plaid Hosted Link session to finish...");
    complete_pending_link_session(
        client,
        context,
        response.link_token,
        Duration::from_secs(args.timeout_seconds),
        Duration::from_millis(args.poll_interval_ms),
    )
}

fn run_finish(args: AuthFinishArgs, client: &PlaidClient, context: &mut ResolvedContext) -> Result<Value> {
    let link_token = args
        .link_token
        .or_else(|| {
            context
                .pending_link_session
                .as_ref()
                .map(|pending_link_session| pending_link_session.link_token.clone())
        })
        .ok_or_else(|| {
            Error::Config("missing pending Plaid Hosted Link session, run `plaid auth login` first".into())
        })?;
    complete_pending_link_session(
        client,
        context,
        link_token,
        Duration::from_secs(args.timeout_seconds),
        Duration::from_millis(args.poll_interval_ms),
    )
}

fn exchange_public_token(
    client: &PlaidClient,
    context: &mut ResolvedContext,
    public_token: String,
    no_store: bool,
) -> Result<ExchangePublicTokenResponse> {
    let credentials = credentials(context)?;
    let response: ExchangePublicTokenResponse = decode_response(
        client.post(
            credentials,
            "/item/public_token/exchange",
            serialize_payload(ExchangePublicTokenRequest { public_token })?,
        )?,
        "Plaid public token exchange response",
    )?;

    if !no_store {
        context.store_access_token(response.access_token.clone(), Some(response.item_id.clone()))?;
    }

    Ok(response)
}

fn complete_pending_link_session(
    client: &PlaidClient,
    context: &mut ResolvedContext,
    link_token: String,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<Value> {
    let deadline = Instant::now() + timeout;

    loop {
        match link_token_session_status(client, context, &link_token)? {
            HostedLinkSessionStatus::Completed {
                link_session_id,
                request_id,
                public_token,
            } => {
                let exchange = exchange_public_token(client, context, public_token, false)?;
                return serialize_payload(AuthHostedLinkCompletedOutput {
                    status: "ok",
                    authenticated: true,
                    stored: true,
                    link_token,
                    link_session_id,
                    access_token: exchange.access_token,
                    item_id: exchange.item_id,
                    request_id: exchange.request_id.or(request_id),
                });
            }
            HostedLinkSessionStatus::Exited {
                link_session_id,
                request_id,
                on_exit,
            } => {
                if context.pending_link_session.is_some() {
                    context.clear_pending_link_session()?;
                }
                return serialize_payload(AuthHostedLinkExitedOutput {
                    status: "exited",
                    authenticated: false,
                    link_token,
                    link_session_id,
                    request_id,
                    on_exit,
                    next_step:
                        "Plaid Link finished without producing a public token. Run `plaid auth login` to start over."
                            .into(),
                });
            }
            HostedLinkSessionStatus::Pending if Instant::now() >= deadline => {
                return serialize_payload(render_pending_output(
                    context.pending_link_session.as_ref(),
                    &link_token,
                    BrowserLaunch {
                        attempted: false,
                        opened: false,
                        error: None,
                    },
                    "Plaid Hosted Link is still pending. Keep the browser flow going, then rerun `plaid auth finish`."
                        .into(),
                ));
            }
            HostedLinkSessionStatus::Pending => {
                thread::sleep(poll_interval.max(Duration::from_millis(100)));
            }
        }
    }
}

fn link_token_session_status(
    client: &PlaidClient,
    context: &ResolvedContext,
    link_token: &str,
) -> Result<HostedLinkSessionStatus> {
    let response: LinkTokenGetResponse = decode_response(
        client.post(
            credentials(context)?,
            "/link/token/get",
            serialize_payload(LinkTokenGetRequest {
                link_token: link_token.to_owned(),
            })?,
        )?,
        "Plaid link token get response",
    )?;
    let LinkTokenGetResponse {
        link_token: returned_link_token,
        request_id,
        link_sessions,
    } = response;
    if returned_link_token != link_token {
        return Err(Error::Config(format!(
            "Plaid link token get response returned mismatched link_token {returned_link_token:?} for requested token {link_token:?}"
        )));
    }

    let Some(session) = link_sessions.into_iter().last() else {
        return Ok(HostedLinkSessionStatus::Pending);
    };

    let public_tokens = session
        .results
        .into_iter()
        .flat_map(|results| results.item_add_results.into_iter())
        .filter_map(|result| result.public_token)
        .collect::<Vec<_>>();

    if public_tokens.len() > 1 {
        return Err(Error::Config(
            "Plaid Hosted Link returned multiple public tokens; plaid-cli currently supports one Item per login".into(),
        ));
    }

    if let Some(public_token) = public_tokens.into_iter().next() {
        return Ok(HostedLinkSessionStatus::Completed {
            link_session_id: session.link_session_id,
            request_id,
            public_token,
        });
    }

    if session.finished_at.is_some() {
        return Ok(HostedLinkSessionStatus::Exited {
            link_session_id: session.link_session_id,
            request_id,
            on_exit: session.on_exit,
        });
    }

    Ok(HostedLinkSessionStatus::Pending)
}

fn render_pending_output(
    pending_link_session: Option<&PendingLinkSessionState>,
    link_token: &str,
    browser_launch: BrowserLaunch,
    next_step: String,
) -> AuthHostedLinkPendingOutput {
    AuthHostedLinkPendingOutput {
        status: "pending",
        link_token: link_token.to_owned(),
        hosted_link_url: pending_link_session.and_then(|session| session.hosted_link_url.clone()),
        redirect_uri: pending_link_session.and_then(|session| session.redirect_uri.clone()),
        completion_redirect_uri: pending_link_session.and_then(|session| session.completion_redirect_uri.clone()),
        expiration: pending_link_session.and_then(|session| session.expiration.clone()),
        browser_open_attempted: browser_launch.attempted,
        opened_browser: browser_launch.opened,
        browser_open_error: browser_launch.error,
        next_step,
    }
}

fn decode_response<T: DeserializeOwned>(value: Value, context: &str) -> Result<T> {
    serde_json::from_value(value).map_err(|error| Error::Config(format!("{context} was malformed: {error}")))
}

fn launch_browser(url: &str) -> BrowserLaunch {
    eprintln!("Opening browser for Plaid Hosted Link login...");
    match open_browser(url) {
        Ok(()) => BrowserLaunch {
            attempted: true,
            opened: true,
            error: None,
        },
        Err(error) => {
            eprintln!("Could not open the browser automatically. Open this URL manually:\n{url}");
            BrowserLaunch {
                attempted: true,
                opened: false,
                error: Some(error.to_string()),
            }
        }
    }
}

fn open_browser(url: &str) -> Result<()> {
    let (command, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("open", &[url])
    } else if cfg!(target_os = "windows") {
        ("cmd", &["/C", "start", "", url])
    } else {
        ("xdg-open", &[url])
    };

    Command::new(command)
        .args(args)
        .spawn()
        .map_err(|error| Error::Io(format!("failed to launch browser with {command}: {error}")))?;

    Ok(())
}

fn auth_status(client: &PlaidClient, context: &ResolvedContext) -> Result<Value> {
    if context.access_token.is_none() {
        return Ok(json!({
            "status": "ok",
            "authenticated": false,
            "reason": "no_stored_access_token",
            "environment": context.environment,
            "base_url": context.base_url,
            "plaid_version": context.plaid_version,
            "client_name": context.client_name,
            "has_client_id": context.client_id.is_some(),
            "has_secret": context.secret.is_some(),
            "item_id": context.item_id,
            "pending_link_session": context.pending_link_session,
        }));
    }

    let credentials = credentials(context)?;
    let probe = match client.post(
        credentials,
        "/item/get",
        serialize_payload(AccessTokenRequest {
            access_token: context.require_access_token()?.to_owned(),
        })?,
    ) {
        Ok(body) => json!({
            "status_code": 200,
            "body": body,
        }),
        Err(Error::Api { status_code, body }) => json!({
            "status_code": status_code,
            "body": body,
        }),
        Err(error) => return Err(error),
    };

    let authenticated = probe
        .get("status_code")
        .and_then(Value::as_u64)
        .map(|status_code| status_code < 400)
        .unwrap_or(false);

    Ok(json!({
        "status": "ok",
        "authenticated": authenticated,
        "environment": context.environment,
        "base_url": context.base_url,
        "plaid_version": context.plaid_version,
        "client_name": context.client_name,
        "has_client_id": context.client_id.is_some(),
        "has_secret": context.secret.is_some(),
        "item_id": context.item_id,
        "pending_link_session": context.pending_link_session,
        "probe": probe,
    }))
}
