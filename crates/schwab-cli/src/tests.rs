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
fn auth_authorize_url_generates_and_persists_state() {
    let temp_dir = temp_dir("schwab-auth-authorize");
    let config_path = temp_dir.join("config.json");

    let output: Value = run_command(&[
        "schwab",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--client-id",
        "client-id",
        "--client-secret",
        "client-secret",
        "--redirect-uri",
        "https://example.com/callback",
        "--compact",
        "auth",
        "authorize-url",
    ]);

    let state = output
        .get("state")
        .and_then(Value::as_str)
        .expect("authorize output should include state");
    assert_eq!(state.len(), 32);
    assert!(output
        .get("authorize_url")
        .and_then(Value::as_str)
        .expect("authorize url should exist")
        .contains(&format!("state={state}")));

    let stored = StateStore::new(config_path).load().expect("stored state should load");
    assert_eq!(stored.redirect_uri.as_deref(), Some("https://example.com/callback"));
    assert_eq!(stored.pending_oauth_state.as_deref(), Some(state));
}

#[test]
fn auth_exchange_url_rejects_state_mismatch() {
    let temp_dir = temp_dir("schwab-auth-state-mismatch");
    let config_path = temp_dir.join("config.json");
    write_state(
        &config_path,
        SchwabState {
            client_id: Some("client-id".into()),
            client_secret: Some("client-secret".into()),
            redirect_uri: Some("https://example.com/callback".into()),
            pending_oauth_state: Some("expected-state".into()),
            ..SchwabState::default()
        },
    );

    let error = run_command_error(&[
        "schwab",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "auth",
        "exchange-url",
        "https://example.com/callback?code=auth-code&state=wrong-state",
    ]);

    assert_eq!(error.get("kind").and_then(Value::as_str), Some("arguments"));
    assert!(error
        .get("message")
        .and_then(Value::as_str)
        .expect("error message should exist")
        .contains("did not match stored OAuth state"));
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
fn orders_preview_posts_expected_body() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        vec![ResponseSpec::json(
            200,
            json!({
                "orderId": 77,
                "orderValidationResult": {
                    "alerts": []
                }
            }),
        )],
        capture.clone(),
    );
    let temp_dir = temp_dir("schwab-orders-preview");
    let config_path = temp_dir.join("config.json");
    write_state(
        &config_path,
        SchwabState {
            base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            ..SchwabState::default()
        },
    );

    let output: Value = run_command(&[
        "schwab",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "orders",
        "preview",
        "--account",
        "hash-123",
        "--body",
        "{\"orderType\":\"LIMIT\",\"price\":12.34}",
    ]);

    let request = captured_requests(&capture);
    assert_eq!(request.len(), 1);
    assert_eq!(request[0].method, "POST");
    assert_eq!(request[0].path, "/accounts/hash-123/previewOrder");
    assert_eq!(
        request[0].json_body::<Value>(),
        json!({ "orderType": "LIMIT", "price": 12.34 })
    );
    assert_eq!(output.get("orderId").and_then(Value::as_i64), Some(77));
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
fn preferences_get_includes_trader_headers() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        vec![ResponseSpec::json(
            200,
            json!({
                "accounts": []
            }),
        )],
        capture.clone(),
    );
    let temp_dir = temp_dir("schwab-preferences-headers");
    let config_path = temp_dir.join("config.json");
    write_state(
        &config_path,
        SchwabState {
            base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            third_party_id: Some("third-party-id".into()),
            client_channel: Some("GW".into()),
            client_app_id: Some("AD00007919".into()),
            resource_version: Some("1.0".into()),
            rrbus_pilot_rollout: Some("Region=TUP".into()),
            ..SchwabState::default()
        },
    );

    let _: Value = run_command(&[
        "schwab",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "preferences",
        "get",
    ]);

    let request = captured_requests(&capture);
    assert_eq!(request.len(), 1);
    assert_eq!(request[0].path, "/userPreference");
    assert_eq!(request[0].header("accept"), Some("application/json"));
    assert_eq!(request[0].header("content-type"), Some("application/json"));
    assert_eq!(request[0].header("thirdpartyid"), Some("third-party-id"));
    assert_eq!(request[0].header("schwab-client-channel"), Some("GW"));
    assert_eq!(request[0].header("schwab-client-appid"), Some("AD00007919"));
    assert_eq!(request[0].header("schwab-resource-version"), Some("1.0"));
    assert_eq!(request[0].header("schwab-rrbus-pilotrollout"), Some("Region=TUP"));
    assert!(
        request[0]
            .header("schwab-client-correlid")
            .expect("correlation id header should exist")
            .len()
            >= 36
    );
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
            client_channel: Some("GW".into()),
            client_app_id: Some("AD00007919".into()),
            client_function_id: Some("TR123".into()),
            resource_version: Some("2".into()),
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
    assert_eq!(request[0].header("accept"), Some("application/json"));
    assert_eq!(request[0].header("content-type"), Some("application/json"));
    assert_eq!(request[0].header("schwab-client-channel"), Some("GW"));
    assert_eq!(request[0].header("schwab-client-appid"), Some("AD00007919"));
    assert_eq!(request[0].header("schwab-client-functionid"), Some("TR123"));
    assert_eq!(request[0].header("schwab-resource-version"), Some("2"));
    assert!(
        request[0]
            .header("schwab-client-correlid")
            .expect("correlation id header should exist")
            .len()
            >= 36
    );
    assert!(output.contains_key("AAPL"));
}

#[test]
fn market_chain_builds_expected_query() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        vec![ResponseSpec::json(
            200,
            json!({
                "symbol": "AAPL",
                "status": "SUCCESS"
            }),
        )],
        capture.clone(),
    );
    let temp_dir = temp_dir("schwab-market-chain");
    let config_path = temp_dir.join("config.json");
    write_state(
        &config_path,
        SchwabState {
            market_data_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            ..SchwabState::default()
        },
    );

    let output: Value = run_command(&[
        "schwab",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "market",
        "chain",
        "AAPL",
        "--contract-type",
        "CALL",
        "--strike-count",
        "4",
        "--include-underlying-quote",
        "--strategy",
        "SINGLE",
        "--from-date",
        "2026-04-01",
        "--to-date",
        "2026-04-30",
        "--exp-month",
        "APR",
        "--entitlement",
        "NP",
    ]);

    let request = captured_requests(&capture);
    assert_eq!(request.len(), 1);
    assert_eq!(request[0].path, "/chains");
    assert_eq!(request[0].query_value("symbol"), Some("AAPL"));
    assert_eq!(request[0].query_value("contractType"), Some("CALL"));
    assert_eq!(request[0].query_value("strikeCount"), Some("4"));
    assert_eq!(request[0].query_value("includeUnderlyingQuote"), Some("true"));
    assert_eq!(request[0].query_value("strategy"), Some("SINGLE"));
    assert_eq!(request[0].query_value("fromDate"), Some("2026-04-01"));
    assert_eq!(request[0].query_value("toDate"), Some("2026-04-30"));
    assert_eq!(request[0].query_value("expMonth"), Some("APR"));
    assert_eq!(request[0].query_value("entitlement"), Some("NP"));
    assert_eq!(output.get("symbol").and_then(Value::as_str), Some("AAPL"));
}

#[test]
fn market_instrument_fetches_by_cusip() {
    let capture = Arc::new(Mutex::new(Vec::new()));
    let server = TestServer::spawn(
        vec![ResponseSpec::json(
            200,
            json!({
                "cusip": "037833100",
                "symbol": "AAPL"
            }),
        )],
        capture.clone(),
    );
    let temp_dir = temp_dir("schwab-market-instrument");
    let config_path = temp_dir.join("config.json");
    write_state(
        &config_path,
        SchwabState {
            market_data_base_url: Some(server.base_url()),
            access_token: Some("access-token".into()),
            ..SchwabState::default()
        },
    );

    let output: Value = run_command(&[
        "schwab",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "market",
        "instrument",
        "037833100",
    ]);

    let request = captured_requests(&capture);
    assert_eq!(request.len(), 1);
    assert_eq!(request[0].path, "/instruments/037833100");
    assert_eq!(output.get("symbol").and_then(Value::as_str), Some("AAPL"));
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

fn run_command_error(args: &[&str]) -> Value {
    let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
    let compact = cli.global.compact;
    let error = run(cli).expect_err("command should fail");
    serde_json::from_str(&render_cli_error(&error, compact)).expect("error output should be valid json")
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
