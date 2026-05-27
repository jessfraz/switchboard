mod api_support;
mod args;
mod client;
mod commands;
mod discovery;
mod error;
mod oauth;
mod output;
mod presets;
mod state;

use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    io::Read,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result as AnyhowResult};
use clap::Parser;
use reqwest::Url;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(crate) use crate::{
    api_support::{
        fetch_capability_summary, merge_bundle_pages, normalize_operation_name, normalize_token,
        parse_api_resource_command, parse_oauth_token_response, render_api_result, require_capability,
        resolve_id_argument, ApiResourceCapability, CapabilitySummary, DynamicArgs, OAuthTokenResponse,
    },
    error::{Error, Result},
    output::render_json,
};
use crate::{
    args::{Cli, Commands, FinishCommand, LoginCommand},
    client::{normalize_api_base_url, JsonResponse, MyChartClient, ResolvedResponse},
    commands::{
        complete_or_wait_for_hosted_authorization, ensure_api_session, redirect_uri_uses_loopback, run_api,
        run_appointments, run_auth, run_authorize_url_command, run_claims, run_connect, run_exchange_url_command,
        run_labs, run_login_command, run_meds, run_notes, run_pack, run_portal, run_timeline, ApiSessionBootstrap,
        ApiSubcommand, AuthAuthorizeUrlArgs, AuthExchangeUrlArgs, AuthLoginArgs, HostedAuthorizationOutcome,
    },
    state::ResolvedContext,
};

/// Run the MyChart CLI and return a process exit code.
pub fn main_entry<I, T>(args: I) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error) => {
            let exit_code = match error.kind() {
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => ExitCode::SUCCESS,
                _ => ExitCode::FAILURE,
            };
            let _ = error.print();
            return exit_code;
        }
    };
    let compact = cli.global.compact;

    match run(cli) {
        Ok((output, compact)) => {
            println!("{}", render_json(&output, compact));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", output::render_cli_error(&error, compact));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> AnyhowResult<(Value, bool)> {
    let compact = cli.global.compact;
    let mut context = ResolvedContext::from_global(&cli.global).context("failed to resolve MyChart runtime context")?;

    let output = match cli.command {
        Commands::Login(command) => run_easy_login(command, &mut context),
        Commands::Finish(command) => run_easy_finish(command, &mut context),
        Commands::Connect(command) => run_connect(command.command, &mut context),
        Commands::Auth(command) => run_auth(command.command, &mut context),
        Commands::Api(command) => {
            if matches!(&command.command, ApiSubcommand::Resource(_)) {
                run_with_api_session(&mut context, move |context| run_api(command.command, context))
            } else {
                run_api(command.command, &mut context)
            }
        }
        Commands::Timeline(command) => {
            run_with_api_session(&mut context, move |context| run_timeline(command, context))
        }
        Commands::Labs(command) => {
            run_with_api_session(&mut context, move |context| run_labs(command.command, context))
        }
        Commands::Notes(command) => {
            run_with_api_session(&mut context, move |context| run_notes(command.command, context))
        }
        Commands::Meds(command) => {
            run_with_api_session(&mut context, move |context| run_meds(command.command, context))
        }
        Commands::Appointments(command) => {
            run_with_api_session(&mut context, move |context| run_appointments(command.command, context))
        }
        Commands::Claims(command) => {
            run_with_api_session(&mut context, move |context| run_claims(command.command, context))
        }
        Commands::Pack(command) => {
            run_with_api_session(&mut context, move |context| run_pack(command.command, context))
        }
        Commands::Portal(command) => run_portal(command.command, &mut context),
    }
    .context("MyChart command failed")?;

    Ok((output, compact))
}

fn run_easy_login(command: LoginCommand, context: &mut ResolvedContext) -> Result<Value> {
    if !command.target.is_empty() {
        let tokens = command
            .target
            .iter()
            .map(|token| OsString::from(token.as_str()))
            .collect::<Vec<_>>();
        crate::commands::connect::run_resolve_output(tokens, context)?;
    }

    let redirect_uri = context.require_redirect_uri(command.options.redirect_uri.clone())?;
    if command.dynamic_client || redirect_uri_uses_loopback(&redirect_uri)? {
        return run_login_command(
            AuthLoginArgs {
                options: command.options,
                timeout_seconds: command.timeout_seconds,
                no_open: command.no_open,
                dynamic_client: command.dynamic_client,
            },
            context,
        );
    }

    let output = run_authorize_url_command(
        AuthAuthorizeUrlArgs {
            options: command.options,
            no_store: false,
            no_open: command.no_open,
        },
        context,
    )?;
    match complete_or_wait_for_hosted_authorization(
        context,
        output,
        command.callback_url,
        "Finish the browser login, paste the copied login code back into this terminal, or run `mychart finish '<auth-code>'` later.",
    )? {
        HostedAuthorizationOutcome::Completed(output) => Ok(output),
        HostedAuthorizationOutcome::Pending(output) => Ok(output),
    }
}

fn run_easy_finish(command: FinishCommand, context: &mut ResolvedContext) -> Result<Value> {
    run_exchange_url_command(
        AuthExchangeUrlArgs {
            callback_input: command.callback_input,
            no_store: command.no_store,
        },
        context,
    )
}

fn run_with_api_session<F>(context: &mut ResolvedContext, operation: F) -> Result<Value>
where
    F: FnOnce(&mut ResolvedContext) -> Result<Value>,
{
    match ensure_api_session(context)? {
        ApiSessionBootstrap::Ready => operation(context),
        ApiSessionBootstrap::Pending(output) => Ok(output),
    }
}

fn api_client(base_url: &str) -> Result<MyChartClient> {
    MyChartClient::new(base_url.to_owned())
}

fn portal_client(base_url: &str) -> Result<MyChartClient> {
    MyChartClient::new(base_url.to_owned())
}

fn build_authorize_url(
    authorize_endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    base_url: &str,
    oauth_state: &str,
    code_verifier: &str,
    scopes: &[String],
) -> Result<Url> {
    let mut url = Url::parse(authorize_endpoint)
        .map_err(|error| Error::Config(format!("invalid authorize endpoint {authorize_endpoint:?}: {error}")))?;
    let code_challenge = base64_url_encode(&sha256(code_verifier.as_bytes()));
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("response_type", "code");
        pairs.append_pair("client_id", client_id);
        pairs.append_pair("redirect_uri", redirect_uri);
        pairs.append_pair("scope", &scopes.join(" "));
        pairs.append_pair("state", oauth_state);
        pairs.append_pair("aud", &normalize_api_base_url(base_url)?);
        pairs.append_pair("code_challenge", &code_challenge);
        pairs.append_pair("code_challenge_method", "S256");
    }
    Ok(url)
}

fn default_patient_scopes() -> Vec<String> {
    vec!["openid".into()]
}

fn dedupe_preserving_order(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for value in values {
        if seen.insert(value.clone()) {
            deduped.push(value);
        }
    }
    deduped
}

fn ensure_code_verifier(verifier: String) -> Result<String> {
    if !(43..=128).contains(&verifier.len()) {
        return Err(Error::Arguments(
            "PKCE code verifier must be between 43 and 128 characters".into(),
        ));
    }
    Ok(verifier)
}

pub(crate) fn generate_nonce(bytes: usize) -> Result<String> {
    Ok(base64_url_encode(&random_bytes(bytes)?))
}

fn random_bytes(bytes: usize) -> Result<Vec<u8>> {
    #[cfg(unix)]
    {
        let mut buffer = vec![0u8; bytes];
        let mut file = fs::File::open("/dev/urandom")
            .map_err(|error| Error::Io(format!("failed to open /dev/urandom for PKCE generation: {error}")))?;
        file.read_exact(&mut buffer)
            .map_err(|error| Error::Io(format!("failed to read random bytes for PKCE generation: {error}")))?;
        Ok(buffer)
    }

    #[cfg(not(unix))]
    {
        let _ = bytes;
        Err(Error::Config(
            "automatic PKCE generation currently needs an explicit --state and --code-verifier on non-unix platforms"
                .into(),
        ))
    }
}

fn parse_key_value(input: &str) -> std::result::Result<(String, String), String> {
    let (key, value) = input.split_once('=').ok_or_else(|| "expected KEY=VALUE".to_owned())?;
    if key.trim().is_empty() {
        return Err("query/form key cannot be empty".into());
    }
    Ok((key.trim().to_owned(), value.to_owned()))
}

fn ensure_portal_success_status(response: &ResolvedResponse) -> Result<()> {
    if response.status_code < 400 {
        return Ok(());
    }

    Err(Error::Api {
        status_code: response.status_code,
        body: json!({
            "final_url": response.final_url.as_str(),
            "content_type": response.content_type,
            "page": summarize_page(&response.final_url, &response.body_text),
            "body": parse_portal_response_body(response),
        }),
    })
}

fn ensure_json_success(response: &JsonResponse) -> Result<()> {
    if response.status_code < 400 {
        return Ok(());
    }

    Err(Error::Api {
        status_code: response.status_code,
        body: json!({
            "final_url": response.final_url.as_str(),
            "content_type": response.content_type,
            "body": response.body,
            "body_text": response.body_text,
        }),
    })
}

fn parse_portal_response_body(response: &ResolvedResponse) -> Value {
    if let Some(content_type) = response.content_type.as_deref() {
        if content_type.contains("json") {
            return serde_json::from_str(&response.body_text)
                .unwrap_or_else(|_| Value::String(response.body_text.clone()));
        }
    }

    serde_json::from_str(&response.body_text).unwrap_or_else(|_| Value::String(response.body_text.clone()))
}

fn summarize_page(url: &Url, body: &str) -> Value {
    json!({
        "title": extract_title(body),
        "csrf_token": extract_verification_token(body),
        "is_login_page": is_login_page(body),
        "looks_like_auth_challenge": looks_like_verification_challenge(url, body),
    })
}

fn extract_title(body: &str) -> Option<String> {
    let start = body.find("<title>")?;
    let rest = &body[start + "<title>".len()..];
    let end = rest.find("</title>")?;
    Some(rest[..end].trim().to_owned())
}

fn extract_verification_token(body: &str) -> Option<String> {
    extract_input_value(body, "__RequestVerificationToken")
}

fn extract_input_value(body: &str, name: &str) -> Option<String> {
    let marker = format!("name=\"{name}\"");
    let marker_index = body.find(&marker)?;
    let tag_start = body[..marker_index].rfind("<input")?;
    let tag_end = body[marker_index..].find('>')?;
    let tag = &body[tag_start..marker_index + tag_end];
    extract_attribute(tag, "value")
}

fn extract_attribute(tag: &str, attribute: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let marker = format!("{attribute}={quote}");
        if let Some(start) = tag.find(&marker) {
            let rest = &tag[start + marker.len()..];
            let end = rest.find(quote)?;
            return Some(rest[..end].to_owned());
        }
    }
    None
}

fn is_login_page(body: &str) -> bool {
    extract_title(body)
        .map(|title| title.to_ascii_lowercase().contains("login page"))
        .unwrap_or(false)
        && body.contains("id=\"loginForm\"")
}

fn looks_like_verification_challenge(url: &Url, body: &str) -> bool {
    let path = url.path().to_ascii_lowercase();
    if path.contains("/authentication/") && !path.ends_with("/authentication/login") {
        return true;
    }

    !is_login_page(body) && body.to_ascii_lowercase().contains("two-factor")
}

fn extract_login_error(url: &Url) -> Option<String> {
    url.query_pairs()
        .find_map(|(key, value)| (key == "error").then(|| value.to_string()))
}

fn login_error_message(code: &str) -> String {
    match code {
        "usernameloginfailed" => "MyChart rejected the username or password".into(),
        "accountlocked" => "MyChart reported that the account is locked".into(),
        other => format!("MyChart rejected the login flow with error code {other}"),
    }
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    base64_encode_with_alphabet(
        bytes,
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/",
        true,
    )
}

pub(crate) fn base64_url_encode(bytes: &[u8]) -> String {
    base64_encode_with_alphabet(
        bytes,
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_",
        false,
    )
}

fn base64_encode_with_alphabet(bytes: &[u8], alphabet: &[u8; 64], padded: bool) -> String {
    let mut encoded = String::new();

    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);

        let index0 = first >> 2;
        let index1 = ((first & 0b0000_0011) << 4) | (second >> 4);
        let index2 = ((second & 0b0000_1111) << 2) | (third >> 6);
        let index3 = third & 0b0011_1111;

        encoded.push(alphabet[index0 as usize] as char);
        encoded.push(alphabet[index1 as usize] as char);
        if chunk.len() > 1 {
            encoded.push(alphabet[index2 as usize] as char);
        } else if padded {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(alphabet[index3 as usize] as char);
        } else if padded {
            encoded.push('=');
        }
    }

    encoded
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let hash = Sha256::digest(bytes);
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&hash);
    digest
}

fn expires_at_epoch_seconds(expires_in: u64) -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().saturating_add(expires_in))
        .unwrap_or(expires_in)
}

fn split_scopes(scope: Option<&str>) -> Value {
    scope
        .map(|scope| {
            Value::Array(
                scope
                    .split_whitespace()
                    .map(|item| Value::String(item.to_owned()))
                    .collect(),
            )
        })
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests;
