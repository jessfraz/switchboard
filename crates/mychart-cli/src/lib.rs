mod client;
mod commands;
mod discovery;
mod error;
mod state;

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::Read,
    path::PathBuf,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::{Args, Parser, Subcommand};
use reqwest::{Method, Url};
use serde::Deserialize;
use serde_json::{json, Value};

pub(crate) use crate::error::{Error, Result};
#[cfg(test)]
use crate::state::{MyChartState, StateStore};
use crate::{
    client::{normalize_api_base_url, JsonResponse, MyChartClient, ResolvedResponse},
    commands::{
        run_api, run_appointments, run_auth, run_claims, run_connect, run_labs, run_meds, run_notes, run_pack,
        run_portal, run_timeline, ApiCommand, AppointmentsCommand, AuthCommand, ClaimsCommand, ConnectCommand,
        LabsCommand, MedsCommand, NotesCommand, PackCommand, PortalCommand, TimelineCommand,
    },
    state::{
        ResolvedContext, ENV_MYCHART_ACCESS_TOKEN, ENV_MYCHART_ACCOUNT, ENV_MYCHART_BASE_URL, ENV_MYCHART_CLIENT_ID,
        ENV_MYCHART_CLIENT_SECRET, ENV_MYCHART_CONFIG, ENV_MYCHART_PORTAL_BASE_URL, ENV_MYCHART_REDIRECT_URI,
        ENV_MYCHART_REFRESH_TOKEN, ENV_MYCHART_USERNAME,
    },
};

const AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  mychart connect search ucla\n",
    "  mychart connect ucla medical center\n",
    "  mychart auth login --base-url https://fhir.example.org/api/FHIR/R4 \\\n",
    "    --client-id <id> --redirect-uri http://127.0.0.1:8910/callback --scope patient/*.read\n",
    "  mychart auth authorize-url --base-url https://fhir.example.org/api/FHIR/R4 \\\n",
    "    --client-id <id> --redirect-uri http://127.0.0.1:8910/callback\n",
    "  mychart timeline --limit 25\n",
    "  mychart labs a1c ferritin tsh --spark\n",
    "  mychart appointments upcoming --limit 5\n",
    "  mychart appointments find derm --next 30d\n",
    "  mychart meds reconcile --all-providers\n",
    "  mychart notes search --query migraine\n",
    "  mychart notes get note-123\n",
    "  mychart claims audit --since 1y\n",
    "  mychart pack doctor\n",
    "  mychart auth exchange-code --code <oauth-code>\n",
    "  mychart api resources --details\n",
    "  mychart api appointment search --patient 123 --date ge2026-03-01 --status booked\n",
    "  mychart api observation get obs-123\n",
    "  mychart portal auth login-password --portal-base-url https://my.uclahealth.org/MyChart \\\n",
    "    --username you@example.com --password 'super-secret'\n",
    "\n",
    "This CLI targets the patient-facing Epic SMART on FHIR surface first, with a resource-driven command grammar\n",
    "that is pleasant for both humans and switchboard to synthesize. The legacy portal session commands stay under\n",
    "`mychart portal ...` for the weird corners Epic still refuses to expose cleanly.\n",
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

    let output = match cli.command {
        Commands::Connect(command) => run_connect(command.command, &mut context),
        Commands::Auth(command) => run_auth(command.command, &mut context),
        Commands::Api(command) => run_api(command.command, &mut context),
        Commands::Timeline(command) => run_timeline(command, &context),
        Commands::Labs(command) => run_labs(command.command, &context),
        Commands::Notes(command) => run_notes(command.command, &context),
        Commands::Meds(command) => run_meds(command.command, &context),
        Commands::Appointments(command) => run_appointments(command.command, &context),
        Commands::Claims(command) => run_claims(command.command, &context),
        Commands::Pack(command) => run_pack(command.command, &context),
        Commands::Portal(command) => run_portal(command.command, &mut context),
    }
    .map_err(|error| (error, compact))?;

    Ok((output, compact))
}

#[derive(Debug, Parser)]
#[command(
    name = "mychart",
    version,
    about = "CLI for patient-facing Epic SMART on FHIR workflows, provider discovery, and MyChart portal fallbacks",
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
    #[arg(long, global = true, env = ENV_MYCHART_CONFIG, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, global = true, env = ENV_MYCHART_ACCOUNT, value_name = "ACCOUNT")]
    account: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_BASE_URL, value_name = "URL")]
    base_url: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_PORTAL_BASE_URL, value_name = "URL")]
    portal_base_url: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_CLIENT_ID, value_name = "CLIENT_ID")]
    client_id: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_CLIENT_SECRET, value_name = "CLIENT_SECRET")]
    client_secret: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_REDIRECT_URI, value_name = "URL")]
    redirect_uri: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_ACCESS_TOKEN, value_name = "TOKEN")]
    access_token: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_REFRESH_TOKEN, value_name = "TOKEN")]
    refresh_token: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_USERNAME, value_name = "USERNAME")]
    username: Option<String>,

    #[arg(long, global = true)]
    compact: bool,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Connect(ConnectCommand),
    Auth(AuthCommand),
    Api(ApiCommand),
    Timeline(TimelineCommand),
    Labs(LabsCommand),
    Notes(NotesCommand),
    Meds(MedsCommand),
    Appointments(AppointmentsCommand),
    Claims(ClaimsCommand),
    Pack(PackCommand),
    Portal(PortalCommand),
}

fn api_client(base_url: &str) -> Result<MyChartClient> {
    MyChartClient::new(base_url.to_owned())
}

fn portal_client(base_url: &str) -> Result<MyChartClient> {
    MyChartClient::new(base_url.to_owned())
}

fn fetch_capability_summary(client: &MyChartClient) -> Result<CapabilitySummary> {
    let response = client.fetch_capability_statement()?;
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
    vec![
        "openid".into(),
        "fhirUser".into(),
        "offline_access".into(),
        "patient/*.read".into(),
        "patient/*.write".into(),
    ]
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

fn generate_nonce(bytes: usize) -> Result<String> {
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

fn prepare_resource_body(resource: &ApiResourceCapability, id: Option<String>, mut body: Value) -> Result<Value> {
    let object = body.as_object_mut().ok_or_else(|| {
        Error::Arguments("FHIR resource bodies must be JSON objects, not loose arrays or scalars".into())
    })?;

    match object.get("resourceType").and_then(Value::as_str) {
        Some(resource_type) if resource_type != resource.resource_type => {
            return Err(Error::Arguments(format!(
                "body resourceType {:?} does not match requested resource {:?}",
                resource_type, resource.resource_type
            )))
        }
        None => {
            object.insert("resourceType".into(), Value::String(resource.resource_type.clone()));
        }
        _ => {}
    }

    if let Some(id) = id {
        match object.get("id").and_then(Value::as_str) {
            Some(existing) if existing != id => {
                return Err(Error::Arguments(format!(
                    "body id {existing:?} does not match requested resource id {id:?}"
                )))
            }
            None => {
                object.insert("id".into(), Value::String(id));
            }
            _ => {}
        }
    }

    Ok(body)
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

fn base64_url_encode(bytes: &[u8]) -> String {
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
        "create" => "create".into(),
        "update" | "put" => "update".into(),
        "delete" | "remove" => "delete".into(),
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
            "missing resource operation, expected get/read, search, create, update, or delete".into(),
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

    fn require_json_body(&mut self) -> Result<Value> {
        let inline_body = self.take_optional_single("body")?;
        let body_file = self.take_optional_single("body-file")?;
        match (inline_body, body_file) {
            (Some(_), Some(_)) => Err(Error::Arguments("pass either --body or --body-file, not both".into())),
            (Some(body), None) => serde_json::from_str(&body)
                .map_err(|error| Error::Arguments(format!("failed to parse --body as JSON: {error}"))),
            (None, Some(path)) => {
                let contents = fs::read_to_string(&path)
                    .map_err(|error| Error::Io(format!("failed to read body file {path}: {error}")))?;
                serde_json::from_str(&contents)
                    .map_err(|error| Error::Arguments(format!("failed to parse JSON in {path}: {error}")))
            }
            (None, None) => Err(Error::Arguments(
                "missing JSON request body, pass --body '{...}' or --body-file path.json".into(),
            )),
        }
    }

    fn take_query_pairs(&mut self) -> Result<Vec<(String, String)>> {
        let options = std::mem::take(&mut self.options);
        let flags = std::mem::take(&mut self.flags);
        self.positionals.clear();
        DynamicArgs {
            options,
            flags,
            positionals: Vec::new(),
        }
        .into_query_pairs()
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
struct OAuthTokenResponse {
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
mod tests {
    use std::{
        collections::BTreeMap,
        ffi::OsString,
        fs,
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use clap::Parser;
    use serde_json::{json, Value};

    use super::{run, Cli, GlobalArgs, MyChartState, ResolvedContext, StateStore};

    #[test]
    fn authorize_url_discovers_smart_endpoints_and_stores_pkce_state() {
        let server = TestServer::spawn(vec![ResponseSpec::json(
            200,
            capability_statement_json("http://placeholder", &[]),
            Vec::new(),
        )]);
        let temp_dir = temp_dir("mychart-authorize-url");
        let config_path = temp_dir.join("config.json");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--base-url",
            &format!("{}/", server.base_url()),
            "--client-id",
            "client-123",
            "--redirect-uri",
            "http://127.0.0.1:8910/callback",
            "--compact",
            "auth",
            "authorize-url",
        ]);

        assert_eq!(output["status"], "ok");
        assert!(output["authorize_url"]
            .as_str()
            .expect("authorize url should be string")
            .contains("response_type=code"));
        let authorize_url = reqwest::Url::parse(
            output["authorize_url"]
                .as_str()
                .expect("authorize url should be string"),
        )
        .expect("authorize url should parse");
        let expected_base_url = server.base_url();
        let aud = authorize_url
            .query_pairs()
            .find(|(key, _)| key == "aud")
            .map(|(_, value)| value.to_string())
            .expect("aud query param should be present");
        assert_eq!(aud, expected_base_url);

        let state = StateStore::new(config_path).load().expect("state should load");
        let account = state
            .accounts
            .get("default")
            .expect("default account should be persisted");
        assert_eq!(state.current_account.as_deref(), Some("default"));
        assert_eq!(account.api_base_url.as_deref(), Some(expected_base_url.as_str()));
        assert_eq!(account.client_id.as_deref(), Some("client-123"));
        assert!(account.pending_code_verifier.is_some());
    }

    #[test]
    fn exchange_code_stores_tokens_for_api_use() {
        let server = TestServer::spawn(vec![
            ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
            ResponseSpec::json(
                200,
                json!({
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "token_type": "Bearer",
                    "scope": "patient/*.read patient/*.write",
                    "patient": "patient-123",
                    "expires_in": 3600
                }),
                Vec::new(),
            ),
        ]);
        let temp_dir = temp_dir("mychart-exchange");
        let config_path = temp_dir.join("config.json");
        StateStore::new(config_path.clone())
            .save(&MyChartState {
                api_base_url: Some(server.base_url()),
                client_id: Some("client-123".into()),
                redirect_uri: Some("http://127.0.0.1:8910/callback".into()),
                pending_code_verifier: Some("abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJK".into()),
                ..MyChartState::default()
            })
            .expect("state should save");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "auth",
            "exchange-code",
            "--code",
            "oauth-code",
        ]);

        assert_eq!(output["status"], "authenticated");

        let state = StateStore::new(config_path).load().expect("state should load");
        let account = state
            .accounts
            .get("default")
            .expect("default account should be persisted");
        assert_eq!(account.access_token.as_deref(), Some("access-token"));
        assert_eq!(account.patient_id.as_deref(), Some("patient-123"));
    }

    #[test]
    fn exchange_code_uses_basic_auth_for_confidential_clients() {
        let server = TestServer::spawn(vec![
            ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
            ResponseSpec::json(
                200,
                json!({
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "token_type": "Bearer",
                    "scope": "patient/*.read offline_access",
                    "patient": "patient-123",
                    "expires_in": 3600
                }),
                Vec::new(),
            ),
        ]);
        let temp_dir = temp_dir("mychart-exchange-confidential");
        let config_path = temp_dir.join("config.json");
        StateStore::new(config_path.clone())
            .save(&MyChartState {
                api_base_url: Some(server.base_url()),
                client_id: Some("d45049c3-3441-40ef-ab4d-b9cd86a17225".into()),
                client_secret: Some("this-is-the-secret-2/7".into()),
                redirect_uri: Some("http://127.0.0.1:8910/callback".into()),
                pending_code_verifier: Some("abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJK".into()),
                ..MyChartState::default()
            })
            .expect("state should save");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "auth",
            "exchange-code",
            "--code",
            "oauth-code",
        ]);

        assert_eq!(output["status"], "authenticated");
        let requests = server.requests();
        let token_request = requests.get(1).expect("token request should be captured");
        assert!(token_request.contains(
            "authorization: Basic ZDQ1MDQ5YzMtMzQ0MS00MGVmLWFiNGQtYjljZDg2YTE3MjI1OnRoaXMtaXMtdGhlLXNlY3JldC0yJTJGNw=="
        ));
        assert!(!token_request.contains("client_secret="));
        assert!(!token_request.contains("client_id="));
    }

    #[test]
    fn refresh_uses_basic_auth_for_confidential_clients() {
        let server = TestServer::spawn(vec![
            ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
            ResponseSpec::json(
                200,
                json!({
                    "access_token": "new-access-token",
                    "refresh_token": "next-refresh-token",
                    "token_type": "Bearer",
                    "scope": "patient/*.read offline_access",
                    "patient": "patient-123",
                    "expires_in": 3600
                }),
                Vec::new(),
            ),
        ]);
        let temp_dir = temp_dir("mychart-refresh-confidential");
        let config_path = temp_dir.join("config.json");
        StateStore::new(config_path.clone())
            .save(&MyChartState {
                api_base_url: Some(server.base_url()),
                client_id: Some("d45049c3-3441-40ef-ab4d-b9cd86a17225".into()),
                client_secret: Some("this-is-the-secret-2/7".into()),
                redirect_uri: Some("http://127.0.0.1:8910/callback".into()),
                refresh_token: Some("refresh-token".into()),
                ..MyChartState::default()
            })
            .expect("state should save");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "auth",
            "refresh",
        ]);

        assert_eq!(output["status"], "refreshed");
        let requests = server.requests();
        let token_request = requests.get(1).expect("token request should be captured");
        assert!(token_request.contains(
            "authorization: Basic ZDQ1MDQ5YzMtMzQ0MS00MGVmLWFiNGQtYjljZDg2YTE3MjI1OnRoaXMtaXMtdGhlLXNlY3JldC0yJTJGNw=="
        ));
        assert!(!token_request.contains("client_secret="));
        assert!(!token_request.contains("client_id="));
    }

    #[test]
    fn auth_login_receives_loopback_callback_and_exchanges_code() {
        let server = TestServer::spawn(vec![
            ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
            ResponseSpec::json(200, capability_statement_json("http://placeholder", &[]), Vec::new()),
            ResponseSpec::json(
                200,
                json!({
                    "access_token": "access-token",
                    "refresh_token": "refresh-token",
                    "token_type": "Bearer",
                    "scope": "patient/*.read",
                    "patient": "patient-123",
                    "expires_in": 3600
                }),
                Vec::new(),
            ),
        ]);
        let temp_dir = temp_dir("mychart-auth-login");
        let config_path = temp_dir.join("config.json");
        let callback_listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let callback_port = callback_listener
            .local_addr()
            .expect("listener should have local addr")
            .port();
        drop(callback_listener);
        let redirect_uri = format!("http://127.0.0.1:{callback_port}/callback");
        let config_path_for_thread = config_path.clone();
        let server_base_url = format!("{}/", server.base_url());

        let handle = thread::spawn(move || {
            run_command(&[
                "mychart",
                "--config",
                config_path_for_thread.to_str().expect("config path should be utf-8"),
                "--base-url",
                &server_base_url,
                "--client-id",
                "client-123",
                "--redirect-uri",
                &redirect_uri,
                "--compact",
                "auth",
                "login",
                "--no-open",
                "--scope",
                "patient/*.read",
                "--state",
                "test-state",
                "--code-verifier",
                "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJK",
            ])
        });

        let callback_sent = wait_for_callback_response(
            callback_port,
            "GET /callback?code=oauth-code&state=test-state HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        );
        assert!(callback_sent.contains("You can close this tab"));

        let output = handle.join().expect("auth login thread should finish");
        assert_eq!(output["status"], "authenticated");
        assert_eq!(output["patient_id"], "patient-123");

        let state = StateStore::new(config_path).load().expect("state should load");
        let account = state
            .accounts
            .get("default")
            .expect("default account should be persisted");
        assert_eq!(account.access_token.as_deref(), Some("access-token"));
        assert_eq!(account.patient_id.as_deref(), Some("patient-123"));
    }

    #[test]
    fn api_resources_lists_patient_facing_resource_metadata() {
        let server = TestServer::spawn(vec![ResponseSpec::json(
            200,
            capability_statement_json(
                "http://placeholder",
                &[
                    resource_capability("Patient", &["read", "search-type"]),
                    resource_capability("Observation", &["create", "read", "search-type", "update"]),
                ],
            ),
            Vec::new(),
        )]);
        let temp_dir = temp_dir("mychart-resources");
        let config_path = temp_dir.join("config.json");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--base-url",
            &server.base_url(),
            "--compact",
            "api",
            "resources",
        ]);

        assert_eq!(output["resource_count"], 2);
        assert_eq!(output["resources"][0]["resource"], "Observation");
        assert_eq!(output["resources"][1]["resource"], "Patient");
    }

    #[test]
    fn api_search_maps_dynamic_flags_to_fhir_query_params() {
        let server = TestServer::spawn(vec![
            ResponseSpec::json(
                200,
                capability_statement_json(
                    "http://placeholder",
                    &[resource_capability("Appointment", &["read", "search-type"])],
                ),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [{"resource": {"resourceType": "Appointment", "id": "appt-1"}}]
                }),
                Vec::new(),
            ),
        ]);
        let temp_dir = temp_dir("mychart-search");
        let config_path = temp_dir.join("config.json");
        StateStore::new(config_path.clone())
            .save(&MyChartState {
                api_base_url: Some(server.base_url()),
                access_token: Some("access-token".into()),
                ..MyChartState::default()
            })
            .expect("state should save");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "api",
            "appointment",
            "search",
            "--patient",
            "patient-123",
            "--date",
            "ge2026-03-01",
            "--count",
            "1",
        ]);

        assert_eq!(output["status"], "ok");
        let requests = server.requests();
        assert!(requests[1].contains("GET /Appointment?"));
        assert!(requests[1].contains("patient=patient-123"));
        assert!(requests[1].contains("date=ge2026-03-01"));
        assert!(requests[1].contains("_count=1"));
        assert!(requests[1].contains("authorization: Bearer access-token"));
    }

    #[test]
    fn portal_login_still_works_under_portal_namespace() {
        let server = TestServer::spawn(vec![
            ResponseSpec::html(
                200,
                login_page_html("csrf-token"),
                vec![("Set-Cookie".into(), "MyChartAffinity=affinity-cookie; Path=/".into())],
            ),
            ResponseSpec::empty(
                302,
                vec![
                    ("Location".into(), "/inside.asp".into()),
                    ("Set-Cookie".into(), "MyChartSession=session-cookie; Path=/".into()),
                ],
            ),
            ResponseSpec::html(200, app_page_html("Dashboard"), Vec::new()),
        ]);
        let temp_dir = temp_dir("mychart-portal-login");
        let config_path = temp_dir.join("config.json");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--portal-base-url",
            &server.base_url(),
            "--compact",
            "portal",
            "auth",
            "login-password",
            "--username",
            "person@example.com",
            "--password",
            "super-secret",
        ]);

        assert_eq!(output["status"], "authenticated");

        let state = StateStore::new(config_path).load().expect("state should load");
        let account = state
            .accounts
            .get("default")
            .expect("default account should be persisted");
        assert_eq!(account.portal_base_url.as_deref(), Some(server.base_url().as_str()));
        assert_eq!(account.cookies.len(), 2);
    }

    #[test]
    fn connect_resolve_uses_cached_brand_catalog() {
        let temp_dir = temp_dir("mychart-connect-resolve");
        let config_path = temp_dir.join("config.json");
        write_brands_cache(
            &temp_dir.join("brands-cache.json"),
            json!({
                "source_url": "https://open.epic.com/Endpoints/Brands",
                "fetched_at_epoch_seconds": 1_800_000_000u64,
                "bundle_last_updated": "2026-03-27T03:00:03Z",
                "brands": [{
                    "brand_id": "brand-1",
                    "brand_name": "UCLA Medical Center",
                    "account_slug": "ucla-medical-center",
                    "fhir_base_url": "https://arrprox.mednet.ucla.edu/FHIRPRD/api/FHIR/R4",
                    "endpoint_id": "endpoint-1",
                    "endpoint_name": "UCLA Medical Center",
                    "managing_organization_id": "341",
                    "managing_organization_name": "UCLA Health",
                    "state": "CA",
                    "country": "USA",
                    "facilities": [{
                        "name": "UCLA Santa Monica",
                        "city": "Santa Monica",
                        "state": "CA"
                    }]
                }]
            }),
        );

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "connect",
            "ucla",
            "medical",
            "center",
        ]);

        assert_eq!(output["status"], "connected");
        assert_eq!(output["selected_account"], "ucla-medical-center");

        let state = StateStore::new(config_path).load().expect("state should load");
        assert_eq!(state.current_account.as_deref(), Some("ucla-medical-center"));
        let account = state
            .accounts
            .get("ucla-medical-center")
            .expect("named account should be stored");
        assert_eq!(
            account.api_base_url.as_deref(),
            Some("https://arrprox.mednet.ucla.edu/FHIRPRD/api/FHIR/R4")
        );
        assert_eq!(
            account
                .discovery
                .as_ref()
                .and_then(|discovery| discovery.brand_name.as_deref()),
            Some("UCLA Medical Center")
        );
    }

    #[test]
    fn connect_resolve_reports_ambiguity_for_broad_queries() {
        let temp_dir = temp_dir("mychart-connect-ambiguous");
        let config_path = temp_dir.join("config.json");
        write_brands_cache(
            &temp_dir.join("brands-cache.json"),
            json!({
                "source_url": "https://open.epic.com/Endpoints/Brands",
                "fetched_at_epoch_seconds": 1_800_000_000u64,
                "bundle_last_updated": "2026-03-27T03:00:03Z",
                "brands": [
                    {
                        "brand_id": "brand-1",
                        "brand_name": "UCLA Medical Center",
                        "account_slug": "ucla-medical-center",
                        "fhir_base_url": "https://arrprox.mednet.ucla.edu/FHIRPRD/api/FHIR/R4",
                        "endpoint_id": "endpoint-1",
                        "endpoint_name": "UCLA Medical Center",
                        "managing_organization_id": "341",
                        "managing_organization_name": "UCLA Health",
                        "state": "CA",
                        "country": "USA",
                        "facilities": []
                    },
                    {
                        "brand_id": "brand-2",
                        "brand_name": "UCLA Health Medicare Advantage Plan",
                        "account_slug": "ucla-health-medicare-advantage-plan",
                        "fhir_base_url": "https://arrprox.mednet.ucla.edu/FHIRPRD/HEALTHPLAN/api/FHIR/R4",
                        "endpoint_id": "endpoint-2",
                        "endpoint_name": "UCLA Health Medicare Advantage Plan",
                        "managing_organization_id": "341",
                        "managing_organization_name": "UCLA Health",
                        "state": null,
                        "country": null,
                        "facilities": []
                    }
                ]
            }),
        );

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "connect",
            "ucla",
        ]);

        assert_eq!(output["status"], "ambiguous");
        assert_eq!(output["matches"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn labs_shorthand_returns_trend_series() {
        let server = TestServer::spawn(vec![
            ResponseSpec::json(
                200,
                capability_statement_json(
                    "http://placeholder",
                    &[resource_capability("Observation", &["read", "search-type"])],
                ),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [
                        {
                            "resource": {
                                "resourceType": "Observation",
                                "id": "obs-1",
                                "effectiveDateTime": "2026-03-01T00:00:00Z",
                                "code": {"text": "Hemoglobin A1c"},
                                "valueQuantity": {"value": 6.3, "unit": "%"}
                            }
                        },
                        {
                            "resource": {
                                "resourceType": "Observation",
                                "id": "obs-2",
                                "effectiveDateTime": "2026-02-01T00:00:00Z",
                                "code": {"text": "Hemoglobin A1c"},
                                "valueQuantity": {"value": 6.1, "unit": "%"}
                            }
                        }
                    ]
                }),
                Vec::new(),
            ),
        ]);
        let temp_dir = temp_dir("mychart-labs-trend");
        let config_path = temp_dir.join("config.json");
        StateStore::new(config_path.clone())
            .save(&MyChartState {
                api_base_url: Some(server.base_url()),
                access_token: Some("access-token".into()),
                patient_id: Some("patient-123".into()),
                ..MyChartState::default()
            })
            .expect("state should save");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "labs",
            "a1c",
            "--spark",
        ]);

        assert_eq!(output["status"], "ok");
        assert_eq!(output["series"][0]["label"], "Hemoglobin A1c");
        assert_eq!(output["series"][0]["point_count"], 2);
        assert!(!output["series"][0]["spark"]
            .as_str()
            .expect("sparkline should be present")
            .is_empty());
    }

    #[test]
    fn appointments_upcoming_filters_past_and_cancelled_entries() {
        let server = TestServer::spawn(vec![
            ResponseSpec::json(
                200,
                capability_statement_json(
                    "http://placeholder",
                    &[resource_capability("Appointment", &["read", "search-type"])],
                ),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [
                        {
                            "resource": {
                                "resourceType": "Appointment",
                                "id": "appt-future",
                                "status": "booked",
                                "start": "2100-01-01T10:00:00Z",
                                "description": "Future visit"
                            }
                        },
                        {
                            "resource": {
                                "resourceType": "Appointment",
                                "id": "appt-past",
                                "status": "booked",
                                "start": "2000-01-01T10:00:00Z",
                                "description": "Past visit"
                            }
                        },
                        {
                            "resource": {
                                "resourceType": "Appointment",
                                "id": "appt-cancelled",
                                "status": "cancelled",
                                "start": "2100-01-02T10:00:00Z",
                                "description": "Cancelled visit"
                            }
                        }
                    ]
                }),
                Vec::new(),
            ),
        ]);
        let temp_dir = temp_dir("mychart-appointments-upcoming");
        let config_path = temp_dir.join("config.json");
        StateStore::new(config_path.clone())
            .save(&MyChartState {
                api_base_url: Some(server.base_url()),
                access_token: Some("access-token".into()),
                patient_id: Some("patient-123".into()),
                ..MyChartState::default()
            })
            .expect("state should save");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "appointments",
            "upcoming",
        ]);

        assert_eq!(output["status"], "ok");
        assert_eq!(output["appointments"].as_array().map(Vec::len), Some(1));
        assert_eq!(output["appointments"][0]["id"], "appt-future");
    }

    #[test]
    fn appointments_find_filters_by_text_and_future_window() {
        let server = TestServer::spawn(vec![
            ResponseSpec::json(
                200,
                capability_statement_json(
                    "http://placeholder",
                    &[json!({
                        "type": "Appointment",
                        "interaction": [
                            {"code": "read"},
                            {"code": "search-type"}
                        ],
                        "searchParam": [
                            {"name": "patient", "type": "reference"},
                            {"name": "date", "type": "date"}
                        ]
                    })],
                ),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [
                        {
                            "resource": {
                                "resourceType": "Appointment",
                                "id": "appt-derm-soon",
                                "status": "booked",
                                "start": "2100-01-10T10:00:00Z",
                                "description": "Dermatology consult",
                                "specialty": [{"text": "Dermatology"}]
                            }
                        },
                        {
                            "resource": {
                                "resourceType": "Appointment",
                                "id": "appt-cardio",
                                "status": "booked",
                                "start": "2100-01-11T10:00:00Z",
                                "description": "Cardiology follow-up",
                                "specialty": [{"text": "Cardiology"}]
                            }
                        },
                        {
                            "resource": {
                                "resourceType": "Appointment",
                                "id": "appt-derm-late",
                                "status": "booked",
                                "start": "2100-03-20T10:00:00Z",
                                "description": "Dermatology follow-up",
                                "specialty": [{"text": "Dermatology"}]
                            }
                        }
                    ]
                }),
                Vec::new(),
            ),
        ]);
        let temp_dir = temp_dir("mychart-appointments-find");
        let config_path = temp_dir.join("config.json");
        StateStore::new(config_path.clone())
            .save(&MyChartState {
                api_base_url: Some(server.base_url()),
                access_token: Some("access-token".into()),
                patient_id: Some("patient-123".into()),
                ..MyChartState::default()
            })
            .expect("state should save");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "appointments",
            "find",
            "derm",
            "--next",
            "2100-02-01",
        ]);

        assert_eq!(output["status"], "ok");
        assert_eq!(output["query"], "derm");
        assert_eq!(output["appointments"].as_array().map(Vec::len), Some(1));
        assert_eq!(output["appointments"][0]["id"], "appt-derm-soon");
        let requests = server.requests();
        assert!(requests[1].contains("date=ge"));
    }

    #[test]
    fn notes_get_fetches_binary_body_text() {
        let server = TestServer::spawn(vec![
            ResponseSpec::json(
                200,
                capability_statement_json(
                    "http://placeholder",
                    &[resource_capability("DocumentReference", &["read", "search-type"])],
                ),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "DocumentReference",
                    "id": "note-1",
                    "date": "2099-12-02",
                    "type": {"text": "Progress Note"},
                    "description": "Neurology note",
                    "author": [{"display": "Dr. Headache"}],
                    "content": [{
                        "attachment": {
                            "title": "Note body",
                            "contentType": "text/plain",
                            "url": "Binary/note-1-body"
                        }
                    }]
                }),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Binary",
                    "contentType": "text/plain",
                    "data": "UGF0aWVudCByZXBvcnRzIG1pZ3JhaW5lIGltcHJvdmVtZW50Lg=="
                }),
                Vec::new(),
            ),
        ]);
        let temp_dir = temp_dir("mychart-notes-get");
        let config_path = temp_dir.join("config.json");
        StateStore::new(config_path.clone())
            .save(&MyChartState {
                api_base_url: Some(server.base_url()),
                access_token: Some("access-token".into()),
                patient_id: Some("patient-123".into()),
                ..MyChartState::default()
            })
            .expect("state should save");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "notes",
            "get",
            "note-1",
        ]);

        assert_eq!(output["status"], "ok");
        assert_eq!(output["note"]["body_text"], "Patient reports migraine improvement.");
        assert_eq!(
            output["note"]["content"][0]["body_text"],
            "Patient reports migraine improvement."
        );
        let requests = server.requests();
        assert!(requests[1].contains("GET /DocumentReference/note-1"));
        assert!(requests[2].contains("GET /Binary/note-1-body"));
    }

    #[test]
    fn meds_reconcile_can_merge_all_provider_accounts() {
        let server_a = TestServer::spawn(vec![
            ResponseSpec::json(
                200,
                capability_statement_json(
                    "http://placeholder",
                    &[resource_capability("MedicationRequest", &["read", "search-type"])],
                ),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [{
                        "resource": {
                            "resourceType": "MedicationRequest",
                            "id": "med-a",
                            "status": "active",
                            "intent": "order",
                            "authoredOn": "2100-01-01",
                            "medicationCodeableConcept": {"text": "Aspirin"},
                            "requester": {"display": "Dr. A"}
                        }
                    }]
                }),
                Vec::new(),
            ),
        ]);
        let server_b = TestServer::spawn(vec![
            ResponseSpec::json(
                200,
                capability_statement_json(
                    "http://placeholder",
                    &[resource_capability("MedicationRequest", &["read", "search-type"])],
                ),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [{
                        "resource": {
                            "resourceType": "MedicationRequest",
                            "id": "med-b",
                            "status": "active",
                            "intent": "order",
                            "authoredOn": "2100-01-02",
                            "medicationCodeableConcept": {"text": "Aspirin"},
                            "requester": {"display": "Dr. B"}
                        }
                    }]
                }),
                Vec::new(),
            ),
        ]);
        let temp_dir = temp_dir("mychart-meds-all-providers");
        let config_path = temp_dir.join("config.json");
        StateStore::new(config_path.clone())
            .save(&MyChartState {
                current_account: Some("ucla".into()),
                accounts: BTreeMap::from([
                    (
                        "ucla".into(),
                        crate::state::MyChartAccountState {
                            api_base_url: Some(server_a.base_url()),
                            access_token: Some("access-a".into()),
                            patient_id: Some("patient-a".into()),
                            discovery: Some(crate::state::AccountDiscoveryState {
                                brand_name: Some("UCLA Medical Center".into()),
                                ..crate::state::AccountDiscoveryState::default()
                            }),
                            ..crate::state::MyChartAccountState::default()
                        },
                    ),
                    (
                        "cedars".into(),
                        crate::state::MyChartAccountState {
                            api_base_url: Some(server_b.base_url()),
                            access_token: Some("access-b".into()),
                            patient_id: Some("patient-b".into()),
                            discovery: Some(crate::state::AccountDiscoveryState {
                                brand_name: Some("Cedars-Sinai".into()),
                                ..crate::state::AccountDiscoveryState::default()
                            }),
                            ..crate::state::MyChartAccountState::default()
                        },
                    ),
                    (
                        "stale".into(),
                        crate::state::MyChartAccountState {
                            api_base_url: Some("https://example.invalid/FHIR/R4".into()),
                            ..crate::state::MyChartAccountState::default()
                        },
                    ),
                ]),
                ..MyChartState::default()
            })
            .expect("state should save");

        let context = resolved_context(&config_path);
        let output = crate::commands::meds::run_reconcile_output(
            crate::commands::meds::MedsReconcileArgs {
                patient: None,
                all_accounts: true,
                since: None,
                limit: 100,
                all_pages: false,
            },
            &context,
        )
        .expect("med reconciliation should succeed");

        assert_eq!(output.status, "ok");
        assert_eq!(output.patient_id, None);
        assert_eq!(output.accounts_used.len(), 2);
        assert_eq!(output.accounts_skipped.len(), 1);
        assert_eq!(output.duplicate_name_candidates.len(), 1);
        assert_eq!(output.duplicate_name_candidates[0].name, "aspirin");
        assert_eq!(output.duplicate_name_candidates[0].count, 2);
        let accounts = output
            .medications
            .iter()
            .map(|entry| entry.account.as_str())
            .collect::<Vec<_>>();
        assert!(accounts.contains(&"ucla"));
        assert!(accounts.contains(&"cedars"));
    }

    #[test]
    fn appointments_upcoming_can_merge_all_provider_accounts() {
        let server_a = TestServer::spawn(vec![
            ResponseSpec::json(
                200,
                capability_statement_json(
                    "http://placeholder",
                    &[resource_capability("Appointment", &["read", "search-type"])],
                ),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [{
                        "resource": {
                            "resourceType": "Appointment",
                            "id": "appt-a",
                            "status": "booked",
                            "start": "2100-01-01T10:00:00Z",
                            "description": "UCLA visit"
                        }
                    }]
                }),
                Vec::new(),
            ),
        ]);
        let server_b = TestServer::spawn(vec![
            ResponseSpec::json(
                200,
                capability_statement_json(
                    "http://placeholder",
                    &[resource_capability("Appointment", &["read", "search-type"])],
                ),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [{
                        "resource": {
                            "resourceType": "Appointment",
                            "id": "appt-b",
                            "status": "booked",
                            "start": "2100-01-02T10:00:00Z",
                            "description": "Cedars visit"
                        }
                    }]
                }),
                Vec::new(),
            ),
        ]);
        let temp_dir = temp_dir("mychart-appointments-all-providers");
        let config_path = temp_dir.join("config.json");
        StateStore::new(config_path.clone())
            .save(&MyChartState {
                current_account: Some("ucla".into()),
                accounts: BTreeMap::from([
                    (
                        "ucla".into(),
                        crate::state::MyChartAccountState {
                            api_base_url: Some(server_a.base_url()),
                            access_token: Some("access-a".into()),
                            patient_id: Some("patient-a".into()),
                            discovery: Some(crate::state::AccountDiscoveryState {
                                brand_name: Some("UCLA Medical Center".into()),
                                ..crate::state::AccountDiscoveryState::default()
                            }),
                            ..crate::state::MyChartAccountState::default()
                        },
                    ),
                    (
                        "cedars".into(),
                        crate::state::MyChartAccountState {
                            api_base_url: Some(server_b.base_url()),
                            access_token: Some("access-b".into()),
                            patient_id: Some("patient-b".into()),
                            discovery: Some(crate::state::AccountDiscoveryState {
                                brand_name: Some("Cedars-Sinai".into()),
                                ..crate::state::AccountDiscoveryState::default()
                            }),
                            ..crate::state::MyChartAccountState::default()
                        },
                    ),
                ]),
                ..MyChartState::default()
            })
            .expect("state should save");

        let context = resolved_context(&config_path);
        let output = crate::commands::appointments::run_upcoming_output(
            crate::commands::appointments::AppointmentsUpcomingArgs {
                patient: None,
                all_accounts: true,
                since: None,
                limit: 10,
                all_pages: false,
            },
            &context,
        )
        .expect("upcoming appointments should succeed");

        assert_eq!(output.status, "ok");
        assert_eq!(output.patient_id, None);
        assert_eq!(output.accounts_used.len(), 2);
        assert_eq!(output.appointments.len(), 2);
        assert_eq!(output.appointments[0].account, "ucla");
        assert_eq!(output.appointments[1].account, "cedars");
    }

    #[test]
    fn claims_audit_flags_duplicate_and_problem_claims() {
        let server = TestServer::spawn(vec![
            ResponseSpec::json(
                200,
                capability_statement_json(
                    "http://placeholder",
                    &[resource_capability("ExplanationOfBenefit", &["read", "search-type"])],
                ),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [
                        {
                            "resource": {
                                "resourceType": "ExplanationOfBenefit",
                                "id": "claim-1",
                                "status": "active",
                                "outcome": "complete",
                                "use": "claim",
                                "billablePeriod": {"start": "2100-01-01"},
                                "provider": {"display": "UCLA Health"},
                                "total": [{"amount": {"value": 250.0, "currency": "USD"}}],
                                "item": [{"productOrService": {"text": "MRI Brain"}}]
                            }
                        },
                        {
                            "resource": {
                                "resourceType": "ExplanationOfBenefit",
                                "id": "claim-2",
                                "status": "active",
                                "outcome": "complete",
                                "use": "claim",
                                "billablePeriod": {"start": "2100-01-01"},
                                "provider": {"display": "UCLA Health"},
                                "total": [{"amount": {"value": 250.0, "currency": "USD"}}],
                                "item": [{"productOrService": {"text": "MRI Brain"}}]
                            }
                        },
                        {
                            "resource": {
                                "resourceType": "ExplanationOfBenefit",
                                "id": "claim-3",
                                "status": "active",
                                "outcome": "partial",
                                "use": "claim",
                                "billablePeriod": {"start": "2100-01-03"},
                                "provider": {"display": "UCLA Health"},
                                "total": [{"amount": {"value": 99.0, "currency": "USD"}}],
                                "item": [{"productOrService": {"text": "Lab Panel"}}]
                            }
                        }
                    ]
                }),
                Vec::new(),
            ),
        ]);
        let temp_dir = temp_dir("mychart-claims-audit");
        let config_path = temp_dir.join("config.json");
        StateStore::new(config_path.clone())
            .save(&MyChartState {
                api_base_url: Some(server.base_url()),
                access_token: Some("access-token".into()),
                patient_id: Some("patient-123".into()),
                ..MyChartState::default()
            })
            .expect("state should save");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "claims",
            "audit",
        ]);

        assert_eq!(output["status"], "ok");
        assert_eq!(
            output["duplicate_charge_candidates"]
                .as_array()
                .expect("duplicate groups should be an array")
                .len(),
            1
        );
        assert_eq!(
            output["denied_or_problematic_claims"]
                .as_array()
                .expect("problem claims should be an array")
                .len(),
            1
        );
    }

    #[test]
    fn pack_doctor_assembles_visit_packet() {
        let server = TestServer::spawn(vec![
            ResponseSpec::json(
                200,
                capability_statement_json(
                    "http://placeholder",
                    &[
                        resource_capability("Appointment", &["read", "search-type"]),
                        resource_capability("Observation", &["read", "search-type"]),
                        resource_capability("MedicationRequest", &["read", "search-type"]),
                        resource_capability("Condition", &["read", "search-type"]),
                        resource_capability("Encounter", &["read", "search-type"]),
                        resource_capability("DocumentReference", &["read", "search-type"]),
                    ],
                ),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [{
                        "resource": {
                            "resourceType": "Appointment",
                            "id": "appt-1",
                            "status": "booked",
                            "start": "2100-01-01T10:00:00Z",
                            "description": "Neurology follow-up"
                        }
                    }]
                }),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [{
                        "resource": {
                            "resourceType": "Observation",
                            "id": "obs-1",
                            "effectiveDateTime": "2100-01-01T08:00:00Z",
                            "code": {"text": "Ferritin"},
                            "valueQuantity": {"value": 14.0, "unit": "ng/mL"},
                            "interpretation": [{"text": "low"}]
                        }
                    }]
                }),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [{
                        "resource": {
                            "resourceType": "MedicationRequest",
                            "id": "med-1",
                            "status": "active",
                            "authoredOn": "2099-12-15",
                            "medicationCodeableConcept": {"text": "Topiramate"},
                            "dosageInstruction": [{"text": "Take once daily"}]
                        }
                    }]
                }),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [{
                        "resource": {
                            "resourceType": "Condition",
                            "id": "cond-1",
                            "recordedDate": "2099-12-10",
                            "clinicalStatus": {"text": "active"},
                            "verificationStatus": {"text": "confirmed"},
                            "code": {"text": "Migraine"}
                        }
                    }]
                }),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [{
                        "resource": {
                            "resourceType": "Encounter",
                            "id": "enc-1",
                            "period": {"start": "2099-12-01"},
                            "status": "finished",
                            "class": {"display": "outpatient"},
                            "type": [{"text": "Office visit"}]
                        }
                    }]
                }),
                Vec::new(),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "resourceType": "Bundle",
                    "entry": [{
                        "resource": {
                            "resourceType": "DocumentReference",
                            "id": "note-1",
                            "date": "2099-12-02",
                            "type": {"text": "Progress Note"},
                            "description": "Neurology note",
                            "author": [{"display": "Dr. Headache"}]
                        }
                    }]
                }),
                Vec::new(),
            ),
        ]);
        let temp_dir = temp_dir("mychart-pack-doctor");
        let config_path = temp_dir.join("config.json");
        StateStore::new(config_path.clone())
            .save(&MyChartState {
                api_base_url: Some(server.base_url()),
                access_token: Some("access-token".into()),
                patient_id: Some("patient-123".into()),
                ..MyChartState::default()
            })
            .expect("state should save");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "pack",
            "doctor",
        ]);

        assert_eq!(output["status"], "ok");
        assert_eq!(output["upcoming_appointment"]["description"], "Neurology follow-up");
        assert_eq!(output["recent_labs"][0]["label"], "Ferritin");
        assert_eq!(output["active_medications"][0]["name"], "Topiramate");
        assert_eq!(output["active_conditions"][0]["condition"], "Migraine");
        assert!(!output["suggested_questions"]
            .as_array()
            .expect("suggested questions should be an array")
            .is_empty());
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            hex(&super::sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    fn run_command(args: &[&str]) -> Value {
        let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
        let compact = cli.global.compact;
        let (value, _) = run(cli).unwrap_or_else(|(error, _)| panic!("{}", error.render(compact)));
        value
    }

    fn resolved_context(config_path: &std::path::Path) -> crate::state::ResolvedContext {
        ResolvedContext::from_global(&GlobalArgs {
            config: Some(config_path.to_path_buf()),
            account: None,
            base_url: None,
            portal_base_url: None,
            client_id: None,
            client_secret: None,
            redirect_uri: None,
            access_token: None,
            refresh_token: None,
            username: None,
            compact: true,
        })
        .expect("context should resolve")
    }

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{nanos:x}"));
        fs::create_dir_all(&path).expect("temp dir should exist");
        path
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
    }

    fn wait_for_callback_response(port: u16, request: &str) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match std::net::TcpStream::connect(("127.0.0.1", port)) {
                Ok(mut stream) => {
                    stream
                        .write_all(request.as_bytes())
                        .expect("callback request should write");
                    return read_request(&mut stream);
                }
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                    if std::time::Instant::now() >= deadline {
                        panic!("callback listener did not start in time");
                    }
                    thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(error) => panic!("failed to connect to callback listener: {error}"),
            }
        }
    }

    fn write_brands_cache(path: &std::path::Path, value: Value) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("brands cache parent should exist");
        }
        fs::write(
            path,
            serde_json::to_vec_pretty(&value).expect("brands cache should serialize"),
        )
        .expect("brands cache should write");
    }

    fn capability_statement_json(base_url: &str, resources: &[Value]) -> Value {
        json!({
            "resourceType": "CapabilityStatement",
            "fhirVersion": "4.0.1",
            "software": {
                "name": "Epic",
                "version": "February 2026"
            },
            "implementation": {
                "url": base_url
            },
            "rest": [{
                "mode": "server",
                "security": {
                    "extension": [{
                        "url": "http://fhir-registry.smarthealthit.org/StructureDefinition/oauth-uris",
                        "extension": [
                            {"url": "authorize", "valueUri": format!("{base_url}/oauth2/authorize")},
                            {"url": "token", "valueUri": format!("{base_url}/oauth2/token")}
                        ]
                    }]
                },
                "resource": if resources.is_empty() {
                    vec![resource_capability("Patient", &["read", "search-type"])]
                } else {
                    resources.to_vec()
                }
            }]
        })
    }

    fn resource_capability(resource_type: &str, interactions: &[&str]) -> Value {
        json!({
            "type": resource_type,
            "interaction": interactions.iter().map(|interaction| json!({ "code": interaction })).collect::<Vec<_>>(),
            "searchParam": [{
                "name": "patient",
                "type": "reference"
            }]
        })
    }

    fn login_page_html(token: &str) -> String {
        format!(
            "<html><head><title>Generic MyChart - Login Page</title></head><body>\
             <form id=\"loginForm\"></form>\
             <form class=\"hidden\" action=\"/Authentication/Login/DoLogin\">\
             <input name=\"__RequestVerificationToken\" type=\"hidden\" value=\"{token}\" />\
             </form>\
             </body></html>"
        )
    }

    fn app_page_html(title: &str) -> String {
        format!(
            "<html><head><title>{title}</title></head><body>\
             <div id=\"app\">hello from {title}</div>\
             <input name=\"__RequestVerificationToken\" type=\"hidden\" value=\"page-token\" />\
             </body></html>"
        )
    }

    #[derive(Clone)]
    struct ResponseSpec {
        status_code: u16,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl ResponseSpec {
        fn html(status_code: u16, body: String, headers: Vec<(String, String)>) -> Self {
            let mut headers = headers;
            headers.push(("Content-Type".into(), "text/html; charset=utf-8".into()));
            Self {
                status_code,
                headers,
                body,
            }
        }

        fn json(status_code: u16, body: Value, headers: Vec<(String, String)>) -> Self {
            let mut headers = headers;
            headers.push(("Content-Type".into(), "application/fhir+json".into()));
            Self {
                status_code,
                headers,
                body: serde_json::to_string(&body).expect("body should serialize"),
            }
        }

        fn empty(status_code: u16, headers: Vec<(String, String)>) -> Self {
            Self {
                status_code,
                headers,
                body: String::new(),
            }
        }
    }

    struct TestServer {
        address: String,
        requests: Arc<Mutex<Vec<String>>>,
        _handle: thread::JoinHandle<()>,
    }

    impl TestServer {
        fn spawn(responses: Vec<ResponseSpec>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
            let address = listener.local_addr().expect("listener should have local addr");
            let requests = Arc::new(Mutex::new(Vec::new()));
            let requests_clone = requests.clone();

            let handle = thread::spawn(move || {
                for response in responses {
                    let (mut stream, _) = listener.accept().expect("server should accept request");
                    let request = read_request(&mut stream);
                    if let Ok(mut captured) = requests_clone.lock() {
                        captured.push(request);
                    }

                    let mut headers = response.headers;
                    let body = response
                        .body
                        .replace("http://placeholder", &format!("http://{address}"));
                    headers.push(("Content-Length".into(), body.len().to_string()));
                    headers.push(("Connection".into(), "close".into()));

                    let mut response_text = format!(
                        "HTTP/1.1 {} {}\r\n",
                        response.status_code,
                        status_text(response.status_code)
                    );
                    for (name, value) in headers {
                        response_text.push_str(&format!("{name}: {value}\r\n"));
                    }
                    response_text.push_str("\r\n");
                    response_text.push_str(&body);
                    stream
                        .write_all(response_text.as_bytes())
                        .expect("response should write");
                }
            });

            Self {
                address: format!("http://{address}"),
                requests,
                _handle: handle,
            }
        }

        fn base_url(&self) -> String {
            self.address.clone()
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().expect("requests lock").clone()
        }
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut buffer = Vec::new();
        let mut temp = [0u8; 1024];
        loop {
            let bytes_read = stream.read(&mut temp).expect("request should read");
            if bytes_read == 0 {
                break;
            }
            buffer.extend_from_slice(&temp[..bytes_read]);
            if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8(buffer).expect("request should be utf-8")
    }

    fn status_text(code: u16) -> &'static str {
        match code {
            200 => "OK",
            201 => "Created",
            204 => "No Content",
            302 => "Found",
            400 => "Bad Request",
            401 => "Unauthorized",
            500 => "Internal Server Error",
            _ => "OK",
        }
    }
}
