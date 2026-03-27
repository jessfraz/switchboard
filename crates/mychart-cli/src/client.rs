use std::collections::BTreeMap;

use reqwest::{
    blocking::Client,
    header::{CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE},
    redirect::Policy,
    Method, Url,
};
use serde::{Deserialize, Serialize};

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

impl MyChartClient {
    pub(crate) fn new(base_url: String) -> Result<Self> {
        let trimmed = base_url.trim().trim_end_matches('/').to_owned();
        Url::parse(&format!("{trimmed}/"))
            .map_err(|error| Error::Config(format!("invalid base URL {trimmed:?}: {error}")))?;

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
