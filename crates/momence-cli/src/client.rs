use reqwest::{blocking::Client, Method, Url};
use serde_json::{json, Value};

use crate::{Error, Result};

pub(crate) enum AuthMode {
    Basic { username: String, password: String },
    Bearer(String),
}

pub(crate) enum RequestBody {
    None,
    Json(Value),
    Form(Vec<(String, String)>),
}

pub(crate) struct RequestSpec {
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) query: Vec<(String, String)>,
    pub(crate) body: RequestBody,
    pub(crate) auth: AuthMode,
}

pub(crate) struct MomenceClient {
    base_url: String,
    http: Client,
}

impl MomenceClient {
    pub(crate) fn new(base_url: String) -> Result<Self> {
        let trimmed = base_url.trim().trim_end_matches('/').to_owned();
        Url::parse(&trimmed).map_err(|error| Error::Config(format!("invalid base URL {trimmed:?}: {error}")))?;

        let http = Client::builder()
            .user_agent(format!("momence-cli/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| Error::Http(format!("failed to build HTTP client: {error}")))?;

        Ok(Self {
            base_url: trimmed,
            http,
        })
    }

    pub(crate) fn build_url(&self, path: &str, query: &[(String, String)]) -> Result<Url> {
        let joined = format!("{}/{}", self.base_url, path.trim_start_matches('/'));
        let mut url =
            Url::parse(&joined).map_err(|error| Error::Config(format!("invalid request URL {joined:?}: {error}")))?;
        {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        Ok(url)
    }

    pub(crate) fn execute(&self, spec: RequestSpec) -> Result<Value> {
        let url = self.build_url(&spec.path, &spec.query)?;
        let mut request = self.http.request(spec.method, url);

        request = match spec.auth {
            AuthMode::Basic { username, password } => request.basic_auth(username, Some(password)),
            AuthMode::Bearer(token) => request.bearer_auth(token),
        };

        request = match spec.body {
            RequestBody::None => request,
            RequestBody::Json(body) => request.json(&body),
            RequestBody::Form(form) => request.form(&form),
        };

        let response = request
            .send()
            .map_err(|error| Error::Http(format!("request to Momence failed: {error}")))?;
        let status = response.status();
        let status_code = status.as_u16();
        let body_text = response
            .text()
            .map_err(|error| Error::Http(format!("failed to read Momence response: {error}")))?;

        if !status.is_success() {
            return Err(Error::Api {
                status_code,
                body: parse_body(&body_text).unwrap_or_else(|| json!({ "raw_body": body_text })),
            });
        }

        if body_text.trim().is_empty() {
            return Ok(json!({
                "status": "ok",
                "status_code": status_code,
            }));
        }

        Ok(parse_body(&body_text).unwrap_or_else(|| {
            json!({
                "status": "ok",
                "status_code": status_code,
                "raw_body": body_text,
            })
        }))
    }
}

fn parse_body(body: &str) -> Option<Value> {
    serde_json::from_str(body).ok()
}
