use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::ExitCode,
    sync::{Arc, Mutex},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::json;

use super::{
    main_entry, render_cli_error, run,
    state::{MomenceState, StateStore},
    Cli,
};

#[test]
fn main_entry_returns_success_for_help() {
    assert_eq!(main_entry(["momence", "--help"]), ExitCode::SUCCESS);
}

#[test]
fn login_password_stores_tokens_and_prints_response() {
    let capture = Arc::new(Mutex::new(None));
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

    let output: LoginPasswordResponse = run_command(&[
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

    let request = captured_request(&capture);
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/api/v2/auth/token");
    assert_eq!(
        request.header("authorization"),
        Some("Basic Y2xpZW50LWlkOmNsaWVudC1zZWNyZXQ=")
    );
    assert_eq!(request.form_value("grant_type").as_deref(), Some("password"));
    assert_eq!(request.form_value("username").as_deref(), Some("member@example.com"));
    assert_eq!(request.form_value("password").as_deref(), Some("super-secret"));

    let state = StateStore::new(config_path).load().expect("stored state should load");
    assert_eq!(state.access_token.as_deref(), Some("access-token"));
    assert_eq!(state.refresh_token.as_deref(), Some("refresh-token"));
    assert_eq!(output.access_token, "access-token");
}

#[test]
fn member_sessions_list_sends_bearer_token_and_query() {
    let capture = Arc::new(Mutex::new(None));
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

    let output: MemberSessionsResponse = run_command(&[
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

    let request = captured_request(&capture);
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/api/v2/member/sessions");
    assert_eq!(request.query_value("page"), Some("0"));
    assert_eq!(request.query_value("pageSize"), Some("100"));
    assert_eq!(request.query_value("startAfter"), Some("2026-03-01T00:00:00Z"));
    assert_eq!(request.header("authorization"), Some("Bearer stored-access-token"));
    assert_eq!(output.payload.len(), 1);
    assert_eq!(output.payload[0].id, 1);
}

#[test]
fn cancel_booking_prints_empty_success_payload() {
    let capture = Arc::new(Mutex::new(None));
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

    let output: EmptySuccessResponse = run_command(&[
        "momence",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "member",
        "sessions",
        "cancel",
        "77",
    ]);

    let request = captured_request(&capture);
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.path, "/api/v2/member/sessions/77");
    assert_eq!(output.status, "ok");
    assert_eq!(output.status_code, 200);
}

#[derive(Debug, Deserialize)]
struct LoginPasswordResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct MemberSessionsResponse {
    payload: Vec<MemberSessionPayload>,
}

#[derive(Debug, Deserialize)]
struct MemberSessionPayload {
    id: u64,
}

#[derive(Debug, Deserialize)]
struct EmptySuccessResponse {
    status: String,
    status_code: u16,
}

fn run_command<T: DeserializeOwned>(args: &[&str]) -> T {
    let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
    let compact = cli.global.compact;
    let (value, _) = run(cli).unwrap_or_else(|error| panic!("{}", render_cli_error(&error, compact)));
    serde_json::from_value(value).expect("command output should match expected type")
}

struct TestServer {
    address: String,
    _handle: thread::JoinHandle<()>,
}

impl TestServer {
    fn spawn(body: String, status_code: u16, capture: Option<Arc<Mutex<Option<CapturedRequest>>>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let address = listener.local_addr().expect("local addr should exist");

        let handle = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let request = read_request(&mut stream);
                if let Some(capture) = capture {
                    if let Ok(mut guard) = capture.lock() {
                        *guard = Some(request);
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

#[derive(Clone, Debug)]
struct CapturedRequest {
    method: String,
    path: String,
    query: BTreeMap<String, Vec<String>>,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl CapturedRequest {
    fn parse(buffer: &[u8]) -> Self {
        let headers_end = find_headers_end(buffer).expect("request should include headers");
        let headers = String::from_utf8_lossy(&buffer[..headers_end]);
        let mut lines = headers.lines();
        let request_line = lines.next().expect("request line should exist");
        let mut request_parts = request_line.split_whitespace();
        let method = request_parts.next().expect("method should exist").to_owned();
        let target = request_parts.next().expect("target should exist");
        let (path, query) = split_target(target);
        let headers = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            })
            .collect();

        Self {
            method,
            path,
            query,
            headers,
            body: buffer[headers_end + 4..].to_vec(),
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    fn form_value(&self, name: &str) -> Option<String> {
        let form = parse_form_encoded(&self.body);
        form.get(name).and_then(|values| values.last()).cloned()
    }

    fn query_value(&self, name: &str) -> Option<&str> {
        self.query_or_form_value(name, &self.query)
    }

    fn query_or_form_value<'a>(&'a self, name: &str, values: &'a BTreeMap<String, Vec<String>>) -> Option<&'a str> {
        values.get(name)?.last().map(String::as_str)
    }
}

fn captured_request(capture: &Arc<Mutex<Option<CapturedRequest>>>) -> CapturedRequest {
    capture
        .lock()
        .expect("capture lock should work")
        .clone()
        .expect("request should be captured")
}

fn read_request(stream: &mut std::net::TcpStream) -> CapturedRequest {
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

    CapturedRequest::parse(&buffer)
}

fn find_headers_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn split_target(target: &str) -> (String, BTreeMap<String, Vec<String>>) {
    let Some((path, query)) = target.split_once('?') else {
        return (target.to_owned(), BTreeMap::new());
    };

    (path.to_owned(), parse_www_form(query))
}

fn parse_form_encoded(body: &[u8]) -> BTreeMap<String, Vec<String>> {
    if body.is_empty() {
        return BTreeMap::new();
    }

    let body = String::from_utf8_lossy(body);
    parse_www_form(&body)
}

fn parse_www_form(input: &str) -> BTreeMap<String, Vec<String>> {
    let mut values = BTreeMap::new();
    for pair in input.split('&').filter(|pair| !pair.is_empty()) {
        let (raw_key, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode_form_component(raw_key);
        let value = decode_form_component(raw_value);
        values.entry(key).or_insert_with(Vec::new).push(value);
    }
    values
}

fn decode_form_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = &value[index + 1..index + 3];
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    decoded.push(byte);
                    index += 3;
                } else {
                    decoded.push(bytes[index]);
                    index += 1;
                }
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8(decoded).expect("form component should decode as utf-8")
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
