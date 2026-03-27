use std::collections::BTreeMap;

use reqwest::{
    blocking::Client,
    header::{ACCEPT, CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE},
    redirect::Policy,
    Method, Url,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Result};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct StoredCookie {
    pub(crate) name: String,
    pub(crate) value: String,
}

#[derive(Clone, Debug)]
pub(crate) enum RequestBody {
    None,
    Form(Vec<(String, String)>),
}

#[derive(Clone, Debug)]
pub(crate) struct RequestSpec {
    pub(crate) method: Method,
    pub(crate) path: String,
    pub(crate) query: Vec<(String, String)>,
    pub(crate) body: RequestBody,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedResponse {
    pub(crate) status_code: u16,
    pub(crate) final_url: Url,
    pub(crate) location: Option<String>,
    pub(crate) content_type: Option<String>,
    pub(crate) body_text: String,
    pub(crate) redirect_chain: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct JsonResponse {
    pub(crate) status_code: u16,
    pub(crate) final_url: Url,
    pub(crate) content_type: Option<String>,
    pub(crate) body_text: String,
    pub(crate) body: Value,
}

#[derive(Clone, Debug)]
struct ResponseChunk {
    request_url: Url,
    status_code: u16,
    location: Option<String>,
    content_type: Option<String>,
    body_text: String,
    set_cookies: Vec<StoredCookie>,
}

pub(crate) struct MyChartClient {
    base_url: String,
    http: Client,
}

pub(crate) fn normalize_api_base_url(base_url: &str) -> Result<String> {
    let trimmed = base_url.trim().trim_end_matches('/').to_owned();
    Url::parse(&format!("{trimmed}/"))
        .map_err(|error| Error::Config(format!("invalid base URL {trimmed:?}: {error}")))?;
    Ok(trimmed)
}

impl MyChartClient {
    pub(crate) fn new(base_url: String) -> Result<Self> {
        let trimmed = normalize_api_base_url(&base_url)?;

        let http = Client::builder()
            .redirect(Policy::none())
            .user_agent(format!("mychart-cli/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| Error::Http(format!("failed to build HTTP client: {error}")))?;

        Ok(Self {
            base_url: trimmed,
            http,
        })
    }

    pub(crate) fn fetch_capability_statement(&self) -> Result<JsonResponse> {
        self.execute_json(
            Method::GET,
            "metadata",
            &[("_format".into(), "json".into())],
            None,
            None,
        )
    }

    pub(crate) fn exchange_oauth_token(
        &self,
        token_url: &str,
        form: &[(String, String)],
        authorization_header: Option<&str>,
    ) -> Result<JsonResponse> {
        let url = Url::parse(token_url)
            .map_err(|error| Error::Config(format!("invalid OAuth token endpoint {token_url:?}: {error}")))?;
        let mut request = self.http.request(Method::POST, url).form(form);
        if let Some(authorization_header) = authorization_header {
            request = request.header("Authorization", authorization_header);
        }
        self.execute_json_request(request)
    }

    pub(crate) fn execute_bearer_json(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        access_token: &str,
        body: Option<&Value>,
    ) -> Result<JsonResponse> {
        self.execute_json(method, path, query, Some(access_token), body)
    }

    pub(crate) fn execute_bearer_json_absolute(
        &self,
        method: Method,
        url: &str,
        access_token: &str,
        body: Option<&Value>,
    ) -> Result<JsonResponse> {
        self.execute_json_absolute(method, url, Some(access_token), body)
    }

    pub(crate) fn execute(
        &self,
        mut spec: RequestSpec,
        cookies: &mut BTreeMap<String, String>,
        follow_redirects: bool,
    ) -> Result<ResolvedResponse> {
        let mut redirect_chain = Vec::new();
        let mut response = self.execute_once(&spec, cookies)?;
        apply_set_cookies(cookies, &response.set_cookies);

        while follow_redirects && is_redirect_status(response.status_code) {
            if redirect_chain.len() >= 10 {
                return Err(Error::Http("too many redirects while talking to MyChart".into()));
            }

            let location = response
                .location
                .clone()
                .ok_or_else(|| Error::Http("MyChart redirect response was missing a Location header".into()))?;
            let next_url = response
                .request_url
                .join(&location)
                .map_err(|error| Error::Http(format!("failed to resolve MyChart redirect {location:?}: {error}")))?;
            redirect_chain.push(next_url.to_string());

            spec = RequestSpec {
                method: redirect_method(response.status_code, &spec.method),
                path: next_url.to_string(),
                query: Vec::new(),
                body: redirect_body(response.status_code, &spec.method, &spec.body),
            };

            response = self.execute_once(&spec, cookies)?;
            apply_set_cookies(cookies, &response.set_cookies);
        }

        Ok(ResolvedResponse {
            status_code: response.status_code,
            final_url: response.request_url,
            location: response.location,
            content_type: response.content_type,
            body_text: response.body_text,
            redirect_chain,
        })
    }

    fn execute_json(
        &self,
        method: Method,
        path: &str,
        query: &[(String, String)],
        access_token: Option<&str>,
        body: Option<&Value>,
    ) -> Result<JsonResponse> {
        let url = self.build_url(path, query)?;
        self.execute_json_request(self.configure_json_request(method, url, access_token, body))
    }

    fn execute_json_absolute(
        &self,
        method: Method,
        url: &str,
        access_token: Option<&str>,
        body: Option<&Value>,
    ) -> Result<JsonResponse> {
        let url = Url::parse(url).map_err(|error| Error::Config(format!("invalid request URL {url:?}: {error}")))?;
        self.execute_json_request(self.configure_json_request(method, url, access_token, body))
    }

    fn configure_json_request(
        &self,
        method: Method,
        url: Url,
        access_token: Option<&str>,
        body: Option<&Value>,
    ) -> reqwest::blocking::RequestBuilder {
        let mut request = self
            .http
            .request(method, url)
            .header(ACCEPT, "application/fhir+json, application/json");
        if let Some(access_token) = access_token {
            request = request.bearer_auth(access_token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        request
    }

    fn execute_json_request(&self, request: reqwest::blocking::RequestBuilder) -> Result<JsonResponse> {
        let response = request
            .send()
            .map_err(|error| Error::Http(format!("request to MyChart failed: {error}")))?;
        let final_url = response.url().clone();
        let status_code = response.status().as_u16();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let body_text = response
            .text()
            .map_err(|error| Error::Http(format!("failed to read MyChart response: {error}")))?;

        Ok(JsonResponse {
            status_code,
            final_url,
            body: parse_json_body(content_type.as_deref(), &body_text),
            content_type,
            body_text,
        })
    }

    fn execute_once(&self, spec: &RequestSpec, cookies: &BTreeMap<String, String>) -> Result<ResponseChunk> {
        let url = self.build_url(&spec.path, &spec.query)?;
        let mut request = self.http.request(spec.method.clone(), url.clone());

        if !cookies.is_empty() {
            request = request.header(COOKIE, render_cookie_header(cookies));
        }

        request = match &spec.body {
            RequestBody::None => request,
            RequestBody::Form(fields) => request.form(fields),
        };

        let response = request
            .send()
            .map_err(|error| Error::Http(format!("request to MyChart failed: {error}")))?;
        let status_code = response.status().as_u16();
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let set_cookies = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(parse_set_cookie_header)
            .collect::<Vec<_>>();
        let body_text = response
            .text()
            .map_err(|error| Error::Http(format!("failed to read MyChart response: {error}")))?;

        Ok(ResponseChunk {
            request_url: url,
            status_code,
            location,
            content_type,
            body_text,
            set_cookies,
        })
    }

    fn build_url(&self, path: &str, query: &[(String, String)]) -> Result<Url> {
        let mut url = if path.starts_with("http://") || path.starts_with("https://") {
            Url::parse(path).map_err(|error| Error::Config(format!("invalid request URL {path:?}: {error}")))?
        } else {
            Url::parse(&format!("{}/", self.base_url))
                .and_then(|base| base.join(path.trim_start_matches('/')))
                .map_err(|error| Error::Config(format!("invalid request path {path:?}: {error}")))?
        };

        {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }

        Ok(url)
    }
}

pub(crate) fn apply_set_cookies(cookies: &mut BTreeMap<String, String>, set_cookies: &[StoredCookie]) {
    for cookie in set_cookies {
        if cookie.value.is_empty() {
            cookies.remove(&cookie.name);
        } else {
            cookies.insert(cookie.name.clone(), cookie.value.clone());
        }
    }
}

pub(crate) fn cookie_names(cookies: &BTreeMap<String, String>) -> Vec<String> {
    cookies.keys().cloned().collect()
}

fn parse_set_cookie_header(value: &reqwest::header::HeaderValue) -> Option<StoredCookie> {
    let value = value.to_str().ok()?;
    let first_segment = value.split(';').next()?.trim();
    let (name, cookie_value) = first_segment.split_once('=')?;
    Some(StoredCookie {
        name: name.trim().to_owned(),
        value: cookie_value.trim().to_owned(),
    })
}

fn render_cookie_header(cookies: &BTreeMap<String, String>) -> String {
    cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn parse_json_body(content_type: Option<&str>, body_text: &str) -> Value {
    if body_text.trim().is_empty() {
        return Value::Null;
    }

    if let Some(content_type) = content_type {
        if content_type.contains("json") || content_type.contains("fhir+json") {
            return serde_json::from_str(body_text).unwrap_or_else(|_| Value::String(body_text.to_owned()));
        }
    }

    serde_json::from_str(body_text).unwrap_or_else(|_| Value::String(body_text.to_owned()))
}

fn is_redirect_status(status_code: u16) -> bool {
    matches!(status_code, 301 | 302 | 303 | 307 | 308)
}

fn redirect_method(status_code: u16, current_method: &Method) -> Method {
    match status_code {
        303 => Method::GET,
        301 | 302 if *current_method != Method::GET && *current_method != Method::HEAD => Method::GET,
        307 | 308 => current_method.clone(),
        _ => current_method.clone(),
    }
}

fn redirect_body(status_code: u16, current_method: &Method, current_body: &RequestBody) -> RequestBody {
    let next_method = redirect_method(status_code, current_method);
    if next_method == Method::GET || next_method == Method::HEAD {
        RequestBody::None
    } else {
        current_body.clone()
    }
}
