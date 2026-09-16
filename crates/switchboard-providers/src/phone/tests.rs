use std::{path::PathBuf, process::Command, time::Duration};

use switchboard_core::{
    Adapter, AuthSecretRefs, ExecutionMode, PlanningTarget, ProviderKind, ResolvedAuth, ResolvedNamespace,
    ToolArgument, ToolRequest,
};

use crate::phone::{request::RunRequest, PhoneAdapter};

fn target() -> PlanningTarget {
    PlanningTarget {
        namespace: ResolvedNamespace::new(
            "phone.personal",
            ProviderKind::Phone,
            "Personal",
            "phone_personal",
            false,
            Some(PathBuf::from("/tmp/phone-test")),
        )
        .expect("namespace"),
        auth: ResolvedAuth::new(
            "phone_personal",
            "Personal",
            AuthSecretRefs::PhoneCli {
                api_key: None,
                api_secret: None,
                model_api_key: None,
            },
        )
        .expect("auth"),
    }
}

fn request() -> ToolRequest {
    ToolRequest::new(
        "phone.call.run",
        "phone.personal",
        ExecutionMode::Apply,
        vec![
            ToolArgument::option("destination", "+12125550100").expect("option"),
            ToolArgument::option("task", "Ask for opening hours").expect("option"),
            ToolArgument::option("caller-name", "Test").expect("option"),
        ],
    )
    .expect("request")
}

#[test]
fn call_plans_require_exact_approval_and_cli_accepts_the_serialized_request() {
    let adapter = PhoneAdapter::new().expect("adapter");
    let request = request();
    let descriptor = adapter.find_tool(&request.tool).expect("tool");
    let action = adapter.plan(&target(), &request, descriptor).expect("plan");
    assert!(action.approval_required);
    assert!(action.summary.contains("+12125550100"));
    assert!(action.summary.contains("Ask for opening hours"));
    let mut input = RunRequest::parse(&action.args).expect("parse");
    input.call_id = Some("op_test_call");
    let bytes = serde_json::to_vec(&input).expect("serialize producer");
    let consumed: phone_cli::domain::CallRequest = serde_json::from_slice(&bytes).expect("real CLI consumer");
    let authorized = consumed.authorize(true).expect("CLI validation");
    assert_eq!(authorized.request().call_id.to_string(), "op_test_call");
    assert_eq!(authorized.request().task, input.task);
    assert_eq!(authorized.request().max_duration_seconds, 600);
}

#[test]
fn malformed_or_ambiguous_call_arguments_are_rejected_before_planning() {
    for (name, value) in [("task", "invalid\0task"), ("caller-name", "two\nlines")] {
        let args = request()
            .args
            .iter()
            .map(|arg| {
                if arg.name() == name {
                    ToolArgument::option(name, value).expect("option")
                } else {
                    arg.clone()
                }
            })
            .collect::<Vec<_>>();
        assert!(RunRequest::parse(&args.into()).is_err());
    }
    for extra in [
        ToolArgument::option("destination", "+12125550101").expect("option"),
        ToolArgument::option("worker-command", "/bin/sh").expect("option"),
        ToolArgument::flag("approve").expect("flag"),
        ToolArgument::option("max-duration-seconds", "0").expect("option"),
        ToolArgument::option("max-duration-seconds", "3601").expect("option"),
    ] {
        let mut args = request().args.iter().cloned().collect::<Vec<_>>();
        args.push(extra);
        assert!(RunRequest::parse(&args.into()).is_err());
    }
    let adapter = PhoneAdapter::new().expect("adapter");
    let request = request();
    let mut unscoped = target();
    unscoped.namespace.state_dir = None;
    assert!(adapter
        .plan(&unscoped, &request, adapter.find_tool(&request.tool).expect("tool"))
        .is_err());
}

#[cfg(unix)]
#[test]
fn private_stdin_transport_preserves_a_full_brief_and_does_not_need_a_file() {
    let input = vec![b'x'; 40_000];
    let output = crate::phone::runtime::capture(Command::new("cat"), input.clone(), Duration::from_secs(5))
        .expect("real child capture");
    assert!(output.status.success());
    assert_eq!(output.stdout, input);
    assert!(output.stderr.is_empty());
}
