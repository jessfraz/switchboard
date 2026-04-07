use std::{
    fs::File,
    io::{self, IsTerminal, Read, Write},
    process::Command,
};

use clap::{Args, Subcommand};
use reqwest::Url;
use serde_json::{json, Value};

use crate::{
    client::{AuthMode, RequestBody, RequestSpec, SchwabClient},
    Error, ResolvedContext, Result,
};

#[derive(Debug, Args)]
pub(crate) struct AuthCommand {
    #[command(subcommand)]
    pub(crate) command: AuthSubcommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AuthSubcommand {
    Login(AuthLoginArgs),
    #[command(name = "authorize-url")]
    AuthorizeUrl(AuthAuthorizeUrlArgs),
    #[command(name = "exchange-code")]
    ExchangeCode(AuthExchangeCodeArgs),
    #[command(name = "exchange-url")]
    ExchangeUrl(AuthExchangeUrlArgs),
    Refresh(AuthRefreshArgs),
    Status,
    Logout,
}

#[derive(Debug, Args, Clone)]
pub(crate) struct AuthAuthorizeOptions {
    #[arg(long, value_name = "URL")]
    redirect_uri: Option<String>,

    #[arg(long = "scope", value_name = "SCOPE")]
    scopes: Vec<String>,

    #[arg(long)]
    state: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AuthLoginArgs {
    #[command(flatten)]
    options: AuthAuthorizeOptions,

    #[arg(long = "callback-url", value_name = "URL")]
    callback_url: Option<String>,

    #[arg(long)]
    no_open: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AuthAuthorizeUrlArgs {
    #[command(flatten)]
    options: AuthAuthorizeOptions,
}

#[derive(Debug, Args)]
pub(crate) struct AuthExchangeCodeArgs {
    #[arg(long)]
    code: String,

    #[arg(long, value_name = "URL")]
    redirect_uri: Option<String>,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AuthExchangeUrlArgs {
    callback_input: String,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AuthRefreshArgs {
    #[arg(long)]
    refresh_token: Option<String>,

    #[arg(long)]
    no_store: bool,
}

pub(crate) fn run_auth(command: AuthSubcommand, context: &mut ResolvedContext) -> Result<Value> {
    let client = trader_client(context)?;

    match command {
        AuthSubcommand::Login(args) => {
            let prepared = prepare_authorization(context, args.options)?;
            let interactive = stdin_is_interactive();

            if args.callback_url.is_none() {
                if interactive && !args.no_open {
                    eprintln!("Opening browser for Schwab OAuth login...");
                    open_browser(&prepared.authorize_url)?;
                } else {
                    eprintln!("Open this URL in a browser:\n{}", prepared.authorize_url);
                }
            }

            let callback_input = match args.callback_url {
                Some(callback_url) => Some(callback_url),
                None => prompt_for_callback_input(interactive)?,
            };

            match callback_input {
                Some(callback_input) => {
                    let callback = parse_login_callback_input(&callback_input)?;
                    validate_callback(context, &callback)?;
                    exchange_code(&client, context, callback.code, callback.redirect_uri, false)
                }
                None => Ok(json!({
                    "status": "pending",
                    "authorize_url": prepared.authorize_url,
                    "redirect_uri": prepared.redirect_uri,
                    "scope": prepared.scopes,
                    "state": prepared.oauth_state,
                    "next_step": "Finish the browser login, then paste the full callback URL here or run `schwab auth exchange-url '<callback-url>'` later.",
                })),
            }
        }
        AuthSubcommand::AuthorizeUrl(args) => {
            let prepared = prepare_authorization(context, args.options)?;
            Ok(json!({
                "authorize_url": prepared.authorize_url,
                "redirect_uri": prepared.redirect_uri,
                "scope": prepared.scopes,
                "state": prepared.oauth_state,
            }))
        }
        AuthSubcommand::ExchangeCode(args) => exchange_code(
            &client,
            context,
            args.code,
            context.require_redirect_uri(args.redirect_uri)?,
            args.no_store,
        ),
        AuthSubcommand::ExchangeUrl(args) => {
            let callback = parse_callback_input(&args.callback_input)?;
            validate_callback(context, &callback)?;
            exchange_code(&client, context, callback.code, callback.redirect_uri, args.no_store)
        }
        AuthSubcommand::Refresh(args) => {
            let (client_id, client_secret) = context.require_client_credentials()?;
            let refresh_token = args
                .refresh_token
                .or_else(|| context.refresh_token.clone())
                .ok_or_else(|| Error::Config("missing refresh token, pass --refresh-token or login first".into()))?;

            let response = client.execute(RequestSpec {
                method: reqwest::Method::POST,
                path: context.token_url.clone(),
                query: Vec::new(),
                headers: Vec::new(),
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
                context.store_oauth_token_response(&response)?;
            }

            Ok(response)
        }
        AuthSubcommand::Status => auth_status(&client, context),
        AuthSubcommand::Logout => {
            context.clear_auth_state()?;
            Ok(json!({
                "status": "logged_out",
                "base_url": context.base_url,
                "market_data_base_url": context.market_data_base_url,
            }))
        }
    }
}

struct PreparedAuthorization {
    authorize_url: String,
    redirect_uri: String,
    scopes: Vec<String>,
    oauth_state: String,
}

fn exchange_code(
    client: &SchwabClient,
    context: &mut ResolvedContext,
    code: String,
    redirect_uri: String,
    no_store: bool,
) -> Result<Value> {
    let (client_id, client_secret) = context.require_client_credentials()?;
    let response = client.execute(RequestSpec {
        method: reqwest::Method::POST,
        path: context.token_url.clone(),
        query: Vec::new(),
        headers: Vec::new(),
        body: RequestBody::Form(vec![
            ("grant_type".into(), "authorization_code".into()),
            ("code".into(), code),
            ("redirect_uri".into(), redirect_uri.clone()),
        ]),
        auth: AuthMode::Basic {
            username: client_id.to_owned(),
            password: client_secret.to_owned(),
        },
    })?;

    if !no_store {
        context.remember_redirect_uri(redirect_uri)?;
        context.store_oauth_token_response(&response)?;
    }

    Ok(response)
}

fn auth_status(client: &SchwabClient, context: &ResolvedContext) -> Result<Value> {
    if context.access_token.is_none() {
        return Ok(json!({
            "status": "ok",
            "authenticated": false,
            "reason": "no_stored_token",
            "base_url": context.base_url,
            "market_data_base_url": context.market_data_base_url,
            "authorize_url": context.authorize_url,
            "token_url": context.token_url,
            "redirect_uri": context.redirect_uri,
            "has_client_id": context.client_id.is_some(),
            "has_client_secret": context.client_secret.is_some(),
            "refresh_token_available": context.refresh_token.is_some(),
            "scope": split_scope(context.scope.as_deref()),
            "expires_at_epoch_seconds": context.expires_at_epoch_seconds,
            "cached_account_numbers": context.account_number_cache().len(),
        }));
    }

    let probe = match client.execute(RequestSpec {
        method: reqwest::Method::GET,
        path: "/userPreference".into(),
        query: Vec::new(),
        headers: context.trader_headers(),
        body: RequestBody::None,
        auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
    }) {
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
        "base_url": context.base_url,
        "market_data_base_url": context.market_data_base_url,
        "authorize_url": context.authorize_url,
        "token_url": context.token_url,
        "redirect_uri": context.redirect_uri,
        "has_client_id": context.client_id.is_some(),
        "has_client_secret": context.client_secret.is_some(),
        "refresh_token_available": context.refresh_token.is_some(),
        "scope": split_scope(context.scope.as_deref()),
        "expires_at_epoch_seconds": context.expires_at_epoch_seconds,
        "cached_account_numbers": context.account_number_cache().len(),
        "probe": probe,
    }))
}

fn trader_client(context: &ResolvedContext) -> Result<SchwabClient> {
    SchwabClient::new(
        context.base_url.clone(),
        format!("schwab-cli/{}", env!("CARGO_PKG_VERSION")),
    )
}

fn resolve_scopes(scopes: Vec<String>) -> Vec<String> {
    if scopes.is_empty() {
        vec!["readonly".into()]
    } else {
        scopes
    }
}

fn split_scope(scope: Option<&str>) -> Vec<String> {
    scope
        .unwrap_or_default()
        .split_whitespace()
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

struct CallbackInput {
    code: String,
    redirect_uri: String,
    state: Option<String>,
}

fn prepare_authorization(
    context: &mut ResolvedContext,
    options: AuthAuthorizeOptions,
) -> Result<PreparedAuthorization> {
    let client_id = context.require_client_id()?;
    let redirect_uri = context.require_redirect_uri(options.redirect_uri)?;
    let scopes = resolve_scopes(options.scopes);
    let oauth_state = options.state.unwrap_or(generate_oauth_state()?);
    let mut url = Url::parse(&context.authorize_url).map_err(|error| {
        Error::Config(format!(
            "invalid Schwab authorize URL {:?}: {error}",
            context.authorize_url
        ))
    })?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("response_type", "code");
        pairs.append_pair("client_id", client_id);
        pairs.append_pair("redirect_uri", &redirect_uri);
        pairs.append_pair("scope", &scopes.join(" "));
        pairs.append_pair("state", &oauth_state);
    }

    context.remember_authorization_request(redirect_uri.clone(), oauth_state.clone())?;

    Ok(PreparedAuthorization {
        authorize_url: url.into(),
        redirect_uri,
        scopes,
        oauth_state,
    })
}

fn parse_callback_input(input: &str) -> Result<CallbackInput> {
    let url =
        Url::parse(input).map_err(|error| Error::Arguments(format!("callback input must be a valid URL: {error}")))?;
    let code = url
        .query_pairs()
        .find_map(|(key, value)| (key == "code").then(|| value.into_owned()))
        .ok_or_else(|| Error::Arguments("callback URL is missing the code query parameter".into()))?;
    let state = url
        .query_pairs()
        .find_map(|(key, value)| (key == "state").then(|| value.into_owned()));

    let mut redirect_uri = url.clone();
    redirect_uri.set_query(None);
    redirect_uri.set_fragment(None);

    Ok(CallbackInput {
        code,
        redirect_uri: redirect_uri.to_string(),
        state,
    })
}

fn parse_login_callback_input(input: &str) -> Result<CallbackInput> {
    let trimmed = input.trim().trim_end_matches(';').trim();
    let candidate = if let Some(command_start) = trimmed.find(" exchange-url ") {
        trimmed[command_start + " exchange-url ".len()..].trim()
    } else {
        trimmed
    };
    let candidate = unquote(candidate);
    parse_callback_input(candidate)
}

fn unquote(value: &str) -> &str {
    let quoted = (value.starts_with('\'') && value.ends_with('\'')) || (value.starts_with('"') && value.ends_with('"'));
    if quoted && value.len() >= 2 {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn stdin_is_interactive() -> bool {
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

fn prompt_for_callback_input(interactive: bool) -> Result<Option<String>> {
    if !interactive {
        return Ok(None);
    }

    read_callback_input().map(Some)
}

fn read_callback_input() -> Result<String> {
    let mut stderr = io::stderr().lock();
    writeln!(
        stderr,
        "Paste the full callback URL from the hosted callback page, then press Enter."
    )
    .map_err(|error| Error::Io(format!("failed to write Schwab login prompt: {error}")))?;
    write!(stderr, "> ").map_err(|error| Error::Io(format!("failed to write Schwab login prompt: {error}")))?;
    stderr
        .flush()
        .map_err(|error| Error::Io(format!("failed to flush Schwab login prompt: {error}")))?;
    drop(stderr);

    let stdin = io::stdin();
    let mut input = String::new();
    let bytes_read = stdin
        .read_line(&mut input)
        .map_err(|error| Error::Io(format!("failed to read Schwab callback input: {error}")))?;
    if bytes_read == 0 {
        return Err(Error::Io(
            "stdin closed before Schwab callback input was provided".into(),
        ));
    }

    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(Error::Arguments(
            "callback input was empty, rerun `schwab auth login` and paste the full callback URL".into(),
        ));
    }

    Ok(trimmed.to_owned())
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

fn validate_callback(context: &ResolvedContext, callback: &CallbackInput) -> Result<()> {
    if let Some(expected_redirect_uri) = context.redirect_uri.as_deref() {
        let expected = normalize_redirect_uri(expected_redirect_uri)?;
        let received = normalize_redirect_uri(&callback.redirect_uri)?;
        if received != expected {
            return Err(Error::Arguments(format!(
                "callback redirect URI {received:?} did not match stored redirect URI {expected:?}"
            )));
        }
    }

    if let Some(expected_state) = context.pending_oauth_state.as_deref() {
        match callback.state.as_deref() {
            Some(received_state) if received_state == expected_state => {}
            Some(received_state) => {
                return Err(Error::Arguments(format!(
                    "callback state {received_state:?} did not match stored OAuth state"
                )))
            }
            None => {
                return Err(Error::Arguments(
                    "callback URL is missing the state query parameter".into(),
                ))
            }
        }
    }

    Ok(())
}

fn normalize_redirect_uri(value: &str) -> Result<String> {
    let mut url =
        Url::parse(value).map_err(|error| Error::Arguments(format!("redirect URI must be a valid URL: {error}")))?;
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn generate_oauth_state() -> Result<String> {
    Ok(hex_encode(&random_bytes(16)?))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn random_bytes(bytes: usize) -> Result<Vec<u8>> {
    #[cfg(unix)]
    {
        let mut buffer = vec![0u8; bytes];
        let mut file = File::open("/dev/urandom").map_err(|error| {
            Error::Io(format!(
                "failed to open /dev/urandom for OAuth state generation: {error}"
            ))
        })?;
        file.read_exact(&mut buffer).map_err(|error| {
            Error::Io(format!(
                "failed to read random bytes for OAuth state generation: {error}"
            ))
        })?;
        Ok(buffer)
    }

    #[cfg(not(unix))]
    {
        let _ = bytes;
        Err(Error::Config(
            "automatic OAuth state generation currently needs an explicit --state on non-unix platforms".into(),
        ))
    }
}
