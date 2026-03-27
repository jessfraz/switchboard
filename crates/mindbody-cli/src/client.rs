use reqwest::{blocking::Client, Method, Url};
use serde_json::{json, Value};

use crate::{Error, Result};

pub(crate) struct Credentials<'a> {
    pub(crate) api_key: &'a str,
    pub(crate) client_key: &'a str,
    pub(crate) client_secret: &'a str,
}

pub(crate) enum RequestBody {
    None,
    Json(Value),
}

pub(crate) struct RequestSpec {
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) query: Vec<(String, String)>,
    pub(crate) body: RequestBody,
    pub(crate) idempotency_key: Option<String>,
}

pub(crate) struct MindbodyClient {
    base_url: String,
    http: Client,
}

impl MindbodyClient {
    pub(crate) fn new(base_url: String, app_name: String) -> Result<Self> {
        let trimmed = base_url.trim().trim_end_matches('/').to_owned();
        Url::parse(&trimmed).map_err(|error| Error::Config(format!("invalid base URL {trimmed:?}: {error}")))?;

        let http = Client::builder()
            .user_agent(app_name)
            .build()
            .map_err(|error| Error::Http(format!("failed to build HTTP client: {error}")))?;

        Ok(Self {
            base_url: trimmed,
            http,
        })
    }

    fn build_url(&self, path: &str, query: &[(String, String)]) -> Result<Url> {
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

    pub(crate) fn execute(&self, credentials: Credentials<'_>, spec: RequestSpec) -> Result<Value> {
        let url = self.build_url(&spec.path, &spec.query)?;
        let mut request = self
            .http
            .request(spec.method, url)
            .header("API-Key", credentials.api_key)
            .basic_auth(credentials.client_key, Some(credentials.client_secret));

        if let Some(idempotency_key) = spec.idempotency_key {
            request = request.header("Idempotency-Key", idempotency_key);
        }

        request = match spec.body {
            RequestBody::None => request,
            RequestBody::Json(body) => request.json(&body),
        };

        let response = request
            .send()
            .map_err(|error| Error::Http(format!("request to Mindbody failed: {error}")))?;
        let status = response.status();
        let status_code = status.as_u16();
        let body_text = response
            .text()
            .map_err(|error| Error::Http(format!("failed to read Mindbody response: {error}")))?;

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
