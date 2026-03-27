mod client;
mod commands;
mod state;

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result as AnyhowResult};
use clap::{Args, Parser, Subcommand};
use reqwest::Method;
use serde_json::{json, Value};

pub(crate) use crate::{client::MindbodyClient, state::ResolvedContext};
use crate::{
    client::{Credentials, RequestBody, RequestSpec},
    commands::{
        run_account, run_bookings, run_classes, run_liability_waivers, run_locations, run_passes, run_pricing,
        run_purchases, AccountCommand, BookingCommand, ClassCommand, LiabilityWaiverCommand, LocationCommand,
        PassCommand, PricingCommand, PurchaseCommand,
    },
    state::{
        ENV_MINDBODY_API_KEY, ENV_MINDBODY_APP_NAME, ENV_MINDBODY_BASE_URL, ENV_MINDBODY_CLIENT_KEY,
        ENV_MINDBODY_CLIENT_SECRET, ENV_MINDBODY_CONFIG, ENV_MINDBODY_USER_ID,
    },
};

const AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  mindbody locations search --search-text pilates --address '90210'\n",
    "  mindbody classes list --location-id 86784 --available-for-booking true --start-date-time 2026-03-27T00:00:00Z\n",
    "  mindbody pricing class --location-id 86784 5134512\n",
    "  mindbody bookings create --location-id 86784 --class-id 5134512 \\\n",
    "    --reconciliation-type pass --reconciliation-id 598a6916-7876-406e-9537-db6af825f9a2\n",
    "  mindbody purchases list --location-id 86784 --from-purchase-date-time 2026-03-01T00:00:00Z\n",
    "  mindbody liability-waivers sign --booking-id f5405d87-46a0-4b48-a384-e26159e130d6 \\\n",
    "    --liability-waiver-hashed-text <hash> --signature-png-file signature.png\n",
    "\n",
    "This CLI is aimed at booking Pilates classes and the surrounding Mindbody member account chaos,\n",
    "with a switchboard-friendly command grammar instead of raw endpoint confetti.\n",
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
            eprintln!("{}", render_cli_error(&error, compact));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> AnyhowResult<(Value, bool)> {
    let compact = cli.global.compact;
    let context = ResolvedContext::from_global(&cli.global).context("failed to resolve Mindbody runtime context")?;
    let client = MindbodyClient::new(context.base_url.clone(), context.app_name.clone())
        .context("failed to build Mindbody client")?;

    let output = match cli.command {
        Commands::Account(command) => run_account(command.command, &context),
        Commands::Locations(command) => run_locations(command.command, &client, &context),
        Commands::Classes(command) => run_classes(command.command, &client, &context),
        Commands::Pricing(command) => run_pricing(command.command, &client, &context),
        Commands::Bookings(command) => run_bookings(command.command, &client, &context),
        Commands::Passes(command) => run_passes(command.command, &client, &context),
        Commands::Purchases(command) => run_purchases(command.command, &client, &context),
        Commands::LiabilityWaivers(command) => run_liability_waivers(command.command, &client, &context),
    }
    .context("Mindbody command failed")?;

    Ok((output, compact))
}

#[derive(Debug, Parser)]
#[command(
    name = "mindbody",
    version,
    about = "CLI for booking Pilates classes and handling Mindbody member account workflows",
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
    #[arg(long, global = true, env = ENV_MINDBODY_CONFIG, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, global = true, env = ENV_MINDBODY_BASE_URL, value_name = "URL")]
    base_url: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_API_KEY, value_name = "API_KEY")]
    api_key: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_CLIENT_KEY, value_name = "CLIENT_KEY")]
    client_key: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_CLIENT_SECRET, value_name = "CLIENT_SECRET")]
    client_secret: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_USER_ID, value_name = "USER_ID")]
    user_id: Option<String>,

    #[arg(long, global = true, env = ENV_MINDBODY_APP_NAME, value_name = "NAME")]
    app_name: Option<String>,

    #[arg(long, global = true)]
    compact: bool,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Account(AccountCommand),
    Locations(LocationCommand),
    Classes(ClassCommand),
    Pricing(PricingCommand),
    Bookings(BookingCommand),
    Passes(PassCommand),
    Purchases(PurchaseCommand),
    LiabilityWaivers(LiabilityWaiverCommand),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("invalid arguments: {0}")]
    Arguments(String),
    #[error("Mindbody API returned HTTP {status_code}")]
    Api { status_code: u16, body: Value },
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
            Self::Arguments(message) => json!({
                "status": "error",
                "kind": "arguments",
                "message": message,
            }),
            Self::Api { status_code, body } => json!({
                "status": "error",
                "kind": "api",
                "status_code": status_code,
                "body": body,
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

pub(crate) type Result<T> = std::result::Result<T, Error>;

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

pub(crate) fn execute(
    client: &MindbodyClient,
    context: &ResolvedContext,
    method: Method,
    path: &str,
    query: Vec<(String, String)>,
) -> Result<Value> {
    let (api_key, client_key, client_secret) = context.require_credentials()?;
    client.execute(
        Credentials {
            api_key,
            client_key,
            client_secret,
        },
        RequestSpec {
            method,
            path: path.into(),
            query,
            body: RequestBody::None,
            idempotency_key: None,
        },
    )
}

pub(crate) fn execute_json(
    client: &MindbodyClient,
    context: &ResolvedContext,
    method: Method,
    path: &str,
    query: Vec<(String, String)>,
    body: Value,
    idempotency_key: Option<String>,
) -> Result<Value> {
    let (api_key, client_key, client_secret) = context.require_credentials()?;
    client.execute(
        Credentials {
            api_key,
            client_key,
            client_secret,
        },
        RequestSpec {
            method,
            path: path.into(),
            query,
            body: RequestBody::Json(body),
            idempotency_key,
        },
    )
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
        process::ExitCode,
        sync::{Arc, Mutex},
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use clap::Parser;
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

        let output = run_command(&[
            "mindbody",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "account",
            "status",
        ]);

        assert_eq!(output["status"], "ok");
        assert_eq!(output["provider"], "mindbody");
        assert_eq!(output["user_id"], "pilates.user");
        assert_eq!(output["has_api_key"], true);
        assert_eq!(output["has_client_key"], true);
        assert_eq!(output["has_client_secret"], false);
    }

    #[test]
    fn locations_search_sends_headers_and_query() {
        let capture = Arc::new(Mutex::new(String::new()));
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

        let output = run_command(&[
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

        let request = capture.lock().expect("capture lock should work").clone().to_lowercase();
        assert!(request.starts_with("get /locations?"));
        assert!(request.contains("searchtext=pilates"));
        assert!(request.contains("countrycode=us"));
        assert!(request.contains("maxresults=5"));
        assert!(request.contains("api-key: api-key"));
        assert!(request.contains("authorization: basic"));
        assert_eq!(output["items"][0]["id"], 86784);
    }

    #[test]
    fn bookings_create_with_pass_builds_typed_body() {
        let capture = Arc::new(Mutex::new(String::new()));
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

        let output = run_command(&[
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

        let request = capture.lock().expect("capture lock should work").clone();
        let request_lower = request.to_lowercase();
        assert!(request.starts_with("POST /bookings"));
        assert!(request_lower.contains("idempotency-key: 123e4567-e89b-12d3-a456-426614174000"));
        assert!(request.contains("\"uniqueUserId\":\"pilates.user\""));
        assert!(request.contains("\"type\":\"Pass\""));
        assert!(request.contains("\"paymentDetails\":null"));
        assert_eq!(output["booking"]["id"], "booking-1");
    }

    #[test]
    fn pricing_option_booking_requires_purchase_fields() {
        let error = run_command_error(&[
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

        assert_eq!(error["kind"], "arguments");
        assert!(error["message"]
            .as_str()
            .expect("message should be present")
            .contains("--pricing-option-total"));
    }

    #[test]
    fn bookings_cancel_uses_user_id_and_query_flag() {
        let capture = Arc::new(Mutex::new(String::new()));
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

        let output = run_command(&[
            "mindbody",
            "--config",
            config_path.to_str().expect("config path should be utf-8"),
            "--compact",
            "bookings",
            "cancel",
            "booking-123",
            "--suppress-cancellation-confirmation-email",
        ]);

        let request = capture.lock().expect("capture lock should work").clone();
        assert!(request
            .starts_with("DELETE /users/pilates.user/bookings/booking-123?suppressCancellationConfirmationEmail=true"));
        assert_eq!(output["message"], "Cancellation successful.");
    }

    #[test]
    fn purchases_list_uses_filters_and_user_scope() {
        let capture = Arc::new(Mutex::new(String::new()));
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

        let output = run_command(&[
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

        let request = capture.lock().expect("capture lock should work").clone();
        assert!(request.starts_with("GET /users/pilates.user/purchases?"));
        assert!(request.contains("locationId=86784"));
        assert!(request.contains("subscriberId=42"));
        assert!(request.contains("fromPurchaseDateTime=2026-03-01T00%3A00%3A00Z"));
        assert!(request.contains("toPurchaseDateTime=2026-03-31T23%3A59%3A59Z"));
        assert!(request.contains("maxResults=10"));
        assert!(request.contains("offset=5"));
        assert!(request.contains("orderBy=purchaseDateTime"));
        assert!(request.contains("order=desc"));
        assert_eq!(output["items"][0]["id"], "purchase-1");
    }

    #[test]
    fn liability_waiver_sign_reads_png_and_posts_base64() {
        let capture = Arc::new(Mutex::new(String::new()));
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

        let output = run_command(&[
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

        let request = capture.lock().expect("capture lock should work").clone();
        assert!(request.starts_with("POST /signedliabilitywaivers"));
        assert!(request.contains("\"bookingId\":\"f5405d87-46a0-4b48-a384-e26159e130d6\""));
        assert!(request.contains(
            "\"liabilityWaiverHashedText\":\"21A951F270098C71D8C127EC57B31FDA811B37E64ABF28DD1906F84159C73D64\""
        ));
        assert!(request.contains("\"pngBase64UserSignaturePicture\":\"iVBORw0KGgoAAAAA\""));
        assert_eq!(output["status"], "ok");
    }

    fn run_command(args: &[&str]) -> Value {
        let cli = Cli::try_parse_from(args.iter().map(OsString::from)).expect("CLI should parse");
        let compact = cli.global.compact;
        let (value, _) = run(cli).unwrap_or_else(|error| panic!("{}", render_cli_error(&error, compact)));
        value
    }

    fn run_command_error(args: &[&str]) -> Value {
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
        fn spawn(body: String, status_code: u16, capture: Option<Arc<Mutex<String>>>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
            let address = listener.local_addr().expect("local addr should exist");

            let handle = thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let request = read_request(&mut stream);
                    if let Some(capture) = capture {
                        if let Ok(mut guard) = capture.lock() {
                            *guard = request;
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

        String::from_utf8_lossy(&buffer).into_owned()
    }

    fn find_headers_end(buffer: &[u8]) -> Option<usize> {
        buffer.windows(4).position(|window| window == b"\r\n\r\n")
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
}
