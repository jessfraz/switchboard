use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, Mutex},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

use super::{
    main_entry, render_cli_error, run,
    state::{SchwabState, StateStore},
    Cli,
};

#[test]
fn main_entry_returns_success_for_help() {
    assert_eq!(main_entry(["schwab", "--help"]), ExitCode::SUCCESS);
}

#[test]
fn auth_exchange_code_stores_tokens() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        vec![ResponseSpec::json(
            200,
            json!({
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "token_type": "Bearer",
                "scope": "readonly",
                "expires_in": 1800
            }),
        )],
        capture.clone(),
    );
    let temp_dir = temp_dir("schwab-auth-exchange");
    let config_path = temp_dir.join("config.json");

    let output: AuthExchangeResponse = run_command(&[
        "schwab",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--base-url",
        &server.base_url(),
        "--token-url",
        &format!("{}/oauth/token", server.base_url()),
        "--client-id",
        "client-id",
        "--client-secret",
        "client-secret",
        "--redirect-uri",
        "https://127.0.0.1/callback",
        "--compact",
        "auth",
        "exchange-code",
        "--code",
        "auth-code",
    ]);

    let request = captured_requests(&capture);
    assert_eq!(request.len(), 1);
    assert_eq!(request[0].method, "POST");
    assert_eq!(request[0].path, "/oauth/token");
    assert_eq!(
        request[0].header("authorization"),
        Some("Basic Y2xpZW50LWlkOmNsaWVudC1zZWNyZXQ=")
    );
    assert_eq!(
        request[0].form_value("grant_type").as_deref(),
        Some("authorization_code")
    );
    assert_eq!(request[0].form_value("code").as_deref(), Some("auth-code"));
    assert_eq!(
        request[0].form_value("redirect_uri").as_deref(),
        Some("https://127.0.0.1/callback")
    );

    let state = StateStore::new(config_path).load().expect("stored state should load");
    assert_eq!(state.access_token.as_deref(), Some("access-token"));
    assert_eq!(state.refresh_token.as_deref(), Some("refresh-token"));
    assert_eq!(state.redirect_uri.as_deref(), Some("https://127.0.0.1/callback"));
    assert_eq!(output.access_token, "access-token");
}

#[test]
fn accounts_get_resolves_plain_account_number_to_hash() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        vec![
            ResponseSpec::json(
                200,
                json!([
                    {
                        "accountNumber": "123456789",
                        "hashValue": "hash-123"
                    }
                ]),
            ),
            ResponseSpec::json(
                200,
                json!({
                    "securitiesAccount": {
                        "accountNumber": "hash-123",
                        "type": "BROKERAGE"
                    }
                }),
            ),
        ],
        capture.clone(),
    );
    let temp_dir = temp_dir("schwab-accounts-get");
    let config_path = temp_dir.join("config.json");
    write_state(
        &config_path,
        SchwabState {
            base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            ..SchwabState::default()
        },
    );

    let output: AccountGetResponse = run_command(&[
        "schwab",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "accounts",
        "get",
        "123456789",
        "--positions",
    ]);

    let request = captured_requests(&capture);
    assert_eq!(request.len(), 2);
    assert_eq!(request[0].path, "/accounts/accountNumbers");
    assert_eq!(request[1].path, "/accounts/hash-123");
    assert_eq!(request[1].query_value("fields"), Some("positions"));
    assert_eq!(
        StateStore::new(config_path)
            .load()
            .expect("state should load")
            .account_numbers[0]
            .hash_value
            .as_deref(),
        Some("hash-123")
    );
    assert_eq!(
        output.securities_account.get("accountNumber").and_then(Value::as_str),
        Some("hash-123")
    );
}

#[test]
fn orders_place_returns_location_metadata() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        vec![ResponseSpec::with_headers(
            201,
            String::new(),
            vec![("Location".into(), "https://example.test/orders/55".into())],
        )],
        capture.clone(),
    );
    let temp_dir = temp_dir("schwab-orders-place");
    let config_path = temp_dir.join("config.json");
    write_state(
        &config_path,
        SchwabState {
            base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            ..SchwabState::default()
        },
    );

    let output: EmptySuccessResponse = run_command(&[
        "schwab",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "orders",
        "place",
        "--account",
        "hash-123",
        "--body",
        "{\"orderType\":\"MARKET\"}",
    ]);

    let request = captured_requests(&capture);
    assert_eq!(request.len(), 1);
    assert_eq!(request[0].method, "POST");
    assert_eq!(request[0].path, "/accounts/hash-123/orders");
    assert_eq!(request[0].json_body::<Value>(), json!({ "orderType": "MARKET" }));
    assert_eq!(output.status_code, 201);
    assert_eq!(output.location.as_deref(), Some("https://example.test/orders/55"));
}

#[test]
fn transactions_list_builds_expected_query() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        vec![ResponseSpec::json(
            200,
            json!([
                {
                    "type": "TRADE",
                    "transactionId": 99
                }
            ]),
        )],
        capture.clone(),
    );
    let temp_dir = temp_dir("schwab-transactions-list");
    let config_path = temp_dir.join("config.json");
    write_state(
        &config_path,
        SchwabState {
            base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            account_numbers: vec![super::state::AccountNumberHashEntry {
                account_number: Some("123456789".into()),
                hash_value: Some("hash-123".into()),
                synced_at_epoch_seconds: None,
            }],
            ..SchwabState::default()
        },
    );

    let output: Vec<Value> = run_command(&[
        "schwab",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "transactions",
        "list",
        "--account",
        "123456789",
        "--start-date",
        "2026-03-01T00:00:00.000Z",
        "--end-date",
        "2026-03-27T23:59:59.000Z",
        "--types",
        "TRADE",
        "--symbol",
        "AAPL",
    ]);

    let request = captured_requests(&capture);
    assert_eq!(request.len(), 1);
    assert_eq!(request[0].path, "/accounts/hash-123/transactions");
    assert_eq!(request[0].query_value("startDate"), Some("2026-03-01T00:00:00.000Z"));
    assert_eq!(request[0].query_value("endDate"), Some("2026-03-27T23:59:59.000Z"));
    assert_eq!(request[0].query_value("types"), Some("TRADE"));
    assert_eq!(request[0].query_value("symbol"), Some("AAPL"));
    assert_eq!(output.len(), 1);
}

#[test]
fn market_quotes_use_market_data_base_url() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        vec![ResponseSpec::json(
            200,
            json!({
                "AAPL": {
                    "assetMainType": "EQUITY"
                }
            }),
        )],
        capture.clone(),
    );
    let temp_dir = temp_dir("schwab-market-quotes");
    let config_path = temp_dir.join("config.json");
    write_state(
        &config_path,
        SchwabState {
            market_data_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            ..SchwabState::default()
        },
    );

    let output: BTreeMap<String, Value> = run_command(&[
        "schwab",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "market",
        "quotes",
        "--symbol",
        "AAPL,MSFT",
        "--field",
        "quote,reference",
        "--indicative",
    ]);

    let request = captured_requests(&capture);
    assert_eq!(request.len(), 1);
    assert_eq!(request[0].path, "/quotes");
    assert_eq!(request[0].query_value("symbols"), Some("AAPL,MSFT"));
    assert_eq!(request[0].query_value("fields"), Some("quote,reference"));
    assert_eq!(request[0].query_value("indicative"), Some("true"));
    assert!(output.contains_key("AAPL"));
}

#[derive(Debug, Deserialize)]
struct AuthExchangeResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct AccountGetResponse {
    #[serde(rename = "securitiesAccount")]
    securities_account: Value,
}

#[derive(Debug, Deserialize)]
struct EmptySuccessResponse {
    status_code: u16,
    location: Option<String>,
}

fn run_command<T: DeserializeOwned>(args: &[&str]) -> T {
    let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
    let compact = cli.global.compact;
    let (value, _) = run(cli).unwrap_or_else(|error| panic!("{}", render_cli_error(&error, compact)));
    serde_json::from_value(value).expect("command output should match expected type")
}

fn write_state(path: &Path, state: SchwabState) {
    let store = StateStore::new(path.to_path_buf());
    store.save(&state).expect("state should save");
}

fn captured_requests(capture: &Arc<Mutex<Vec<CapturedRequest>>>) -> Vec<CapturedRequest> {
    capture.lock().expect("capture should lock").clone()
}

struct TestServer {
    address: String,
    _handle: thread::JoinHandle<()>,
}

impl TestServer {
    fn spawn(responses: Vec<ResponseSpec>, capture: Arc<Mutex<Vec<CapturedRequest>>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let address = listener.local_addr().expect("local addr should exist");
        let response_count = responses.len();
        let queued = Arc::new(Mutex::new(VecDeque::from(responses)));

        let handle = {
            let queued = Arc::clone(&queued);
            thread::spawn(move || {
                for _ in 0..response_count {
                    let (mut stream, _) = listener.accept().expect("request should connect");
                    let request = read_request(&mut stream);
                    capture.lock().expect("capture should lock").push(request);
                    let response = queued
                        .lock()
                        .expect("responses should lock")
                        .pop_front()
                        .expect("response should exist");
                    let status_text = match response.status_code {
                        200 => "OK",
                        201 => "Created",
                        400 => "Bad Request",
                        401 => "Unauthorized",
                        404 => "Not Found",
                        _ => "OK",
                    };
                    let mut headers = String::from("Content-Type: application/json\r\nConnection: close\r\n");
                    for (key, value) in response.headers {
                        headers.push_str(&format!("{key}: {value}\r\n"));
                    }
                    let response_text = format!(
                        "HTTP/1.1 {} {}\r\n{}Content-Length: {}\r\n\r\n{}",
                        response.status_code,
                        status_text,
                        headers,
                        response.body.len(),
                        response.body
                    );
                    let _ = stream.write_all(response_text.as_bytes());
                }
            })
        };

        Self {
            address: format!("http://{address}"),
            _handle: handle,
        }
    }

    fn base_url(&self) -> String {
        self.address.clone()
    }
}

#[derive(Clone)]
struct ResponseSpec {
    status_code: u16,
    body: String,
    headers: Vec<(String, String)>,
}

impl ResponseSpec {
    fn json(status_code: u16, body: Value) -> Self {
        Self {
            status_code,
            body: body.to_string(),
            headers: Vec::new(),
        }
    }

    fn with_headers(status_code: u16, body: String, headers: Vec<(String, String)>) -> Self {
        Self {
            status_code,
            body,
            headers,
        }
    }
}

#[derive(Clone, Debug)]
struct CapturedRequest {
    method: String,
    path: String,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: String,
}

impl CapturedRequest {
    fn header(&self, key: &str) -> Option<&str> {
        self.headers.get(key).map(String::as_str)
    }

    fn query_value(&self, key: &str) -> Option<&str> {
        self.query.get(key).map(String::as_str)
    }

    fn form_value(&self, key: &str) -> Option<String> {
        decode_form_pairs(&self.body)
            .into_iter()
            .find_map(|(candidate, value)| (candidate == key).then_some(value))
    }

    fn json_body<T: DeserializeOwned>(&self) -> T {
        serde_json::from_str(&self.body).expect("request body should be valid json")
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> CapturedRequest {
    let mut buffer = Vec::new();
    let mut temp = [0u8; 1024];
    let header_end = loop {
        let read = stream.read(&mut temp).expect("request bytes should read");
        if read == 0 {
            break None;
        }
        buffer.extend_from_slice(&temp[..read]);
        if let Some(index) = find_bytes(&buffer, b"\r\n\r\n") {
            break Some(index + 4);
        }
    }
    .expect("request should include headers");

    let headers_bytes = &buffer[..header_end];
    let headers_text = String::from_utf8_lossy(headers_bytes);
    let mut lines = headers_text.split("\r\n").filter(|line| !line.is_empty());
    let request_line = lines.next().expect("request line should exist");
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().expect("method should exist").to_owned();
    let target = request_parts.next().expect("target should exist");
    let (path, query) = parse_target(target);

    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect::<BTreeMap<_, _>>();

    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut temp).expect("request body should read");
        if read == 0 {
            break;
        }
        body.extend_from_slice(&temp[..read]);
    }

    CapturedRequest {
        method,
        path,
        query,
        headers,
        body: String::from_utf8_lossy(&body[..content_length]).into_owned(),
    }
}

fn parse_target(target: &str) -> (String, BTreeMap<String, String>) {
    if let Some((path, query)) = target.split_once('?') {
        (path.to_owned(), decode_form_pairs(query).into_iter().collect())
    } else {
        (target.to_owned(), BTreeMap::new())
    }
}

fn decode_form_pairs(input: &str) -> Vec<(String, String)> {
    input
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn percent_decode(input: &str) -> String {
    let mut decoded = Vec::new();
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = from_hex(bytes[index + 1]);
                let low = from_hex(bytes[index + 2]);
                if let (Some(high), Some(low)) = (high, low) {
                    decoded.push((high << 4) | low);
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
    String::from_utf8(decoded).expect("percent-decoded bytes should be utf-8")
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn temp_dir(prefix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{unique}"));
    fs::create_dir_all(&path).expect("temp dir should create");
    path
}
