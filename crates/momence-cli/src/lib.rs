mod client;
mod commands;
mod state;

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use anyhow::{Context, Result as AnyhowResult};
use clap::{Args, Parser, Subcommand};
use reqwest::Method;
use serde::Serialize;
use serde_json::Value;

pub(crate) use crate::{client::MomenceClient, state::ResolvedContext};
use crate::{
    client::RequestBody,
    commands::{run_auth, run_member, AuthCommand, MemberCommand},
    state::{
        ENV_MOMENCE_ACCESS_TOKEN, ENV_MOMENCE_BASE_URL, ENV_MOMENCE_CLIENT_ID, ENV_MOMENCE_CLIENT_SECRET,
        ENV_MOMENCE_CONFIG, ENV_MOMENCE_REFRESH_TOKEN,
    },
};

const AFTER_HELP: &str = concat!(
    "Examples:\n",
    "  momence auth login-password --client-id <id> --client-secret <secret> \\\n",
    "    --username you@example.com --password 'super-secret'\n",
    "  momence member sessions list --start-after 2026-03-01T00:00:00Z\n",
    "  momence member host sessions --type fitness --sort-by startsAt\n",
    "  momence member addresses create --body '{\"address\":\"123 Main St\",\"city\":\"LA\",\"country\":\"US\",\"zipcode\":\"90001\"}'\n",
    "  momence member checkout compatible-memberships --body-file cart.json\n",
    "\n",
    "This CLI is aimed at Momence member workflows, booking Pilates classes and the surrounding account-management chaos.\n",
    "Use --body or --body-file for endpoints that accept JSON request payloads.\n",
);

/// Run the Momence CLI and return a process exit code.
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
    let mut context = ResolvedContext::from_global(&cli.global).context("failed to resolve Momence runtime context")?;
    let client = MomenceClient::new(context.base_url.clone()).context("failed to build Momence client")?;

    let output = match cli.command {
        Commands::Auth(command) => run_auth(command.command, &client, &mut context),
        Commands::Member(command) => run_member(command.command, &client, &mut context),
    }
    .context("Momence command failed")?;

    Ok((output, compact))
}

#[derive(Debug, Parser)]
#[command(
    name = "momence",
    version,
    about = "CLI for booking Pilates classes and handling Momence member account workflows",
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
    #[arg(long, global = true, env = ENV_MOMENCE_CONFIG, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, global = true, env = ENV_MOMENCE_BASE_URL, value_name = "URL")]
    base_url: Option<String>,

    #[arg(long, global = true, env = ENV_MOMENCE_CLIENT_ID, value_name = "CLIENT_ID")]
    client_id: Option<String>,

    #[arg(long, global = true, env = ENV_MOMENCE_CLIENT_SECRET, value_name = "CLIENT_SECRET")]
    client_secret: Option<String>,

    #[arg(long, global = true, env = ENV_MOMENCE_ACCESS_TOKEN, value_name = "TOKEN")]
    access_token: Option<String>,

    #[arg(long, global = true, env = ENV_MOMENCE_REFRESH_TOKEN, value_name = "TOKEN")]
    refresh_token: Option<String>,

    #[arg(long, global = true)]
    compact: bool,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Auth(AuthCommand),
    Member(MemberCommand),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("invalid arguments: {0}")]
    Arguments(String),
    #[error("Momence API returned HTTP {status_code}")]
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
        match self {
            Self::Arguments(message) => render_json(
                &MessageErrorResponse {
                    status: "error",
                    kind: "arguments",
                    message,
                },
                compact,
            ),
            Self::Api { status_code, body } => render_json(
                &ApiErrorResponse {
                    status: "error",
                    kind: "api",
                    status_code: *status_code,
                    body,
                },
                compact,
            ),
            Self::Config(message) => render_json(
                &MessageErrorResponse {
                    status: "error",
                    kind: "config",
                    message,
                },
                compact,
            ),
            Self::Http(message) => render_json(
                &MessageErrorResponse {
                    status: "error",
                    kind: "http",
                    message,
                },
                compact,
            ),
            Self::Io(message) => render_json(
                &MessageErrorResponse {
                    status: "error",
                    kind: "io",
                    message,
                },
                compact,
            ),
        }
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

fn render_cli_error(error: &anyhow::Error, compact: bool) -> String {
    if let Some(error) = error.chain().find_map(|cause| cause.downcast_ref::<Error>()) {
        return error.render(compact);
    }

    render_json(
        &OwnedMessageErrorResponse {
            status: "error",
            kind: "internal",
            message: format!("{error:#}"),
        },
        compact,
    )
}

pub(crate) fn execute_bearer(
    client: &MomenceClient,
    token: String,
    method: Method,
    path: &str,
    query: Vec<(String, String)>,
    body: Option<Value>,
) -> Result<Value> {
    client.execute(crate::client::RequestSpec {
        method,
        path: path.into(),
        query,
        body: match body {
            Some(body) => RequestBody::Json(body),
            None => RequestBody::None,
        },
        auth: crate::client::AuthMode::Bearer(token),
    })
}

pub(crate) fn execute_bearer_json(
    client: &MomenceClient,
    token: String,
    method: Method,
    path: &str,
    query: Vec<(String, String)>,
    body: Value,
) -> Result<Value> {
    execute_bearer(client, token, method, path, query, Some(body))
}

fn render_json<T: Serialize>(value: &T, compact: bool) -> String {
    let serialized = if compact {
        serde_json::to_string(value)
    } else {
        serde_json::to_string_pretty(value)
    };

    match serialized {
        Ok(serialized) => serialized,
        Err(error) => render_serialization_error(error),
    }
}

#[derive(Serialize)]
struct MessageErrorResponse<'a> {
    status: &'static str,
    kind: &'static str,
    message: &'a str,
}

#[derive(Serialize)]
struct OwnedMessageErrorResponse {
    status: &'static str,
    kind: &'static str,
    message: String,
}

#[derive(Serialize)]
struct ApiErrorResponse<'a> {
    status: &'static str,
    kind: &'static str,
    status_code: u16,
    body: &'a Value,
}

fn render_serialization_error(error: serde_json::Error) -> String {
    serde_json::to_string(&OwnedMessageErrorResponse {
        status: "error",
        kind: "serialization",
        message: error.to_string(),
    })
    .unwrap_or_else(|_| {
        "{\"status\":\"error\",\"kind\":\"serialization\",\"message\":\"failed to serialize error payload\"}".to_owned()
    })
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
        let capture = Arc::new(Mutex::new(String::new()));
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

        let request = capture.lock().expect("capture lock should work").clone();
        assert!(request.starts_with("POST /api/v2/auth/token"));
        assert!(request.contains("authorization: Basic"));
        assert!(request.contains("grant_type=password"));
        assert!(request.contains("username=member%40example.com"));
        assert!(request.contains("password=super-secret"));

        let state = StateStore::new(config_path).load().expect("stored state should load");
        assert_eq!(state.access_token.as_deref(), Some("access-token"));
        assert_eq!(state.refresh_token.as_deref(), Some("refresh-token"));
        assert_eq!(output.access_token, "access-token");
    }

    #[test]
    fn member_sessions_list_sends_bearer_token_and_query() {
        let capture = Arc::new(Mutex::new(String::new()));
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

        let request = capture.lock().expect("capture lock should work").clone();
        assert!(
            request.starts_with("GET /api/v2/member/sessions?page=0&pageSize=100&startAfter=2026-03-01T00%3A00%3A00Z")
        );
        assert!(request.contains("authorization: Bearer stored-access-token"));
        assert_eq!(output.payload.len(), 1);
        assert_eq!(output.payload[0].id, 1);
    }

    #[test]
    fn cancel_booking_prints_empty_success_payload() {
        let capture = Arc::new(Mutex::new(String::new()));
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

        let request = capture.lock().expect("capture lock should work").clone();
        assert!(request.starts_with("DELETE /api/v2/member/sessions/77"));
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

        String::from_utf8_lossy(&buffer).replace('\r', "")
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
}
