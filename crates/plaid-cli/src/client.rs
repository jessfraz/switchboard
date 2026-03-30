use reqwest::{blocking::Client, Url};
use serde_json::{json, Value};

use crate::{Error, Result};

pub(crate) struct PlaidCredentials<'a> {
    pub(crate) client_id: &'a str,
    pub(crate) secret: &'a str,
}

pub(crate) struct PlaidClient {
    base_url: String,
    plaid_version: String,
    http: Client,
}

impl PlaidClient {
    pub(crate) fn new(base_url: String, plaid_version: String) -> Result<Self> {
        let trimmed = base_url.trim().trim_end_matches('/').to_owned();
        Url::parse(&trimmed).map_err(|error| Error::Config(format!("invalid base URL {trimmed:?}: {error}")))?;

        let http = Client::builder()
            .user_agent(format!("plaid-cli/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| Error::Http(format!("failed to build HTTP client: {error}")))?;

        Ok(Self {
            base_url: trimmed,
            plaid_version,
            http,
        })
    }

    pub(crate) fn post(&self, credentials: PlaidCredentials<'_>, path: &str, body: Value) -> Result<Value> {
        let url = self.build_url(path)?;
        let response = self
            .http
            .post(url)
            .header("PLAID-CLIENT-ID", credentials.client_id)
            .header("PLAID-SECRET", credentials.secret)
            .header("Plaid-Version", &self.plaid_version)
            .json(&body)
            .send()
            .map_err(|error| Error::Http(format!("request to Plaid failed: {error}")))?;

        let status = response.status();
        let status_code = status.as_u16();
        let body_text = response
            .text()
            .map_err(|error| Error::Http(format!("failed to read Plaid response: {error}")))?;

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

    fn build_url(&self, path: &str) -> Result<Url> {
        let joined = format!("{}/{}", self.base_url, path.trim_start_matches('/'));
        Url::parse(&joined).map_err(|error| Error::Config(format!("invalid request URL {joined:?}: {error}")))
    }
}

fn parse_body(body: &str) -> Option<Value> {
    serde_json::from_str(body).ok()
}
