mod models;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use serde::de::DeserializeOwned;
use switchboard_core::{
    CoverageStatus, Error, ExecutionTarget, Failure, PlannedAction, ReadCoverage, Result, ToolArguments, ToolName,
    ToolOutput, ToolRef, ToolRefKind,
};

use crate::github::{
    api::{bounded, encode, repository},
    context::models::*,
    GitHubAdapter,
};

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum ContextSection {
    Discussion,
    Files,
    LinkedPrs,
    Checks,
}

impl ContextSection {
    fn parse(value: &str, pull_request: bool) -> Result<Self> {
        match value {
            "discussion" => Ok(Self::Discussion),
            "linked-prs" => Ok(Self::LinkedPrs),
            "files" if pull_request => Ok(Self::Files),
            "checks" if pull_request => Ok(Self::Checks),
            _ => Err(Error::InvalidArguments(format!("unsupported context include: {value}"))),
        }
    }
}

pub(super) struct ContextOptions {
    repo: String,
    number: u64,
    pull_request: bool,
    includes: BTreeSet<ContextSection>,
    limit: u64,
    body_limit: usize,
}

impl ContextOptions {
    pub(super) fn parse(tool: &ToolName, args: &ToolArguments) -> Result<Self> {
        let pull_request = tool.as_str() == "github.pull_request.context";
        let defaults = if pull_request {
            "discussion,files,checks"
        } else {
            "discussion,linked-prs"
        };
        let includes = args
            .value("include")
            .unwrap_or(defaults)
            .split(',')
            .filter(|part| !part.is_empty() && *part != "none")
            .map(|value| ContextSection::parse(value, pull_request))
            .collect::<Result<BTreeSet<_>>>()?;
        Ok(Self {
            repo: repository(args)?,
            number: bounded(args, "number", 0, 1, i32::MAX as u64)?,
            pull_request,
            includes,
            limit: bounded(args, "limit", 10, 1, 100)?,
            body_limit: bounded(args, "body-limit", 2000, 0, 20_000)? as usize,
        })
    }

    fn query(&self, details: bool) -> String {
        let resource = if self.pull_request { "pullRequest" } else { "issue" };
        let mut fields = "number title url state body updatedAt".to_owned();
        if self.pull_request {
            fields.push_str(" headRefOid headRefName baseRefName isDraft reviewDecision mergeable");
        }
        if details {
            // Fetch the core again with the optional sections, so returned checks
            // and files always belong to the reported PR head in this response.
            if self.includes.contains(&ContextSection::Discussion) {
                fields.push_str(" comments(last:$limit) {totalCount pageInfo {hasNextPage hasPreviousPage} nodes {author {login} body createdAt url}}");
                if self.pull_request {
                    fields.push_str(" reviews(last:$limit) {totalCount pageInfo {hasNextPage hasPreviousPage} nodes {author {login} body submittedAt url state}}");
                }
            }
            if self.includes.contains(&ContextSection::Files) {
                fields.push_str(" files(first:$limit) {totalCount pageInfo {hasNextPage hasPreviousPage} nodes {path additions deletions}}");
            }
            if self.includes.contains(&ContextSection::LinkedPrs) {
                if self.pull_request {
                    fields.push_str(" timelineItems(last:$limit,itemTypes:[CROSS_REFERENCED_EVENT]) {totalCount pageInfo {hasNextPage hasPreviousPage} nodes {... on CrossReferencedEvent {source {__typename ... on PullRequest {number title url state repository {nameWithOwner}}}}}}");
                } else {
                    fields.push_str(" closedByPullRequestsReferences(first:$limit,includeClosedPrs:true) {totalCount pageInfo {hasNextPage hasPreviousPage} nodes {number title url state repository {nameWithOwner}}}");
                }
            }
            if self.includes.contains(&ContextSection::Checks) {
                fields.push_str(" commits(last:1) {nodes {commit {oid statusCheckRollup {state contexts(first:$limit) {totalCount pageInfo {hasNextPage hasPreviousPage} nodes {__typename ... on CheckRun {name status conclusion detailsUrl} ... on StatusContext {context state targetUrl}}}}}}}");
            }
        }
        let limit = if details { ",$limit:Int!" } else { "" };
        format!("query($owner:String!,$repo:String!,$number:Int!{limit}) {{repository(owner:$owner,name:$repo) {{item:{resource}(number:$number) {{{fields}}}}}}}")
    }
}

impl GitHubAdapter {
    fn context_query<T: DeserializeOwned>(
        &self,
        target: &ExecutionTarget,
        options: &ContextOptions,
        details: bool,
    ) -> Result<T> {
        let (owner, repo) = options
            .repo
            .split_once('/')
            .ok_or_else(|| Error::InvalidArguments("invalid repository".into()))?;
        let mut args = vec![
            "api".into(),
            "graphql".into(),
            "-f".into(),
            format!("query={}", options.query(details)),
            "-f".into(),
            format!("owner={owner}"),
            "-f".into(),
            format!("repo={repo}"),
            "-F".into(),
            format!("number={}", options.number),
        ];
        if details {
            args.extend(["-F".into(), format!("limit={}", options.limit)]);
        }
        let envelope: GraphResponse<T> = self.read_json(target, args)?;
        if !envelope.errors.is_empty() {
            return Err(Error::Execution(format!(
                "GitHub context query failed: {}",
                envelope
                    .errors
                    .iter()
                    .map(|error| error.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
        envelope
            .data
            .and_then(|data| data.repository)
            .and_then(|repo| repo.item)
            .ok_or_else(|| Error::Execution("GitHub context object was unavailable".into()))
    }

    pub(super) fn context(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
        let options = ContextOptions::parse(&action.tool, &action.args)?;
        // Optional GraphQL fields can fail permission checks. Establish a small
        // usable core first, then preserve it if the detail query fails.
        let core: ContextNode = self.context_query(target, &options, false)?;
        let mut result = core.into_context(&options, false)?;
        let mut failures = Vec::new();
        if !options.includes.is_empty() {
            let detail = self
                .context_query::<ContextNode>(target, &options, true)
                .and_then(|node| node.into_context(&options, true));
            match detail {
                Ok(context) => result = context,
                Err(error) => failures.push(Failure::from_error(&error).with_namespace(action.namespace.clone())),
            }
        }
        let coverage = if !failures.is_empty() {
            CoverageStatus::Unknown
        } else {
            result.coverage()
        };
        let reference = ToolRef::new(
            target.namespace.provider.clone(),
            action.namespace.clone(),
            if options.pull_request {
                ToolRefKind::PullRequest
            } else {
                ToolRefKind::Issue
            },
            result.number.to_string(),
        )?
        .with_parent_id(&options.repo)?
        .with_web_url(&result.url)?;
        let mut output = ToolOutput::new(
            action.tool.clone(),
            action.namespace.clone(),
            format!(
                "{}#{}: {} ({})",
                options.repo, result.number, result.title, result.state
            ),
        )
        .with_value_field("context", encode(&result)?)
        .with_value_field("failures", encode(&failures)?)
        .with_ref(reference);
        output.coverage = Some(ReadCoverage {
            status: coverage,
            next_cursor: None,
        });
        Ok(output)
    }
}
