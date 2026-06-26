// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Closed classification of host-managed capability nodes and the shared
//! redaction helpers that observability surfaces consult.
//!
//! Capability values such as [`SchemaValue::Secret`] and
//! [`SchemaValue::QuotaToken`] carry material owned by the host authority, not
//! the guest. Every human-facing surface (CLI display, agent-id rendering,
//! error rendering, tracing) must redact them, and the placement rules must
//! police where they may appear. Rather than each of those consumers
//! re-matching the capability cases, they classify a node through
//! [`HostManagedKind`] and obtain a stable kind name / redacted placeholder
//! from one place. Adding a future capability case means adding one variant
//! here; the consumers pick it up automatically.

use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::{
    ResultValuePayload, SchemaValue, UnionValuePayload, VariantValuePayload,
};
use std::fmt;

/// A closed set of "host-managed" capability kinds.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum HostManagedKind {
    Secret,
    QuotaToken,
}

impl HostManagedKind {
    /// Classifies a [`SchemaType`] node, returning `Some` for capability types.
    pub fn from_type(ty: &SchemaType) -> Option<Self> {
        match ty {
            SchemaType::Secret { .. } => Some(Self::Secret),
            SchemaType::QuotaToken { .. } => Some(Self::QuotaToken),
            _ => None,
        }
    }

    /// Classifies a [`SchemaValue`] node, returning `Some` for capability
    /// values.
    pub fn from_value(value: &SchemaValue) -> Option<Self> {
        match value {
            SchemaValue::Secret(_) => Some(Self::Secret),
            SchemaValue::QuotaToken(_) => Some(Self::QuotaToken),
            _ => None,
        }
    }

    /// Stable kebab-case name of the capability kind.
    pub fn kind_name(self) -> &'static str {
        match self {
            Self::Secret => "secret",
            Self::QuotaToken => "quota-token",
        }
    }

    /// Placeholder rendered in place of the value on redacting surfaces, in the
    /// form `<redacted: kind>`.
    pub fn redacted_placeholder(self) -> &'static str {
        match self {
            Self::Secret => "<redacted: secret>",
            Self::QuotaToken => "<redacted: quota-token>",
        }
    }
}

/// Replace every host-managed capability value (see [`HostManagedKind`]) with a
/// plain string placeholder, recursing through every container.
///
/// This is a lossy, deterministic transform used at untrusted export boundaries
/// (the public oplog / oplog-processor plugin interface) where a capability must
/// neither leak its trusted snapshot nor be lowered to a live owned handle.
/// Quota tokens have no value representation in the WIT wire form other than an
/// owned handle, so the only safe rendering is an ordinary string. Secrets carry
/// a `secret-ref` reference that is likewise capability-identifying authority
/// data, so they are redacted the same way.
///
/// The result is a well-formed [`SchemaValue`] in which every capability leaf
/// has become a [`SchemaValue::String`]. Pair it with
/// [`redact_host_managed_type`] (via [`redact_host_managed_typed_value`]) to keep
/// a typed value's schema graph and value tree kind-consistent.
pub fn redact_host_managed_value(value: SchemaValue) -> SchemaValue {
    if let Some(kind) = HostManagedKind::from_value(&value) {
        return SchemaValue::String(kind.redacted_placeholder().to_string());
    }

    match value {
        SchemaValue::Record { fields } => SchemaValue::Record {
            fields: fields.into_iter().map(redact_host_managed_value).collect(),
        },
        SchemaValue::Variant(p) => SchemaValue::Variant(VariantValuePayload {
            case: p.case,
            payload: p
                .payload
                .map(|inner| Box::new(redact_host_managed_value(*inner))),
        }),
        SchemaValue::Tuple { elements } => SchemaValue::Tuple {
            elements: elements
                .into_iter()
                .map(redact_host_managed_value)
                .collect(),
        },
        SchemaValue::List { elements } => SchemaValue::List {
            elements: elements
                .into_iter()
                .map(redact_host_managed_value)
                .collect(),
        },
        SchemaValue::FixedList { elements } => SchemaValue::FixedList {
            elements: elements
                .into_iter()
                .map(redact_host_managed_value)
                .collect(),
        },
        SchemaValue::Map { entries } => SchemaValue::Map {
            entries: entries
                .into_iter()
                .map(|(k, v)| (redact_host_managed_value(k), redact_host_managed_value(v)))
                .collect(),
        },
        SchemaValue::Option { inner } => SchemaValue::Option {
            inner: inner.map(|inner| Box::new(redact_host_managed_value(*inner))),
        },
        SchemaValue::Result(r) => SchemaValue::Result(match r {
            ResultValuePayload::Ok { value } => ResultValuePayload::Ok {
                value: value.map(|v| Box::new(redact_host_managed_value(*v))),
            },
            ResultValuePayload::Err { value } => ResultValuePayload::Err {
                value: value.map(|v| Box::new(redact_host_managed_value(*v))),
            },
        }),
        SchemaValue::Union(u) => SchemaValue::Union(UnionValuePayload {
            tag: u.tag,
            body: Box::new(redact_host_managed_value(*u.body)),
        }),
        // Capability leaves were handled above; everything else is a
        // non-capability leaf with no nested capability material.
        other => other,
    }
}

/// Replace every host-managed capability type (see [`HostManagedKind`]) with a
/// plain `string` type, recursing through every container and named definition.
///
/// Keeps the schema graph kind-consistent with the value tree produced by
/// [`redact_host_managed_value`], so a redacted [`TypedSchemaValue`] still
/// validates and renders through kind-paired walkers. The replacement uses a
/// default metadata envelope so capability-specific specs and example material
/// never cross the export boundary.
pub fn redact_host_managed_type(ty: SchemaType) -> SchemaType {
    if HostManagedKind::from_type(&ty).is_some() {
        return SchemaType::String {
            metadata: crate::schema::metadata::MetadataEnvelope::default(),
        };
    }

    match ty {
        SchemaType::Record { fields, metadata } => SchemaType::Record {
            fields: fields
                .into_iter()
                .map(|f| crate::schema::schema_type::NamedFieldType {
                    name: f.name,
                    body: redact_host_managed_type(f.body),
                    metadata: f.metadata,
                })
                .collect(),
            metadata,
        },
        SchemaType::Variant { cases, metadata } => SchemaType::Variant {
            cases: cases
                .into_iter()
                .map(|c| crate::schema::schema_type::VariantCaseType {
                    name: c.name,
                    payload: c.payload.map(redact_host_managed_type),
                    metadata: c.metadata,
                })
                .collect(),
            metadata,
        },
        SchemaType::Tuple { elements, metadata } => SchemaType::Tuple {
            elements: elements.into_iter().map(redact_host_managed_type).collect(),
            metadata,
        },
        SchemaType::List { element, metadata } => SchemaType::List {
            element: Box::new(redact_host_managed_type(*element)),
            metadata,
        },
        SchemaType::FixedList {
            element,
            length,
            metadata,
        } => SchemaType::FixedList {
            element: Box::new(redact_host_managed_type(*element)),
            length,
            metadata,
        },
        SchemaType::Map {
            key,
            value,
            metadata,
        } => SchemaType::Map {
            key: Box::new(redact_host_managed_type(*key)),
            value: Box::new(redact_host_managed_type(*value)),
            metadata,
        },
        SchemaType::Option { inner, metadata } => SchemaType::Option {
            inner: Box::new(redact_host_managed_type(*inner)),
            metadata,
        },
        SchemaType::Result { spec, metadata } => SchemaType::Result {
            spec: crate::schema::schema_type::ResultSpec {
                ok: spec.ok.map(|t| Box::new(redact_host_managed_type(*t))),
                err: spec.err.map(|t| Box::new(redact_host_managed_type(*t))),
            },
            metadata,
        },
        SchemaType::Union { spec, metadata } => SchemaType::Union {
            spec: crate::schema::schema_type::UnionSpec {
                branches: spec
                    .branches
                    .into_iter()
                    .map(|b| crate::schema::schema_type::UnionBranch {
                        tag: b.tag,
                        body: redact_host_managed_type(b.body),
                        discriminator: b.discriminator,
                        metadata: b.metadata,
                    })
                    .collect(),
            },
            metadata,
        },
        SchemaType::Future { inner, metadata } => SchemaType::Future {
            inner: inner.map(|t| Box::new(redact_host_managed_type(*t))),
            metadata,
        },
        SchemaType::Stream { inner, metadata } => SchemaType::Stream {
            inner: inner.map(|t| Box::new(redact_host_managed_type(*t))),
            metadata,
        },
        // Capability nodes were handled above; refs, primitives, and rich
        // semantic leaves carry no nested capability material.
        other => other,
    }
}

/// Produce a redacted copy of a [`TypedSchemaValue`] in which every host-managed
/// capability node — in both the schema graph and the value tree — becomes a
/// plain string. See [`redact_host_managed_value`] / [`redact_host_managed_type`].
pub fn redact_host_managed_typed_value(
    typed: crate::schema::graph::TypedSchemaValue,
) -> crate::schema::graph::TypedSchemaValue {
    let (graph, value) = typed.into_parts();
    let graph = crate::schema::graph::SchemaGraph {
        defs: graph
            .defs
            .into_iter()
            .map(|d| crate::schema::graph::SchemaTypeDef {
                id: d.id,
                name: d.name,
                body: redact_host_managed_type(d.body),
            })
            .collect(),
        root: redact_host_managed_type(graph.root),
    };
    crate::schema::graph::TypedSchemaValue::new(graph, redact_host_managed_value(value))
}

/// Wraps a [`SchemaValue`] so its `Debug` output redacts every host-managed
/// capability node (see [`HostManagedKind`]).
///
/// Use this at tracing / diagnostic / error-formatting boundaries that would
/// otherwise dump raw capability references or lease snapshots into logs. The
/// derived `Debug` of [`SchemaValue`] is left untouched so internal code can
/// still inspect raw values; redaction is opt-in at the observability boundary.
pub struct RedactedSchemaValue<'a>(&'a SchemaValue);

/// Wraps `value` for redacted `Debug` rendering. See [`RedactedSchemaValue`].
pub fn redacted_schema_value_debug(value: &SchemaValue) -> RedactedSchemaValue<'_> {
    RedactedSchemaValue(value)
}

impl fmt::Debug for RedactedSchemaValue<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_redacted(self.0, f)
    }
}

fn fmt_redacted(value: &SchemaValue, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if let Some(kind) = HostManagedKind::from_value(value) {
        return f.write_str(kind.redacted_placeholder());
    }

    match value {
        SchemaValue::Record { fields } => {
            f.write_str("Record { fields: ")?;
            fmt_seq(fields, f)?;
            f.write_str(" }")
        }
        SchemaValue::Variant(p) => {
            write!(f, "Variant {{ case: {}, payload: ", p.case)?;
            fmt_opt(p.payload.as_deref(), f)?;
            f.write_str(" }")
        }
        SchemaValue::Tuple { elements } => {
            f.write_str("Tuple { elements: ")?;
            fmt_seq(elements, f)?;
            f.write_str(" }")
        }
        SchemaValue::List { elements } => {
            f.write_str("List { elements: ")?;
            fmt_seq(elements, f)?;
            f.write_str(" }")
        }
        SchemaValue::FixedList { elements } => {
            f.write_str("FixedList { elements: ")?;
            fmt_seq(elements, f)?;
            f.write_str(" }")
        }
        SchemaValue::Map { entries } => {
            f.write_str("Map { entries: [")?;
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                f.write_str("(")?;
                fmt_redacted(k, f)?;
                f.write_str(", ")?;
                fmt_redacted(v, f)?;
                f.write_str(")")?;
            }
            f.write_str("] }")
        }
        SchemaValue::Option { inner } => {
            f.write_str("Option { inner: ")?;
            fmt_opt(inner.as_deref(), f)?;
            f.write_str(" }")
        }
        SchemaValue::Result(r) => match r {
            crate::schema::schema_value::ResultValuePayload::Ok { value } => {
                f.write_str("Ok(")?;
                fmt_opt(value.as_deref(), f)?;
                f.write_str(")")
            }
            crate::schema::schema_value::ResultValuePayload::Err { value } => {
                f.write_str("Err(")?;
                fmt_opt(value.as_deref(), f)?;
                f.write_str(")")
            }
        },
        SchemaValue::Union(u) => {
            write!(f, "Union {{ tag: {:?}, body: ", u.tag)?;
            fmt_redacted(&u.body, f)?;
            f.write_str(" }")
        }
        // Capability nodes are handled above. Everything else is a
        // non-capability leaf with no nested capability material, so its
        // derived `Debug` is safe to emit verbatim.
        other => write!(f, "{other:?}"),
    }
}

fn fmt_seq(values: &[SchemaValue], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str("[")?;
    for (i, v) in values.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        fmt_redacted(v, f)?;
    }
    f.write_str("]")
}

fn fmt_opt(value: Option<&SchemaValue>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match value {
        Some(v) => {
            f.write_str("Some(")?;
            fmt_redacted(v, f)?;
            f.write_str(")")
        }
        None => f.write_str("None"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EnvironmentId;
    use crate::schema::schema_type::{QuotaTokenSpec, SecretSpec};
    use crate::schema::schema_value::QuotaTokenValuePayload;
    use chrono::{TimeZone, Utc};
    use test_r::test;

    fn secret_value() -> SchemaValue {
        crate::schema::conversion::secret_to_value("shhh-do-not-log".to_string())
    }

    fn quota_token_value() -> SchemaValue {
        SchemaValue::QuotaToken(QuotaTokenValuePayload {
            environment_id: EnvironmentId::new(uuid::Uuid::nil()),
            resource_name: "gpu-quota".to_string(),
            expected_use: 1,
            last_credit: 0,
            last_credit_at: Utc.timestamp_opt(0, 0).unwrap(),
        })
    }

    #[test]
    fn classifies_capability_types_and_values() {
        assert_eq!(
            HostManagedKind::from_type(&SchemaType::secret(SecretSpec::default())),
            Some(HostManagedKind::Secret)
        );
        assert_eq!(
            HostManagedKind::from_type(&SchemaType::quota_token(QuotaTokenSpec::default())),
            Some(HostManagedKind::QuotaToken)
        );
        assert_eq!(HostManagedKind::from_type(&SchemaType::string()), None);

        assert_eq!(
            HostManagedKind::from_value(&secret_value()),
            Some(HostManagedKind::Secret)
        );
        assert_eq!(
            HostManagedKind::from_value(&quota_token_value()),
            Some(HostManagedKind::QuotaToken)
        );
        assert_eq!(HostManagedKind::from_value(&SchemaValue::U32(7)), None);
    }

    #[test]
    fn kind_names_and_placeholders_are_stable() {
        assert_eq!(HostManagedKind::Secret.kind_name(), "secret");
        assert_eq!(HostManagedKind::QuotaToken.kind_name(), "quota-token");
        assert_eq!(
            HostManagedKind::Secret.redacted_placeholder(),
            "<redacted: secret>"
        );
        assert_eq!(
            HostManagedKind::QuotaToken.redacted_placeholder(),
            "<redacted: quota-token>"
        );
    }

    #[test]
    fn redacted_debug_hides_secret_material() {
        let rendered = format!("{:?}", redacted_schema_value_debug(&secret_value()));
        assert_eq!(rendered, "<redacted: secret>");
        assert!(!rendered.contains("shhh-do-not-log"));
    }

    #[test]
    fn redacted_debug_hides_quota_token_snapshot() {
        let rendered = format!("{:?}", redacted_schema_value_debug(&quota_token_value()));
        assert_eq!(rendered, "<redacted: quota-token>");
        assert!(!rendered.contains("gpu-quota"));
    }

    #[test]
    fn redacted_debug_recurses_through_all_container_paths() {
        use crate::schema::schema_value::{
            ResultValuePayload, UnionValuePayload, VariantValuePayload,
        };

        let cases: Vec<SchemaValue> = vec![
            // Variant payload.
            SchemaValue::Variant(VariantValuePayload {
                case: 0,
                payload: Some(Box::new(secret_value())),
            }),
            // Tuple / fixed-list elements.
            SchemaValue::Tuple {
                elements: vec![quota_token_value()],
            },
            SchemaValue::FixedList {
                elements: vec![secret_value()],
            },
            // Map key and value.
            SchemaValue::Map {
                entries: vec![(secret_value(), quota_token_value())],
            },
            // Result Ok / Err payloads.
            SchemaValue::Result(ResultValuePayload::Ok {
                value: Some(Box::new(secret_value())),
            }),
            SchemaValue::Result(ResultValuePayload::Err {
                value: Some(Box::new(quota_token_value())),
            }),
            // Union body.
            SchemaValue::Union(UnionValuePayload {
                tag: "b".to_string(),
                body: Box::new(secret_value()),
            }),
        ];

        for value in &cases {
            let rendered = format!("{:?}", redacted_schema_value_debug(value));
            assert!(
                !rendered.contains("shhh-do-not-log"),
                "secret leaked through {value:?}: {rendered}"
            );
            assert!(
                !rendered.contains("gpu-quota"),
                "quota token leaked through {value:?}: {rendered}"
            );
            assert!(
                rendered.contains("<redacted:"),
                "expected a redacted placeholder in {rendered}"
            );
        }
    }

    #[test]
    fn redacted_debug_recurses_into_containers() {
        let value = SchemaValue::Record {
            fields: vec![
                SchemaValue::String("svc".to_string()),
                SchemaValue::List {
                    elements: vec![secret_value(), quota_token_value()],
                },
                SchemaValue::Option {
                    inner: Some(Box::new(secret_value())),
                },
            ],
        };
        let rendered = format!("{:?}", redacted_schema_value_debug(&value));
        assert!(!rendered.contains("shhh-do-not-log"), "{rendered}");
        assert!(!rendered.contains("gpu-quota"), "{rendered}");
        assert!(rendered.contains("<redacted: secret>"), "{rendered}");
        assert!(rendered.contains("<redacted: quota-token>"), "{rendered}");
        // Non-capability material is preserved.
        assert!(rendered.contains("svc"), "{rendered}");
    }

    #[test]
    fn redact_value_replaces_capability_leaves_with_strings() {
        assert_eq!(
            redact_host_managed_value(quota_token_value()),
            SchemaValue::String("<redacted: quota-token>".to_string())
        );
        assert_eq!(
            redact_host_managed_value(secret_value()),
            SchemaValue::String("<redacted: secret>".to_string())
        );
    }

    #[test]
    fn redact_value_preserves_structure_and_redacts_nested() {
        let value = SchemaValue::Record {
            fields: vec![
                SchemaValue::String("svc".to_string()),
                SchemaValue::List {
                    elements: vec![secret_value(), quota_token_value()],
                },
                SchemaValue::Option {
                    inner: Some(Box::new(quota_token_value())),
                },
            ],
        };
        let redacted = redact_host_managed_value(value);
        assert_eq!(
            redacted,
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("svc".to_string()),
                    SchemaValue::List {
                        elements: vec![
                            SchemaValue::String("<redacted: secret>".to_string()),
                            SchemaValue::String("<redacted: quota-token>".to_string()),
                        ],
                    },
                    SchemaValue::Option {
                        inner: Some(Box::new(SchemaValue::String(
                            "<redacted: quota-token>".to_string()
                        ))),
                    },
                ],
            }
        );
    }

    #[test]
    fn redact_type_replaces_capability_types_with_plain_string() {
        assert_eq!(
            redact_host_managed_type(SchemaType::quota_token(QuotaTokenSpec::default())),
            SchemaType::string()
        );
        assert_eq!(
            redact_host_managed_type(SchemaType::secret(SecretSpec::default())),
            SchemaType::string()
        );
    }

    #[test]
    fn redact_typed_value_keeps_graph_and_value_kind_consistent() {
        use crate::schema::graph::{SchemaGraph, SchemaTypeDef, TypedSchemaValue};
        use crate::schema::metadata::{MetadataEnvelope, TypeId};
        use crate::schema::schema_type::NamedFieldType;

        // A named definition whose body is a capability type, referenced from
        // the root record alongside an inline capability field.
        let graph = SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: TypeId::new("cap"),
                name: None,
                body: SchemaType::quota_token(QuotaTokenSpec::default()),
            }],
            root: SchemaType::Record {
                fields: vec![
                    NamedFieldType {
                        name: "via_ref".to_string(),
                        body: SchemaType::Ref {
                            id: TypeId::new("cap"),
                            metadata: MetadataEnvelope::default(),
                        },
                        metadata: MetadataEnvelope::default(),
                    },
                    NamedFieldType {
                        name: "inline".to_string(),
                        body: SchemaType::secret(SecretSpec::default()),
                        metadata: MetadataEnvelope::default(),
                    },
                ],
                metadata: MetadataEnvelope::default(),
            },
        };
        let value = SchemaValue::Record {
            fields: vec![quota_token_value(), secret_value()],
        };

        let redacted = redact_host_managed_typed_value(TypedSchemaValue::new(graph, value));
        let (graph, value) = redacted.into_parts();

        // The referenced definition body is rewritten to a plain string.
        assert_eq!(graph.defs[0].body, SchemaType::string());

        // Inline capability type becomes a string; the ref is left intact and
        // resolves to the rewritten string definition.
        match &graph.root {
            SchemaType::Record { fields, .. } => {
                assert!(matches!(fields[0].body, SchemaType::Ref { .. }));
                assert_eq!(fields[1].body, SchemaType::string());
            }
            other => panic!("expected record root, got {other:?}"),
        }

        // Both capability values become plain string placeholders.
        assert_eq!(
            value,
            SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("<redacted: quota-token>".to_string()),
                    SchemaValue::String("<redacted: secret>".to_string()),
                ],
            }
        );
    }
}
