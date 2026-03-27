mod client;
mod state;

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result as AnyhowResult};
use clap::{Args, Parser, Subcommand};
use reqwest::{Method, Url};
use serde_json::{json, Value};

use crate::{
    client::{MyChartClient, RequestBody, RequestSpec, ResolvedResponse},
    state::{ResolvedContext, ENV_MYCHART_BASE_URL, ENV_MYCHART_USERNAME},
};

const AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  mychart auth login-password --base-url https://my.uclahealth.org/MyChart \\\n",
    "    --username you@example.com --password 'super-secret'\n",
    "  mychart auth status\n",
    "  mychart request get /inside.asp\n",
    "  mychart request get /Visits/UpcomingAppointments\n",
    "  mychart request post /Authentication/Login/TwoFactorAuthentication \\\n",
    "    --form __RequestVerificationToken=token --form Code=123456\n",
    "\n",
    "This CLI is aimed at Epic MyChart portals.\n",
    "Pass --base-url once for your organization and it will be persisted in a local 0600 config file alongside session cookies.\n",
    "It also exposes request primitives for the flows Epic still refuses to make pleasant.\n",
    "If your organization uses two-factor authentication, `auth login-password` may return `verification_required` with a `next_url` instead of a fully authenticated session.\n",
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
    let mut context = ResolvedContext::from_global(&cli.global).context("failed to resolve MyChart runtime context")?;
    let client = MyChartClient::new(context.base_url.clone()).context("failed to build MyChart client")?;

    let output = match cli.command {
        Commands::Auth(command) => run_auth(command.command, &client, &mut context),
        Commands::Request(command) => run_request(command.command, &client, &mut context),
    }
    .context("MyChart command failed")?;

    Ok((output, compact))
}

#[derive(Debug, Parser)]
#[command(
    name = "mychart",
    version,
    about = "CLI for Epic MyChart auth flows and authenticated portal requests",
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
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, global = true, env = ENV_MYCHART_BASE_URL, value_name = "URL")]
    base_url: Option<String>,

    #[arg(long, global = true, env = ENV_MYCHART_USERNAME, value_name = "USERNAME")]
    username: Option<String>,

    #[arg(long, global = true)]
    compact: bool,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Auth(AuthCommand),
    Request(RequestCommand),
}

#[derive(Debug, Args)]
struct AuthCommand {
    #[command(subcommand)]
    command: AuthSubcommand,
}

#[derive(Debug, Subcommand)]
enum AuthSubcommand {
    #[command(name = "login-password")]
    LoginPassword(AuthLoginPasswordArgs),
    Status,
    Logout,
}

#[derive(Debug, Args)]
struct AuthLoginPasswordArgs {
    #[arg(long)]
    username: Option<String>,

    #[arg(long)]
    password: String,

    #[arg(long)]
    no_store: bool,
}

#[derive(Debug, Args)]
struct RequestCommand {
    #[command(subcommand)]
    command: RequestSubcommand,
}

#[derive(Debug, Subcommand)]
enum RequestSubcommand {
    Get(RequestGetArgs),
    Post(RequestPostArgs),
}

#[derive(Debug, Args)]
struct RequestGetArgs {
    #[arg(value_name = "PATH")]
    path: String,

    #[arg(long = "query", value_parser = parse_key_value, value_name = "KEY=VALUE")]
    query: Vec<(String, String)>,

    #[arg(long = "no-follow-redirects")]
    no_follow_redirects: bool,
}

#[derive(Debug, Args)]
struct RequestPostArgs {
    #[arg(value_name = "PATH")]
    path: String,

    #[arg(long = "query", value_parser = parse_key_value, value_name = "KEY=VALUE")]
    query: Vec<(String, String)>,

    #[arg(long = "form", value_parser = parse_key_value, value_name = "KEY=VALUE")]
    form: Vec<(String, String)>,

    #[arg(long = "no-follow-redirects")]
    no_follow_redirects: bool,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("MyChart API returned HTTP {status_code}")]
    Api { status_code: u16, body: Value },
    #[error("authentication failure: {message}")]
    Auth { message: String, details: Value },
    #[error("invalid arguments: {0}")]
    Arguments(String),
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
            Self::Api { status_code, body } => json!({
                "status": "error",
                "kind": "api",
                "status_code": status_code,
                "body": body,
            }),
            Self::Auth { message, details } => json!({
                "status": "error",
                "kind": "auth",
                "message": message,
                "details": details,
            }),
            Self::Arguments(message) => json!({
                "status": "error",
                "kind": "arguments",
                "message": message,
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

fn run_auth(command: AuthSubcommand, client: &MyChartClient, context: &mut ResolvedContext) -> Result<Value> {
    match command {
        AuthSubcommand::LoginPassword(args) => {
            let username = context.require_username(args.username)?;
            let mut cookies = context.cookies.clone();

            let login_page = client.execute(
                RequestSpec {
                    method: Method::GET,
                    path: "/Authentication/Login".into(),
                    query: Vec::new(),
                    body: RequestBody::None,
                },
                &mut cookies,
                true,
            )?;
            ensure_success_status(&login_page)?;

            let csrf_token = extract_verification_token(&login_page.body_text).ok_or_else(|| Error::Auth {
                message: "failed to locate the MyChart login CSRF token".into(),
                details: json!({
                    "final_url": login_page.final_url.as_str(),
                    "page": summarize_page(&login_page.final_url, &login_page.body_text),
                }),
            })?;

            let login_payload = json!({
                "Type": "StandardLogin",
                "Credentials": {
                    "LoginIdentifier": base64_encode(username.as_bytes()),
                    "Password": base64_encode(args.password.as_bytes()),
                }
            })
            .to_string();

            let response = client.execute(
                RequestSpec {
                    method: Method::POST,
                    path: "/Authentication/Login/DoLogin".into(),
                    query: Vec::new(),
                    body: RequestBody::Form(vec![
                        ("__RequestVerificationToken".into(), csrf_token),
                        ("LoginInfo".into(), login_payload),
                        ("DeviceId".into(), context.device_id.clone()),
                    ]),
                },
                &mut cookies,
                true,
            )?;

            if let Some(error_code) = extract_login_error(&response.final_url) {
                return Err(Error::Auth {
                    message: login_error_message(&error_code),
                    details: json!({
                        "error_code": error_code,
                        "final_url": response.final_url.as_str(),
                        "page": summarize_page(&response.final_url, &response.body_text),
                    }),
                });
            }

            ensure_success_status(&response)?;

            let page = summarize_page(&response.final_url, &response.body_text);
            let verification_required = looks_like_verification_challenge(&response.final_url, &response.body_text);

            if is_login_page(&response.body_text) && !verification_required {
                return Err(Error::Auth {
                    message: "MyChart returned to the login page without establishing a session".into(),
                    details: json!({
                        "final_url": response.final_url.as_str(),
                        "page": page,
                    }),
                });
            }

            if !args.no_store {
                context.update_cookies(cookies.clone());
                context.store_session(Some(username.clone()))?;
            }

            Ok(json!({
                "status": if verification_required { "verification_required" } else { "authenticated" },
                "username": username,
                "base_url": context.base_url,
                "final_url": response.final_url.as_str(),
                "next_url": if verification_required { Value::String(response.final_url.to_string()) } else { Value::Null },
                "redirect_chain": response.redirect_chain,
                "stored": !args.no_store,
                "cookie_names": crate::client::cookie_names(&cookies),
                "page": page,
            }))
        }
        AuthSubcommand::Status => {
            if !context.has_session() {
                return Ok(json!({
                    "status": "ok",
                    "authenticated": false,
                    "reason": "no_stored_session",
                    "base_url": context.base_url,
                    "username": context.username,
                }));
            }

            let mut cookies = context.cookies.clone();
            let response = client.execute(
                RequestSpec {
                    method: Method::GET,
                    path: "/inside.asp".into(),
                    query: Vec::new(),
                    body: RequestBody::None,
                },
                &mut cookies,
                true,
            )?;

            ensure_success_status(&response)?;

            let page = summarize_page(&response.final_url, &response.body_text);
            let authenticated = !is_login_page(&response.body_text);

            if authenticated {
                context.update_cookies(cookies);
                context.store_session(None)?;
            } else {
                context.clear_session()?;
            }

            Ok(json!({
                "status": "ok",
                "authenticated": authenticated,
                "base_url": context.base_url,
                "username": context.username,
                "final_url": response.final_url.as_str(),
                "redirect_chain": response.redirect_chain,
                "cookie_names": if authenticated { context.cookie_names() } else { Vec::<String>::new() },
                "page": page,
            }))
        }
        AuthSubcommand::Logout => {
            if context.has_session() {
                let mut cookies = context.cookies.clone();
                let response = client.execute(
                    RequestSpec {
                        method: Method::GET,
                        path: "/Home/LogOut".into(),
                        query: Vec::new(),
                        body: RequestBody::None,
                    },
                    &mut cookies,
                    true,
                )?;
                ensure_success_status(&response)?;
            }

            context.clear_session()?;
            Ok(json!({
                "status": "logged_out",
                "base_url": context.base_url,
                "username": context.username,
            }))
        }
    }
}

fn run_request(command: RequestSubcommand, client: &MyChartClient, context: &mut ResolvedContext) -> Result<Value> {
    context.require_session()?;

    let (method, path, query, body, follow_redirects) = match command {
        RequestSubcommand::Get(args) => (
            Method::GET,
            args.path,
            args.query,
            RequestBody::None,
            !args.no_follow_redirects,
        ),
        RequestSubcommand::Post(args) => {
            if args.form.is_empty() {
                return Err(Error::Arguments(
                    "missing request form data, provide at least one --form KEY=VALUE pair".into(),
                ));
            }
            (
                Method::POST,
                args.path,
                args.query,
                RequestBody::Form(args.form),
                !args.no_follow_redirects,
            )
        }
    };

    let mut cookies = context.cookies.clone();
    let response = client.execute(
        RequestSpec {
            method: method.clone(),
            path,
            query,
            body,
        },
        &mut cookies,
        follow_redirects,
    )?;

    ensure_success_status(&response)?;

    if is_login_page(&response.body_text) {
        context.clear_session()?;
        return Err(Error::Auth {
            message: "stored MyChart session is not authenticated anymore, run mychart auth login-password again"
                .into(),
            details: json!({
                "final_url": response.final_url.as_str(),
                "page": summarize_page(&response.final_url, &response.body_text),
            }),
        });
    }

    context.update_cookies(cookies);
    context.store_session(None)?;

    Ok(json!({
        "status": "ok",
        "request": {
            "method": method.as_str(),
        },
        "response": {
            "status_code": response.status_code,
            "final_url": response.final_url.as_str(),
            "location": response.location,
            "content_type": response.content_type,
            "redirect_chain": response.redirect_chain,
        },
        "page": summarize_page(&response.final_url, &response.body_text),
        "body": parse_response_body(&response),
    }))
}

fn parse_key_value(input: &str) -> std::result::Result<(String, String), String> {
    let (key, value) = input.split_once('=').ok_or_else(|| "expected KEY=VALUE".to_owned())?;
    if key.trim().is_empty() {
        return Err("query/form key cannot be empty".into());
    }
    Ok((key.trim().to_owned(), value.to_owned()))
}

fn ensure_success_status(response: &ResolvedResponse) -> Result<()> {
    if response.status_code < 400 {
        return Ok(());
    }

    Err(Error::Api {
        status_code: response.status_code,
        body: json!({
            "final_url": response.final_url.as_str(),
            "content_type": response.content_type,
            "page": summarize_page(&response.final_url, &response.body_text),
            "body": parse_response_body(response),
        }),
    })
}

fn parse_response_body(response: &ResolvedResponse) -> Value {
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

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();

    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);

        let index0 = first >> 2;
        let index1 = ((first & 0b0000_0011) << 4) | (second >> 4);
        let index2 = ((second & 0b0000_1111) << 2) | (third >> 6);
        let index3 = third & 0b0011_1111;

        encoded.push(ALPHABET[index0 as usize] as char);
        encoded.push(ALPHABET[index1 as usize] as char);
        if chunk.len() > 1 {
            encoded.push(ALPHABET[index2 as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(ALPHABET[index3 as usize] as char);
        } else {
            encoded.push('=');
        }
    }

    encoded
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
    use serde_json::Value;

    use super::{
        run,
        state::{MyChartState, StateStore},
        Cli,
    };

    #[test]
    fn login_password_posts_logininfo_and_stores_cookies() {
        let server = TestServer::spawn(vec![
            ResponseSpec::html(
                200,
                login_page_html("csrf-token"),
                vec![("Set-Cookie".into(), "MyChartAffinity=affinity-cookie; Path=/".into())],
            ),
            ResponseSpec::empty(
                302,
                vec![
                    ("Location".into(), "/MyChart/inside.asp".into()),
                    ("Set-Cookie".into(), "MyChartSession=session-cookie; Path=/".into()),
                ],
            ),
            ResponseSpec::html(200, app_page_html("Dashboard"), Vec::new()),
        ]);

        let temp_dir = temp_dir("mychart-login");
        let config_path = temp_dir.join("config.json");
        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--base-url",
            &server.base_url(),
            "--compact",
            "auth",
            "login-password",
            "--username",
            "person@example.com",
            "--password",
            "super-secret",
        ]);

        assert_eq!(output["status"], "authenticated");
        assert_eq!(output["page"]["title"], "Dashboard");

        let requests = server.requests();
        assert!(requests[0].starts_with("GET /MyChart/Authentication/Login"));
        assert!(requests[1].starts_with("POST /MyChart/Authentication/Login/DoLogin"));
        assert!(requests[1].contains("__RequestVerificationToken=csrf-token"));
        assert!(requests[1].contains("LoginInfo=%7B"));
        assert!(requests[1].contains("cGVyc29uQGV4YW1wbGUuY29t"));
        assert!(requests[1].contains("c3VwZXItc2VjcmV0"));
        assert!(requests[2].contains("cookie: MyChartAffinity=affinity-cookie; MyChartSession=session-cookie"));

        let state = StateStore::new(config_path).load().expect("state should load");
        assert_eq!(state.username.as_deref(), Some("person@example.com"));
        assert_eq!(state.cookies.len(), 2);
    }

    #[test]
    fn auth_status_clears_expired_sessions() {
        let server = TestServer::spawn(vec![ResponseSpec::html(200, login_page_html("csrf"), Vec::new())]);
        let temp_dir = temp_dir("mychart-status");
        let config_path = temp_dir.join("config.json");
        StateStore::new(config_path.clone())
            .save(&MyChartState {
                base_url: Some(server.base_url()),
                username: Some("person@example.com".into()),
                device_id: Some("device-id".into()),
                cookies: vec![crate::client::StoredCookie {
                    name: "MyChartSession".into(),
                    value: "expired".into(),
                }],
            })
            .expect("state should save");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "auth",
            "status",
        ]);

        assert_eq!(output["authenticated"], false);

        let state = StateStore::new(config_path).load().expect("state should load");
        assert!(state.cookies.is_empty());
    }

    #[test]
    fn request_get_uses_stored_cookies_and_extracts_page_details() {
        let server = TestServer::spawn(vec![ResponseSpec::html(
            200,
            app_page_html("Visits"),
            vec![("Set-Cookie".into(), "MyChartSession=refreshed; Path=/".into())],
        )]);
        let temp_dir = temp_dir("mychart-request");
        let config_path = temp_dir.join("config.json");
        StateStore::new(config_path.clone())
            .save(&MyChartState {
                base_url: Some(server.base_url()),
                username: Some("person@example.com".into()),
                device_id: Some("device-id".into()),
                cookies: vec![crate::client::StoredCookie {
                    name: "MyChartSession".into(),
                    value: "stored-cookie".into(),
                }],
            })
            .expect("state should save");

        let output = run_command(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "request",
            "get",
            "/Visits/UpcomingAppointments",
        ]);

        assert_eq!(output["response"]["status_code"], 200);
        assert_eq!(output["page"]["title"], "Visits");
        assert_eq!(output["body"], app_page_html("Visits"));

        let requests = server.requests();
        assert!(requests[0].contains("cookie: MyChartSession=stored-cookie"));

        let state = StateStore::new(config_path).load().expect("state should load");
        assert_eq!(state.cookies[0].value, "refreshed");
    }

    #[test]
    fn invalid_login_surfaces_auth_error() {
        let server = TestServer::spawn(vec![
            ResponseSpec::html(200, login_page_html("csrf-token"), Vec::new()),
            ResponseSpec::empty(
                302,
                vec![(
                    "Location".into(),
                    "/MyChart/Authentication/Login?error=usernameloginfailed&showLocalPasswordEntry=true".into(),
                )],
            ),
            ResponseSpec::html(200, login_page_html("next-token"), Vec::new()),
        ]);
        let temp_dir = temp_dir("mychart-bad-login");
        let config_path = temp_dir.join("config.json");

        let error = run_error(&[
            "mychart",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--base-url",
            &server.base_url(),
            "--compact",
            "auth",
            "login-password",
            "--username",
            "person@example.com",
            "--password",
            "bad-secret",
        ]);

        assert_eq!(error["kind"], "auth");
        assert_eq!(error["details"]["error_code"], "usernameloginfailed");
    }

    fn run_command(args: &[&str]) -> Value {
        let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
        let compact = cli.global.compact;
        let (value, _) = run(cli).unwrap_or_else(|error| panic!("{}", render_cli_error(&error, compact)));
        value
    }

    fn run_error(args: &[&str]) -> Value {
        let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
        let compact = cli.global.compact;
        let error = run(cli).expect_err("command should fail");
        serde_json::from_str(&render_cli_error(&error, compact)).expect("error output should be valid json")
    }

    fn login_page_html(token: &str) -> String {
        format!(
            "<html><head><title>myUCLAhealth - Login Page</title></head><body>\
             <form id=\"loginForm\"></form>\
             <form Class=\"hidden\" action=\"/MyChart/Authentication/Login/DoLogin\">\
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
                    headers.push(("Content-Length".into(), response.body.len().to_string()));
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
                    response_text.push_str(&response.body);
                    stream
                        .write_all(response_text.as_bytes())
                        .expect("response should write");
                }
            });

            Self {
                address: format!("http://{address}/MyChart"),
                requests,
                _handle: handle,
            }
        }

        fn base_url(&self) -> String {
            self.address.clone()
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().expect("requests lock should work").clone()
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
                let headers = String::from_utf8_lossy(&buffer[..headers_end]).replace('\r', "");
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
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

    fn status_text(status_code: u16) -> &'static str {
        match status_code {
            200 => "OK",
            302 => "Found",
            400 => "Bad Request",
            401 => "Unauthorized",
            500 => "Internal Server Error",
            _ => "OK",
        }
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
