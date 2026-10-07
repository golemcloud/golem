#[path = "../src/model.rs"]
mod model;

use golem_rust::SchemaValue;
use golem_rust::schema::BinaryValuePayload;
use model::{
    AuditRecord, OutcomeSummary, OwnerSummary, PrincipalSummary, RECORD_VERSION, StreamSummary,
    StreamTerminal, summarize_value,
};

#[test]
fn audit_1_record_contract_is_versioned_attributed_summary_only_and_exact_json() {
    let plaintext = "plaintext-must-never-enter-audit";
    let input = SchemaValue::Record {
        fields: vec![
            SchemaValue::String(plaintext.to_string()),
            SchemaValue::Binary(BinaryValuePayload {
                bytes: vec![0xde, 0xad, 0xbe, 0xef],
                mime_type: Some("application/private".to_string()),
            }),
        ],
    };
    let record = AuditRecord {
        version: RECORD_VERSION,
        policy_invocation_key: "policy-call-01".to_string(),
        occurrence_label: "security".to_string(),
        tool_name: "artifact".to_string(),
        command_path: vec!["render".to_string()],
        owner: OwnerSummary {
            component_id: "component-01".to_string(),
            agent_id: "Artifact/audit-owner".to_string(),
        },
        principal: PrincipalSummary::Oidc {
            issuer: "https://issuer.audit.test".to_string(),
            subject: "subject-01".to_string(),
        },
        input: summarize_value(&input),
        outcome: OutcomeSummary::Success {
            result: Some(summarize_value(&SchemaValue::String("ok".to_string()))),
        },
        stdout: StreamSummary {
            declared: true,
            chunks: 2,
            bytes: 9,
            terminal: StreamTerminal::Finished,
        },
        stderr: StreamSummary {
            declared: true,
            chunks: 1,
            bytes: 7,
            terminal: StreamTerminal::ResourceExhausted,
        },
    };

    let actual = serde_json::to_value(record).unwrap();
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/gol40_k1_audit_record_v1.json")).unwrap();
    assert_eq!(actual, expected);

    let encoded = serde_json::to_string(&actual).unwrap();
    for forbidden in [plaintext, "application/private"] {
        assert!(
            !encoded.contains(forbidden),
            "audit summary leaked opaque or payload material {forbidden:?}: {encoded}"
        );
    }
}

#[test]
fn audit_1_error_contract_records_only_error_class_and_custom_name() {
    let outcome = OutcomeSummary::Error {
        error_kind: "tool".to_string(),
        custom_name: Some("render-failed".to_string()),
    };
    assert_eq!(
        serde_json::to_value(outcome).unwrap(),
        serde_json::json!({
            "kind": "error",
            "errorKind": "tool",
            "customName": "render-failed"
        })
    );
}
