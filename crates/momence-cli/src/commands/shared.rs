use std::path::PathBuf;

use clap::{Args, ValueEnum};
use serde_json::Value;

use crate::{Error, Result};

#[derive(Clone, Debug, Args)]
pub(crate) struct JsonBodyArgs {
    #[arg(
        long,
        value_name = "JSON",
        conflicts_with = "body_file",
        help = "Inline JSON request body"
    )]
    body: Option<String>,

    #[arg(
        long = "body-file",
        value_name = "PATH",
        conflicts_with = "body",
        help = "Path to a JSON request body file, use - for stdin"
    )]
    body_file: Option<PathBuf>,
}

impl JsonBodyArgs {
    pub(crate) fn read(&self, schema_name: &str) -> Result<Value> {
        let contents = match (&self.body, &self.body_file) {
            (Some(body), None) => body.clone(),
            (None, Some(path)) if path.as_os_str() == "-" => std::io::read_to_string(std::io::stdin())
                .map_err(|error| Error::Io(format!("failed to read JSON body from stdin: {error}")))?,
            (None, Some(path)) => std::fs::read_to_string(path)
                .map_err(|error| Error::Io(format!("failed to read JSON body from {}: {error}", path.display())))?,
            (None, None) => {
                return Err(Error::Arguments(format!(
                    "missing request body, provide --body or --body-file for {schema_name}"
                )));
            }
            (Some(_), Some(_)) => {
                return Err(Error::Arguments("request body source is ambiguous".into()));
            }
        };

        serde_json::from_str(&contents).map_err(|error| {
            Error::Arguments(format!(
                "request body must be valid JSON matching {schema_name}: {error}"
            ))
        })
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum SortOrder {
    Asc,
    Desc,
}

impl SortOrder {
    fn as_api_value(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }
}

#[derive(Clone, Debug, ValueEnum)]
pub(crate) enum SessionType {
    #[value(name = "private")]
    Private,
    #[value(name = "special-event")]
    SpecialEvent,
    #[value(name = "special-event-new")]
    SpecialEventNew,
    #[value(name = "retreat")]
    Retreat,
    #[value(name = "fitness")]
    Fitness,
    #[value(name = "course")]
    Course,
    #[value(name = "course-class")]
    CourseClass,
    #[value(name = "semester")]
    Semester,
    #[value(name = "recital")]
    Recital,
}

impl SessionType {
    pub(crate) fn as_api_value(&self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::SpecialEvent => "special-event",
            Self::SpecialEventNew => "special-event-new",
            Self::Retreat => "retreat",
            Self::Fitness => "fitness",
            Self::Course => "course",
            Self::CourseClass => "course-class",
            Self::Semester => "semester",
            Self::Recital => "recital",
        }
    }
}

#[derive(Clone, Debug, Args)]
pub(crate) struct PaginationArgs {
    #[arg(long, default_value_t = 0)]
    pub(crate) page: u32,

    #[arg(long = "page-size", default_value_t = 100)]
    pub(crate) page_size: u32,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct SortArgs {
    #[arg(long = "sort-order", value_enum)]
    pub(crate) sort_order: Option<SortOrder>,

    #[arg(long = "sort-by")]
    pub(crate) sort_by: Option<String>,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct DateWindowArgs {
    #[arg(long = "start-after")]
    pub(crate) start_after: Option<String>,

    #[arg(long = "start-before")]
    pub(crate) start_before: Option<String>,

    #[arg(long = "end-after")]
    pub(crate) end_after: Option<String>,

    #[arg(long = "end-before")]
    pub(crate) end_before: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ListAddressesArgs {
    #[command(flatten)]
    pub(crate) pagination: PaginationArgs,

    #[command(flatten)]
    pub(crate) sort: SortArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ActiveMembershipsArgs {
    #[command(flatten)]
    pub(crate) pagination: PaginationArgs,

    #[arg(long)]
    pub(crate) include_frozen: bool,
}

#[derive(Debug, Args)]
pub(crate) struct ListHostLocationsArgs {
    #[command(flatten)]
    pub(crate) pagination: PaginationArgs,

    #[command(flatten)]
    pub(crate) sort: SortArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ListHostMembershipsArgs {
    #[command(flatten)]
    pub(crate) pagination: PaginationArgs,

    #[command(flatten)]
    pub(crate) sort: SortArgs,

    #[arg(long)]
    pub(crate) include_disabled: bool,

    #[arg(long)]
    pub(crate) only_featured: bool,

    #[arg(long = "compatible-with-session-id")]
    pub(crate) compatible_with_session_id: Option<u64>,

    #[arg(long = "compatible-with-appointment-id")]
    pub(crate) compatible_with_appointment_id: Option<u64>,
}

#[derive(Debug, Args)]
pub(crate) struct ListHostSessionsArgs {
    #[command(flatten)]
    pub(crate) pagination: PaginationArgs,

    #[command(flatten)]
    pub(crate) sort: SortArgs,

    #[arg(long)]
    pub(crate) include_cancelled: bool,

    #[arg(long = "type")]
    pub(crate) session_types: Vec<SessionType>,

    #[arg(long = "teacher-id")]
    pub(crate) teacher_id: Option<u64>,

    #[arg(long = "location-id")]
    pub(crate) location_id: Option<u64>,

    #[command(flatten)]
    pub(crate) window: DateWindowArgs,
}

#[derive(Debug, Args)]
pub(crate) struct ListMemberSessionsArgs {
    #[command(flatten)]
    pub(crate) pagination: PaginationArgs,

    #[command(flatten)]
    pub(crate) sort: SortArgs,

    #[arg(long = "start-after")]
    pub(crate) start_after: Option<String>,

    #[arg(long = "end-after")]
    pub(crate) end_after: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct IdArgs {
    #[arg(value_name = "ID")]
    pub(crate) id: u64,
}

#[derive(Debug, Args)]
pub(crate) struct UpdateByIdJsonArgs {
    #[arg(value_name = "ID")]
    pub(crate) id: u64,

    #[command(flatten)]
    pub(crate) body: JsonBodyArgs,
}

pub(crate) fn build_pagination_query(pagination: &PaginationArgs, sort: &SortArgs) -> Vec<(String, String)> {
    let mut query = vec![
        ("page".into(), pagination.page.to_string()),
        ("pageSize".into(), pagination.page_size.to_string()),
    ];

    if let Some(sort_order) = sort.sort_order {
        query.push(("sortOrder".into(), sort_order.as_api_value().into()));
    }
    if let Some(sort_by) = sort.sort_by.as_ref() {
        query.push(("sortBy".into(), sort_by.clone()));
    }

    query
}

pub(crate) fn push_bool_query(query: &mut Vec<(String, String)>, key: &str, value: bool) {
    if value {
        query.push((key.into(), "true".into()));
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
