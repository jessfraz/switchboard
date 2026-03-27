mod commands;
mod materializer;

use std::sync::OnceLock;

use switchboard_core::{
    Adapter, Error, ExecutionTarget, PlannedAction, PlanningTarget, ProviderKind, Result, ToolDescriptor, ToolKind,
    ToolOutput, ToolRequest,
};

use crate::{
    cli::{CliProviderBackend, CliProviderCatalog},
    github::{commands::HANDLERS, materializer::DefaultGitHubCliMaterializer},
};
const MANIFEST_JSON: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/manifests/github.json"));
static CATALOG: OnceLock<CliProviderCatalog> = OnceLock::new();

pub struct GitHubAdapter {
    backend: CliProviderBackend,
}

impl Default for GitHubAdapter {
    fn default() -> Self {
        Self {
            backend: CliProviderBackend::new(Box::new(DefaultGitHubCliMaterializer)),
        }
    }
}

impl GitHubAdapter {
    fn catalog() -> &'static CliProviderCatalog {
        CATALOG.get_or_init(|| {
            CliProviderCatalog::from_embedded(MANIFEST_JSON, HANDLERS)
                .expect("github provider manifest should be valid")
        })
    }

    fn find_command(tool: &str) -> Option<&'static crate::cli::CliCommandSpec> {
        Self::catalog().find_command(tool)
    }

    fn stub_output(target: &ExecutionTarget, action: &PlannedAction) -> ToolOutput {
        ToolOutput::new(
            action.tool.clone(),
            action.namespace.clone(),
            format!("{} via {} (stub)", action.summary, action.backend),
        )
        .with_field("status", "stub")
        .with_field("backend", action.backend.to_string())
        .with_field("auth", target.auth.id.to_string())
        .with_field("note", "github command execution is not wired yet")
    }
}

impl Adapter for GitHubAdapter {
    fn provider(&self) -> ProviderKind {
        ProviderKind::GitHub
    }

    fn tools(&self) -> &'static [ToolDescriptor] {
        Self::catalog().tools()
    }

    fn plan(
        &self,
        target: &PlanningTarget,
        request: &ToolRequest,
        descriptor: &'static ToolDescriptor,
    ) -> Result<PlannedAction> {
        let command = Self::find_command(request.tool.as_str())
            .ok_or_else(|| Error::UnsupportedTool(request.tool.to_string()))?;
        let summary = (command.summarize)(&target.namespace, request)?;
        Ok(PlannedAction::new(
            request,
            target,
            descriptor.kind,
            summary,
            descriptor.backend,
        ))
    }

    fn execute(&self, target: &ExecutionTarget, action: &PlannedAction) -> Result<ToolOutput> {
        if let Some(command) = Self::find_command(action.tool.as_str()) {
            if let Some(executable) = command.executable.as_ref() {
                return self.backend.execute(target, action, executable);
            }

            if matches!(action.kind, ToolKind::Write) {
                return Err(Error::NotImplemented(format!(
                    "{} apply path is not wired to GitHub yet",
                    action.tool
                )));
            }

            return Ok(Self::stub_output(target, action));
        }

        Err(Error::UnsupportedTool(action.tool.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use std::{env, path::PathBuf};

    use serde_json::Value;
    use switchboard_core::{
        Adapter, AuthKind, AuthSecretRefs, ExecutionMode, ExecutionTarget, PlanningTarget, ProviderKind, ResolvedAuth,
        ResolvedCredentials, ResolvedNamespace, SecretRef, ToolArgument, ToolRequest,
    };

    use crate::{
        github::GitHubAdapter,
        test_support::{lock_env, TempScript},
    };

    const NOTIFICATIONS_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/github-notifications.json"
    ));
    const PR_SEARCH_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/github-pull-request-search.json"
    ));
    const PR_READ_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/github-pull-request-read.json"
    ));
    const ISSUE_READ_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/github-issue-read.json"
    ));
    const REPO_VIEW_FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/cli/github-repo-view.json"
    ));
    const GITHUB_SCRIPT_TEMPLATE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/scripts/gh-test.sh"
    ));

    #[test]
    fn notifications_list_executes_through_generic_cli_runtime() {
        let _env_guard = lock_env();
        let script = TempScript::new("gh-test", &render_github_script());
        env::set_var("SWITCHBOARD_GH_BIN", script.path());

        let adapter = GitHubAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "github.notifications.list",
            "github.personal",
            ExecutionMode::Auto,
            vec![
                ToolArgument::flag("all").expect("flag should build"),
                ToolArgument::option("per_page", "50").expect("option should build"),
            ],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let output = adapter
            .execute(&execution_target(), &action)
            .expect("execution should succeed");

        assert_eq!(output.summary, "Listed 2 GitHub notifications for github.personal");
        assert_eq!(output.fields.get("count"), Some(&serde_json::json!(2)));
        assert_eq!(output.refs.len(), 2);
        assert_eq!(output.refs[0].kind, switchboard_core::ToolRefKind::Notification);
        assert_eq!(
            output
                .fields
                .get("notifications")
                .and_then(Value::as_array)
                .and_then(|notifications| notifications.first())
                .and_then(|notification| notification.get("repository"))
                .and_then(Value::as_str),
            Some("KittyCAD/modeling-app")
        );

        let captured = script.capture_contents();
        assert!(captured.contains("GH_CONFIG_DIR=/tmp/gh-personal"));
        assert!(captured.contains("GH_TOKEN=ghp-test-token"));
        assert!(captured.contains("GITHUB_TOKEN="));
        assert!(captured.contains("ARGV=api notifications -F all=true -F per_page=50"));
    }

    #[test]
    fn pull_request_search_executes_through_generic_cli_runtime() {
        let _env_guard = lock_env();
        let script = TempScript::new("gh-test", &render_github_script());
        env::set_var("SWITCHBOARD_GH_BIN", script.path());

        let adapter = GitHubAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "github.pull_request.search",
            "github.personal",
            ExecutionMode::Auto,
            vec![
                ToolArgument::option("query", "is:open review-requested:@me").expect("query should build"),
                ToolArgument::option("limit", "10").expect("limit should build"),
                ToolArgument::option("repo", "openai/codex").expect("repo should build"),
            ],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let output = adapter
            .execute(&execution_target(), &action)
            .expect("execution should succeed");

        assert_eq!(output.summary, "Found 2 GitHub pull requests for github.personal");
        assert_eq!(output.fields.get("count"), Some(&serde_json::json!(2)));
        assert_eq!(output.refs.len(), 2);
        assert_eq!(output.refs[0].kind, switchboard_core::ToolRefKind::PullRequest);
        assert_eq!(output.refs[0].parent_id.as_deref(), Some("openai/codex"));
        assert_eq!(
            output
                .fields
                .get("pull_requests")
                .and_then(Value::as_array)
                .and_then(|pull_requests| pull_requests.first())
                .and_then(|pull_request| pull_request.get("repository"))
                .and_then(Value::as_str),
            Some("openai/codex")
        );

        let captured = script.capture_contents();
        assert!(captured.contains("ARGV=search prs is:open review-requested:@me --json"));
        assert!(captured.contains("--limit 10"));
        assert!(captured.contains("--repo openai/codex"));
    }

    #[test]
    fn pull_request_read_executes_through_generic_cli_runtime() {
        let _env_guard = lock_env();
        let script = TempScript::new("gh-test", &render_github_script());
        env::set_var("SWITCHBOARD_GH_BIN", script.path());

        let adapter = GitHubAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "github.pull_request.read",
            "github.personal",
            ExecutionMode::Auto,
            vec![
                ToolArgument::option("repo", "openai/codex").expect("repo should build"),
                ToolArgument::option("number", "1382").expect("number should build"),
            ],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let output = adapter
            .execute(&execution_target(), &action)
            .expect("execution should succeed");

        assert_eq!(
            output.summary,
            "Read GitHub pull request \"Tighten agenda aggregation for personal + work calendars\" for github.personal"
        );
        assert_eq!(output.refs.len(), 1);
        assert_eq!(output.refs[0].kind, switchboard_core::ToolRefKind::PullRequest);
        assert_eq!(output.refs[0].parent_id.as_deref(), Some("openai/codex"));
        assert_eq!(
            output
                .fields
                .get("pull_request")
                .and_then(|pull_request| pull_request.get("number"))
                .and_then(Value::as_u64),
            Some(1382)
        );

        let captured = script.capture_contents();
        assert!(captured.contains("ARGV=pr view 1382 --repo openai/codex --json"));
    }

    #[test]
    fn issue_read_executes_through_generic_cli_runtime() {
        let _env_guard = lock_env();
        let script = TempScript::new("gh-test", &render_github_script());
        env::set_var("SWITCHBOARD_GH_BIN", script.path());

        let adapter = GitHubAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "github.issue.read",
            "github.personal",
            ExecutionMode::Auto,
            vec![
                ToolArgument::option("repo", "openai/codex").expect("repo should build"),
                ToolArgument::option("number", "77").expect("number should build"),
            ],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let output = adapter
            .execute(&execution_target(), &action)
            .expect("execution should succeed");

        assert_eq!(
            output.summary,
            "Read GitHub issue \"Support stable refs in GitHub issue reads\" for github.personal"
        );
        assert_eq!(output.refs.len(), 1);
        assert_eq!(output.refs[0].kind, switchboard_core::ToolRefKind::Issue);
        assert_eq!(output.refs[0].parent_id.as_deref(), Some("openai/codex"));
        assert_eq!(
            output
                .fields
                .get("issue")
                .and_then(|issue| issue.get("number"))
                .and_then(Value::as_u64),
            Some(77)
        );

        let captured = script.capture_contents();
        assert!(captured.contains("ARGV=issue view 77 --repo openai/codex --json"));
    }

    #[test]
    fn raw_cli_read_executes_arbitrary_gh_command() {
        let _env_guard = lock_env();
        let script = TempScript::new("gh-test", &render_github_script());
        env::set_var("SWITCHBOARD_GH_BIN", script.path());

        let adapter = GitHubAdapter::default();
        let planning = planning_target();
        let request = ToolRequest::new(
            "github.cli.read",
            "github.personal",
            ExecutionMode::Auto,
            vec![ToolArgument::option(
                "argv-json",
                serde_json::json!(["repo", "view", "openai/codex", "--json", "name,owner,isPrivate"]).to_string(),
            )
            .expect("option should build")],
        )
        .expect("request should build");
        let descriptor = adapter.find_tool(&request.tool).expect("tool should exist");
        let action = adapter
            .plan(&planning, &request, descriptor)
            .expect("plan should succeed");
        let output = adapter
            .execute(&execution_target(), &action)
            .expect("execution should succeed");

        assert_eq!(
            output
                .fields
                .get("response")
                .and_then(|response| response.get("name"))
                .and_then(Value::as_str),
            Some("codex")
        );
        assert_eq!(
            output
                .fields
                .get("response")
                .and_then(|response| response.get("owner"))
                .and_then(|owner| owner.get("login"))
                .and_then(Value::as_str),
            Some("openai")
        );

        let captured = script.capture_contents();
        assert!(captured.contains("ARGV=repo view openai/codex --json name,owner,isPrivate"));
    }

    fn render_github_script() -> String {
        GITHUB_SCRIPT_TEMPLATE
            .replace("__NOTIFICATIONS_FIXTURE__", NOTIFICATIONS_FIXTURE)
            .replace("__PR_SEARCH_FIXTURE__", PR_SEARCH_FIXTURE)
            .replace("__PR_READ_FIXTURE__", PR_READ_FIXTURE)
            .replace("__ISSUE_READ_FIXTURE__", ISSUE_READ_FIXTURE)
            .replace("__REPO_VIEW_FIXTURE__", REPO_VIEW_FIXTURE)
    }

    fn planning_target() -> PlanningTarget {
        PlanningTarget {
            namespace: ResolvedNamespace::new(
                "github.personal",
                ProviderKind::GitHub,
                "GitHub personal",
                "github.personal_auth",
                false,
                Some(PathBuf::from("/tmp/gh-personal")),
            )
            .expect("namespace should build"),
            auth: ResolvedAuth::new(
                "github.personal_auth",
                ProviderKind::GitHub,
                AuthKind::GitHubToken,
                "jessfraz",
                AuthSecretRefs::GitHubToken {
                    token: SecretRef::new("github.personal_token").expect("secret ref should build"),
                },
            )
            .expect("auth should build"),
        }
    }

    fn execution_target() -> ExecutionTarget {
        ExecutionTarget {
            namespace: planning_target().namespace,
            auth: planning_target().auth,
            credentials: ResolvedCredentials::GitHubToken {
                token: "ghp-test-token".to_owned().into(),
            },
        }
    }
}
