//! Keyless, fixed-subject Workspace delegation. Tokens and credential exports
//! never pass through ToolOutput, operation arguments, or the audit decoder.
use std::{
    io::Read,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{
    blocking::{Client, RequestBuilder},
    redirect::Policy,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use switchboard_core::{Error, GoogleDelegation, Result, SecretString};

use crate::process_runtime::ProcessContext;

pub const ADMIN_URL: &str = "https://admin.google.com/ac/owl/domainwidedelegation";
const TOKEN_CREATOR: &str = "roles/iam.serviceAccountTokenCreator";
const DESCRIPTION: &str = "Managed by Switchboard for fixed-subject Gmail settings delegation";
const MAX_RESPONSE: u64 = 1024 * 1024;

pub fn uses_delegation(argv: &[String]) -> bool {
    argv.len() >= 4
        && argv[..3] == ["gmail", "users", "settings"]
        && !argv
            .iter()
            .any(|arg| matches!(arg.as_str(), "--help" | "-h" | "--dry-run"))
}

pub fn requires_delegation(argv: &[String]) -> bool {
    uses_delegation(argv)
        && argv.get(3).is_some_and(|arg| arg == "sendAs")
        && argv
            .get(4)
            .is_some_and(|arg| matches!(arg.as_str(), "create" | "delete" | "verify"))
}

pub fn validate_subject(argv: &[String], subject: &str) -> Result<()> {
    #[derive(Deserialize)]
    struct Params {
        #[serde(rename = "userId")]
        user_id: Option<String>,
    }
    for (index, arg) in argv.iter().enumerate() {
        let value = arg.strip_prefix("--params=").or_else(|| {
            (arg == "--params")
                .then(|| argv.get(index + 1).map(String::as_str))
                .flatten()
        });
        if let Some(value) = value {
            let params: Params =
                serde_json::from_str(value).map_err(|_| Error::InvalidArguments("invalid Gmail parameters".into()))?;
            if params
                .user_id
                .as_deref()
                .is_some_and(|user| user != "me" && !user.eq_ignore_ascii_case(subject))
            {
                return Err(Error::InvalidArguments("delegated Gmail userId must be me or the configured account; changing the delegated subject requires setup".into()));
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct OAuthCredentials {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    #[serde(rename = "type")]
    kind: Option<String>,
}
#[derive(Deserialize)]
struct Token {
    access_token: String,
    expires_in: u64,
}
#[derive(Deserialize)]
struct Identity {
    email: String,
    #[serde(default)]
    verified_email: bool,
    hd: Option<String>,
}
#[derive(Deserialize, Serialize)]
struct Claims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    scope: String,
    iat: u64,
    exp: u64,
}
#[derive(Deserialize, Serialize)]
struct SignRequest<'a> {
    payload: &'a str,
}
#[derive(Deserialize)]
struct SignedJwt {
    #[serde(rename = "signedJwt")]
    signed_jwt: String,
}
#[derive(Deserialize)]
struct SendAsList {
    #[serde(rename = "sendAs")]
    send_as: Vec<SendAs>,
}
#[derive(Deserialize)]
struct SendAs {
    #[serde(rename = "sendAsEmail")]
    email: String,
    #[serde(rename = "isPrimary", default)]
    primary: bool,
}
#[derive(Deserialize)]
struct OAuthFailure {
    error: String,
}

struct Http {
    client: Client,
    token_url: String,
    userinfo_url: String,
    signing_url: String,
    gmail_url: String,
    cloud_url: String,
    iam_url: String,
    usage_url: String,
}
impl Http {
    fn production() -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(Policy::none())
                .build()
                .map_err(|_| Error::Execution("could not initialize Google authentication transport".into()))?,
            token_url: "https://oauth2.googleapis.com/token".into(),
            userinfo_url: "https://www.googleapis.com/oauth2/v2/userinfo".into(),
            signing_url: "https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts".into(),
            gmail_url: "https://gmail.googleapis.com/gmail/v1/users/me/settings/sendAs".into(),
            cloud_url: "https://cloudresourcemanager.googleapis.com/v1/projects".into(),
            iam_url: "https://iam.googleapis.com/v1/projects".into(),
            usage_url: "https://serviceusage.googleapis.com/v1/projects".into(),
        })
    }
    fn response(&self, request: RequestBuilder, stage: &str) -> Result<reqwest::blocking::Response> {
        let budget = switchboard_core::process::remaining_timeout(Duration::from_secs(20))
            .map_err(|_| Error::Execution(format!("{stage}: authentication deadline exhausted")))?;
        request.timeout(budget).send().map_err(|_| {
            Error::Execution(format!(
                "{stage}: Google request failed; no fallback credentials were used"
            ))
        })
    }
    fn send<T: DeserializeOwned>(&self, request: RequestBuilder, stage: &str) -> Result<T> {
        self.decode(self.response(request, stage)?, stage)
    }
    fn decode<T: DeserializeOwned>(&self, response: reqwest::blocking::Response, stage: &str) -> Result<T> {
        let status = response.status();
        let mut body = Vec::new();
        response
            .take(MAX_RESPONSE + 1)
            .read_to_end(&mut body)
            .map_err(|_| Error::Execution(format!("{stage}: could not read Google response")))?;
        if body.len() as u64 > MAX_RESPONSE {
            return Err(Error::Execution(format!("{stage}: oversized Google response")));
        }
        if !status.is_success() {
            if stage == "signer login"
                && (status.as_u16() == 401
                    || serde_json::from_slice::<OAuthFailure>(&body)
                        .ok()
                        .is_some_and(|failure| failure.error == "invalid_grant"))
            {
                return Err(Error::AuthenticationRejected {
                    reason:
                        "configured Google signer session expired or was revoked; use auth login for this namespace"
                            .into(),
                });
            }
            let hint = serde_json::from_slice::<OAuthFailure>(&body).ok().map(|failure| match failure.error.as_str() {
                "unauthorized_client" | "access_denied" => "Workspace authorization is missing or still propagating; confirm the numeric client ID and both scopes at the administrator link",
                "invalid_grant" => "the configured signer session or Workspace subject is unavailable; verify the account before retrying",
                _ => "authorization was refused",
            }).unwrap_or(match status.as_u16() {
                401 => "the configured credential was rejected; log in to that account again",
                403 => "check enabled APIs, signing permission and Workspace authorization",
                404 => "the configured project or service account was not found",
                409 | 412 => "resource changed; resume setup to read current state before writing again",
                429 => "Google rate limited this request; wait before retrying",
                _ => "inspect Google availability and resume; no alternate credentials were used",
            });
            return Err(Error::Execution(format!(
                "{stage}: Google HTTP {}: {hint}",
                status.as_u16()
            )));
        }
        serde_json::from_slice(&body).map_err(|_| Error::Execution(format!("{stage}: invalid Google response")))
    }
    fn source_token(&self, credentials: &OAuthCredentials, expected: &str) -> Result<SecretString> {
        if credentials
            .kind
            .as_deref()
            .is_some_and(|kind| kind != "authorized_user")
        {
            return Err(Error::Config(
                "delegation signer must use its configured user OAuth session, not a downloaded service-account key"
                    .into(),
            ));
        }
        let response: Token = self.send(
            self.client.post(&self.token_url).form(&[
                ("grant_type", "refresh_token"),
                ("client_id", credentials.client_id.as_str()),
                ("client_secret", credentials.client_secret.as_str()),
                ("refresh_token", credentials.refresh_token.as_str()),
            ]),
            "signer login",
        )?;
        let token = checked_token(response)?;
        let identity: Identity = self.send(
            self.client.get(&self.userinfo_url).bearer_auth(token.expose()),
            "signer identity",
        )?;
        if !identity.verified_email || !identity.email.eq_ignore_ascii_case(expected) {
            return Err(Error::AccountMismatch {
                expected: expected.to_owned(),
                actual: identity.email,
            });
        }
        if identity.hd.as_ref().map_or(true, |domain| domain.trim().is_empty()) {
            return Err(Error::UnsupportedOperation("Google did not identify this account as belonging to a hosted Workspace domain. Verified custom-domain consumer accounts cannot use this setup. No Cloud resources or grants were changed.".into()));
        }
        Ok(token)
    }
    fn mint(&self, config: &GoogleDelegation, source: &SecretString) -> Result<SecretString> {
        config.validate()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::Execution("system clock is invalid".into()))?
            .as_secs();
        let payload = serde_json::to_string(&Claims {
            iss: &config.service_account,
            sub: &config.subject,
            aud: "https://oauth2.googleapis.com/token",
            scope: GoogleDelegation::SCOPES.join(" "),
            iat: now,
            exp: now + 3600,
        })
        .map_err(|_| Error::Execution("could not encode delegation claims".into()))?;
        let signed: SignedJwt = self.send(
            self.client
                .post(format!("{}/{}:signJwt", self.signing_url, config.service_account))
                .bearer_auth(source.expose())
                .json(&SignRequest { payload: &payload }),
            "delegated signing",
        )?;
        if signed.signed_jwt.is_empty() {
            return Err(Error::Execution("delegated signing: empty JWT".into()));
        }
        let response: Token = self.send(
            self.client.post(&self.token_url).form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", signed.signed_jwt.as_str()),
            ]),
            "Workspace authorization",
        )?;
        let token = checked_token(response)?;
        self.verify_subject(config, &token)?;
        Ok(token)
    }
    fn verify_subject(&self, config: &GoogleDelegation, token: &SecretString) -> Result<()> {
        let aliases: SendAsList = self.send(
            self.client
                .get(&self.gmail_url)
                .bearer_auth(token.expose())
                .query(&[("fields", "sendAs(sendAsEmail,isPrimary)")]),
            "delegated account identity",
        )?;
        let mut primary = aliases.send_as.iter().filter(|alias| alias.primary);
        let actual = primary
            .next()
            .ok_or_else(|| Error::Execution("delegated account did not report a primary address".into()))?;
        if primary.next().is_some() || !actual.email.eq_ignore_ascii_case(&config.subject) {
            return Err(Error::AccountMismatch {
                expected: config.subject.clone(),
                actual: actual.email.clone(),
            });
        }
        Ok(())
    }
}

fn checked_token(response: Token) -> Result<SecretString> {
    if response.access_token.is_empty()
        || response.expires_in == 0
        || response.expires_in > 3600
        || response
            .access_token
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(Error::Execution("Google returned an invalid short-lived token".into()));
    }
    Ok(response.access_token.into())
}

fn source_credentials(config: &GoogleDelegation) -> Result<OAuthCredentials> {
    config.validate()?;
    let mut runtime = ProcessContext::new();
    runtime.set_env(
        "GOOGLE_WORKSPACE_CLI_CONFIG_DIR",
        config.signer_state_dir.display().to_string(),
    );
    runtime.set_env("GOOGLE_WORKSPACE_CLI_KEYRING_BACKEND", "file");
    for env in [
        "GOOGLE_WORKSPACE_CLI_TOKEN",
        "GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE",
        "GOOGLE_WORKSPACE_CLI_CLIENT_ID",
        "GOOGLE_WORKSPACE_CLI_CLIENT_SECRET",
    ] {
        runtime.clear_env(env);
    }
    runtime.set_env(
        "GOOGLE_APPLICATION_CREDENTIALS",
        config.signer_state_dir.join("credentials.json").display().to_string(),
    );
    let binary = std::env::var_os("SWITCHBOARD_GWS_BIN").unwrap_or_else(|| "gws".into());
    let mut command = Command::new(binary);
    command.args(["auth", "export", "--unmasked"]);
    runtime.apply_to_command(&mut command);
    switchboard_core::process::clear_one_password_environment(&mut command);
    let output =
        switchboard_core::process::output_with_timeout(&mut command, Duration::from_secs(20)).map_err(|_| {
            Error::Execution(
                "could not read the configured Google signer session; log in to that namespace locally".into(),
            )
        })?;
    // Never include raw stdout/stderr in failures. This path deliberately does
    // not use CliOutput's normal user-visible error/response decoding.
    if !output.status.success() || output.stdout.len() > MAX_RESPONSE as usize {
        return Err(Error::AuthenticationRejected { reason: "configured Google signer session is unavailable; use auth login for this namespace. No browser or global credential fallback was attempted".into() });
    }
    serde_json::from_slice(&output.stdout).map_err(|_| Error::AuthenticationRejected {
        reason: "configured Google signer session is invalid; use auth login for this namespace".into(),
    })
}

/// A fresh token is minted for each child invocation. No token is persisted or
/// reused across subjects, namespaces, policy changes or separate CLI runs.
pub fn delegated_token(config: &GoogleDelegation) -> Result<SecretString> {
    let http = Http::production()?;
    let source = http.source_token(&source_credentials(config)?, &config.signer_account)?;
    http.mint(config, &source)
}

pub fn verify(config: &GoogleDelegation) -> Result<()> {
    delegated_token(config).map(|_| ())
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct CloudGrant {
    pub client_id: String,
}
#[derive(Deserialize)]
struct Project {
    #[serde(rename = "projectNumber")]
    number: String,
}
#[derive(Deserialize)]
struct Service {
    state: String,
}
#[derive(Deserialize)]
struct Operation {
    name: String,
    #[serde(default)]
    done: bool,
    error: Option<OperationError>,
}
#[derive(Deserialize)]
struct OperationError {
    code: i64,
}
#[derive(Deserialize, Serialize)]
struct ServiceAccount {
    email: String,
    #[serde(rename = "oauth2ClientId")]
    client_id: String,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    description: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateAccount<'a> {
    account_id: &'a str,
    service_account: AccountDescription,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountDescription {
    display_name: &'static str,
    description: &'static str,
}
#[derive(Clone, Deserialize, Serialize)]
struct IamPolicy {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    bindings: Vec<Binding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
struct Binding {
    role: String,
    members: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    condition: Option<Condition>,
}
#[derive(Clone, Deserialize, Serialize)]
struct Condition {
    expression: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetPolicy<'a> {
    policy: &'a IamPolicy,
    update_mask: &'static str,
}

impl Http {
    fn wait(&self, base: &str, operation: Operation, token: &SecretString) -> Result<()> {
        if operation.name.is_empty()
            || operation.name.contains(['?', '#', '%'])
            || operation.name.split('/').any(|part| part == "..")
        {
            return Err(Error::Execution(
                "Google returned an invalid setup operation name".into(),
            ));
        }
        let mut state = operation;
        for _ in 0..10 {
            if state.done {
                return if let Some(error) = state.error {
                    Err(Error::Execution(format!(
                        "Google setup operation failed (code {}); resume after resolving project permissions",
                        error.code
                    )))
                } else {
                    Ok(())
                };
            }
            std::thread::sleep(Duration::from_millis(500));
            state = self.send(
                self.client
                    .get(format!("{base}/{}", state.name))
                    .bearer_auth(token.expose()),
                "Google setup operation",
            )?;
        }
        Err(Error::Execution(
            "Google is still enabling the API. Setup is resumable; run auth login again with --apply".into(),
        ))
    }
    fn provision(&self, config: &GoogleDelegation, token: &SecretString) -> Result<CloudGrant> {
        config.validate()?;
        let project: Project = self.send(
            self.client
                .get(format!("{}/{}", self.cloud_url, config.project))
                .bearer_auth(token.expose()),
            "Cloud project",
        )?;
        if project.number.is_empty() || !project.number.bytes().all(|c| c.is_ascii_digit()) {
            return Err(Error::Execution(
                "Cloud project returned an invalid project number".into(),
            ));
        }
        for api in [
            "iam.googleapis.com",
            "iamcredentials.googleapis.com",
            "gmail.googleapis.com",
        ] {
            let url = format!("{}/{}/services/{api}", self.usage_url, project.number);
            let service: Service = self.send(
                self.client.get(&url).bearer_auth(token.expose()),
                "API enablement preflight",
            )?;
            if service.state != "ENABLED" {
                let operation = self.send(
                    self.client
                        .post(format!("{url}:enable"))
                        .bearer_auth(token.expose())
                        .json(&Empty {}),
                    "API enablement",
                )?;
                let base = self
                    .usage_url
                    .strip_suffix("/projects")
                    .ok_or_else(|| Error::Config("invalid Service Usage origin".into()))?;
                self.wait(base, operation, token)?;
                let enabled: Service = self.send(
                    self.client.get(&url).bearer_auth(token.expose()),
                    "API enablement readback",
                )?;
                if enabled.state != "ENABLED" {
                    return Err(Error::Execution(
                        "API enablement was not confirmed; resume setup before proceeding".into(),
                    ));
                }
            }
        }
        let account_url = format!(
            "{}/{}/serviceAccounts/{}",
            self.iam_url, config.project, config.service_account
        );
        let response = self.response(
            self.client.get(&account_url).bearer_auth(token.expose()),
            "service-account preflight",
        )?;
        let account: ServiceAccount = if response.status() == reqwest::StatusCode::NOT_FOUND {
            let account_id = config
                .service_account
                .split_once('@')
                .ok_or_else(|| Error::Config("invalid service-account email".into()))?
                .0;
            let _: ServiceAccount = self.send(
                self.client
                    .post(format!("{}/{}/serviceAccounts", self.iam_url, config.project))
                    .bearer_auth(token.expose())
                    .json(&CreateAccount {
                        account_id,
                        service_account: AccountDescription {
                            display_name: "Switchboard Gmail settings",
                            description: DESCRIPTION,
                        },
                    }),
                "service-account creation",
            )?;
            self.send(
                self.client.get(&account_url).bearer_auth(token.expose()),
                "service-account creation readback",
            )?
        } else {
            self.decode(response, "service-account preflight")?
        };
        if account.email != config.service_account
            || account.disabled
            || account.description != DESCRIPTION
            || account.client_id.is_empty()
            || !account.client_id.bytes().all(|c| c.is_ascii_digit())
        {
            return Err(Error::Execution("service account does not match this setup, is disabled, or has no numeric client ID; no IAM policy was changed".into()));
        }
        let policy_url = format!("{account_url}:getIamPolicy");
        let mut policy: IamPolicy = self.send(
            self.client
                .post(&policy_url)
                .bearer_auth(token.expose())
                .query(&[("options.requestedPolicyVersion", "3")]),
            "signer policy preflight",
        )?;
        let member = format!("user:{}", config.signer_account);
        let has_grant = |policy: &IamPolicy| {
            policy.bindings.iter().any(|binding| {
                binding.role == TOKEN_CREATOR && binding.condition.is_none() && binding.members.contains(&member)
            })
        };
        if !has_grant(&policy) {
            if policy.etag.as_ref().map_or(true, |etag| etag.is_empty()) {
                return Err(Error::Execution(
                    "IAM policy omitted its concurrency ETag; no policy was changed".into(),
                ));
            }
            if let Some(binding) = policy
                .bindings
                .iter_mut()
                .find(|binding| binding.role == TOKEN_CREATOR && binding.condition.is_none())
            {
                binding.members.push(member.clone());
            } else {
                policy.bindings.push(Binding {
                    role: TOKEN_CREATOR.into(),
                    members: vec![member.clone()],
                    condition: None,
                });
            }
            let _: IamPolicy = self.send(
                self.client
                    .post(format!("{account_url}:setIamPolicy"))
                    .bearer_auth(token.expose())
                    .json(&SetPolicy {
                        policy: &policy,
                        update_mask: "bindings,etag",
                    }),
                "signer policy update",
            )?;
            let readback: IamPolicy = self.send(
                self.client
                    .post(&policy_url)
                    .bearer_auth(token.expose())
                    .query(&[("options.requestedPolicyVersion", "3")]),
                "signer policy readback",
            )?;
            if !has_grant(&readback) {
                return Err(Error::Execution(
                    "signing permission was not present in IAM readback; do not assume setup succeeded".into(),
                ));
            }
        }
        Ok(CloudGrant {
            client_id: account.client_id,
        })
    }
}

#[derive(Serialize)]
struct Empty {}

/// Called only by the CLI's explicitly applied, saved setup plan. Does not
/// authorize Workspace scopes or create downloadable service-account keys.
pub fn provision(config: &GoogleDelegation) -> Result<CloudGrant> {
    let http = Http::production()?;
    let token = http.source_token(&source_credentials(config)?, &config.signer_account)?;
    http.provision(config, &token)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
        time::{Duration, Instant},
    };

    use serde::Deserialize;

    use crate::google::delegation::{
        requires_delegation, source_credentials, uses_delegation, validate_subject, Claims, GoogleDelegation, Http,
        IamPolicy, OAuthCredentials, SecretString, DESCRIPTION, TOKEN_CREATOR,
    };

    #[derive(Debug)]
    struct Request {
        method: String,
        target: String,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
    }
    struct Fixture {
        http: Http,
        worker: thread::JoinHandle<Vec<Request>>,
    }
    impl Fixture {
        fn new(responses: Vec<(u16, String)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("test fixture should succeed");
            listener.set_nonblocking(true).expect("test fixture should succeed");
            let origin = format!("http://{}", listener.local_addr().expect("test fixture should succeed"));
            let worker = thread::spawn(move || {
                let mut requests = Vec::new();
                for (status, body) in responses {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    let stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                assert!(Instant::now() < deadline, "expected another HTTP request");
                                thread::sleep(Duration::from_millis(5));
                            }
                            Err(error) => panic!("accept: {error}"),
                        }
                    };
                    stream.set_nonblocking(false).expect("test fixture should succeed");
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .expect("test fixture should succeed");
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    reader.read_line(&mut line).expect("test fixture should succeed");
                    let mut parts = line.split_whitespace();
                    let method = parts.next().expect("test fixture should succeed").to_owned();
                    let target = parts.next().expect("test fixture should succeed").to_owned();
                    let mut headers = BTreeMap::new();
                    loop {
                        line.clear();
                        reader.read_line(&mut line).expect("test fixture should succeed");
                        if line == "\r\n" {
                            break;
                        }
                        let (key, value) = line.split_once(':').expect("test fixture should succeed");
                        headers.insert(key.to_ascii_lowercase(), value.trim().to_owned());
                    }
                    let length = headers
                        .get("content-length")
                        .map_or(0, |value| value.parse::<usize>().expect("test fixture should succeed"));
                    let mut bytes = vec![0; length];
                    reader.read_exact(&mut bytes).expect("test fixture should succeed");
                    requests.push(Request {
                        method,
                        target,
                        headers,
                        body: bytes,
                    });
                    write!(reader.get_mut(), "HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("test fixture should succeed");
                }
                requests
            });
            let http = Http {
                client: reqwest::blocking::Client::builder()
                    .no_proxy()
                    .timeout(Duration::from_secs(3))
                    .build()
                    .expect("test fixture should succeed"),
                token_url: format!("{origin}/token"),
                userinfo_url: format!("{origin}/userinfo"),
                signing_url: format!("{origin}/sign"),
                gmail_url: format!("{origin}/gmail"),
                cloud_url: format!("{origin}/cloud"),
                iam_url: format!("{origin}/iam"),
                usage_url: format!("{origin}/usage/projects"),
            };
            Self { http, worker }
        }
        fn finish(self) -> Vec<Request> {
            self.worker.join().expect("test fixture should succeed")
        }
    }
    fn config() -> GoogleDelegation {
        GoogleDelegation {
            project: "example-project".into(),
            service_account: "settings@example-project.iam.gserviceaccount.com".into(),
            domain: "example.com".into(),
            subject: "person@example.com".into(),
            signer_account: "signer@example.com".into(),
            signer_state_dir: "/tmp/signer".into(),
        }
    }
    fn credentials() -> OAuthCredentials {
        OAuthCredentials {
            client_id: "oauth-client".into(),
            client_secret: "oauth-secret".into(),
            refresh_token: "refresh-secret".into(),
            kind: Some("authorized_user".into()),
        }
    }
    fn ok(body: &str) -> (u16, String) {
        (200, body.into())
    }
    fn fields(request: &Request) -> BTreeMap<String, String> {
        reqwest::Url::parse(&format!("http://fixture/?{}", String::from_utf8_lossy(&request.body)))
            .expect("test fixture should succeed")
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }
    #[test]
    fn keyless_flow_checks_both_accounts_and_uses_only_fixed_claims() {
        let _lock = crate::test_support::lock_env();
        let parent_token = std::env::var_os("GOOGLE_WORKSPACE_CLI_TOKEN");
        let fixture = Fixture::new(vec![
            ok(r#"{"access_token":"signer-token","expires_in":3600}"#),
            ok(r#"{"email":"signer@example.com","verified_email":true,"hd":"example.com"}"#),
            ok(r#"{"signedJwt":"signed-private-assertion"}"#),
            ok(r#"{"access_token":"delegated-token","expires_in":3600}"#),
            ok(r#"{"sendAs":[{"sendAsEmail":"person@example.com","isPrimary":true}]}"#),
        ]);
        let config = config();
        let source = fixture
            .http
            .source_token(&credentials(), &config.signer_account)
            .expect("test fixture should succeed");
        let delegated = fixture
            .http
            .mint(&config, &source)
            .expect("test fixture should succeed");
        assert_eq!(delegated.expose(), "delegated-token");
        #[cfg(unix)]
        {
            let script = crate::test_support::TempScript::new(
                "gws",
                r#"#!/bin/sh
[ "$GOOGLE_WORKSPACE_CLI_TOKEN" = 'delegated-token' ] || exit 1
[ "$GOOGLE_WORKSPACE_CLI_CONFIG_DIR" = '/tmp/signer' ] || exit 1
[ "$GOOGLE_WORKSPACE_CLI_KEYRING_BACKEND" = 'file' ] || exit 1
[ -z "$GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE" ] || exit 1
[ -z "$GOOGLE_WORKSPACE_CLI_CLIENT_SECRET" ] || exit 1
[ -z "$OP_SERVICE_ACCOUNT_TOKEN" ] || exit 1
printf 'accepted'
"#,
            );
            let runtime = crate::google::materializer::delegated_context(&config.signer_state_dir, &delegated);
            let mut child = std::process::Command::new(script.path());
            runtime.apply_to_command(&mut child);
            switchboard_core::process::clear_one_password_environment(&mut child);
            let output = switchboard_core::process::output_with_timeout(&mut child, Duration::from_secs(3))
                .expect("delegated token should reach only the selected child");
            assert!(output.status.success());
            assert_eq!(output.stdout, b"accepted");
            assert_eq!(std::env::var_os("GOOGLE_WORKSPACE_CLI_TOKEN"), parent_token);
        }
        let requests = fixture.finish();
        assert_eq!(
            requests.iter().map(|r| r.method.as_str()).collect::<Vec<_>>(),
            ["POST", "GET", "POST", "POST", "GET"]
        );
        assert_eq!(fields(&requests[0])["grant_type"], "refresh_token");
        assert_eq!(fields(&requests[0])["refresh_token"], "refresh-secret");
        assert_eq!(requests[1].headers["authorization"], "Bearer signer-token");
        assert_eq!(requests[2].target, format!("/sign/{}:signJwt", config.service_account));
        #[derive(Deserialize)]
        struct SignedPayload {
            payload: String,
        }
        let signed: SignedPayload = serde_json::from_slice(&requests[2].body).expect("test fixture should succeed");
        let claims: Claims<'_> = serde_json::from_str(&signed.payload).expect("test fixture should succeed");
        assert_eq!(claims.sub, config.subject);
        assert_eq!(claims.iss, config.service_account);
        assert_eq!(claims.scope, GoogleDelegation::SCOPES.join(" "));
        assert_eq!(claims.aud, "https://oauth2.googleapis.com/token");
        assert_eq!(claims.exp - claims.iat, 3600);
        assert_eq!(fields(&requests[3])["assertion"], "signed-private-assertion");
        assert_eq!(requests[4].headers["authorization"], "Bearer delegated-token");
        assert_eq!(requests[4].target, "/gmail?fields=sendAs%28sendAsEmail%2CisPrimary%29");
        assert!(!format!("{delegated:?}").contains("delegated-token"));
    }
    #[test]
    fn wrong_or_unverified_signer_stops_before_signing() {
        for identity in [
            r#"{"email":"other@example.com","verified_email":true}"#,
            r#"{"email":"signer@example.com","verified_email":false}"#,
        ] {
            let fixture = Fixture::new(vec![
                ok(r#"{"access_token":"signer-token","expires_in":3600}"#),
                ok(identity),
            ]);
            assert!(fixture.http.source_token(&credentials(), "signer@example.com").is_err());
            assert_eq!(fixture.finish().len(), 2);
        }
    }
    #[test]
    fn wrong_primary_subject_and_revoked_grant_never_return_a_token() {
        let fixture = Fixture::new(vec![
            ok(r#"{"signedJwt":"assertion"}"#),
            ok(r#"{"access_token":"delegated-token","expires_in":3600}"#),
            ok(r#"{"sendAs":[{"sendAsEmail":"other@example.com","isPrimary":true}]}"#),
        ]);
        assert!(fixture
            .http
            .mint(&config(), &SecretString::from("source".to_owned()))
            .is_err());
        fixture.finish();
        let fixture = Fixture::new(vec![
            ok(r#"{"signedJwt":"assertion"}"#),
            (
                400,
                r#"{"error":"unauthorized_client","error_description":"sensitive-provider-body"}"#.into(),
            ),
        ]);
        let error = fixture
            .http
            .mint(&config(), &SecretString::from("source".to_owned()))
            .expect_err("test operation should fail")
            .to_string();
        assert!(error.contains("Workspace authorization"));
        assert!(!error.contains("sensitive-provider-body"));
        assert_eq!(fixture.finish().len(), 2);
    }
    #[test]
    fn verified_custom_domain_consumer_stops_before_cloud_access() {
        let fixture = Fixture::new(vec![
            ok(r#"{"access_token":"signer-token","expires_in":3600}"#),
            ok(r#"{"email":"signer@example.com","verified_email":true}"#),
        ]);
        let error = fixture
            .http
            .source_token(&credentials(), "signer@example.com")
            .expect_err("consumer identity must not pass")
            .to_string();
        assert!(error.contains("custom-domain consumer"));
        assert_eq!(fixture.finish().len(), 2);
    }

    #[test]
    fn repeated_mints_do_not_cache_a_token_or_hide_revocation() {
        let fixture = Fixture::new(vec![
            ok(r#"{"signedJwt":"assertion"}"#),
            ok(r#"{"access_token":"first","expires_in":3600}"#),
            ok(r#"{"sendAs":[{"sendAsEmail":"person@example.com","isPrimary":true}]}"#),
            (403, "sensitive-provider-body".into()),
        ]);
        let source = SecretString::from("source".to_owned());
        assert_eq!(
            fixture
                .http
                .mint(&config(), &source)
                .expect("test fixture should succeed")
                .expose(),
            "first"
        );
        let error = fixture
            .http
            .mint(&config(), &source)
            .expect_err("test operation should fail")
            .to_string();
        assert!(error.contains("403"));
        assert!(!error.contains("sensitive-provider-body"));
        assert_eq!(fixture.finish().len(), 4);
    }
    #[test]
    fn existing_cloud_setup_reconciles_without_writes_or_extra_grants() {
        let config = config();
        let account = format!(
            r#"{{"email":"{}","oauth2ClientId":"12345","description":"{DESCRIPTION}"}}"#,
            config.service_account
        );
        let policy = format!(
            r#"{{"version":3,"etag":"v1","bindings":[{{"role":"{TOKEN_CREATOR}","members":["user:signer@example.com"]}}]}}"#
        );
        let fixture = Fixture::new(vec![
            ok(r#"{"projectNumber":"123"}"#),
            ok(r#"{"state":"ENABLED"}"#),
            ok(r#"{"state":"ENABLED"}"#),
            ok(r#"{"state":"ENABLED"}"#),
            ok(&account),
            ok(&policy),
        ]);
        assert_eq!(
            fixture
                .http
                .provision(&config, &"source".to_owned().into())
                .expect("test fixture should succeed")
                .client_id,
            "12345"
        );
        let requests = fixture.finish();
        assert!(requests
            .iter()
            .all(|r| r.method == "GET" || r.target.ends_with(":getIamPolicy?options.requestedPolicyVersion=3")));
        assert!(requests.last().expect("test fixture should succeed").body.is_empty());
    }
    #[test]
    fn iam_reconciliation_preserves_conditions_and_requires_readback() {
        let config = config();
        let account = format!(
            r#"{{"email":"{}","oauth2ClientId":"12345","description":"{DESCRIPTION}"}}"#,
            config.service_account
        );
        let before = r#"{"version":3,"etag":"v1","bindings":[{"role":"roles/viewer","members":["user:other@example.com"],"condition":{"expression":"true","title":"Keep this"}}]}"#;
        let after = format!(
            r#"{{"version":3,"etag":"v2","bindings":[{{"role":"{TOKEN_CREATOR}","members":["user:signer@example.com"]}}]}}"#
        );
        let fixture = Fixture::new(vec![
            ok(r#"{"projectNumber":"123"}"#),
            ok(r#"{"state":"ENABLED"}"#),
            ok(r#"{"state":"ENABLED"}"#),
            ok(r#"{"state":"ENABLED"}"#),
            ok(&account),
            ok(before),
            ok(&after),
            ok(&after),
        ]);
        fixture
            .http
            .provision(&config, &"source".to_owned().into())
            .expect("test fixture should succeed");
        let requests = fixture.finish();
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Update {
            policy: IamPolicy,
            update_mask: String,
        }
        let update: Update = serde_json::from_slice(&requests[6].body).expect("test fixture should succeed");
        assert_eq!(update.update_mask, "bindings,etag");
        assert_eq!(update.policy.version, 3);
        assert_eq!(update.policy.etag.as_deref(), Some("v1"));
        assert_eq!(
            update.policy.bindings[0]
                .condition
                .as_ref()
                .expect("test fixture should succeed")
                .title,
            "Keep this"
        );
        assert_eq!(update.policy.bindings[0].members, ["user:other@example.com"]);
        assert_eq!(update.policy.bindings[1].members, ["user:signer@example.com"]);
        assert!(requests[7]
            .target
            .ends_with(":getIamPolicy?options.requestedPolicyVersion=3"));
    }
    #[test]
    fn capability_routing_and_subject_boundary_are_explicit() {
        let args = |command: &[&str]| command.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>();
        assert!(uses_delegation(&args(&[
            "gmail", "users", "settings", "sendAs", "list"
        ])));
        assert!(!requires_delegation(&args(&[
            "gmail", "users", "settings", "sendAs", "list"
        ])));
        assert!(requires_delegation(&args(&[
            "gmail", "users", "settings", "sendAs", "create"
        ])));
        assert!(!uses_delegation(&args(&["gmail", "users", "getProfile"])));
        assert!(!uses_delegation(&args(&[
            "gmail", "users", "settings", "sendAs", "create", "--help"
        ])));
        assert!(validate_subject(
            &args(&["--params", r#"{"userId":"other@example.com"}"#]),
            "person@example.com"
        )
        .is_err());
        assert!(validate_subject(&args(&["--params", r#"{"userId":"me"}"#]), "person@example.com").is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn credential_export_is_namespace_scoped_and_failures_redact_raw_output() {
        let _lock = crate::test_support::lock_env();
        let script = crate::test_support::TempScript::new(
            "gws",
            r#"#!/bin/sh
[ "$1 $2 $3" = 'auth export --unmasked' ] || exit 1
[ "$GOOGLE_WORKSPACE_CLI_CONFIG_DIR" = '/tmp/signer' ] || exit 1
[ "$GOOGLE_WORKSPACE_CLI_KEYRING_BACKEND" = 'file' ] || exit 1
[ -z "$GOOGLE_WORKSPACE_CLI_TOKEN" ] || exit 1
[ -z "$OP_SERVICE_ACCOUNT_TOKEN" ] || exit 1
printf '%s' '{"client_id":"expected-client","client_secret":"synthetic-secret","refresh_token":"synthetic-refresh","type":"authorized_user"}'
"#,
        );
        let failure = crate::test_support::TempScript::new(
            "gws",
            "#!/bin/sh\nprintf 'raw-secret-output'\nprintf 'raw-secret-error' >&2\nexit 1\n",
        );
        std::env::set_var("SWITCHBOARD_GWS_BIN", script.path());
        std::env::set_var("GOOGLE_WORKSPACE_CLI_TOKEN", "ambient-secret");
        std::env::set_var("OP_SERVICE_ACCOUNT_TOKEN", "ambient-bootstrap");
        let exported = source_credentials(&config());
        std::env::set_var("SWITCHBOARD_GWS_BIN", failure.path());
        let error = source_credentials(&config())
            .err()
            .expect("export should fail")
            .to_string();
        std::env::remove_var("SWITCHBOARD_GWS_BIN");
        std::env::remove_var("GOOGLE_WORKSPACE_CLI_TOKEN");
        std::env::remove_var("OP_SERVICE_ACCOUNT_TOKEN");
        assert_eq!(
            exported.expect("configured source should export privately").client_id,
            "expected-client"
        );
        assert!(error.contains("unavailable"));
        assert!(!error.contains("raw-secret"));
    }
    #[test]
    fn applied_setup_enables_and_creates_only_planned_resources_then_reads_them_back() {
        let config = config();
        let account = format!(
            r#"{{"email":"{}","oauth2ClientId":"12345","description":"{DESCRIPTION}"}}"#,
            config.service_account
        );
        let granted = format!(
            r#"{{"version":3,"etag":"v2","bindings":[{{"role":"{TOKEN_CREATOR}","members":["user:signer@example.com"]}}]}}"#
        );
        let fixture = Fixture::new(vec![
            ok(r#"{"projectNumber":"123"}"#),
            ok(r#"{"state":"DISABLED"}"#),
            ok(r#"{"name":"operations/enable-iam","done":true}"#),
            ok(r#"{"state":"ENABLED"}"#),
            ok(r#"{"state":"ENABLED"}"#),
            ok(r#"{"state":"ENABLED"}"#),
            (404, "{}".into()),
            ok(&account),
            ok(&account),
            ok(r#"{"version":3,"etag":"v1","bindings":[]}"#),
            ok(&granted),
            ok(&granted),
        ]);
        assert_eq!(
            fixture
                .http
                .provision(&config, &"source".to_owned().into())
                .expect("applied setup should succeed")
                .client_id,
            "12345"
        );
        let requests = fixture.finish();
        assert_eq!(requests[2].method, "POST");
        assert_eq!(
            requests[2].target,
            "/usage/projects/123/services/iam.googleapis.com:enable"
        );
        assert_eq!(requests[3].method, "GET");
        assert_eq!(requests[7].method, "POST");
        assert_eq!(requests[7].target, "/iam/example-project/serviceAccounts");
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Created {
            account_id: String,
        }
        let created: Created = serde_json::from_slice(&requests[7].body).expect("native create request should decode");
        assert_eq!(created.account_id, "settings");
        assert_eq!(requests[8].method, "GET");
        assert!(requests[8].target.ends_with(&config.service_account));
        assert!(requests[11].target.contains(":getIamPolicy"));
        assert!(!requests.iter().any(|request| request.target.contains("/keys")));
    }

    #[test]
    fn uncertain_account_creation_is_reconciled_by_stable_id_without_a_second_create() {
        let config = config();
        let account = format!(
            r#"{{"email":"{}","oauth2ClientId":"12345","description":"{DESCRIPTION}"}}"#,
            config.service_account
        );
        let granted = format!(
            r#"{{"version":3,"etag":"v2","bindings":[{{"role":"{TOKEN_CREATOR}","members":["user:signer@example.com"]}}]}}"#
        );
        let mut responses = vec![
            ok(r#"{"projectNumber":"123"}"#),
            ok(r#"{"state":"ENABLED"}"#),
            ok(r#"{"state":"ENABLED"}"#),
            ok(r#"{"state":"ENABLED"}"#),
            (404, "{}".into()),
            (503, "private-provider-error".into()),
        ];
        responses.extend(vec![
            ok(r#"{"projectNumber":"123"}"#),
            ok(r#"{"state":"ENABLED"}"#),
            ok(r#"{"state":"ENABLED"}"#),
            ok(r#"{"state":"ENABLED"}"#),
            ok(&account),
            ok(&granted),
        ]);
        let fixture = Fixture::new(responses);
        let source = "source".to_owned().into();
        let error = fixture
            .http
            .provision(&config, &source)
            .expect_err("ambiguous create must stop")
            .to_string();
        assert!(!error.contains("private-provider-error"));
        fixture
            .http
            .provision(&config, &source)
            .expect("restart should reconcile the saved account ID");
        let requests = fixture.finish();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.method == "POST" && request.target == "/iam/example-project/serviceAccounts")
                .count(),
            1
        );
    }
}
