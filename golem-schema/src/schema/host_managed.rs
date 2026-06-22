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
use crate::schema::schema_value::SchemaValue;
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
    use crate::schema::schema_value::{QuotaTokenValuePayload, SecretValuePayload};
    use chrono::{TimeZone, Utc};
    use test_r::test;

    fn secret_value() -> SchemaValue {
        SchemaValue::Secret(SecretValuePayload {
            secret_ref: "shhh-do-not-log".to_string(),
        })
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
}
