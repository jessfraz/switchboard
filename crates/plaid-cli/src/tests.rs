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
use serde_json::{json, Value};

use super::{
    main_entry, render_cli_error, run,
    state::{PlaidEnvironment, PlaidState, StateStore, DEFAULT_PLAID_VERSION},
    Cli,
};

#[test]
fn main_entry_returns_success_for_help() {
    assert_eq!(main_entry(["plaid", "--help"]), ExitCode::SUCCESS);
}

#[test]
fn exchange_public_token_stores_access_token_and_item_id() {
    let capture = Arc::new(Mutex::new(None));
    let server = TestServer::spawn(
        json!({
            "access_token": "access-sandbox-1234",
            "item_id": "item-1234",
            "request_id": "request-1"
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("plaid-exchange-public-token");
    let config_path = temp_dir.join("config.json");

    let output: ExchangePublicTokenResponse = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--environment",
        "sandbox",
        "--base-url",
        &server.base_url(),
        "--client-id",
        "client-id",
        "--secret",
        "secret-value",
        "--compact",
        "auth",
        "exchange-public-token",
        "--public-token",
        "public-sandbox-abc",
    ]);

    let request = captured_request(&capture);
    let body: Value = request.json_body();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/item/public_token/exchange");
    assert_eq!(request.header("plaid-client-id"), Some("client-id"));
    assert_eq!(request.header("plaid-secret"), Some("secret-value"));
    assert_eq!(request.header("plaid-version"), Some(DEFAULT_PLAID_VERSION));
    assert_eq!(body["public_token"], "public-sandbox-abc");

    let state = StateStore::new(config_path).load().expect("state should load");
    assert_eq!(state.environment, Some(PlaidEnvironment::Sandbox));
    assert_eq!(state.access_token.as_deref(), Some("access-sandbox-1234"));
    assert_eq!(state.item_id.as_deref(), Some("item-1234"));
    assert_eq!(output.access_token, "access-sandbox-1234");
}

#[test]
fn link_token_create_builds_user_products_and_transactions_options() {
    let capture = Arc::new(Mutex::new(None));
    let server = TestServer::spawn(
        json!({
            "link_token": "link-sandbox-1234",
            "expiration": "2026-04-01T00:00:00Z",
            "request_id": "request-2"
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("plaid-link-token");
    let config_path = temp_dir.join("config.json");

    let output: LinkTokenCreateResponse = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--base-url",
        &server.base_url(),
        "--client-id",
        "client-id",
        "--secret",
        "secret-value",
        "--client-name",
        "switchboard",
        "--compact",
        "link",
        "token-create",
        "--client-user-id",
        "user-123",
        "--product",
        "transactions",
        "--product",
        "auth",
        "--country-code",
        "US",
        "--country-code",
        "CA",
        "--days-requested",
        "180",
        "--webhook",
        "https://example.com/plaid-webhook",
    ]);

    let request = captured_request(&capture);
    let body: Value = request.json_body();
    assert_eq!(request.path, "/link/token/create");
    assert_eq!(body["client_name"], "switchboard");
    assert_eq!(body["language"], "en");
    assert_eq!(body["user"]["client_user_id"], "user-123");
    assert_eq!(body["products"], json!(["transactions", "auth"]));
    assert_eq!(body["country_codes"], json!(["US", "CA"]));
    assert_eq!(body["transactions"]["days_requested"], 180);
    assert_eq!(body["webhook"], "https://example.com/plaid-webhook");
    assert_eq!(output.link_token, "link-sandbox-1234");
}

#[test]
fn accounts_balance_uses_stored_access_token_and_filters() {
    let capture = Arc::new(Mutex::new(None));
    let server = TestServer::spawn(
        json!({
            "accounts": [
                {
                    "account_id": "acc-123",
                    "balances": {
                        "available": 100.5,
                        "current": 125.0
                    }
                }
            ],
            "item": {
                "item_id": "item-1234"
            },
            "request_id": "request-3"
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("plaid-accounts-balance");
    let config_path = temp_dir.join("config.json");
    let store = StateStore::new(config_path.clone());
    store
        .save(&PlaidState {
            base_url: Some(server.base_url()),
            client_id: Some("client-id".into()),
            secret: Some("secret-value".into()),
            access_token: Some("stored-access-token".into()),
            item_id: Some("item-1234".into()),
            ..PlaidState::default()
        })
        .expect("state should save");

    let output: AccountsBalanceResponse = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "accounts",
        "balance",
        "--account-id",
        "acc-123",
        "--min-last-updated-datetime",
        "2026-03-30T00:00:00Z",
    ]);

    let request = captured_request(&capture);
    let body: Value = request.json_body();
    assert_eq!(request.path, "/accounts/balance/get");
    assert_eq!(body["access_token"], "stored-access-token");
    assert_eq!(body["options"]["account_ids"], json!(["acc-123"]));
    assert_eq!(body["options"]["min_last_updated_datetime"], "2026-03-30T00:00:00Z");
    assert_eq!(output.accounts[0].account_id, "acc-123");
}

#[test]
fn transactions_sync_sends_cursor_count_and_options() {
    let capture = Arc::new(Mutex::new(None));
    let server = TestServer::spawn(
        json!({
            "added": [],
            "modified": [],
            "removed": [],
            "next_cursor": "cursor-next",
            "has_more": false,
            "request_id": "request-4"
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("plaid-transactions-sync");
    let config_path = temp_dir.join("config.json");
    let store = StateStore::new(config_path.clone());
    store
        .save(&PlaidState {
            base_url: Some(server.base_url()),
            client_id: Some("client-id".into()),
            secret: Some("secret-value".into()),
            access_token: Some("stored-access-token".into()),
            ..PlaidState::default()
        })
        .expect("state should save");

    let output: TransactionsSyncResponse = run_command(&[
        "plaid",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "transactions",
        "sync",
        "--cursor",
        "now",
        "--count",
        "250",
        "--days-requested",
        "180",
        "--account-id",
        "acc-123",
        "--include-original-description",
    ]);

    let request = captured_request(&capture);
    let body: Value = request.json_body();
    assert_eq!(request.path, "/transactions/sync");
    assert_eq!(body["cursor"], "now");
    assert_eq!(body["count"], 250);
    assert_eq!(body["options"]["account_id"], "acc-123");
    assert_eq!(body["options"]["days_requested"], 180);
    assert_eq!(body["options"]["include_original_description"], true);
    assert_eq!(output.next_cursor, "cursor-next");
}

#[test]
fn sandbox_public_token_create_rejects_transaction_options_without_transactions_product() {
    let error: JsonErrorResponse = run_command_error(&[
        "plaid",
        "--client-id",
        "client-id",
        "--secret",
        "secret-value",
        "sandbox",
        "public-token-create",
        "--institution-id",
        "ins_109508",
        "--product",
        "auth",
        "--days-requested",
        "180",
    ]);

    assert_eq!(error.kind, "arguments");
    assert_eq!(
        error.message,
        "transaction sandbox options require --product transactions"
    );
}

#[derive(Debug, Deserialize)]
struct ExchangePublicTokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct LinkTokenCreateResponse {
    link_token: String,
}

#[derive(Debug, Deserialize)]
struct AccountsBalanceResponse {
    accounts: Vec<AccountSummary>,
}

#[derive(Debug, Deserialize)]
struct AccountSummary {
    account_id: String,
}

#[derive(Debug, Deserialize)]
struct TransactionsSyncResponse {
    next_cursor: String,
}

#[derive(Debug, Deserialize)]
struct JsonErrorResponse {
    kind: String,
    message: String,
}

fn run_command<T: DeserializeOwned>(args: &[&str]) -> T {
    let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
    let compact = cli.global.compact;
    let (value, _) = run(cli).unwrap_or_else(|error| panic!("{}", render_cli_error(&error, compact)));
    serde_json::from_value(value).expect("command output should match expected type")
}

fn run_command_error<T: DeserializeOwned>(args: &[&str]) -> T {
    let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
    let compact = cli.global.compact;
    let error = run(cli).expect_err("command should fail");
    let rendered = render_cli_error(&error, compact);
    serde_json::from_str(&rendered).expect("error output should match expected type")
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
        let path = request_parts
            .next()
            .expect("target should exist")
            .split('?')
            .next()
            .expect("path should exist")
            .to_owned();
        let headers = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            })
            .collect();

        Self {
            method,
            path,
            headers,
            body: buffer[headers_end + 4..].to_vec(),
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    fn json_body<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).expect("request body should be valid json")
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
