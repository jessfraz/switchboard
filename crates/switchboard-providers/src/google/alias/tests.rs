//! Stateful provider fixtures exercise the real adapter and CLI runtime without
//! credentials or external writes.

use std::{env, fs};

use serde::{Deserialize, Serialize};
use switchboard_core::{
    Adapter, AuthSecretRefs, ExecutionMode, ExecutionTarget, GoogleDelegation, OperationId, PlannedAction,
    PlanningTarget, ProviderKind, ResolvedAuth, ResolvedCredentials, ResolvedNamespace, StoredOperation, ToolArgument,
    ToolOutput, ToolRequest, VerificationStatus,
};

use crate::{
    cli::{CliProviderBackend, CliRuntimeMaterializer},
    google::GoogleWorkspaceAdapter,
    process_runtime::ProcessContext,
    test_support::{lock_env, TempScript},
};

const ACCOUNT: &str = "person@example.com";
const EMAIL: &str = "alias@example.com";
const TOOL: &str = "google.mail.alias.ensure";

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Sender {
    send_as_email: String,
    display_name: String,
    reply_to_address: String,
    treat_as_alias: bool,
    is_default: bool,
    verification_status: String,
}

impl Sender {
    fn existing(status: &str) -> Self {
        Self {
            send_as_email: EMAIL.into(),
            display_name: "Existing Name".into(),
            reply_to_address: "replies@example.com".into(),
            treat_as_alias: false,
            is_default: true,
            verification_status: status.into(),
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct State {
    sender: Option<Sender>,
    receiving_alias: bool,
    wrong_primary: bool,
    omit_primary_verification: bool,
    omit_target_verification: bool,
    wrong_receiving_owner: bool,
    directory_read_error: bool,
    gmail_read_error: bool,
    missing_directory_readback: bool,
    fail_gmail_create: bool,
    lose_gmail_create_response: bool,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum SenderOutcome {
    Existing,
    Created,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ReceivingOutcome {
    NotChecked,
    Existing,
    Created,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum SenderVerification {
    Accepted,
    Pending,
}

#[derive(Debug, Deserialize)]
struct Fields {
    email: String,
    account: String,
    sender: SenderOutcome,
    receiving_alias: ReceivingOutcome,
    verification_status: SenderVerification,
}

fn fields(output: &ToolOutput) -> Fields {
    serde_json::from_value(serde_json::to_value(&output.fields).expect("serialize output fields"))
        .expect("typed alias output")
}

struct FixtureMaterializer;

impl CliRuntimeMaterializer for FixtureMaterializer {
    fn prepare(&self, _target: &ExecutionTarget, _argv: &[String]) -> switchboard_core::Result<ProcessContext> {
        Ok(ProcessContext::new())
    }
}

fn adapter() -> GoogleWorkspaceAdapter {
    let mut adapter = GoogleWorkspaceAdapter::new().expect("Google catalog");
    adapter.backend = CliProviderBackend::new(Box::new(FixtureMaterializer));
    adapter
}

fn target() -> ExecutionTarget {
    let delegation = GoogleDelegation {
        project: "example-project".into(),
        service_account: "settings@example-project.iam.gserviceaccount.com".into(),
        domain: "example.com".into(),
        subject: ACCOUNT.into(),
        signer_account: ACCOUNT.into(),
        signer_state_dir: env::temp_dir().join("switchboard-alias-fixture-signer"),
    };
    ExecutionTarget {
        namespace: ResolvedNamespace::new(
            "google.fixture",
            ProviderKind::GoogleWorkspace,
            ACCOUNT,
            "google.fixture-auth",
            false,
            Some(env::temp_dir().join("switchboard-alias-fixture")),
        )
        .expect("namespace"),
        auth: ResolvedAuth::new(
            "google.fixture-auth",
            ACCOUNT,
            AuthSecretRefs::GoogleDelegated {
                delegation: delegation.clone(),
            },
        )
        .expect("auth"),
        credentials: ResolvedCredentials::GoogleDelegated { delegation },
    }
}

fn plan(
    adapter: &GoogleWorkspaceAdapter,
    target: &ExecutionTarget,
    email: &str,
    display_name: Option<&str>,
) -> switchboard_core::Result<PlannedAction> {
    let mut args = vec![ToolArgument::option("email", email).expect("email argument")];
    if let Some(name) = display_name {
        args.push(ToolArgument::option("display-name", name).expect("name argument"));
    }
    let request = ToolRequest::new(TOOL, "google.fixture", ExecutionMode::Draft, args).expect("request");
    let descriptor = adapter.find_tool(&request.tool).expect("curated alias tool");
    adapter
        .plan(
            &PlanningTarget {
                namespace: target.namespace.clone(),
                auth: target.auth.clone(),
            },
            &request,
            descriptor,
        )
        .map(|plan| plan.with_operation_id(OperationId::new("alias-fixture-operation").expect("operation ID")))
}

fn stored(action: &PlannedAction) -> StoredOperation {
    let mut operation = StoredOperation::from_plan(action.operation_id.clone().expect("operation ID"), action);
    if operation.approval_required {
        operation.approve("fixture", None).expect("approval");
    }
    operation.claim_execution().expect("execution claim");
    operation
}

fn read_state(fixture: &TempScript) -> State {
    let path = fixture.path().parent().expect("fixture directory").join("state.json");
    serde_json::from_slice(&fs::read(path).expect("saved provider state")).expect("typed state")
}

fn calls(fixture: &TempScript) -> Vec<Vec<String>> {
    let path = fixture.path().parent().expect("fixture directory").join("calls.jsonl");
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("native argv"))
        .collect()
}

fn is_command(argv: &[String], path: &[&str]) -> bool {
    argv.iter()
        .take(path.len())
        .map(String::as_str)
        .eq(path.iter().copied())
}

const DIRECTORY_INSERT: &[&str] = &["admin-reports", "users", "aliases", "insert"];
const GMAIL_CREATE: &[&str] = &["gmail", "users", "settings", "sendAs", "create"];

fn writes(fixture: &TempScript) -> Vec<Vec<String>> {
    calls(fixture)
        .into_iter()
        .filter(|argv| is_command(argv, DIRECTORY_INSERT) || is_command(argv, GMAIL_CREATE))
        .collect()
}

fn option<'a>(argv: &'a [String], name: &str) -> &'a str {
    argv.iter()
        .position(|value| value == name)
        .and_then(|index| argv.get(index + 1))
        .map(String::as_str)
        .expect("native option")
}

fn fixture(state: State) -> TempScript {
    let fixture = TempScript::new("gws-alias", SCRIPT);
    let path = fixture.path().parent().expect("fixture directory").join("state.json");
    fs::write(path, serde_json::to_vec(&state).expect("serialize state")).expect("save state");
    env::set_var("SWITCHBOARD_GWS_BIN", fixture.path());
    fixture
}

#[test]
fn alias_plan_requires_fixed_subject_delegation_and_same_domain() {
    let adapter = adapter();
    let mut target = target();
    for email in [
        "alias@other.example",
        "alias@example.com.evil",
        "invalid",
        "alias@@example.com",
    ] {
        assert!(plan(&adapter, &target, email, None).is_err(), "accepted {email}");
    }
    target.auth = ResolvedAuth::new("google.fixture-auth", ACCOUNT, AuthSecretRefs::GoogleCli).expect("OAuth auth");
    let error = plan(&adapter, &target, EMAIL, None).expect_err("ordinary OAuth cannot create send-as aliases");
    assert!(
        error.to_string().contains("Workspace administrator authorization"),
        "{error}"
    );
}

#[test]
fn alias_plan_is_local_and_execution_rechecks_current_auth() {
    let _guard = lock_env();
    let fixture = fixture(State::default());
    let adapter = adapter();
    let mut target = target();
    let action = plan(&adapter, &target, EMAIL, None).expect("alias plan");
    assert!(calls(&fixture).is_empty(), "planning must not call Google");
    target.auth = ResolvedAuth::new("google.fixture-auth", ACCOUNT, AuthSecretRefs::GoogleCli).expect("OAuth auth");
    target.credentials = ResolvedCredentials::GoogleCli;
    assert!(adapter.execute(&target, &action).is_err());
    assert!(writes(&fixture).is_empty());
}

#[test]
fn execution_requires_a_durable_operation_before_calling_google() {
    let _guard = lock_env();
    let fixture = fixture(State::default());
    let adapter = adapter();
    let target = target();
    let mut action = plan(&adapter, &target, EMAIL, None).expect("alias plan");
    action.operation_id = None;
    assert!(adapter.execute(&target, &action).is_err());
    assert!(calls(&fixture).is_empty());
}

#[test]
fn readback_cannot_verify_an_absent_sender() {
    let _guard = lock_env();
    let fixture = fixture(State::default());
    let adapter = adapter();
    let target = target();
    let action = plan(&adapter, &target, EMAIL, None).expect("alias plan");
    let mut operation = stored(&action);
    operation.mark_uncertain("response lost").expect("uncertain operation");
    assert_eq!(
        adapter.verify(&target, &operation).expect("readback").status,
        VerificationStatus::Mismatch
    );
    assert!(writes(&fixture).is_empty());
}

#[test]
fn existing_accepted_sender_preserves_settings_without_directory_access() {
    let _guard = lock_env();
    let fixture = fixture(State {
        sender: Some(Sender::existing("accepted")),
        directory_read_error: true,
        ..State::default()
    });
    let adapter = adapter();
    let target = target();
    let action = plan(&adapter, &target, EMAIL, Some("Requested Different Name")).expect("alias plan");
    let output = adapter.execute(&target, &action).expect("existing sender");
    let result = fields(&output);
    assert_eq!(result.email, EMAIL);
    assert_eq!(result.account, ACCOUNT);
    assert_eq!(result.sender, SenderOutcome::Existing);
    assert_eq!(result.receiving_alias, ReceivingOutcome::NotChecked);
    assert_eq!(result.verification_status, SenderVerification::Accepted);
    assert!(writes(&fixture).is_empty());
    assert!(calls(&fixture)
        .iter()
        .all(|argv| argv.first().map(String::as_str) == Some("gmail")));
    let sender = read_state(&fixture).sender.expect("existing sender");
    assert_eq!(sender.display_name, "Existing Name");
    assert_eq!(sender.reply_to_address, "replies@example.com");
    assert!(!sender.treat_as_alias);
    assert!(sender.is_default);
    let mut operation = stored(&action);
    operation.mark_applied(&output).expect("applied operation");
    assert_eq!(
        adapter.verify(&target, &operation).expect("readback").status,
        VerificationStatus::Verified
    );
}

#[test]
fn primary_without_verification_status_does_not_block_accepted_alias() {
    let _guard = lock_env();
    let fixture = fixture(State {
        sender: Some(Sender::existing("accepted")),
        omit_primary_verification: true,
        ..State::default()
    });
    let adapter = adapter();
    let target = target();
    let action = plan(&adapter, &target, EMAIL, None).expect("alias plan");
    let output = adapter
        .execute(&target, &action)
        .expect("accepted alias with native primary shape");
    let result = fields(&output);
    assert_eq!(result.verification_status, SenderVerification::Accepted);
    assert_eq!(result.sender, SenderOutcome::Existing);
    assert_eq!(result.receiving_alias, ReceivingOutcome::NotChecked);
    assert!(writes(&fixture).is_empty());
    let mut operation = stored(&action);
    operation.mark_applied(&output).expect("applied operation");
    assert_eq!(
        adapter.verify(&target, &operation).expect("readback").status,
        VerificationStatus::Verified
    );
}

#[test]
fn primary_address_without_verification_status_is_an_existing_ready_sender() {
    let _guard = lock_env();
    let fixture = fixture(State {
        omit_primary_verification: true,
        directory_read_error: true,
        ..State::default()
    });
    let adapter = adapter();
    let target = target();
    let action = plan(&adapter, &target, ACCOUNT, None).expect("primary sender plan");
    let output = adapter.execute(&target, &action).expect("existing primary sender");
    let result = fields(&output);
    assert_eq!(result.email, ACCOUNT);
    assert_eq!(result.sender, SenderOutcome::Existing);
    assert_eq!(result.receiving_alias, ReceivingOutcome::NotChecked);
    assert_eq!(result.verification_status, SenderVerification::Accepted);
    assert!(writes(&fixture).is_empty());
    assert!(calls(&fixture)
        .iter()
        .all(|argv| argv.first().map(String::as_str) == Some("gmail")));
    let mut operation = stored(&action);
    operation.mark_applied(&output).expect("applied operation");
    assert_eq!(
        adapter.verify(&target, &operation).expect("readback").status,
        VerificationStatus::Verified
    );
}

#[test]
fn target_with_unknown_verification_is_never_claimed_ready() {
    let _guard = lock_env();
    for (status, omit_status) in [("accepted", true), ("verificationStatusUnspecified", false)] {
        let fixture = fixture(State {
            sender: Some(Sender::existing(status)),
            omit_target_verification: omit_status,
            ..State::default()
        });
        let adapter = adapter();
        let target = target();
        let action = plan(&adapter, &target, EMAIL, None).expect("alias plan");
        assert!(adapter.execute(&target, &action).is_err());
        let mut operation = stored(&action);
        operation.mark_uncertain("response lost").expect("uncertain operation");
        if let Ok(receipt) = adapter.verify(&target, &operation) {
            assert_ne!(receipt.status, VerificationStatus::Verified);
        }
        assert!(writes(&fixture).is_empty());
    }
}

#[test]
fn pending_sender_is_preserved_and_not_claimed_ready() {
    let _guard = lock_env();
    let fixture = fixture(State {
        sender: Some(Sender::existing("pending")),
        ..State::default()
    });
    let adapter = adapter();
    let target = target();
    let action = plan(&adapter, &target, EMAIL, None).expect("alias plan");
    let output = adapter.execute(&target, &action).expect("pending result");
    assert_eq!(fields(&output).verification_status, SenderVerification::Pending);
    assert!(writes(&fixture).is_empty());
    let mut operation = stored(&action);
    operation.mark_applied(&output).expect("applied operation");
    assert_eq!(
        adapter.verify(&target, &operation).expect("pending readback").status,
        VerificationStatus::Unavailable
    );
}

#[test]
fn absent_sender_creates_receiving_alias_then_sender_and_verifies() {
    let _guard = lock_env();
    let fixture = fixture(State::default());
    let adapter = adapter();
    let target = target();
    let action = plan(&adapter, &target, EMAIL, Some("Example Person")).expect("alias plan");
    let output = adapter.execute(&target, &action).expect("create alias");
    let result = fields(&output);
    assert_eq!(result.sender, SenderOutcome::Created);
    assert_eq!(result.receiving_alias, ReceivingOutcome::Created);
    assert_eq!(result.verification_status, SenderVerification::Accepted);
    let writes = writes(&fixture);
    assert_eq!(writes.len(), 2);
    assert!(is_command(&writes[0], DIRECTORY_INSERT));
    assert!(is_command(&writes[1], GMAIL_CREATE));
    assert_eq!(option(&writes[0], "--api-version"), "directory_v1");
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct UserParams {
        user_key: String,
    }
    let params: UserParams = serde_json::from_str(option(&writes[0], "--params")).expect("Directory target");
    assert_eq!(params.user_key, ACCOUNT);
    let sender = read_state(&fixture).sender.expect("created sender");
    assert_eq!(sender.send_as_email, EMAIL);
    assert_eq!(sender.display_name, "Example Person");
    assert_eq!(sender.reply_to_address, EMAIL);
    assert!(sender.treat_as_alias);
    assert!(!sender.is_default);
    let mut operation = stored(&action);
    operation.mark_applied(&output).expect("applied operation");
    assert_eq!(
        adapter.verify(&target, &operation).expect("readback").status,
        VerificationStatus::Verified
    );
}

#[test]
fn existing_receiving_alias_is_reused_without_duplicate_insertion() {
    let _guard = lock_env();
    let fixture = fixture(State {
        receiving_alias: true,
        ..State::default()
    });
    let adapter = adapter();
    let target = target();
    let action = plan(&adapter, &target, EMAIL, None).expect("alias plan");
    let output = adapter.execute(&target, &action).expect("existing receiving alias");
    assert_eq!(fields(&output).receiving_alias, ReceivingOutcome::Existing);
    assert_eq!(writes(&fixture).len(), 1);
    assert!(is_command(&writes(&fixture)[0], GMAIL_CREATE));
}

#[test]
fn primary_account_mismatch_stops_before_directory_mutation() {
    let _guard = lock_env();
    let fixture = fixture(State {
        wrong_primary: true,
        ..State::default()
    });
    let adapter = adapter();
    let target = target();
    let action = plan(&adapter, &target, EMAIL, None).expect("alias plan");
    assert!(adapter.execute(&target, &action).is_err());
    assert!(writes(&fixture).is_empty());
}

#[test]
fn provider_read_failure_is_never_treated_as_an_absent_alias() {
    let _guard = lock_env();
    for state in [
        State {
            directory_read_error: true,
            ..State::default()
        },
        State {
            gmail_read_error: true,
            ..State::default()
        },
    ] {
        let fixture = fixture(state);
        let adapter = adapter();
        let target = target();
        let action = plan(&adapter, &target, EMAIL, None).expect("alias plan");
        assert!(adapter.execute(&target, &action).is_err());
        assert!(writes(&fixture).is_empty());
    }
}

#[test]
fn receiving_alias_with_wrong_owner_does_not_create_a_sender() {
    let _guard = lock_env();
    let fixture = fixture(State {
        receiving_alias: true,
        wrong_receiving_owner: true,
        ..State::default()
    });
    let adapter = adapter();
    let target = target();
    let action = plan(&adapter, &target, EMAIL, None).expect("alias plan");
    assert!(adapter.execute(&target, &action).is_err());
    assert!(writes(&fixture).is_empty());
}

#[test]
fn receiving_alias_insert_requires_readback_before_gmail_creation() {
    let _guard = lock_env();
    let fixture = fixture(State {
        missing_directory_readback: true,
        ..State::default()
    });
    let adapter = adapter();
    let target = target();
    let action = plan(&adapter, &target, EMAIL, None).expect("alias plan");
    assert!(adapter.execute(&target, &action).is_err());
    assert_eq!(writes(&fixture).len(), 1);
    assert!(is_command(&writes(&fixture)[0], DIRECTORY_INSERT));
    assert!(read_state(&fixture).sender.is_none());
}

#[test]
fn partial_retry_reuses_completed_directory_step() {
    let _guard = lock_env();
    let fixture = fixture(State {
        fail_gmail_create: true,
        ..State::default()
    });
    let adapter = adapter();
    let target = target();
    let action = plan(&adapter, &target, EMAIL, None).expect("alias plan");
    assert!(adapter.execute(&target, &action).is_err());
    assert!(read_state(&fixture).receiving_alias);
    assert!(read_state(&fixture).sender.is_none());
    let retry = plan(&adapter, &target, EMAIL, None).expect("retry plan");
    let result = fields(&adapter.execute(&target, &retry).expect("resume partial completion"));
    assert_eq!(result.receiving_alias, ReceivingOutcome::Existing);
    assert_eq!(result.sender, SenderOutcome::Created);
    assert_eq!(
        writes(&fixture)
            .iter()
            .filter(|argv| is_command(argv, DIRECTORY_INSERT))
            .count(),
        1
    );
}

#[test]
fn lost_gmail_create_response_is_recoverable_by_readback_without_recreation() {
    let _guard = lock_env();
    let fixture = fixture(State {
        lose_gmail_create_response: true,
        ..State::default()
    });
    let adapter = adapter();
    let target = target();
    let action = plan(&adapter, &target, EMAIL, None).expect("alias plan");
    let mut operation = stored(&action);
    match adapter.execute(&target, &action) {
        Ok(output) => operation.mark_applied(&output).expect("applied result"),
        Err(error) => operation.mark_uncertain(error.to_string()).expect("uncertain result"),
    }
    assert!(read_state(&fixture).sender.is_some());
    assert_eq!(
        adapter.verify(&target, &operation).expect("recovered readback").status,
        VerificationStatus::Verified
    );
    let retry = plan(&adapter, &target, EMAIL, None).expect("retry plan");
    let output = adapter
        .execute(&target, &retry)
        .expect("existing sender after lost response");
    assert_eq!(fields(&output).sender, SenderOutcome::Existing);
    assert_eq!(
        writes(&fixture)
            .iter()
            .filter(|argv| is_command(argv, GMAIL_CREATE))
            .count(),
        1
    );
}

// Persist provider state so retries/readback address the resource actually
// written, rather than replaying a predetermined successful response.
const SCRIPT: &str = r#"#!/usr/bin/env python3
import json
from pathlib import Path
import sys

args = sys.argv[1:]
if args == ['--version']:
    print('gws 0.22.5')
    raise SystemExit(0)
if '--help' in args:
    print('gws fixture help')
    raise SystemExit(0)
root = Path(__file__).parent
state_path = root / 'state.json'
state = json.loads(state_path.read_text())
with (root / 'calls.jsonl').open('a') as calls:
    calls.write(json.dumps(args) + '\n')

def value(flag):
    for index, argument in enumerate(args):
        if argument == flag:
            return args[index + 1]
        if argument.startswith(flag + '='):
            return argument[len(flag) + 1:]
    return None

def emit(payload):
    print(json.dumps(payload))
    raise SystemExit(0)

def fail(message):
    print(message, file=sys.stderr)
    raise SystemExit(1)

def save():
    state_path.write_text(json.dumps(state))

primary = {'sendAsEmail': 'different@example.com' if state['wrong_primary'] else 'person@example.com',
           'isPrimary': True, 'isDefault': not (state['sender'] or {}).get('isDefault', False),
           'displayName': 'Primary Name',
           'verificationStatus': 'accepted'}
if state['omit_primary_verification']:
    primary.pop('verificationStatus')
sender_response = dict(state['sender']) if state['sender'] else None
if sender_response and state['omit_target_verification']:
    sender_response.pop('verificationStatus')
if args[:4] == ['gmail', 'users', 'settings', 'sendAs']:
    params = json.loads(value('--params') or '{}')
    if params.get('userId') not in ('me', 'person@example.com'):
        fail('wrong Gmail user')
    method = args[4]
    if method in ('list', 'get') and state['gmail_read_error']:
        fail('Gmail read refused')
    if method == 'list':
        emit({'sendAs': [primary] + ([sender_response] if sender_response else [])})
    if method == 'get':
        email = params.get('sendAsEmail')
        if email == primary['sendAsEmail']:
            emit(primary)
        if state['sender'] and email == state['sender']['sendAsEmail']:
            emit(sender_response)
        fail('sender absent: 404')
    if method == 'create':
        if state['fail_gmail_create']:
            state['fail_gmail_create'] = False
            save()
            fail('Gmail creation refused')
        if not state['receiving_alias']:
            fail('sendAsEmail is not a valid user or group')
        if state['sender']:
            fail('duplicate sender')
        sender = json.loads(value('--json') or '{}')
        if sender.get('sendAsEmail') != 'alias@example.com':
            fail('wrong requested sender')
        sender.setdefault('displayName', '')
        sender.setdefault('replyToAddress', '')
        sender.setdefault('treatAsAlias', False)
        sender.setdefault('isDefault', False)
        sender['verificationStatus'] = 'accepted'
        state['sender'] = sender
        save()
        if state['lose_gmail_create_response']:
            fail('lost response after Gmail creation')
        emit(sender)
if args[:3] == ['admin-reports', 'users', 'aliases']:
    if value('--api-version') != 'directory_v1':
        fail('wrong Directory API version')
    params = json.loads(value('--params') or '{}')
    if params.get('userKey') != 'person@example.com':
        fail('wrong Directory owner')
    alias = {'alias': 'alias@example.com',
             'primaryEmail': 'different@example.com' if state['wrong_receiving_owner'] else 'person@example.com'}
    method = args[3]
    if method == 'list':
        if state['directory_read_error']:
            fail('Directory read refused')
        present = state['receiving_alias'] and not state['missing_directory_readback']
        emit({'aliases': [alias] if present else []})
    if method == 'insert':
        if state['receiving_alias']:
            fail('duplicate receiving alias')
        if json.loads(value('--json') or '{}').get('alias') != 'alias@example.com':
            fail('wrong receiving alias')
        state['receiving_alias'] = True
        save()
        emit(alias)
fail('unexpected command: ' + repr(args))
"#;
