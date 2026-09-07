use std::fmt::Debug;

use serde::{de::DeserializeOwned, Serialize};
use switchboard_core::{AuditEventId, AuthRef, NamespaceId, OperationId, ToolName};

#[test]
fn namespace_deserialization_rejects_invalid_identifiers() {
    for invalid in ["", " ", "\n\t"] {
        assert!(NamespaceId::new(invalid).is_err());
        let encoded = serde_json::to_string(invalid).expect("string should serialize");
        assert!(serde_json::from_str::<NamespaceId>(&encoded).is_err());
    }
}

#[test]
fn tool_deserialization_rejects_invalid_identifiers() {
    for invalid in ["", " ", "unknown.issue.get", ".github", "GitHub.issue.get"] {
        assert!(ToolName::new(invalid).is_err());
        let encoded = serde_json::to_string(invalid).expect("string should serialize");
        assert!(serde_json::from_str::<ToolName>(&encoded).is_err());
    }
}

#[test]
fn operation_deserialization_rejects_invalid_identifiers() {
    for invalid in ["", " ", "\n\t"] {
        assert!(OperationId::new(invalid).is_err());
        let encoded = serde_json::to_string(invalid).expect("string should serialize");
        assert!(serde_json::from_str::<OperationId>(&encoded).is_err());
    }
}

#[test]
fn auth_deserialization_rejects_invalid_identifiers() {
    for invalid in ["", " ", "\n\t"] {
        assert!(AuthRef::new(invalid).is_err());
        let encoded = serde_json::to_string(invalid).expect("string should serialize");
        assert!(serde_json::from_str::<AuthRef>(&encoded).is_err());
    }
}

#[test]
fn audit_deserialization_rejects_invalid_identifiers() {
    for invalid in ["", " ", "\n\t"] {
        assert!(AuditEventId::new(invalid).is_err());
        let encoded = serde_json::to_string(invalid).expect("string should serialize");
        assert!(serde_json::from_str::<AuditEventId>(&encoded).is_err());
    }
}

#[test]
fn valid_identifiers_keep_their_string_representation() {
    assert_string_round_trip(
        NamespaceId::new("google.personal").expect("valid namespace"),
        "google.personal",
    );
    assert_string_round_trip(
        ToolName::new("github.issue.get").expect("valid tool"),
        "github.issue.get",
    );
    assert_string_round_trip(OperationId::new("op-123").expect("valid operation"), "op-123");
    assert_string_round_trip(AuthRef::new("google_personal").expect("valid auth"), "google_personal");
    assert_string_round_trip(AuditEventId::new("audit-123").expect("valid audit event"), "audit-123");

    // Validation preserves existing constructor semantics, including surrounding whitespace.
    assert_string_round_trip(NamespaceId::new(" personal ").expect("valid namespace"), " personal ");
    assert_string_round_trip(
        ToolName::new("github").expect("valid provider-only tool name"),
        "github",
    );
}

fn assert_string_round_trip<T: Debug + PartialEq + Serialize + DeserializeOwned>(value: T, expected: &str) {
    let encoded = serde_json::to_string(&value).expect("identifier should serialize");
    assert_eq!(
        encoded,
        serde_json::to_string(expected).expect("string should serialize")
    );
    let decoded: T = serde_json::from_str(&encoded).expect("valid identifier should deserialize");
    assert_eq!(decoded, value);
}
