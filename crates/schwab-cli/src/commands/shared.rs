use std::{fs, path::PathBuf};

use reqwest::Method;
use serde_json::Value;

use crate::{
    client::{AuthMode, RequestBody, RequestSpec, SchwabClient},
    Error, ResolvedContext, Result,
};

#[derive(Debug, clap::Args)]
pub(crate) struct JsonBodyArgs {
    #[arg(long, value_name = "JSON", conflicts_with = "body_file")]
    pub(crate) body: Option<String>,

    #[arg(long, value_name = "PATH", conflicts_with = "body")]
    pub(crate) body_file: Option<PathBuf>,
}

pub(crate) fn load_json_body(args: &JsonBodyArgs) -> Result<Value> {
    let raw = match (&args.body, &args.body_file) {
        (Some(body), None) => body.clone(),
        (None, Some(path)) => fs::read_to_string(path)
            .map_err(|error| Error::Io(format!("failed to read JSON body file {}: {error}", path.display())))?,
        (None, None) => {
            return Err(Error::Arguments(
                "missing request body, pass --body or --body-file".into(),
            ))
        }
        (Some(_), Some(_)) => return Err(Error::Arguments("pass only one of --body or --body-file".into())),
    };

    serde_json::from_str(&raw).map_err(|error| Error::Arguments(format!("invalid JSON request body: {error}")))
}

pub(crate) fn optional_query(query: &mut Vec<(String, String)>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        query.push((key.to_owned(), value));
    }
}

pub(crate) fn optional_bool_query(query: &mut Vec<(String, String)>, key: &str, value: bool) {
    if value {
        query.push((key.to_owned(), "true".into()));
    }
}

pub(crate) fn resolve_account_id(
    client: &SchwabClient,
    context: &mut ResolvedContext,
    account: &str,
) -> Result<String> {
    let trimmed = account.trim();
    if trimmed.is_empty() {
        return Err(Error::Arguments("account value may not be empty".into()));
    }

    if let Some(hash) = context.account_hash_for_plain_text(trimmed) {
        return Ok(hash.to_owned());
    }

    if !looks_like_plain_account_number(trimmed) {
        return Ok(trimmed.to_owned());
    }

    let response = client.execute(RequestSpec {
        method: Method::GET,
        path: "/accounts/accountNumbers".into(),
        query: Vec::new(),
        headers: context.trader_headers(),
        body: RequestBody::None,
        auth: AuthMode::Bearer(context.require_access_token()?.to_owned()),
    })?;
    context.remember_account_numbers(&response)?;

    if let Some(hash) = context.account_hash_for_plain_text(trimmed) {
        return Ok(hash.to_owned());
    }

    Err(Error::Arguments(format!(
        "Schwab did not return a hashed account mapping for plain account number {trimmed}"
    )))
}

fn looks_like_plain_account_number(value: &str) -> bool {
    value.len() >= 4 && value.chars().all(|ch| ch.is_ascii_digit())
}
