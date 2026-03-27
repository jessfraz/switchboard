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
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

use crate::{args::GlobalArgs, run, state::ResolvedContext, Cli};

#[derive(Debug, Deserialize)]
pub(super) struct MessageErrorOutput {
    pub(super) status: String,
    pub(super) kind: String,
    pub(super) message: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct AuthorizeUrlOutput {
    pub(super) status: String,
    pub(super) opened_browser: bool,
    pub(super) authorize_url: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct AuthenticatedOutput {
    pub(super) status: String,
    pub(super) patient_id: Option<String>,
    pub(super) dynamic_client_id: Option<String>,
    pub(super) renewal_method: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AuthorizationPendingOutput {
    pub(super) status: String,
    pub(super) opened_browser: bool,
    pub(super) next_step: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct StatusOutput {
    pub(super) status: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ApiResourcesOutput {
    pub(super) resource_count: usize,
    pub(super) resources: Vec<ApiResourceSummary>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ApiResourceSummary {
    pub(super) resource: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ApiGetOutput {
    pub(super) status: String,
    pub(super) resource: String,
    pub(super) body: ApiResourceBody,
}

#[derive(Debug, Deserialize)]
pub(super) struct ApiResourceBody {
    pub(super) id: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct LabsOutput {
    pub(super) status: String,
    pub(super) series: Vec<LabSeries>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LabSeries {
    pub(super) label: String,
    pub(super) point_count: usize,
    pub(super) spark: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AppointmentsOutput {
    pub(super) status: String,
    #[serde(default)]
    pub(super) query: Option<String>,
    pub(super) appointments: Vec<AppointmentSummary>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AppointmentSummary {
    pub(super) id: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct NoteGetOutput {
    pub(super) status: String,
    pub(super) note: NoteDetails,
}

#[derive(Debug, Deserialize)]
pub(super) struct NoteDetails {
    pub(super) body_text: Option<String>,
    pub(super) content: Vec<NoteAttachment>,
}

#[derive(Debug, Deserialize)]
pub(super) struct NoteAttachment {
    pub(super) body_text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ClaimsAuditOutput {
    pub(super) status: String,
    pub(super) duplicate_charge_candidates: Vec<Value>,
    pub(super) denied_or_problematic_claims: Vec<Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PackDoctorOutput {
    pub(super) status: String,
    pub(super) upcoming_appointment: PackAppointment,
    pub(super) recent_labs: Vec<PackLab>,
    pub(super) active_medications: Vec<PackMedication>,
    pub(super) active_conditions: Vec<PackCondition>,
    pub(super) suggested_questions: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PackAppointment {
    pub(super) description: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct PackLab {
    pub(super) label: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct PackMedication {
    pub(super) name: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct PackCondition {
    pub(super) condition: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct DynamicClientRegistrationBody {
    pub(super) software_id: String,
    pub(super) jwks: JsonWebKeySet,
}

#[derive(Debug, Deserialize)]
pub(super) struct JsonWebKeySet {
    pub(super) keys: Vec<JsonWebKey>,
}

#[derive(Debug, Deserialize)]
pub(super) struct JsonWebKey {
    pub(super) kty: String,
}

pub(super) fn run_command(args: &[&str]) -> Value {
    let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
    let compact = cli.global.compact;
    let (value, _) = run(cli).unwrap_or_else(|error| panic!("{}", crate::output::render_cli_error(&error, compact)));
    value
}

pub(super) fn run_command_json<T>(args: &[&str]) -> T
where
    T: DeserializeOwned,
{
    serde_json::from_value(run_command(args)).expect("command output should deserialize")
}

pub(super) fn run_command_error(args: &[&str]) -> String {
    let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
    let compact = cli.global.compact;
    let error = run(cli).expect_err("CLI should fail");
    crate::output::render_cli_error(&error, compact)
}

pub(super) fn run_command_error_json<T>(args: &[&str]) -> T
where
    T: DeserializeOwned,
{
    serde_json::from_str(&run_command_error(args)).expect("command error should deserialize")
}

pub(super) fn resolved_context(config_path: &std::path::Path) -> crate::state::ResolvedContext {
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
        debug_auth: false,
        compact: true,
    })
    .expect("context should resolve")
}

pub(super) fn temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should move forward")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{nanos:x}"));
    fs::create_dir_all(&path).expect("temp dir should exist");
    path
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
}

pub(super) fn wait_for_callback_response(port: u16, request: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(mut stream) => {
                stream
                    .write_all(request.as_bytes())
                    .expect("callback request should write");
                return read_response_text(&mut stream);
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

pub(super) fn write_brands_cache(path: &std::path::Path, value: Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("brands cache parent should exist");
    }
    fs::write(
        path,
        serde_json::to_vec_pretty(&value).expect("brands cache should serialize"),
    )
    .expect("brands cache should write");
}

pub(super) fn capability_statement_json(base_url: &str, resources: &[Value]) -> Value {
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
                        {"url": "token", "valueUri": format!("{base_url}/oauth2/token")},
                        {"url": "register", "valueUri": format!("{base_url}/oauth2/register")}
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

pub(super) fn resource_capability(resource_type: &str, interactions: &[&str]) -> Value {
    json!({
        "type": resource_type,
        "interaction": interactions.iter().map(|interaction| json!({ "code": interaction })).collect::<Vec<_>>(),
        "searchParam": [{
            "name": "patient",
            "type": "reference"
        }]
    })
}

pub(super) fn login_page_html(token: &str) -> String {
    format!(
        "<html><head><title>Generic MyChart - Login Page</title></head><body>\
             <form id=\"loginForm\"></form>\
             <form class=\"hidden\" action=\"/Authentication/Login/DoLogin\">\
             <input name=\"__RequestVerificationToken\" type=\"hidden\" value=\"{token}\" />\
             </form>\
             </body></html>"
    )
}

pub(super) fn app_page_html(title: &str) -> String {
    format!(
        "<html><head><title>{title}</title></head><body>\
             <div id=\"app\">hello from {title}</div>\
             <input name=\"__RequestVerificationToken\" type=\"hidden\" value=\"page-token\" />\
             </body></html>"
    )
}

#[derive(Clone, Debug)]
pub(super) struct CapturedRequest {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) query: BTreeMap<String, Vec<String>>,
    pub(super) headers: BTreeMap<String, String>,
    pub(super) body: Vec<u8>,
}

impl CapturedRequest {
    pub(super) fn parse(buffer: &[u8]) -> Self {
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

    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    pub(super) fn query_value(&self, name: &str) -> Option<&str> {
        self.query.get(name)?.last().map(String::as_str)
    }

    pub(super) fn form_value(&self, name: &str) -> Option<String> {
        let form = parse_www_form(&String::from_utf8_lossy(&self.body));
        form.get(name).and_then(|values| values.last()).cloned()
    }

    pub(super) fn json_body<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).expect("request body should deserialize")
    }
}

#[derive(Clone)]
pub(super) struct ResponseSpec {
    status_code: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl ResponseSpec {
    pub(super) fn html(status_code: u16, body: String, headers: Vec<(String, String)>) -> Self {
        let mut headers = headers;
        headers.push(("Content-Type".into(), "text/html; charset=utf-8".into()));
        Self {
            status_code,
            headers,
            body,
        }
    }

    pub(super) fn json(status_code: u16, body: Value, headers: Vec<(String, String)>) -> Self {
        let mut headers = headers;
        headers.push(("Content-Type".into(), "application/fhir+json".into()));
        Self {
            status_code,
            headers,
            body: serde_json::to_string(&body).expect("body should serialize"),
        }
    }

    pub(super) fn empty(status_code: u16, headers: Vec<(String, String)>) -> Self {
        Self {
            status_code,
            headers,
            body: String::new(),
        }
    }
}

pub(super) struct TestServer {
    address: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    _handle: thread::JoinHandle<()>,
}

impl TestServer {
    pub(super) fn spawn(responses: Vec<ResponseSpec>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
        let address = listener.local_addr().expect("listener should have local addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let requests_clone = requests.clone();

        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("server should accept request");
                let request = read_captured_request(&mut stream);
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

    pub(super) fn base_url(&self) -> String {
        self.address.clone()
    }

    pub(super) fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().expect("requests lock").clone()
    }
}

fn read_response_text(stream: &mut std::net::TcpStream) -> String {
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

fn read_captured_request(stream: &mut std::net::TcpStream) -> CapturedRequest {
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
        let key = url_decode_component(raw_key);
        let value = url_decode_component(raw_value);
        values.entry(key).or_insert_with(Vec::new).push(value);
    }
    values
}

fn url_decode_component(input: &str) -> String {
    let mut decoded = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = decode_hex(bytes[index + 1]);
                let low = decode_hex(bytes[index + 2]);
                decoded.push((high << 4) | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).expect("decoded form component should be utf-8")
}

fn decode_hex(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => panic!("invalid hex byte {byte:?}"),
    }
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
