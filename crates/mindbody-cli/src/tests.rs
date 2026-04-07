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

use super::{main_entry, render_cli_error, run, Cli};

#[test]
fn main_entry_returns_success_for_help() {
    assert_eq!(main_entry(["mindbody", "--help"]), ExitCode::SUCCESS);
}

#[test]
fn account_status_reports_resolved_context_without_leaking_secrets() {
    let temp_dir = temp_dir("mindbody-account-status");
    let config_path = temp_dir.join("config.json");
    write_config(
        &config_path,
        json!({
            "base_url": "https://mb-api.mindbodyonline.com/affiliate/api/v1",
            "app_name": "switchboard-mindbody-test",
            "user_id": "pilates.user",
            "api_key": "api-key",
            "client_key": "client-key"
        }),
    );

    let output: AccountStatusResponse = run_command(&[
        "mindbody",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "account",
        "status",
    ]);

    assert_eq!(output.status, "ok");
    assert_eq!(output.provider, "mindbody");
    assert_eq!(output.user_id, "pilates.user");
    assert!(output.has_api_key);
    assert!(output.has_client_key);
    assert!(!output.has_client_secret);
}

#[test]
fn locations_search_sends_headers_and_query() {
    let capture = Arc::new(Mutex::new(None));
    let server = TestServer::spawn(
        json!({
            "items": [
                {
                    "id": 86784,
                    "name": "Pilates Palace"
                }
            ],
            "offset": 0,
            "maxResults": 1,
            "totalResults": 1
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("mindbody-locations");
    let config_path = temp_dir.join("config.json");
    write_config(
        &config_path,
        json!({
            "base_url": server.base_url(),
            "api_key": "api-key",
            "client_key": "client-key",
            "client_secret": "client-secret"
        }),
    );

    let output: LocationsSearchResponse = run_command(&[
        "mindbody",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "locations",
        "search",
        "--search-text",
        "pilates",
        "--country-code",
        "US",
        "--max-results",
        "5",
    ]);

    let request = captured_request(&capture);
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/locations");
    assert_eq!(request.query_value("searchText"), Some("pilates"));
    assert_eq!(request.query_value("countryCode"), Some("US"));
    assert_eq!(request.query_value("maxResults"), Some("5"));
    assert_eq!(request.header("api-key"), Some("api-key"));
    assert!(request
        .header("authorization")
        .is_some_and(|value| value.starts_with("Basic ")));
    assert_eq!(output.items[0].id, 86784);
}

#[test]
fn bookings_create_with_pass_builds_typed_body() {
    let capture = Arc::new(Mutex::new(None));
    let server = TestServer::spawn(
        json!({
            "booking": {
                "id": "booking-1"
            }
        })
        .to_string(),
        201,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("mindbody-booking-pass");
    let config_path = temp_dir.join("config.json");
    write_config(
        &config_path,
        json!({
            "base_url": server.base_url(),
            "api_key": "api-key",
            "client_key": "client-key",
            "client_secret": "client-secret",
            "user_id": "pilates.user"
        }),
    );

    let output: BookingCreateResponse = run_command(&[
        "mindbody",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "bookings",
        "create",
        "--location-id",
        "86784",
        "--class-id",
        "5134512",
        "--reconciliation-type",
        "pass",
        "--reconciliation-id",
        "598a6916-7876-406e-9537-db6af825f9a2",
        "--idempotency-key",
        "123e4567-e89b-12d3-a456-426614174000",
        "--suppress-booking-confirmation-email",
    ]);

    let request = captured_request(&capture);
    let body: BookingCreateRequestBody = request.json_body();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/bookings");
    assert_eq!(
        request.header("idempotency-key"),
        Some("123e4567-e89b-12d3-a456-426614174000")
    );
    assert_eq!(body.unique_user_id, "pilates.user");
    assert_eq!(body.booking_reconciliation.r#type, "Pass");
    assert!(body.payment_details.is_none());
    assert_eq!(output.booking.id, "booking-1");
}

#[test]
fn pricing_option_booking_requires_purchase_fields() {
    let error: JsonErrorResponse = run_command_error(&[
        "mindbody",
        "--api-key",
        "api-key",
        "--client-key",
        "client-key",
        "--client-secret",
        "client-secret",
        "--user-id",
        "pilates.user",
        "bookings",
        "create",
        "--location-id",
        "86784",
        "--class-id",
        "5134512",
        "--reconciliation-type",
        "pricing-option",
        "--reconciliation-id",
        "153458",
    ]);

    assert_eq!(error.kind, "arguments");
    assert_eq!(error.message, "pricing-option bookings require --pricing-option-total");
}

#[test]
fn pricing_option_booking_posts_purchase_fields_and_payment_details() {
    let capture = Arc::new(Mutex::new(None));
    let server = TestServer::spawn(
        json!({
            "booking": {
                "id": "booking-2"
            }
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("mindbody-booking-pricing-option");
    let config_path = temp_dir.join("config.json");
    let payment_path = temp_dir.join("payment.json");
    write_config(
        &config_path,
        json!({
            "base_url": server.base_url(),
            "api_key": "api-key",
            "client_key": "client-key",
            "client_secret": "client-secret",
            "user_id": "pilates.user"
        }),
    );
    fs::write(
        &payment_path,
        json!({
            "creditCardNumber": "4111111111111111",
            "creditCardExpirationYear": 2030,
            "creditCardExpirationMonth": 12,
            "creditCardCvv": "123",
            "billingName": "Ada Lovelace",
            "billingAddressLine1": "123 Main St",
            "billingAddressLine2": "Unit 4",
            "billingCity": "Los Angeles",
            "billingState": "CA",
            "billingPostalCode": "90001"
        })
        .to_string(),
    )
    .expect("payment json should write");

    let output: BookingCreateResponse = run_command(&[
        "mindbody",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "bookings",
        "create",
        "--location-id",
        "86784",
        "--class-id",
        "5134512",
        "--reconciliation-type",
        "pricing-option",
        "--reconciliation-id",
        "153458",
        "--pricing-option-total",
        "42.5",
        "--suppress-purchase-receipt-email",
        "--subscriber-marketing-opt-in",
        "--user-first",
        "Ada",
        "--user-last",
        "Lovelace",
        "--user-email",
        "ada@example.com",
        "--user-phone",
        "555-0100",
        "--payment-file",
        payment_path.to_str().expect("payment path should be utf-8"),
    ]);

    let request = captured_request(&capture);
    let body: BookingCreateRequestBody = request.json_body();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/bookings");
    assert_eq!(body.unique_user_id, "pilates.user");
    assert_eq!(body.booking_reconciliation.r#type, "PricingOption");
    assert_eq!(body.booking_reconciliation.pricing_option_total, Some(42.5));
    assert_eq!(body.suppress_purchase_receipt_email, Some(true));
    assert_eq!(body.subscriber_marketing_opt_in, Some(true));
    assert_eq!(body.user_first.as_deref(), Some("Ada"));
    assert_eq!(body.user_last.as_deref(), Some("Lovelace"));
    assert_eq!(body.user_email.as_deref(), Some("ada@example.com"));
    assert_eq!(body.user_phone.as_deref(), Some("555-0100"));
    assert_eq!(
        body.payment_details,
        Some(BookingPaymentDetails {
            credit_card_number: "4111111111111111".into(),
            credit_card_expiration_year: 2030,
            credit_card_expiration_month: 12,
            credit_card_cvv: Some("123".into()),
            billing_name: "Ada Lovelace".into(),
            billing_address_line_1: "123 Main St".into(),
            billing_address_line_2: Some("Unit 4".into()),
            billing_city: "Los Angeles".into(),
            billing_state: "CA".into(),
            billing_postal_code: "90001".into(),
        })
    );
    assert_eq!(output.booking.id, "booking-2");
}

#[test]
fn bookings_cancel_uses_user_id_and_query_flag() {
    let capture = Arc::new(Mutex::new(None));
    let server = TestServer::spawn(
        json!({
            "message": "Cancellation successful.",
            "pass": {
                "id": "d1e9b2be-655a-4419-b7dd-30aa84f7aa70"
            }
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("mindbody-booking-cancel");
    let config_path = temp_dir.join("config.json");
    write_config(
        &config_path,
        json!({
            "base_url": server.base_url(),
            "api_key": "api-key",
            "client_key": "client-key",
            "client_secret": "client-secret",
            "user_id": "pilates.user"
        }),
    );

    let output: CancelBookingResponse = run_command(&[
        "mindbody",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "bookings",
        "cancel",
        "booking-123",
        "--suppress-cancellation-confirmation-email",
    ]);

    let request = captured_request(&capture);
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.path, "/users/pilates.user/bookings/booking-123");
    assert_eq!(
        request.query_value("suppressCancellationConfirmationEmail"),
        Some("true")
    );
    assert_eq!(output.message, "Cancellation successful.");
}

#[test]
fn purchases_list_uses_filters_and_user_scope() {
    let capture = Arc::new(Mutex::new(None));
    let server = TestServer::spawn(
        json!({
            "items": [
                {
                    "id": "purchase-1"
                }
            ],
            "offset": 0,
            "maxResults": 1,
            "totalResults": 1
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("mindbody-purchases");
    let config_path = temp_dir.join("config.json");
    write_config(
        &config_path,
        json!({
            "base_url": server.base_url(),
            "api_key": "api-key",
            "client_key": "client-key",
            "client_secret": "client-secret",
            "user_id": "pilates.user"
        }),
    );

    let output: PurchasesListResponse = run_command(&[
        "mindbody",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "purchases",
        "list",
        "--location-id",
        "86784",
        "--subscriber-id",
        "42",
        "--from-purchase-date-time",
        "2026-03-01T00:00:00Z",
        "--to-purchase-date-time",
        "2026-03-31T23:59:59Z",
        "--max-results",
        "10",
        "--offset",
        "5",
        "--order-by",
        "purchaseDateTime",
        "--order",
        "desc",
    ]);

    let request = captured_request(&capture);
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/users/pilates.user/purchases");
    assert_eq!(request.query_value("locationId"), Some("86784"));
    assert_eq!(request.query_value("subscriberId"), Some("42"));
    assert_eq!(
        request.query_value("fromPurchaseDateTime"),
        Some("2026-03-01T00:00:00Z")
    );
    assert_eq!(request.query_value("toPurchaseDateTime"), Some("2026-03-31T23:59:59Z"));
    assert_eq!(request.query_value("maxResults"), Some("10"));
    assert_eq!(request.query_value("offset"), Some("5"));
    assert_eq!(request.query_value("orderBy"), Some("purchaseDateTime"));
    assert_eq!(request.query_value("order"), Some("desc"));
    assert_eq!(output.items[0].id, "purchase-1");
}

#[test]
fn liability_waiver_sign_reads_png_and_posts_base64() {
    let capture = Arc::new(Mutex::new(None));
    let server = TestServer::spawn(
        json!({
            "status": "ok"
        })
        .to_string(),
        200,
        Some(capture.clone()),
    );
    let temp_dir = temp_dir("mindbody-liability-waiver");
    let config_path = temp_dir.join("config.json");
    let signature_path = temp_dir.join("signature.png");
    fs::write(&signature_path, [137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 0]).expect("signature png should write");
    write_config(
        &config_path,
        json!({
            "base_url": server.base_url(),
            "api_key": "api-key",
            "client_key": "client-key",
            "client_secret": "client-secret"
        }),
    );

    let output: OkStatusResponse = run_command(&[
        "mindbody",
        "--config",
        config_path.to_str().expect("config path should be utf-8"),
        "--compact",
        "liability-waivers",
        "sign",
        "--booking-id",
        "f5405d87-46a0-4b48-a384-e26159e130d6",
        "--liability-waiver-hashed-text",
        "21A951F270098C71D8C127EC57B31FDA811B37E64ABF28DD1906F84159C73D64",
        "--signature-png-file",
        signature_path.to_str().expect("signature path should be utf-8"),
    ]);

    let request = captured_request(&capture);
    let body: LiabilityWaiverSignRequestBody = request.json_body();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/signedliabilitywaivers");
    assert_eq!(body.booking_id, "f5405d87-46a0-4b48-a384-e26159e130d6");
    assert_eq!(
        body.liability_waiver_hashed_text,
        "21A951F270098C71D8C127EC57B31FDA811B37E64ABF28DD1906F84159C73D64"
    );
    assert_eq!(body.png_base64_user_signature_picture, "iVBORw0KGgoAAAAA");
    assert_eq!(output.status, "ok");
}

#[derive(Debug, Deserialize)]
struct AccountStatusResponse {
    status: String,
    provider: String,
    user_id: String,
    has_api_key: bool,
    has_client_key: bool,
    has_client_secret: bool,
}

#[derive(Debug, Deserialize)]
struct JsonErrorResponse {
    kind: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct LocationsSearchResponse {
    items: Vec<NumericIdItem>,
}

#[derive(Debug, Deserialize)]
struct NumericIdItem {
    id: u64,
}

#[derive(Debug, Deserialize)]
struct BookingCreateResponse {
    booking: StringIdItem,
}

#[derive(Debug, Deserialize)]
struct StringIdItem {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookingCreateRequestBody {
    unique_user_id: String,
    booking_reconciliation: BookingReconciliation,
    payment_details: Option<BookingPaymentDetails>,
    suppress_purchase_receipt_email: Option<bool>,
    subscriber_marketing_opt_in: Option<bool>,
    user_first: Option<String>,
    user_last: Option<String>,
    user_email: Option<String>,
    user_phone: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BookingReconciliation {
    r#type: String,
    pricing_option_total: Option<f64>,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct BookingPaymentDetails {
    credit_card_number: String,
    credit_card_expiration_year: u16,
    credit_card_expiration_month: u8,
    credit_card_cvv: Option<String>,
    billing_name: String,
    billing_address_line_1: String,
    billing_address_line_2: Option<String>,
    billing_city: String,
    billing_state: String,
    billing_postal_code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiabilityWaiverSignRequestBody {
    booking_id: String,
    liability_waiver_hashed_text: String,
    png_base64_user_signature_picture: String,
}

#[derive(Debug, Deserialize)]
struct CancelBookingResponse {
    message: String,
}

#[derive(Debug, Deserialize)]
struct PurchasesListResponse {
    items: Vec<StringIdItem>,
}

#[derive(Debug, Deserialize)]
struct OkStatusResponse {
    status: String,
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
    serde_json::from_str(&render_cli_error(&error, compact)).expect("error should render as json")
}

fn write_config(path: &PathBuf, value: Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("config dir should exist");
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(&value).expect("config json should serialize"),
    )
    .expect("config should write");
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

    fn query_value(&self, name: &str) -> Option<&str> {
        self.query.get(name)?.last().map(String::as_str)
    }

    fn json_body<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).expect("request body should deserialize")
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
                    let mut parts = line.splitn(2, ':');
                    let name = parts.next()?.trim();
                    if !name.eq_ignore_ascii_case("content-length") {
                        return None;
                    }
                    parts.next()?.trim().parse::<usize>().ok()
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
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("current time should be after epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{nanos:x}"));
    fs::create_dir_all(&path).expect("temp dir should create");
    path
}
