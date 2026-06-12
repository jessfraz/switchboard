use std::{
    io::{self, IsTerminal, Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use reqwest::Url;
use serde_json::{json, Value};

use super::auth_debug;
use crate::{state::ResolvedContext, Error, Result};

pub(super) const HOSTED_CALLBACK_BRIDGE_URL: &str = "http://127.0.0.1:8911/mychart-callback";

const HOSTED_CALLBACK_BRIDGE_BIND_ADDRESS: &str = "127.0.0.1:8911";
const HOSTED_CALLBACK_BRIDGE_PATH: &str = "/mychart-callback";

#[derive(Debug)]
pub(super) struct BrowserLaunch {
    pub(super) attempted: bool,
    pub(super) opened: bool,
    pub(super) error: Value,
}

#[derive(Debug)]
pub(super) struct OAuthCallback {
    pub(super) code: String,
}

pub(super) fn launch_browser_for_authorization(context: &ResolvedContext, url: &str, no_open: bool) -> BrowserLaunch {
    if no_open {
        eprintln!("Open this URL in a browser: {url}");
        return BrowserLaunch {
            attempted: false,
            opened: false,
            error: Value::Null,
        };
    }

    eprintln!("Opening browser for MyChart OAuth login...");
    match open_browser(url) {
        Ok(()) => BrowserLaunch {
            attempted: true,
            opened: true,
            error: Value::Null,
        },
        Err(error) => {
            let rendered = error.render(true);
            eprintln!("Could not open the browser automatically. Open this URL manually:\n{url}");
            auth_debug(
                context,
                "oauth_browser_open_failed",
                json!({
                    "url": url,
                    "error": rendered,
                }),
            );
            BrowserLaunch {
                attempted: true,
                opened: false,
                error: Value::String(rendered),
            }
        }
    }
}

pub(super) fn prompt_for_callback_url() -> Option<String> {
    if !io::stdin().is_terminal() {
        return None;
    }

    eprintln!(
        "Finish the browser login. The CLI should complete automatically if the callback page can reach the local bridge. If it does not, paste the copied login code here. A callback URL or full `mychart finish ...` command works too."
    );
    eprint!("Login code> ");
    let _ = io::stderr().flush();

    let mut input = String::new();
    match io::stdin().read_line(&mut input) {
        Ok(0) => None,
        Ok(_) => extract_callback_input(&input),
        Err(_) => None,
    }
}

pub(super) fn parse_callback_input(
    input: &str,
    expected_redirect_uri: &Url,
    expected_state: Option<&str>,
) -> Result<Url> {
    let candidate = extract_callback_input(input).ok_or_else(|| {
        Error::Arguments(
            "callback input must include either the redirected URL, the copied login code, or the callback payload copied from the page".into(),
        )
    })?;

    if candidate.starts_with("https://") || candidate.starts_with("http://") {
        return Url::parse(&candidate).map_err(|error| {
            Error::Arguments(format!(
                "callback URL must be an absolute URL copied from the browser redirect: {error}"
            ))
        });
    }

    let mut callback_url = expected_redirect_uri.clone();
    callback_url.set_query(None);
    callback_url.set_fragment(None);
    {
        let mut pairs = callback_url.query_pairs_mut();
        let trimmed = candidate.trim().trim_start_matches('?');
        if trimmed.is_empty() {
            return Err(Error::Arguments(
                "callback payload was empty, copy it again from the callback page".into(),
            ));
        }

        if trimmed.contains('=') {
            let parsed_pairs = Url::parse(&format!("https://callback.invalid/?{trimmed}"))
                .map_err(|error| Error::Arguments(format!("callback payload was malformed: {error}")))?;
            let mut has_state = false;
            for (key, value) in parsed_pairs.query_pairs() {
                has_state |= key == "state";
                pairs.append_pair(&key, &value);
            }
            if !has_state && parsed_pairs.query_pairs().any(|(key, _)| key == "code") {
                let expected_state = expected_state.ok_or_else(|| {
                    Error::Config("missing pending OAuth state, run mychart login or authorize-url first".into())
                })?;
                pairs.append_pair("state", expected_state);
            }
        } else {
            let expected_state = expected_state.ok_or_else(|| {
                Error::Config("missing pending OAuth state, run mychart login or authorize-url first".into())
            })?;
            pairs.append_pair("code", trimmed);
            pairs.append_pair("state", expected_state);
        }
    }
    Ok(callback_url)
}

fn extract_callback_input(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(command_start) = trimmed.find(" finish ") {
        let command_tail = trimmed[command_start + " finish ".len()..].trim();
        if !command_tail.is_empty() {
            return extract_callback_input(command_tail);
        }
    }

    if let Some(start) = trimmed.find("https://").or_else(|| trimmed.find("http://")) {
        let candidate = trimmed[start..]
            .trim()
            .trim_matches(|character| matches!(character, '\'' | '"' | '`' | ')' | ']' | '}'))
            .trim_end_matches(';')
            .to_owned();
        return (!candidate.is_empty()).then_some(candidate);
    }

    if let Some(query_start) = trimmed
        .find("code=")
        .or_else(|| trimmed.find("error="))
        .or_else(|| trimmed.find("state="))
        .or_else(|| trimmed.starts_with('?').then_some(0))
    {
        let candidate = trimmed[query_start..]
            .trim()
            .trim_matches(|character| matches!(character, '\'' | '"' | '`'))
            .trim_end_matches(';')
            .to_owned();
        if !candidate.is_empty() {
            return Some(candidate);
        }
    }

    let bare_candidate = trimmed
        .trim_matches(|character| matches!(character, '\'' | '"' | '`'))
        .trim_end_matches(';')
        .to_owned();
    (!bare_candidate.is_empty()).then_some(bare_candidate)
}

pub(super) fn loopback_bind_address(redirect_uri: &Url) -> Result<String> {
    let host = redirect_uri
        .host_str()
        .ok_or_else(|| Error::Arguments("auth login requires a redirect URI with an explicit host".into()))?;
    if host != "127.0.0.1" && host != "localhost" {
        return Err(Error::Arguments(
            "low-level dynamic auth login requires a loopback redirect URI like http://127.0.0.1:8910/callback; for UCLA use `mychart login ucla`".into(),
        ));
    }
    let port = redirect_uri
        .port_or_known_default()
        .ok_or_else(|| Error::Arguments("auth login requires a redirect URI with an explicit port".into()))?;
    Ok(format!("{host}:{port}"))
}

pub(crate) fn redirect_uri_uses_loopback(redirect_uri: &str) -> Result<bool> {
    let parsed = Url::parse(redirect_uri)
        .map_err(|error| Error::Config(format!("invalid redirect URI {redirect_uri:?}: {error}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| Error::Arguments("auth login requires a redirect URI with an explicit host".into()))?;
    Ok(host == "127.0.0.1" || host == "localhost")
}

pub(super) fn wait_for_oauth_callback(
    listener: TcpListener,
    redirect_uri: &Url,
    expected_state: &str,
    timeout: Duration,
) -> Result<OAuthCallback> {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                return read_oauth_callback(&mut stream, redirect_uri, expected_state);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(Error::Auth {
                        message: format!("timed out waiting {} seconds for the OAuth callback", timeout.as_secs()),
                        details: json!({
                            "redirect_uri": redirect_uri.as_str(),
                        }),
                    });
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                return Err(Error::Io(format!("failed while waiting for OAuth callback: {error}")));
            }
        }
    }
}

pub(super) fn wait_for_hosted_callback_bridge(
    context: &ResolvedContext,
    expected_state: &str,
    timeout: Duration,
) -> Result<Option<String>> {
    let listener = match TcpListener::bind(HOSTED_CALLBACK_BRIDGE_BIND_ADDRESS) {
        Ok(listener) => listener,
        Err(error) => {
            auth_debug(
                context,
                "hosted_callback_bridge_bind_failed",
                json!({
                    "bind_address": HOSTED_CALLBACK_BRIDGE_BIND_ADDRESS,
                    "error": error.to_string(),
                }),
            );
            return Ok(None);
        }
    };
    listener
        .set_nonblocking(true)
        .map_err(|error| Error::Io(format!("failed to configure hosted callback bridge listener: {error}")))?;

    eprintln!("Waiting for hosted callback bridge on {HOSTED_CALLBACK_BRIDGE_URL}");
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => match read_hosted_callback_bridge(&mut stream, expected_state) {
                Ok(Some(callback_input)) => return Ok(Some(callback_input)),
                Ok(None) => {}
                Err(error) => {
                    auth_debug(
                        context,
                        "hosted_callback_bridge_request_failed",
                        json!({
                            "error": error.render(true),
                        }),
                    );
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                return Err(Error::Io(format!(
                    "failed while waiting for hosted callback bridge: {error}"
                )));
            }
        }
    }
}

fn read_oauth_callback(stream: &mut TcpStream, redirect_uri: &Url, expected_state: &str) -> Result<OAuthCallback> {
    let request = read_http_request(stream)?;
    let request_target = request_target(&request)?;
    let callback_url = redirect_uri.join(&request_target).map_err(|error| {
        Error::Http(format!(
            "failed to parse OAuth callback request target {request_target:?}: {error}"
        ))
    })?;

    if callback_url.path() != redirect_uri.path() {
        write_http_response(
            stream,
            404,
            "Not Found",
            callback_page_html("Wrong callback path. Return to the terminal and try again."),
        )?;
        return Err(Error::Auth {
            message: "OAuth callback hit the wrong path".into(),
            details: json!({
                "expected_path": redirect_uri.path(),
                "received_path": callback_url.path(),
            }),
        });
    }

    let params = callback_url.query_pairs().collect::<Vec<_>>();
    if let Some(error) = params
        .iter()
        .find(|(key, _)| key == "error")
        .map(|(_, value)| value.to_string())
    {
        let error_description = params
            .iter()
            .find(|(key, _)| key == "error_description")
            .map(|(_, value)| value.to_string());
        write_http_response(
            stream,
            400,
            "Bad Request",
            callback_page_html("OAuth authorization failed. Return to the terminal for details."),
        )?;
        return Err(Error::Auth {
            message: format!("OAuth authorization failed with error {error}"),
            details: json!({
                "error": error,
                "error_description": error_description,
            }),
        });
    }

    let Some(state) = params
        .iter()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.to_string())
    else {
        write_http_response(
            stream,
            400,
            "Bad Request",
            callback_page_html("OAuth callback was missing state. Return to the terminal and start over."),
        )?;
        return Err(Error::Auth {
            message: "OAuth callback was missing the state parameter".into(),
            details: json!({ "callback_url": callback_url.as_str() }),
        });
    };
    if state != expected_state {
        write_http_response(
            stream,
            400,
            "Bad Request",
            callback_page_html("OAuth state mismatch. Return to the terminal and start over."),
        )?;
        return Err(Error::Auth {
            message: "OAuth callback state mismatch".into(),
            details: json!({
                "expected_state": expected_state,
                "received_state": state,
            }),
        });
    }

    let Some(code) = params
        .iter()
        .find(|(key, _)| key == "code")
        .map(|(_, value)| value.to_string())
    else {
        write_http_response(
            stream,
            400,
            "Bad Request",
            callback_page_html("OAuth callback was missing a code. Return to the terminal and start over."),
        )?;
        return Err(Error::Auth {
            message: "OAuth callback was missing the authorization code".into(),
            details: json!({ "callback_url": callback_url.as_str() }),
        });
    };

    write_http_response(
        stream,
        200,
        "OK",
        callback_page_html("MyChart authorization received. You can close this tab and go back to the terminal."),
    )?;

    Ok(OAuthCallback { code })
}

fn read_hosted_callback_bridge(stream: &mut TcpStream, expected_state: &str) -> Result<Option<String>> {
    let request = read_http_request(stream)?;
    let (method, request_target) = request_parts(&request)?;
    if method == "OPTIONS" {
        write_hosted_bridge_response(stream, 204, "No Content", String::new())?;
        return Ok(None);
    }
    if method != "GET" {
        write_hosted_bridge_response(
            stream,
            405,
            "Method Not Allowed",
            callback_page_html("Hosted callback bridge expects GET."),
        )?;
        return Ok(None);
    }

    let callback_url = Url::parse(&format!("http://127.0.0.1:8911{request_target}")).map_err(|error| {
        Error::Http(format!(
            "failed to parse hosted callback bridge request target {request_target:?}: {error}"
        ))
    })?;

    if callback_url.path() != HOSTED_CALLBACK_BRIDGE_PATH {
        write_hosted_bridge_response(
            stream,
            404,
            "Not Found",
            callback_page_html("Wrong hosted callback bridge path."),
        )?;
        return Ok(None);
    }

    let params = callback_url.query_pairs().collect::<Vec<_>>();
    let state = params
        .iter()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.to_string());
    if state.as_deref() != Some(expected_state) {
        write_hosted_bridge_response(
            stream,
            400,
            "Bad Request",
            callback_page_html("Hosted callback bridge state mismatch."),
        )?;
        return Ok(None);
    }

    let callback_input = callback_url.query().unwrap_or_default().to_owned();
    if callback_input.is_empty() {
        write_hosted_bridge_response(
            stream,
            400,
            "Bad Request",
            callback_page_html("Hosted callback bridge received an empty callback payload."),
        )?;
        return Ok(None);
    }

    write_hosted_bridge_response(
        stream,
        200,
        "OK",
        callback_page_html("MyChart authorization received by the local CLI."),
    )?;
    Ok(Some(callback_input))
}

fn read_http_request(stream: &mut TcpStream) -> Result<String> {
    let mut buffer = Vec::new();
    let mut temp = [0u8; 1024];
    loop {
        let bytes_read = stream
            .read(&mut temp)
            .map_err(|error| Error::Io(format!("failed to read OAuth callback request: {error}")))?;
        if bytes_read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..bytes_read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(buffer)
        .map_err(|error| Error::Http(format!("OAuth callback request was not valid UTF-8: {error}")))
}

fn request_target(request: &str) -> Result<String> {
    let (method, target) = request_parts(request)?;
    if method != "GET" {
        return Err(Error::Http(format!(
            "OAuth callback used unsupported HTTP method {method:?}, expected GET"
        )));
    }
    Ok(target)
}

fn request_parts(request: &str) -> Result<(&str, String)> {
    let first_line = request
        .lines()
        .next()
        .ok_or_else(|| Error::Http("received an empty OAuth callback request".into()))?;
    let mut parts = first_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| Error::Http("OAuth callback request line was missing the HTTP method".into()))?;
    let target = parts
        .next()
        .map(ToOwned::to_owned)
        .ok_or_else(|| Error::Http("OAuth callback request line was missing the request target".into()))?;
    Ok((method, target))
}

fn write_http_response(stream: &mut TcpStream, status_code: u16, reason: &str, body: String) -> Result<()> {
    write_http_response_with_headers(stream, status_code, reason, body, &[])
}

fn write_hosted_bridge_response(stream: &mut TcpStream, status_code: u16, reason: &str, body: String) -> Result<()> {
    write_http_response_with_headers(
        stream,
        status_code,
        reason,
        body,
        &[
            ("Access-Control-Allow-Origin", "*"),
            ("Access-Control-Allow-Methods", "GET, OPTIONS"),
            ("Access-Control-Allow-Private-Network", "true"),
            ("Access-Control-Max-Age", "600"),
        ],
    )
}

fn write_http_response_with_headers(
    stream: &mut TcpStream,
    status_code: u16,
    reason: &str,
    body: String,
    extra_headers: &[(&str, &str)],
) -> Result<()> {
    let rendered_extra_headers = extra_headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    let response = format!(
        "HTTP/1.1 {status_code} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n{rendered_extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| Error::Io(format!("failed to write OAuth callback response: {error}")))
}

fn callback_page_html(message: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>MyChart OAuth</title></head><body><p>{message}</p></body></html>"
    )
}

pub(super) fn open_browser(url: &str) -> Result<()> {
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
