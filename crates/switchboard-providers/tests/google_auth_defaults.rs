use std::collections::BTreeSet;

use switchboard_core::{
    Adapter, AuthScopeProfile, AuthSecretRefs, ExecutionMode, PlanningTarget, ProviderKind, ResolvedAuth,
    ResolvedNamespace, ToolArgument, ToolRequest,
};
use switchboard_providers::GoogleWorkspaceAdapter;

#[test]
fn bare_login_requests_all_previously_supported_scopes_for_both_profiles() {
    let adapter = GoogleWorkspaceAdapter::new().expect("Google catalog");
    let expected = BTreeSet::from([
        "https://www.googleapis.com/auth/drive",
        "https://www.googleapis.com/auth/spreadsheets",
        "https://www.googleapis.com/auth/gmail.modify",
        "https://www.googleapis.com/auth/calendar",
        "https://www.googleapis.com/auth/documents",
        "https://www.googleapis.com/auth/presentations",
        "https://www.googleapis.com/auth/tasks",
        "https://www.googleapis.com/auth/contacts",
        "https://www.googleapis.com/auth/cloud-identity.groups",
        "https://www.googleapis.com/auth/cloud-platform",
        "https://www.googleapis.com/auth/admin.directory.user",
        "https://www.googleapis.com/auth/admin.directory.orgunit",
        "https://www.googleapis.com/auth/admin.directory.group",
        "https://www.googleapis.com/auth/apps.groups.settings",
        "https://www.googleapis.com/auth/admin.reports.audit.readonly",
        "https://www.googleapis.com/auth/admin.reports.usage.readonly",
    ]);

    for profile in [AuthScopeProfile::Standard, AuthScopeProfile::WorkspaceAdmin] {
        let target = target(profile);
        let request = request(&["auth", "login"]);
        let descriptor = adapter.find_tool(&request.tool).expect("raw Google write");
        let action = adapter.plan(&target, &request, descriptor).expect("login plan");
        let argv: Vec<String> =
            serde_json::from_str(action.args.value("argv-json").expect("planned argv")).expect("argv JSON");
        assert_eq!(&argv[..3], ["auth", "login", "--scopes"]);
        assert_eq!(argv.len(), 4);
        let scopes = argv[3].split(',').collect::<Vec<_>>();
        let actual = scopes.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(actual, expected);
        assert_eq!(scopes.len(), actual.len(), "duplicate OAuth scopes");
    }
}

#[test]
fn consumer_accounts_receive_standard_scopes_without_workspace_admin_scopes() {
    let adapter = GoogleWorkspaceAdapter::new().expect("Google catalog");
    let expected = BTreeSet::from([
        "https://www.googleapis.com/auth/drive",
        "https://www.googleapis.com/auth/spreadsheets",
        "https://www.googleapis.com/auth/gmail.modify",
        "https://www.googleapis.com/auth/calendar",
        "https://www.googleapis.com/auth/documents",
        "https://www.googleapis.com/auth/presentations",
        "https://www.googleapis.com/auth/tasks",
        "https://www.googleapis.com/auth/contacts",
        "https://www.googleapis.com/auth/cloud-platform",
    ]);
    for profile in [AuthScopeProfile::Standard, AuthScopeProfile::WorkspaceAdmin] {
        for account in ["person@gmail.com", "person@googlemail.com", "Person@GMAIL.COM"] {
            for namespace_label in [account, "personal"] {
                let mut target = target(profile);
                target.namespace.account_label = namespace_label.into();
                target.auth = ResolvedAuth::new("google_test", account, AuthSecretRefs::GoogleCli)
                    .expect("consumer auth identity");
                let request = request(&["auth", "login"]);
                let descriptor = adapter.find_tool(&request.tool).expect("raw Google write");
                let action = adapter
                    .plan(&target, &request, descriptor)
                    .expect("consumer login plan");
                let argv: Vec<String> =
                    serde_json::from_str(action.args.value("argv-json").expect("planned argv")).expect("argv JSON");
                assert_eq!(&argv[..3], ["auth", "login", "--scopes"]);
                assert_eq!(argv.len(), 4);
                assert_eq!(argv[3].split(',').collect::<BTreeSet<_>>(), expected);
            }
        }
    }
}

#[test]
fn workspace_auth_keeps_admin_scopes_when_namespace_label_looks_like_gmail() {
    let adapter = GoogleWorkspaceAdapter::new().expect("Google catalog");
    let mut target = target(AuthScopeProfile::Standard);
    target.namespace.account_label = "label@gmail.com".into();
    let request = request(&["auth", "login"]);
    let descriptor = adapter.find_tool(&request.tool).expect("raw Google write");
    let action = adapter
        .plan(&target, &request, descriptor)
        .expect("Workspace login plan");
    let argv: Vec<String> =
        serde_json::from_str(action.args.value("argv-json").expect("planned argv")).expect("argv JSON");
    let scopes = argv[3].split(',').collect::<BTreeSet<_>>();
    assert_eq!(scopes.len(), 16);
    assert!(scopes.contains("https://www.googleapis.com/auth/cloud-identity.groups"));
}

#[test]
fn explicit_scope_and_service_choices_are_preserved_for_both_profiles() {
    let adapter = GoogleWorkspaceAdapter::new().expect("Google catalog");
    for profile in [AuthScopeProfile::Standard, AuthScopeProfile::WorkspaceAdmin] {
        for argv in [
            vec!["auth", "login", "--readonly"],
            vec!["auth", "login", "--services", "gmail"],
            vec![
                "auth",
                "login",
                "--scopes",
                "https://www.googleapis.com/auth/drive.readonly",
            ],
        ] {
            let request = request(&argv);
            let descriptor = adapter.find_tool(&request.tool).expect("raw Google write");
            let action = adapter
                .plan(&target(profile), &request, descriptor)
                .expect("explicit login plan");
            assert_eq!(action.args, request.args);
        }
    }
}

fn target(profile: AuthScopeProfile) -> PlanningTarget {
    PlanningTarget {
        namespace: ResolvedNamespace::new(
            "google.test",
            ProviderKind::GoogleWorkspace,
            "test@example.invalid",
            "google_test",
            false,
            None,
        )
        .expect("Google namespace")
        .with_auth_scope_profile(profile)
        .expect("Google scope profile"),
        auth: ResolvedAuth::new("google_test", "test@example.invalid", AuthSecretRefs::GoogleCli)
            .expect("CLI-managed Google auth"),
    }
}

fn request(argv: &[&str]) -> ToolRequest {
    ToolRequest::new(
        "google.cli.write",
        "google.test",
        ExecutionMode::Draft,
        vec![
            ToolArgument::option("argv-json", serde_json::to_string(argv).expect("argv JSON")).expect("argv argument"),
        ],
    )
    .expect("Google login request")
}
