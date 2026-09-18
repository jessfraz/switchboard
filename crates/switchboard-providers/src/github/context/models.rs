use serde::{Deserialize, Serialize};
use switchboard_core::{CoverageStatus, Error, Result};

use crate::github::context::{ContextOptions, ContextSection};

#[derive(Deserialize)]
pub(super) struct GraphResponse<T> {
    pub data: Option<GraphData<T>>,
    #[serde(default)]
    pub errors: Vec<GraphError>,
}
#[derive(Deserialize)]
pub(super) struct GraphError {
    pub message: String,
}
#[derive(Deserialize)]
pub(super) struct GraphData<T> {
    pub repository: Option<GraphRepository<T>>,
}
#[derive(Deserialize)]
pub(super) struct GraphRepository<T> {
    pub item: Option<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ContextNode {
    number: u64,
    title: String,
    url: String,
    state: String,
    body: String,
    updated_at: String,
    head_ref_oid: Option<String>,
    head_ref_name: Option<String>,
    base_ref_name: Option<String>,
    is_draft: Option<bool>,
    review_decision: Option<String>,
    mergeable: Option<String>,
    comments: Option<Connection<Comment>>,
    reviews: Option<Connection<Comment>>,
    files: Option<Connection<ChangedFile>>,
    timeline_items: Option<Connection<CrossReference>>,
    closed_by_pull_requests_references: Option<Connection<ClosingPr>>,
    commits: Option<Commits>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Connection<T> {
    total_count: usize,
    page_info: PageInfo,
    nodes: Vec<Option<T>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    has_previous_page: bool,
}

#[derive(Serialize)]
pub(super) struct Section<T> {
    pub items: Vec<T>,
    pub total: usize,
    pub coverage: CoverageStatus,
}
impl<T> Connection<T> {
    fn section(self, limit: usize) -> Result<Section<T>> {
        if self.nodes.len() > limit || self.nodes.len() > self.total_count || self.nodes.iter().any(Option::is_none) {
            return Err(Error::Execution(
                "GitHub returned an incomplete or oversized context section".into(),
            ));
        }
        let coverage = if self.page_info.has_next_page
            || self.page_info.has_previous_page
            || self.total_count > self.nodes.len()
        {
            CoverageStatus::Truncated
        } else {
            CoverageStatus::Complete
        };
        Ok(Section {
            items: self.nodes.into_iter().flatten().collect(),
            total: self.total_count,
            coverage,
        })
    }
}

#[derive(Deserialize, Serialize)]
pub(super) struct Actor {
    login: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Comment {
    author: Option<Actor>,
    body: String,
    #[serde(default)]
    body_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    submitted_at: Option<String>,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
}
#[derive(Deserialize, Serialize)]
pub(super) struct ChangedFile {
    path: String,
    additions: u64,
    deletions: u64,
}
#[derive(Deserialize)]
struct CrossReference {
    source: Option<LinkedSubject>,
}
#[derive(Deserialize)]
#[serde(tag = "__typename")]
enum LinkedSubject {
    PullRequest {
        number: u64,
        title: String,
        url: String,
        state: String,
        repository: RepositoryName,
    },
    #[serde(other)]
    Other,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryName {
    name_with_owner: String,
}
#[derive(Deserialize)]
struct ClosingPr {
    number: u64,
    title: String,
    url: String,
    state: String,
    repository: RepositoryName,
}
#[derive(Serialize)]
pub(super) struct LinkedPr {
    number: u64,
    title: String,
    url: String,
    state: String,
    repository: String,
}
#[derive(Deserialize)]
struct Commits {
    nodes: Vec<CommitNode>,
}
#[derive(Deserialize)]
struct CommitNode {
    commit: Commit,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Commit {
    oid: String,
    status_check_rollup: Option<Rollup>,
}
#[derive(Deserialize)]
struct Rollup {
    state: String,
    contexts: Connection<Check>,
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "__typename")]
pub(super) enum Check {
    CheckRun {
        name: String,
        status: String,
        conclusion: Option<String>,
        #[serde(rename = "detailsUrl")]
        url: Option<String>,
    },
    StatusContext {
        context: String,
        state: String,
        #[serde(rename = "targetUrl")]
        url: Option<String>,
    },
}

#[derive(Serialize)]
pub(super) struct Context {
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub state: String,
    body: String,
    body_truncated: bool,
    updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    head_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    head_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    draft: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    review_decision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mergeable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    discussion: Option<Section<Comment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reviews: Option<Section<Comment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    files: Option<Section<ChangedFile>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    linked_prs: Option<Section<LinkedPr>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    linked_prs_relation: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checks: Option<Section<Check>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    check_state: Option<String>,
}

impl ContextNode {
    pub(super) fn into_context(self, options: &ContextOptions, details: bool) -> Result<Context> {
        if self.number != options.number
            || self.url.is_empty()
            || (options.pull_request && self.head_ref_oid.as_deref().map_or(true, str::is_empty))
        {
            return Err(Error::Execution(
                "GitHub returned a different or incomplete context object".into(),
            ));
        }
        let requested = |section| details && options.includes.contains(&section);
        let mut discussion = section(self.comments, requested(ContextSection::Discussion), options.limit)?;
        let mut reviews = section(
            self.reviews,
            requested(ContextSection::Discussion) && options.pull_request,
            options.limit,
        )?;
        for comments in [&mut discussion, &mut reviews].into_iter().flatten() {
            for comment in &mut comments.items {
                comment.body_truncated = truncate(&mut comment.body, options.body_limit);
            }
        }
        let files = section(self.files, requested(ContextSection::Files), options.limit)?;
        let links = section(
            self.timeline_items,
            requested(ContextSection::LinkedPrs) && options.pull_request,
            options.limit,
        )?;
        let mut linked_prs = links.map(|links| Section {
            items: links
                .items
                .into_iter()
                .filter_map(|item| match item.source {
                    Some(LinkedSubject::PullRequest {
                        number,
                        title,
                        url,
                        state,
                        repository,
                    }) => Some(LinkedPr {
                        number,
                        title,
                        url,
                        state,
                        repository: repository.name_with_owner,
                    }),
                    _ => None,
                })
                .collect(),
            // This is the count/coverage of cross-reference events inspected,
            // not a claim that every issue/PR mentioning this item was found.
            total: links.total,
            coverage: links.coverage,
        });
        if !options.pull_request {
            linked_prs = section(
                self.closed_by_pull_requests_references,
                requested(ContextSection::LinkedPrs),
                options.limit,
            )?
            .map(|links| Section {
                items: links
                    .items
                    .into_iter()
                    .map(|pr| LinkedPr {
                        number: pr.number,
                        title: pr.title,
                        url: pr.url,
                        state: pr.state,
                        repository: pr.repository.name_with_owner,
                    })
                    .collect(),
                total: links.total,
                coverage: links.coverage,
            });
        }
        let linked_prs_relation = linked_prs.as_ref().map(|_| {
            if options.pull_request {
                "recent_cross_references"
            } else {
                "closing_references"
            }
        });
        let (checks, check_state) = if requested(ContextSection::Checks) {
            let commits = self
                .commits
                .ok_or_else(|| Error::Execution("GitHub omitted requested checks".into()))?;
            if commits.nodes.len() != 1 {
                return Err(Error::Execution("GitHub omitted the PR head commit".into()));
            }
            let commit = commits
                .nodes
                .into_iter()
                .next()
                .ok_or_else(|| Error::Execution("GitHub omitted the PR head commit".into()))?
                .commit;
            if Some(&commit.oid) != self.head_ref_oid.as_ref() {
                return Err(Error::Execution("GitHub checks do not match the PR head".into()));
            }
            match commit.status_check_rollup {
                Some(rollup) => (
                    Some(rollup.contexts.section(options.limit as usize)?),
                    Some(rollup.state),
                ),
                None => (
                    Some(Section {
                        items: vec![],
                        total: 0,
                        coverage: CoverageStatus::Complete,
                    }),
                    None,
                ),
            }
        } else {
            (None, None)
        };
        let mut body = self.body;
        let body_truncated = truncate(&mut body, options.body_limit);
        Ok(Context {
            repository: options.repo.clone(),
            number: self.number,
            title: self.title,
            url: self.url,
            state: self.state,
            body,
            body_truncated,
            updated_at: self.updated_at,
            head_sha: self.head_ref_oid,
            head_branch: self.head_ref_name,
            base_branch: self.base_ref_name,
            draft: self.is_draft,
            review_decision: self.review_decision,
            mergeable: self.mergeable,
            discussion,
            reviews,
            files,
            linked_prs,
            linked_prs_relation,
            checks,
            check_state,
        })
    }
}

fn section<T>(connection: Option<Connection<T>>, requested: bool, limit: u64) -> Result<Option<Section<T>>> {
    if !requested {
        return Ok(None);
    }
    connection
        .ok_or_else(|| Error::Execution("GitHub omitted a requested context section".into()))?
        .section(limit as usize)
        .map(Some)
}

fn truncate(text: &mut String, limit: usize) -> bool {
    if let Some((index, _)) = text.char_indices().nth(limit) {
        text.truncate(index);
        true
    } else {
        false
    }
}

impl Context {
    pub(super) fn coverage(&self) -> CoverageStatus {
        let truncated =
            self.body_truncated
                || self.discussion.as_ref().is_some_and(|s| {
                    s.coverage != CoverageStatus::Complete || s.items.iter().any(|c| c.body_truncated)
                })
                || self.reviews.as_ref().is_some_and(|s| {
                    s.coverage != CoverageStatus::Complete || s.items.iter().any(|c| c.body_truncated)
                })
                || self
                    .files
                    .as_ref()
                    .is_some_and(|s| s.coverage != CoverageStatus::Complete)
                || self
                    .linked_prs
                    .as_ref()
                    .is_some_and(|s| s.coverage != CoverageStatus::Complete)
                || self
                    .checks
                    .as_ref()
                    .is_some_and(|s| s.coverage != CoverageStatus::Complete);
        if truncated {
            CoverageStatus::Truncated
        } else {
            CoverageStatus::Complete
        }
    }
}
