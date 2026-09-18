use std::{fmt::Debug, str::FromStr};

use serde::{de::DeserializeOwned, Serialize};
use switchboard_core::{ApprovalState, AuditOutcome, BackendKind, OperationStatus, ToolKind};

fn assert_identifiers<T>(cases: &[(T, &str)], as_str: impl Fn(&T) -> &'static str)
where
    T: Debug + Eq + Serialize + DeserializeOwned + FromStr,
    T::Err: Debug,
{
    for (value, identifier) in cases {
        assert_eq!(as_str(value), *identifier);
        assert_eq!(&identifier.parse::<T>().expect("stored identifier should parse"), value);
        let encoded = serde_json::to_string(value).expect("enum should serialize");
        assert_eq!(
            serde_json::from_str::<String>(&encoded).expect("enum should serialize as a string"),
            *identifier
        );
        assert_eq!(
            &serde_json::from_str::<T>(&encoded).expect("serialized identifier should deserialize"),
            value
        );
        assert!(identifier.to_ascii_uppercase().parse::<T>().is_err());
        assert!(format!(" {identifier}").parse::<T>().is_err());
    }
    assert!("".parse::<T>().is_err());
    assert!("unknown".parse::<T>().is_err());
}

#[test]
fn backend_identifiers_match_existing_wire_format() {
    let cases = [
        (BackendKind::Cli, "cli"),
        (BackendKind::Api, "api"),
        (BackendKind::Local, "local"),
        (BackendKind::Bridge, "bridge"),
    ];
    assert_identifiers(&cases, BackendKind::as_str);
    for (value, identifier) in cases {
        assert_eq!(value.to_string(), identifier);
    }
}

#[test]
fn tool_kind_identifiers_match_existing_wire_format() {
    assert_identifiers(
        &[(ToolKind::Read, "read"), (ToolKind::Write, "write")],
        ToolKind::as_str,
    );
}

#[test]
fn approval_identifiers_match_existing_wire_format() {
    assert_identifiers(
        &[
            (ApprovalState::NotRequired, "not_required"),
            (ApprovalState::Pending, "pending"),
            (ApprovalState::Approved, "approved"),
            (ApprovalState::Rejected, "rejected"),
        ],
        ApprovalState::as_str,
    );
}

#[test]
fn operation_status_identifiers_match_existing_wire_format() {
    assert_identifiers(
        &[
            (OperationStatus::Planned, "planned"),
            (OperationStatus::Executing, "executing"),
            (OperationStatus::Uncertain, "uncertain"),
            (OperationStatus::Applied, "applied"),
            (OperationStatus::Verified, "verified"),
            (OperationStatus::Failed, "failed"),
            (OperationStatus::Compensated, "compensated"),
        ],
        OperationStatus::as_str,
    );
}

#[test]
fn audit_outcome_identifiers_match_existing_wire_format() {
    assert_identifiers(
        &[
            (AuditOutcome::Planned, "planned"),
            (AuditOutcome::Approved, "approved"),
            (AuditOutcome::Rejected, "rejected"),
            (AuditOutcome::Executed, "executed"),
            (AuditOutcome::Failed, "failed"),
            (AuditOutcome::Compensated, "compensated"),
            (AuditOutcome::Blocked, "blocked"),
        ],
        AuditOutcome::as_str,
    );
}
