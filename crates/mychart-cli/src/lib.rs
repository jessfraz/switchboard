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
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::Read,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result as AnyhowResult};
use clap::Parser;
use reqwest::{Method, Url};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::args::{Cli, Commands, FinishCommand, LoginCommand};
pub(crate) use crate::error::{Error, Result};
use crate::{
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

fn fetch_capability_summary(client: &MyChartClient, epic_client_id: Option<&str>) -> Result<CapabilitySummary> {
    let response = client.fetch_capability_statement(epic_client_id)?;
    ensure_json_success(&response)?;
    CapabilitySummary::from_value(response.body)
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

fn resolve_id_argument(args: &mut DynamicArgs) -> Result<String> {
    if let Some(id) = args.take_optional_single("id")? {
        return Ok(id);
    }
    if args.positionals.len() == 1 {
        return Ok(args.positionals.remove(0));
    }
    if args.positionals.is_empty() {
        return Err(Error::Arguments(
            "missing resource id, pass it positionally or with --id".into(),
        ));
    }
    Err(Error::Arguments(
        "too many positional arguments, only the resource id is allowed here".into(),
    ))
}

fn merge_bundle_pages(client: &MyChartClient, first_body: &Value, access_token: &str) -> Result<(Value, usize)> {
    if first_body.get("resourceType").and_then(Value::as_str) != Some("Bundle") {
        return Ok((first_body.clone(), 1));
    }

    let mut merged = first_body.clone();
    let mut entries = merged
        .get("entry")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut pages_fetched = 1;
    let mut next_url = bundle_next_link(first_body);

    while let Some(url) = next_url {
        let response = client.execute_bearer_json_absolute(Method::GET, &url, access_token, None)?;
        ensure_json_success(&response)?;
        if response.body.get("resourceType").and_then(Value::as_str) != Some("Bundle") {
            return Err(Error::Api {
                status_code: response.status_code,
                body: json!({
                    "message": "expected a FHIR Bundle while following next links",
                    "body": response.body,
                }),
            });
        }
        if let Some(next_entries) = response.body.get("entry").and_then(Value::as_array) {
            entries.extend(next_entries.clone());
        }
        pages_fetched += 1;
        next_url = bundle_next_link(&response.body);
    }

    if let Some(object) = merged.as_object_mut() {
        object.insert("entry".into(), Value::Array(entries));
        if let Some(links) = object.get_mut("link").and_then(Value::as_array_mut) {
            links.retain(|link| link.get("relation").and_then(Value::as_str) != Some("next"));
        }
    }

    Ok((merged, pages_fetched))
}

fn bundle_next_link(body: &Value) -> Option<String> {
    body.get("link")?
        .as_array()?
        .iter()
        .find(|link| link.get("relation").and_then(Value::as_str) == Some("next"))
        .and_then(|link| link.get("url").and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn render_api_result(
    resource: &ApiResourceCapability,
    operation: &str,
    response: &JsonResponse,
    body: Value,
    pages_fetched: usize,
) -> Value {
    json!({
        "status": "ok",
        "resource": resource.resource_type,
        "cli_name": resource.cli_name,
        "operation": operation,
        "pages_fetched": pages_fetched,
        "response": {
            "status_code": response.status_code,
            "final_url": response.final_url.as_str(),
            "content_type": response.content_type,
        },
        "body": body,
    })
}

fn require_capability(resource: &ApiResourceCapability, interaction: &str, operation: &str) -> Result<()> {
    if resource.supports(interaction) {
        Ok(())
    } else {
        Err(Error::Arguments(format!(
            "{} does not support {} on this patient endpoint",
            resource.resource_type, operation
        )))
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
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
        0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
        0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
        0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    let bit_len = (bytes.len() as u64) * 8;
    let mut padded = bytes.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut w = [0u32; 64];
    for chunk in padded.chunks_exact(64) {
        for (index, word) in chunk.chunks_exact(4).enumerate().take(16) {
            w[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7) ^ w[index - 15].rotate_right(18) ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17) ^ w[index - 2].rotate_right(19) ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let mut a: u32 = h[0];
        let mut b: u32 = h[1];
        let mut c: u32 = h[2];
        let mut d: u32 = h[3];
        let mut e: u32 = h[4];
        let mut f: u32 = h[5];
        let mut g: u32 = h[6];
        let mut hh: u32 = h[7];

        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut digest = [0u8; 32];
    for (index, word) in h.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
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

fn normalize_operation_name(input: &str) -> String {
    match normalize_token(input).as_str() {
        "get" | "read" => "read".into(),
        "search" | "list" => "search-type".into(),
        other => other.to_owned(),
    }
}

fn normalize_token(input: &str) -> String {
    input
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(|character| character.to_lowercase())
        .collect()
}

fn normalize_query_name(name: &str) -> String {
    if name.starts_with('_') {
        return name.to_owned();
    }

    match name {
        "count" => "_count".into(),
        "include" => "_include".into(),
        "rev-include" | "revinclude" => "_revinclude".into(),
        other => other.to_owned(),
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

#[derive(Debug)]
struct ParsedApiResourceCommand {
    resource: String,
    operation: String,
    args: DynamicArgs,
}

fn parse_api_resource_command(tokens: Vec<OsString>) -> Result<ParsedApiResourceCommand> {
    let tokens = tokens
        .into_iter()
        .map(|token| token.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return Err(Error::Arguments(
            "missing resource name, expected something like `mychart api appointment search --patient 123`".into(),
        ));
    }
    if tokens.len() == 1 {
        return Err(Error::Arguments(
            "missing resource operation, expected get/read or search".into(),
        ));
    }
    Ok(ParsedApiResourceCommand {
        resource: tokens[0].clone(),
        operation: tokens[1].clone(),
        args: DynamicArgs::parse(&tokens[2..])?,
    })
}

#[derive(Debug, Clone, Default)]
struct DynamicArgs {
    options: BTreeMap<String, Vec<String>>,
    flags: BTreeSet<String>,
    positionals: Vec<String>,
}

impl DynamicArgs {
    fn parse(tokens: &[String]) -> Result<Self> {
        let mut parsed = Self::default();
        let mut index = 0;

        while index < tokens.len() {
            let current = &tokens[index];
            if let Some(trimmed) = current.strip_prefix("--") {
                if let Some((name, value)) = trimmed.split_once('=') {
                    parsed.push_option(name, value.to_owned())?;
                    index += 1;
                    continue;
                }

                let next = tokens.get(index + 1);
                if next.is_none() || next.is_some_and(|value| value.starts_with("--")) {
                    parsed.flags.insert(trimmed.to_owned());
                    index += 1;
                } else if let Some(next) = next {
                    parsed.push_option(trimmed, next.clone())?;
                    index += 2;
                }
            } else {
                parsed.positionals.push(current.clone());
                index += 1;
            }
        }

        Ok(parsed)
    }

    fn push_option(&mut self, name: &str, value: String) -> Result<()> {
        if name.trim().is_empty() {
            return Err(Error::Arguments("option names cannot be empty".into()));
        }
        self.options.entry(name.to_owned()).or_default().push(value);
        Ok(())
    }

    fn take_flag(&mut self, name: &str) -> bool {
        self.flags.remove(name)
    }

    fn take_optional_single(&mut self, name: &str) -> Result<Option<String>> {
        match self.options.remove(name) {
            None => Ok(None),
            Some(mut values) if values.len() == 1 => Ok(values.pop()),
            Some(_) => Err(Error::Arguments(format!(
                "--{name} may only be provided once for this operation"
            ))),
        }
    }

    fn into_query_pairs(mut self) -> Result<Vec<(String, String)>> {
        if !self.positionals.is_empty() {
            return Err(Error::Arguments(format!(
                "unexpected positional arguments: {}",
                self.positionals.join(", ")
            )));
        }

        let mut query = Vec::new();
        if let Some(values) = self.options.remove("query") {
            for value in values {
                let (key, value) = parse_key_value(&value).map_err(Error::Arguments)?;
                query.push((key, value));
            }
        }

        for (name, values) in self.options {
            let name = normalize_query_name(&name);
            for value in values {
                query.push((name.clone(), value));
            }
        }

        for flag in self.flags {
            query.push((normalize_query_name(&flag), "true".into()));
        }

        Ok(query)
    }
}

#[derive(Debug, Clone)]
struct ApiResourceCapability {
    resource_type: String,
    cli_name: String,
    interactions: Vec<String>,
    search_params: Vec<ApiSearchParamCapability>,
    supported_profiles: Vec<String>,
}

impl ApiResourceCapability {
    fn supports(&self, interaction: &str) -> bool {
        self.interactions.iter().any(|candidate| candidate == interaction)
    }

    fn render(&self, details: bool) -> Value {
        if !details {
            return json!({
                "resource": self.resource_type,
                "cli_name": self.cli_name,
                "interactions": self.interactions,
                "search_param_count": self.search_params.len(),
            });
        }

        json!({
            "resource": self.resource_type,
            "cli_name": self.cli_name,
            "interactions": self.interactions,
            "supported_profiles": self.supported_profiles,
            "search_params": self.search_params.iter().map(ApiSearchParamCapability::render).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Clone)]
struct ApiSearchParamCapability {
    name: String,
    parameter_type: Option<String>,
    documentation: Option<String>,
}

impl ApiSearchParamCapability {
    fn render(&self) -> Value {
        json!({
            "name": self.name,
            "type": self.parameter_type,
            "documentation": self.documentation,
        })
    }
}

#[derive(Debug, Clone)]
struct CapabilitySummary {
    authorize_url: Option<String>,
    token_url: Option<String>,
    register_url: Option<String>,
    fhir_version: Option<String>,
    software_name: Option<String>,
    software_version: Option<String>,
    implementation_url: Option<String>,
    resources: Vec<ApiResourceCapability>,
}

impl CapabilitySummary {
    fn from_value(value: Value) -> Result<Self> {
        let document: CapabilityDocument = serde_json::from_value(value).map_err(|error| {
            Error::Config(format!(
                "failed to parse capability statement JSON from MyChart: {error}"
            ))
        })?;
        let rest = document
            .rest
            .into_iter()
            .find(|rest| rest.mode.as_deref() == Some("server"))
            .ok_or_else(|| Error::Config("capability statement did not include a server REST block".into()))?;
        let oauth_uris = rest.security.as_ref().and_then(|security| {
            security
                .extension
                .iter()
                .find(|extension| extension.url.ends_with("oauth-uris"))
        });

        let authorize_url = oauth_uris
            .and_then(|extension| extension.extension.iter().find(|child| child.url == "authorize"))
            .and_then(|extension| extension.value_uri.clone());
        let token_url = oauth_uris
            .and_then(|extension| extension.extension.iter().find(|child| child.url == "token"))
            .and_then(|extension| extension.value_uri.clone());
        let register_url = oauth_uris
            .and_then(|extension| extension.extension.iter().find(|child| child.url == "register"))
            .and_then(|extension| extension.value_uri.clone());

        let mut resources = rest
            .resource
            .into_iter()
            .map(|resource| ApiResourceCapability {
                cli_name: cli_resource_name(&resource.resource_type),
                resource_type: resource.resource_type,
                interactions: resource
                    .interaction
                    .into_iter()
                    .map(|interaction| interaction.code)
                    .collect(),
                search_params: resource
                    .search_param
                    .into_iter()
                    .map(|search_param| ApiSearchParamCapability {
                        name: search_param.name,
                        parameter_type: search_param.parameter_type,
                        documentation: search_param.documentation,
                    })
                    .collect(),
                supported_profiles: resource.supported_profile,
            })
            .collect::<Vec<_>>();
        resources.sort_by(|left, right| left.resource_type.cmp(&right.resource_type));

        let (software_name, software_version) = match document.software {
            Some(software) => (software.name, software.version),
            None => (None, None),
        };

        Ok(Self {
            authorize_url,
            token_url,
            register_url,
            fhir_version: document.fhir_version,
            software_name,
            software_version,
            implementation_url: document.implementation.and_then(|implementation| implementation.url),
            resources,
        })
    }

    fn require_authorize_url(&self) -> Result<String> {
        self.authorize_url
            .clone()
            .ok_or_else(|| Error::Config("capability statement did not advertise a SMART authorize endpoint".into()))
    }

    fn require_token_url(&self) -> Result<String> {
        self.token_url
            .clone()
            .ok_or_else(|| Error::Config("capability statement did not advertise a SMART token endpoint".into()))
    }

    fn require_register_url(&self) -> Result<String> {
        self.register_url.clone().ok_or_else(|| {
            Error::Config("capability statement did not advertise a SMART dynamic client registration endpoint".into())
        })
    }

    fn resolve_resource(&self, token: &str) -> Option<ApiResourceCapability> {
        let normalized = normalize_token(token);
        self.resources
            .iter()
            .find(|resource| {
                normalize_token(&resource.resource_type) == normalized
                    || normalize_token(&resource.cli_name) == normalized
            })
            .cloned()
    }
}

fn cli_resource_name(resource_type: &str) -> String {
    let mut cli_name = String::new();
    for (index, character) in resource_type.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                cli_name.push('-');
            }
            cli_name.push(character.to_ascii_lowercase());
        } else {
            cli_name.push(character);
        }
    }
    cli_name
}

#[derive(Debug, Deserialize)]
struct CapabilityDocument {
    #[serde(default)]
    rest: Vec<CapabilityRest>,
    #[serde(rename = "fhirVersion")]
    fhir_version: Option<String>,
    software: Option<CapabilitySoftware>,
    implementation: Option<CapabilityImplementation>,
}

#[derive(Debug, Deserialize)]
struct CapabilitySoftware {
    name: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CapabilityImplementation {
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CapabilityRest {
    mode: Option<String>,
    security: Option<CapabilitySecurity>,
    #[serde(default)]
    resource: Vec<CapabilityResource>,
}

#[derive(Debug, Deserialize)]
struct CapabilitySecurity {
    #[serde(default)]
    extension: Vec<CapabilityExtension>,
}

#[derive(Debug, Deserialize, Clone)]
struct CapabilityExtension {
    url: String,
    #[serde(default)]
    extension: Vec<CapabilityExtension>,
    #[serde(rename = "valueUri")]
    value_uri: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CapabilityResource {
    #[serde(rename = "type")]
    resource_type: String,
    #[serde(default)]
    interaction: Vec<CapabilityInteraction>,
    #[serde(default, rename = "searchParam")]
    search_param: Vec<CapabilitySearchParam>,
    #[serde(default, rename = "supportedProfile")]
    supported_profile: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CapabilityInteraction {
    code: String,
}

#[derive(Debug, Deserialize)]
struct CapabilitySearchParam {
    name: String,
    #[serde(rename = "type")]
    parameter_type: Option<String>,
    documentation: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OAuthTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    token_type: Option<String>,
    scope: Option<String>,
    patient: Option<String>,
    expires_in: Option<u64>,
}

fn parse_oauth_token_response(value: &Value) -> Result<OAuthTokenResponse> {
    serde_json::from_value(value.clone()).map_err(|error| Error::Auth {
        message: "MyChart returned a token response we could not parse".into(),
        details: json!({
            "error": error.to_string(),
            "body": value,
        }),
    })
}

#[cfg(test)]
mod tests;
