use clap::{Args, ValueEnum};
use serde::Serialize;
use serde_json::Value;

use crate::{Error, Result};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum SortDirection {
    Asc,
    Desc,
}

impl SortDirection {
    pub(crate) fn as_api_value(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }
}

#[derive(Clone, Debug, Args)]
pub(crate) struct WindowedQueryArgs {
    #[arg(long = "max-results")]
    pub(crate) max_results: Option<u32>,

    #[arg(long)]
    pub(crate) offset: Option<u32>,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct OrderingArgs {
    #[arg(long = "order-by")]
    pub(crate) order_by: Option<String>,

    #[arg(long, value_enum)]
    pub(crate) order: Option<SortDirection>,
}

pub(crate) fn push_window_query(query: &mut Vec<(String, String)>, window: &WindowedQueryArgs) {
    push_optional_query_u32(query, "maxResults", window.max_results);
    push_optional_query_u32(query, "offset", window.offset);
}

pub(crate) fn push_ordering_query(query: &mut Vec<(String, String)>, ordering: &OrderingArgs) {
    push_optional_query_string(query, "orderBy", ordering.order_by.clone());
    if let Some(order) = ordering.order {
        query.push(("order".into(), order.as_api_value().into()));
    }
}

pub(crate) fn push_optional_query_string(query: &mut Vec<(String, String)>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        query.push((key.into(), value));
    }
}

pub(crate) fn push_optional_query_u64(query: &mut Vec<(String, String)>, key: &str, value: Option<u64>) {
    if let Some(value) = value {
        query.push((key.into(), value.to_string()));
    }
}

pub(crate) fn push_optional_query_u32(query: &mut Vec<(String, String)>, key: &str, value: Option<u32>) {
    if let Some(value) = value {
        query.push((key.into(), value.to_string()));
    }
}

pub(crate) fn push_optional_query_f64(query: &mut Vec<(String, String)>, key: &str, value: Option<f64>) {
    if let Some(value) = value {
        query.push((key.into(), value.to_string()));
    }
}

pub(crate) fn push_optional_query_bool(query: &mut Vec<(String, String)>, key: &str, value: Option<bool>) {
    if let Some(value) = value {
        query.push((key.into(), value.to_string()));
    }
}

pub(crate) fn push_query_csv_u64(query: &mut Vec<(String, String)>, key: &str, values: Vec<u64>) {
    if !values.is_empty() {
        let joined = values
            .into_iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(",");
        query.push((key.into(), joined));
    }
}

pub(crate) fn serialize_payload<T: Serialize>(payload: T) -> Result<Value> {
    serde_json::to_value(payload)
        .map_err(|error| Error::Config(format!("failed to serialize Mindbody payload: {error}")))
}
