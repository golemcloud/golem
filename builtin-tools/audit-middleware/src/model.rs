use golem_rust::SchemaValue;
use serde::{Deserialize, Serialize};

pub const RECORD_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRecord {
    pub version: u32,
    pub policy_invocation_key: String,
    pub occurrence_label: String,
    pub tool_name: String,
    pub command_path: Vec<String>,
    pub owner: OwnerSummary,
    pub principal: PrincipalSummary,
    pub input: ValueSummary,
    pub outcome: OutcomeSummary,
    pub stdout: StreamSummary,
    pub stderr: StreamSummary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnerSummary {
    pub component_id: String,
    pub agent_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum PrincipalSummary {
    Anonymous,
    Oidc {
        issuer: String,
        subject: String,
    },
    Agent {
        component_id: String,
        agent_id: String,
    },
    GolemUser {
        account_id: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueSummary {
    pub root_kind: String,
    pub nodes: u64,
    pub collection_entries: u64,
    pub string_bytes: u64,
    pub binary_bytes: u64,
    pub secret_handles: u64,
    pub quota_tokens: u64,
    pub permission_cards: u64,
    pub streams: u64,
}

impl ValueSummary {
    fn add(&mut self, other: Self) {
        self.nodes += other.nodes;
        self.collection_entries += other.collection_entries;
        self.string_bytes += other.string_bytes;
        self.binary_bytes += other.binary_bytes;
        self.secret_handles += other.secret_handles;
        self.quota_tokens += other.quota_tokens;
        self.permission_cards += other.permission_cards;
        self.streams += other.streams;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum OutcomeSummary {
    Success {
        result: Option<ValueSummary>,
    },
    Error {
        error_kind: String,
        custom_name: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSummary {
    pub declared: bool,
    pub chunks: u64,
    pub bytes: u64,
    pub terminal: StreamTerminal,
}

impl StreamSummary {
    pub fn absent() -> Self {
        Self {
            declared: false,
            chunks: 0,
            bytes: 0,
            terminal: StreamTerminal::NotDeclared,
        }
    }

    pub fn declared() -> Self {
        Self {
            declared: true,
            chunks: 0,
            bytes: 0,
            terminal: StreamTerminal::Finished,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StreamTerminal {
    NotDeclared,
    Finished,
    Cancelled,
    Abandoned,
    ResourceExhausted,
    Failed,
    ConsumerCancelled,
    ForwardingFailed,
}

pub fn summarize_value(value: &SchemaValue) -> ValueSummary {
    fn walk(value: &SchemaValue) -> ValueSummary {
        let mut summary = ValueSummary {
            root_kind: kind(value).to_string(),
            nodes: 1,
            ..Default::default()
        };
        let mut children = Vec::new();
        match value {
            SchemaValue::String(value) => summary.string_bytes = value.len() as u64,
            SchemaValue::Text(value) => summary.string_bytes = value.text.len() as u64,
            SchemaValue::Binary(value) => summary.binary_bytes = value.bytes.len() as u64,
            SchemaValue::Path { path } => summary.string_bytes = path.len() as u64,
            SchemaValue::Url { url } => summary.string_bytes = url.len() as u64,
            SchemaValue::Record { fields } => {
                summary.collection_entries += fields.len() as u64;
                children.extend(fields.iter());
            }
            SchemaValue::Tuple { elements }
            | SchemaValue::List { elements }
            | SchemaValue::FixedList { elements } => {
                summary.collection_entries += elements.len() as u64;
                children.extend(elements.iter());
            }
            SchemaValue::Map { entries } => {
                summary.collection_entries += entries.len() as u64;
                for (key, value) in entries {
                    children.push(key);
                    children.push(value);
                }
            }
            SchemaValue::Variant(payload) => children.extend(payload.payload.as_deref()),
            SchemaValue::Option { inner } => children.extend(inner.as_deref()),
            SchemaValue::Result(payload) => match payload {
                golem_rust::schema::ResultValuePayload::Ok { value }
                | golem_rust::schema::ResultValuePayload::Err { value } => {
                    children.extend(value.as_deref())
                }
            },
            SchemaValue::Union(payload) => children.push(&payload.body),
            SchemaValue::Secret(_) => summary.secret_handles = 1,
            SchemaValue::QuotaToken(_) => summary.quota_tokens = 1,
            SchemaValue::PermissionCard(_) => summary.permission_cards = 1,
            SchemaValue::Stream(_) => summary.streams = 1,
            _ => {}
        }
        for child in children {
            summary.add(walk(child));
        }
        summary
    }

    walk(value)
}

fn kind(value: &SchemaValue) -> &'static str {
    match value {
        SchemaValue::Bool(_) => "bool",
        SchemaValue::S8(_) => "s8",
        SchemaValue::S16(_) => "s16",
        SchemaValue::S32(_) => "s32",
        SchemaValue::S64(_) => "s64",
        SchemaValue::U8(_) => "u8",
        SchemaValue::U16(_) => "u16",
        SchemaValue::U32(_) => "u32",
        SchemaValue::U64(_) => "u64",
        SchemaValue::F32(_) => "f32",
        SchemaValue::F64(_) => "f64",
        SchemaValue::Char(_) => "char",
        SchemaValue::String(_) => "string",
        SchemaValue::Record { .. } => "record",
        SchemaValue::Variant(_) => "variant",
        SchemaValue::Enum { .. } => "enum",
        SchemaValue::Flags { .. } => "flags",
        SchemaValue::Tuple { .. } => "tuple",
        SchemaValue::List { .. } => "list",
        SchemaValue::FixedList { .. } => "fixed-list",
        SchemaValue::Map { .. } => "map",
        SchemaValue::Option { .. } => "option",
        SchemaValue::Result(_) => "result",
        SchemaValue::Text(_) => "text",
        SchemaValue::Binary(_) => "binary",
        SchemaValue::Path { .. } => "path",
        SchemaValue::Url { .. } => "url",
        SchemaValue::Datetime { .. } => "datetime",
        SchemaValue::Duration(_) => "duration",
        SchemaValue::Quantity(_) => "quantity",
        SchemaValue::Union(_) => "union",
        SchemaValue::Secret(_) => "secret",
        SchemaValue::QuotaToken(_) => "quota-token",
        SchemaValue::PermissionCard(_) => "permission-card",
        SchemaValue::Stream(_) => "stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_rust::schema::{BinaryValuePayload, ResultValuePayload};
    use test_r::test;

    #[test]
    fn safe_value_summary_contains_shapes_and_lengths_but_not_values() {
        let secret_text = "plaintext-that-must-not-be-recorded";
        let value = SchemaValue::Record {
            fields: vec![
                SchemaValue::String(secret_text.to_string()),
                SchemaValue::Binary(BinaryValuePayload {
                    bytes: vec![1, 2, 3, 4],
                    mime_type: Some("application/secret".to_string()),
                }),
            ],
        };
        let summary = summarize_value(&value);
        assert_eq!(summary.nodes, 3);
        assert_eq!(summary.collection_entries, 2);
        let encoded = serde_json::to_string(&summary).unwrap();
        assert!(!encoded.contains(secret_text));
        assert!(!encoded.contains("application/secret"));
        assert!(encoded.contains("\"stringBytes\":35"));
        assert!(encoded.contains("\"binaryBytes\":4"));
    }

    #[test]
    fn success_error_and_stream_terminals_are_stable_and_payload_free() {
        let success = OutcomeSummary::Success {
            result: Some(summarize_value(&SchemaValue::Result(
                ResultValuePayload::Ok {
                    value: Some(Box::new(SchemaValue::String("sensitive".to_string()))),
                },
            ))),
        };
        let error = OutcomeSummary::Error {
            error_kind: "tool".to_string(),
            custom_name: Some("denied".to_string()),
        };
        let stream = StreamSummary {
            declared: true,
            chunks: 2,
            bytes: 9,
            terminal: StreamTerminal::ResourceExhausted,
        };
        let encoded = serde_json::to_string(&(success, error, stream)).unwrap();
        assert!(!encoded.contains("sensitive"));
        assert!(encoded.contains("resource-exhausted"));
        assert!(encoded.contains("denied"));
    }

    #[test]
    fn owner_principal_and_occurrence_are_explicit_contract_fields() {
        let first = AuditRecord {
            version: RECORD_VERSION,
            policy_invocation_key: "logical-key".to_string(),
            occurrence_label: "security".to_string(),
            tool_name: "files".to_string(),
            command_path: vec!["read".to_string()],
            owner: OwnerSummary {
                component_id: "component".to_string(),
                agent_id: "owner".to_string(),
            },
            principal: PrincipalSummary::Oidc {
                issuer: "issuer".to_string(),
                subject: "subject".to_string(),
            },
            input: summarize_value(&SchemaValue::String("opaque".to_string())),
            outcome: OutcomeSummary::Success { result: None },
            stdout: StreamSummary::absent(),
            stderr: StreamSummary::absent(),
        };
        let mut second = first.clone();
        second.occurrence_label = "operations".to_string();
        assert_ne!(first.occurrence_label, second.occurrence_label);
        assert_eq!(first.owner.agent_id, "owner");
        assert!(matches!(first.principal, PrincipalSummary::Oidc { .. }));
    }
}
