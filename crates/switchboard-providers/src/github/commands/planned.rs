use switchboard_core::{Error, ResolvedNamespace, Result, ToolRequest};

use crate::cli::CliCommandHandler;

pub(crate) const PULL_REQUEST_COMMENT_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "pull_request_comment",
    summarize: summarize_pull_request_comment,
    build_args: None,
    decode: None,
};

pub(crate) const ISSUE_COMMENT_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "issue_comment",
    summarize: summarize_issue_comment,
    build_args: None,
    decode: None,
};

pub(crate) const REPOSITORY_SEARCH_HANDLER: CliCommandHandler = CliCommandHandler {
    id: "repository_search",
    summarize: summarize_repository_search,
    build_args: None,
    decode: None,
};

fn summarize_pull_request_comment(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let repo = request
        .args
        .value("repo")
        .ok_or_else(|| Error::InvalidArguments(format!("missing required argument --repo for {}", request.tool)))?;
    let number = request
        .args
        .value("number")
        .ok_or_else(|| Error::InvalidArguments(format!("missing required argument --number for {}", request.tool)))?;
    Ok(format!(
        "Draft comment for pull request {repo}#{number} in {}",
        namespace.id
    ))
}

fn summarize_issue_comment(namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let repo = request
        .args
        .value("repo")
        .ok_or_else(|| Error::InvalidArguments(format!("missing required argument --repo for {}", request.tool)))?;
    let number = request
        .args
        .value("number")
        .ok_or_else(|| Error::InvalidArguments(format!("missing required argument --number for {}", request.tool)))?;
    Ok(format!("Draft comment for issue {repo}#{number} in {}", namespace.id))
}

fn summarize_repository_search(_namespace: &ResolvedNamespace, request: &ToolRequest) -> Result<String> {
    let query = request
        .args
        .value("query")
        .ok_or_else(|| Error::InvalidArguments(format!("missing required argument --query for {}", request.tool)))?;
    Ok(format!("Search GitHub repositories matching {query:?}"))
}
