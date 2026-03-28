use reqwest::{
    blocking::Client,
    header::{HeaderMap, LOCATION},
    Method, Url,
};
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

pub(crate) struct ResponseData {
    pub(crate) status_code: u16,
    pub(crate) location: Option<String>,
    pub(crate) body: Option<Value>,
    pub(crate) raw_body: Option<String>,
}

impl ResponseData {
    pub(crate) fn into_output(self) -> Value {
        match (self.body, self.raw_body) {
            (Some(body), _) => body,
            (None, Some(raw_body)) => json!({
                "status": "ok",
                "status_code": self.status_code,
                "location": self.location,
                "raw_body": raw_body,
            }),
            (None, None) => json!({
                "status": "ok",
                "status_code": self.status_code,
                "location": self.location,
            }),
        }
    }
}

pub(crate) struct SchwabClient {
    base_url: String,
    http: Client,
}

impl SchwabClient {
    pub(crate) fn new(base_url: String, user_agent: String) -> Result<Self> {
        let trimmed = normalize_base_url(&base_url)?;
        let http = Client::builder()
            .user_agent(user_agent)
            .build()
            .map_err(|error| Error::Http(format!("failed to build HTTP client: {error}")))?;

        Ok(Self {
            base_url: trimmed,
            http,
        })
    }

    pub(crate) fn build_url(&self, path: &str, query: &[(String, String)]) -> Result<Url> {
        let mut url = if path.starts_with("http://") || path.starts_with("https://") {
            Url::parse(path).map_err(|error| Error::Config(format!("invalid request URL {path:?}: {error}")))?
        } else {
            let joined = format!("{}/{}", self.base_url, path.trim_start_matches('/'));
            Url::parse(&joined).map_err(|error| Error::Config(format!("invalid request URL {joined:?}: {error}")))?
        };
        {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        Ok(url)
    }

    pub(crate) fn execute(&self, spec: RequestSpec) -> Result<Value> {
        self.execute_response(spec).map(ResponseData::into_output)
    }

    pub(crate) fn execute_response(&self, spec: RequestSpec) -> Result<ResponseData> {
        let url = self.build_url(&spec.path, &spec.query)?;
        let mut request = self.http.request(spec.method, url);

        request = match spec.auth {
            AuthMode::Basic { username, password } => request.basic_auth(username, Some(password)),
            AuthMode::Bearer(token) => request.bearer_auth(token),
        };

        request = match spec.body {
            RequestBody::None => request,
            RequestBody::Json(body) => request.json(&body),
            RequestBody::Form(fields) => request.form(&fields),
        };

        let response = request
            .send()
            .map_err(|error| Error::Http(format!("request to Schwab failed: {error}")))?;
        let status = response.status();
        let status_code = status.as_u16();
        let headers = response.headers().clone();
        let body_text = response
            .text()
            .map_err(|error| Error::Http(format!("failed to read Schwab response: {error}")))?;

        if !status.is_success() {
            return Err(Error::Api {
                status_code,
                body: parse_body(&body_text).unwrap_or_else(|| json!({ "raw_body": body_text })),
            });
        }

        Ok(ResponseData {
            status_code,
            location: header_string(&headers, LOCATION),
            body: parse_body(&body_text),
            raw_body: if body_text.trim().is_empty() {
                None
            } else if parse_body(&body_text).is_none() {
                Some(body_text)
            } else {
                None
            },
        })
    }
}

pub(crate) fn normalize_base_url(base_url: &str) -> Result<String> {
    let trimmed = base_url.trim().trim_end_matches('/').to_owned();
    Url::parse(&trimmed).map_err(|error| Error::Config(format!("invalid base URL {trimmed:?}: {error}")))?;
    Ok(trimmed)
}

fn header_string(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

fn parse_body(body: &str) -> Option<Value> {
    serde_json::from_str(body).ok()
}
