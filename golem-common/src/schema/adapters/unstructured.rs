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

//! Canonical schema-native representation of the ergonomic "unstructured
//! text / binary" wrappers.
//!
//! An unstructured-text value is either inline text or a URL that references
//! the text; an unstructured-binary value is either inline bytes or a URL.
//! Both are modelled as a **role-marked two-case `variant`**:
//!
//! ```text
//! variant {              // metadata.role = Role::UnstructuredText
//!   inline: text<…>,     // case 0
//!   url: url,            // case 1
//! }
//! ```
//!
//! (and the binary analogue with `inline: binary<…>` and
//! `Role::UnstructuredBinary`).
//!
//! This is the *single* canonical form: the guest SDKs publish it directly
//! (see `golem_rust::agentic::schema` and the TS SDK's `schema/rich.ts`), the
//! legacy `ElementSchema` adapter lowers to it, and both per-language bridge
//! generators detect it (via [`unstructured_text_restrictions`] /
//! [`unstructured_binary_restrictions`]) to emit the ergonomic wrapper types.
//! The role marker — not the bare `SchemaType::Text` / `SchemaType::Binary`
//! rich scalars — is what identifies the wrapper.

use crate::schema::adapters::error::{SchemaAdapterError, resolve_ref};
use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::Role;
use crate::schema::schema_type::{
    BinaryRestrictions, SchemaType, TextRestrictions, UrlRestrictions, VariantCaseType,
};
use crate::schema::schema_value::{SchemaValue, VariantValuePayload};

/// The case index of the `inline` alternative in the canonical variant.
pub const INLINE_CASE: u32 = 0;
/// The case index of the `url` alternative in the canonical variant.
pub const URL_CASE: u32 = 1;

const INLINE_CASE_NAME: &str = "inline";
const URL_CASE_NAME: &str = "url";

/// Build the canonical role-marked unstructured-text variant
/// `variant { inline: text<restrictions>, url: url } with Role::UnstructuredText`.
pub fn unstructured_text_schema_type(restrictions: TextRestrictions) -> SchemaType {
    let mut ty = SchemaType::variant(vec![
        VariantCaseType {
            name: INLINE_CASE_NAME.to_string(),
            payload: Some(SchemaType::text(restrictions)),
            metadata: Default::default(),
        },
        VariantCaseType {
            name: URL_CASE_NAME.to_string(),
            payload: Some(SchemaType::url(UrlRestrictions::default())),
            metadata: Default::default(),
        },
    ]);
    ty.metadata_mut().role = Some(Role::UnstructuredText);
    ty
}

/// Build the canonical role-marked unstructured-binary variant
/// `variant { inline: binary<restrictions>, url: url } with Role::UnstructuredBinary`.
pub fn unstructured_binary_schema_type(restrictions: BinaryRestrictions) -> SchemaType {
    let mut ty = SchemaType::variant(vec![
        VariantCaseType {
            name: INLINE_CASE_NAME.to_string(),
            payload: Some(SchemaType::binary(restrictions)),
            metadata: Default::default(),
        },
        VariantCaseType {
            name: URL_CASE_NAME.to_string(),
            payload: Some(SchemaType::url(UrlRestrictions::default())),
            metadata: Default::default(),
        },
    ]);
    ty.metadata_mut().role = Some(Role::UnstructuredBinary);
    ty
}

/// Whether `ty` is *itself* a role-marked unstructured-text/binary variant.
///
/// Graph-free: it inspects the node's own `metadata.role` and does **not**
/// resolve [`SchemaType::Ref`]s. Use it where no [`SchemaGraph`] is available
/// (e.g. the type-naming leaf/name classifiers); the guest SDKs and the legacy
/// adapter always publish the wrapper inline, so refs do not arise in practice.
pub fn is_unstructured_variant(ty: &SchemaType) -> bool {
    matches!(
        ty,
        SchemaType::Variant { metadata, .. }
            if matches!(
                metadata.role,
                Some(Role::UnstructuredText) | Some(Role::UnstructuredBinary)
            )
    )
}

/// If `ty` (resolved against `graph`) is the canonical role-marked
/// unstructured-**text** variant, return the inline text restrictions.
///
/// The role marker is authoritative, but the structural shape is also checked
/// (case 0 is an inline `text`, case 1 is a `url`) so a mis-marked variant is
/// rejected rather than silently mis-rendered.
pub fn unstructured_text_restrictions<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> Result<Option<&'a TextRestrictions>, SchemaAdapterError> {
    let resolved = resolve_ref(graph, ty)?;
    if resolved.metadata().role != Some(Role::UnstructuredText) {
        return Ok(None);
    }
    // The role marker is authoritative: a node tagged `Role::UnstructuredText`
    // that is not the canonical `variant { inline, url }` shape is a hard error,
    // not "merely not unstructured".
    let SchemaType::Variant { cases, .. } = resolved else {
        return Err(SchemaAdapterError::LossySchemaType(
            "Role::UnstructuredText must mark a variant type".into(),
        ));
    };
    let [inline, url] = cases.as_slice() else {
        return Err(SchemaAdapterError::LossySchemaType(
            "Role::UnstructuredText variant must have exactly an `inline` and a `url` case".into(),
        ));
    };
    if inline.name != INLINE_CASE_NAME || url.name != URL_CASE_NAME {
        return Err(SchemaAdapterError::LossySchemaType(
            "Role::UnstructuredText variant cases must be exactly `inline`, `url` in that order"
                .into(),
        ));
    }
    let inline_payload = inline.payload.as_ref().ok_or_else(|| {
        SchemaAdapterError::LossySchemaType(
            "Role::UnstructuredText `inline` case must carry a text payload".into(),
        )
    })?;
    let SchemaType::Text { restrictions, .. } = resolve_ref(graph, inline_payload)? else {
        return Err(SchemaAdapterError::LossySchemaType(
            "Role::UnstructuredText `inline` case payload must be a text type".into(),
        ));
    };
    check_url_case(graph, url, "Role::UnstructuredText")?;
    Ok(Some(restrictions))
}

/// If `ty` (resolved against `graph`) is the canonical role-marked
/// unstructured-**binary** variant, return the inline binary restrictions.
pub fn unstructured_binary_restrictions<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> Result<Option<&'a BinaryRestrictions>, SchemaAdapterError> {
    let resolved = resolve_ref(graph, ty)?;
    if resolved.metadata().role != Some(Role::UnstructuredBinary) {
        return Ok(None);
    }
    // The role marker is authoritative: a node tagged `Role::UnstructuredBinary`
    // that is not the canonical `variant { inline, url }` shape is a hard error,
    // not "merely not unstructured".
    let SchemaType::Variant { cases, .. } = resolved else {
        return Err(SchemaAdapterError::LossySchemaType(
            "Role::UnstructuredBinary must mark a variant type".into(),
        ));
    };
    let [inline, url] = cases.as_slice() else {
        return Err(SchemaAdapterError::LossySchemaType(
            "Role::UnstructuredBinary variant must have exactly an `inline` and a `url` case".into(),
        ));
    };
    if inline.name != INLINE_CASE_NAME || url.name != URL_CASE_NAME {
        return Err(SchemaAdapterError::LossySchemaType(
            "Role::UnstructuredBinary variant cases must be exactly `inline`, `url` in that order"
                .into(),
        ));
    }
    let inline_payload = inline.payload.as_ref().ok_or_else(|| {
        SchemaAdapterError::LossySchemaType(
            "Role::UnstructuredBinary `inline` case must carry a binary payload".into(),
        )
    })?;
    let SchemaType::Binary { restrictions, .. } = resolve_ref(graph, inline_payload)? else {
        return Err(SchemaAdapterError::LossySchemaType(
            "Role::UnstructuredBinary `inline` case payload must be a binary type".into(),
        ));
    };
    check_url_case(graph, url, "Role::UnstructuredBinary")?;
    Ok(Some(restrictions))
}

fn check_url_case(
    graph: &SchemaGraph,
    url: &VariantCaseType,
    role: &str,
) -> Result<(), SchemaAdapterError> {
    let url_payload = url.payload.as_ref().ok_or_else(|| {
        SchemaAdapterError::LossySchemaType(format!("{role} `url` case must carry a url payload"))
    })?;
    if !matches!(resolve_ref(graph, url_payload)?, SchemaType::Url { .. }) {
        return Err(SchemaAdapterError::LossySchemaType(format!(
            "{role} `url` case payload must be a url type"
        )));
    }
    Ok(())
}

/// Which flavour of unstructured wrapper a type is, together with the inline
/// case's restrictions.
pub enum UnstructuredKind<'a> {
    /// `variant { inline: text<…>, url } with Role::UnstructuredText`.
    Text(&'a TextRestrictions),
    /// `variant { inline: binary<…>, url } with Role::UnstructuredBinary`.
    Binary(&'a BinaryRestrictions),
}

/// If `ty` (resolved against `graph`) is a canonical role-marked unstructured
/// variant, classify it as text or binary and return the inline restrictions.
///
/// This is the single classifier the request/response and MCP seams branch on,
/// so they never re-match the variant shape (or its role marker) by hand.
pub fn unstructured_kind<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> Result<Option<UnstructuredKind<'a>>, SchemaAdapterError> {
    if let Some(restrictions) = unstructured_text_restrictions(graph, ty)? {
        Ok(Some(UnstructuredKind::Text(restrictions)))
    } else if let Some(restrictions) = unstructured_binary_restrictions(graph, ty)? {
        Ok(Some(UnstructuredKind::Binary(restrictions)))
    } else {
        Ok(None)
    }
}

/// Read the binary restrictions from a request/response body root that is
/// *known* to carry unstructured binary — either the canonical
/// `variant { inline, url }` wrapper or a bare `Binary` rich scalar.
///
/// Unlike [`unstructured_binary_restrictions`] (which returns `None` for a
/// non-wrapper), this errors if the root is neither a binary wrapper nor a bare
/// binary scalar, so a malformed compiled `BinaryBody` schema fails loudly
/// rather than silently degrading to "unrestricted". Used by the HTTP request
/// decoder and the OpenAPI emitter so both read restrictions identically.
pub fn binary_body_restrictions<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> Result<&'a BinaryRestrictions, SchemaAdapterError> {
    if let Some(restrictions) = unstructured_binary_restrictions(graph, ty)? {
        return Ok(restrictions);
    }
    match resolve_ref(graph, ty)? {
        SchemaType::Binary { restrictions, .. } => Ok(restrictions),
        _ => Err(SchemaAdapterError::LossySchemaType(
            "binary body schema root is neither an unstructured-binary wrapper nor a bare binary type"
                .into(),
        )),
    }
}

/// Read the text restrictions from a request/response body root that is *known*
/// to carry unstructured text — either the canonical `variant { inline, url }`
/// wrapper or a bare `Text` rich scalar.
///
/// The text analogue of [`binary_body_restrictions`]; errors if the root is
/// neither a text wrapper nor a bare text scalar.
pub fn text_body_restrictions<'a>(
    graph: &'a SchemaGraph,
    ty: &'a SchemaType,
) -> Result<&'a TextRestrictions, SchemaAdapterError> {
    if let Some(restrictions) = unstructured_text_restrictions(graph, ty)? {
        return Ok(restrictions);
    }
    match resolve_ref(graph, ty)? {
        SchemaType::Text { restrictions, .. } => Ok(restrictions),
        _ => Err(SchemaAdapterError::LossySchemaType(
            "text body schema root is neither an unstructured-text wrapper nor a bare text type"
                .into(),
        )),
    }
}

/// Which flavour of unstructured payload a type is, irrespective of whether it
/// is carried by a canonical `variant { inline, url }` wrapper or a bare
/// `Text` / `Binary` rich scalar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnstructuredPayloadKind {
    Text,
    Binary,
}

/// Classify `ty` (resolved against `graph`) as an unstructured text/binary
/// payload — either a canonical `variant { inline, url }` wrapper or a bare
/// `Text` / `Binary` rich scalar — returning which flavour, or `None`.
///
/// This is the single classifier the request/response and MCP seams branch on
/// when they need to treat *both* shapes uniformly (the wrapper is the
/// canonical form, but a bare rich scalar is still accepted). Callers that need
/// the inline restrictions use [`unstructured_kind`] directly.
pub fn unstructured_or_raw_kind(
    graph: &SchemaGraph,
    ty: &SchemaType,
) -> Result<Option<UnstructuredPayloadKind>, SchemaAdapterError> {
    match unstructured_kind(graph, ty)? {
        Some(UnstructuredKind::Text(_)) => return Ok(Some(UnstructuredPayloadKind::Text)),
        Some(UnstructuredKind::Binary(_)) => return Ok(Some(UnstructuredPayloadKind::Binary)),
        None => {}
    }
    Ok(match resolve_ref(graph, ty)? {
        SchemaType::Text { .. } => Some(UnstructuredPayloadKind::Text),
        SchemaType::Binary { .. } => Some(UnstructuredPayloadKind::Binary),
        _ => None,
    })
}

/// Wrap a raw `text` / `binary` value as the canonical unstructured `inline`
/// case (case 0) when `ty` (resolved against `graph`) is an unstructured
/// `variant { inline, url }` wrapper; otherwise return the raw value unchanged
/// (for a bare `Text` / `Binary` rich scalar).
///
/// This is the inverse of [`decode_unstructured_value`]'s inline case: callers
/// that turn a raw transport payload into a schema value (e.g. an HTTP request
/// body) use it so the produced value matches the field's schema regardless of
/// whether that schema is the wrapper or the bare rich scalar.
pub fn wrap_unstructured_inline_for_schema(
    graph: &SchemaGraph,
    ty: &SchemaType,
    raw: SchemaValue,
) -> Result<SchemaValue, SchemaAdapterError> {
    if unstructured_kind(graph, ty)?.is_some() {
        Ok(unstructured_inline_value(raw))
    } else {
        Ok(raw)
    }
}

/// A decoded unstructured *output* value, classified against its schema.
pub enum UnstructuredOutput<'a> {
    /// An inline payload, validated to match the schema's kind: guaranteed to
    /// be a `SchemaValue::Text` for a text output or `SchemaValue::Binary` for a
    /// binary output (whether the schema is the canonical wrapper or a bare
    /// rich scalar).
    Inline(&'a SchemaValue),
    /// A URL reference (only the canonical wrapper's `url` case yields this).
    Url(&'a str),
}

/// Decode an unstructured *output* value against its schema `ty`, accepting both
/// the canonical `variant { inline, url }` wrapper and a bare `Text` / `Binary`
/// rich scalar, and validating that the runtime `value` matches the declared
/// schema kind.
///
/// Returns `Ok(None)` when `ty` is not an unstructured carrier, so the caller
/// falls back to component-model rendering. Returns an error when `ty` *is* an
/// unstructured carrier but `value` does not match it (e.g. an
/// unstructured-text wrapper whose `inline` payload is binary, or a bare `Text`
/// schema paired with a non-text value) — the boundary surfaces the mismatch
/// rather than silently rendering by value shape.
///
/// This is the single schema-driven classifier the HTTP-response and MCP
/// tool/resource output seams branch on, so none of them re-derive the
/// wrapper-vs-bare / inline-vs-url / value-kind logic by hand.
pub fn decode_unstructured_output<'a>(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &'a SchemaValue,
) -> Result<Option<UnstructuredOutput<'a>>, SchemaAdapterError> {
    // Canonical wrapper schema: accept *either* a wrapper runtime value
    // (`variant { inline, url }`) or a raw `Text` / `Binary` rich scalar (DE:
    // both runtime representations are valid under a wrapper schema). The `url`
    // case can only ever arrive via the wrapper value path — a bare scalar
    // never yields a redirect.
    if let Some(kind) = unstructured_kind(graph, ty)? {
        let expected = match kind {
            UnstructuredKind::Text(_) => UnstructuredPayloadKind::Text,
            UnstructuredKind::Binary(_) => UnstructuredPayloadKind::Binary,
        };
        return Ok(Some(match value {
            SchemaValue::Variant(_) => match decode_unstructured_value(value)? {
                UnstructuredValueCase::Url(url) => UnstructuredOutput::Url(url),
                UnstructuredValueCase::Inline(inline) => {
                    check_unstructured_value_kind(inline, expected)?;
                    UnstructuredOutput::Inline(inline)
                }
            },
            raw => {
                check_unstructured_value_kind(raw, expected)?;
                UnstructuredOutput::Inline(raw)
            }
        }));
    }

    // Bare `Text` / `Binary` rich scalar: the value must match the schema kind.
    let expected = match resolve_ref(graph, ty)? {
        SchemaType::Text { .. } => UnstructuredPayloadKind::Text,
        SchemaType::Binary { .. } => UnstructuredPayloadKind::Binary,
        _ => return Ok(None),
    };
    check_unstructured_value_kind(value, expected)?;
    Ok(Some(UnstructuredOutput::Inline(value)))
}

fn check_unstructured_value_kind(
    value: &SchemaValue,
    expected: UnstructuredPayloadKind,
) -> Result<(), SchemaAdapterError> {
    let matches = match expected {
        UnstructuredPayloadKind::Text => matches!(value, SchemaValue::Text(_)),
        UnstructuredPayloadKind::Binary => matches!(value, SchemaValue::Binary(_)),
    };
    if matches {
        Ok(())
    } else {
        let kind = match expected {
            UnstructuredPayloadKind::Text => "text",
            UnstructuredPayloadKind::Binary => "binary",
        };
        Err(SchemaAdapterError::LossySchemaType(format!(
            "expected a {kind} value for an unstructured {kind} output"
        )))
    }
}

/// The selected case of a canonical unstructured `variant { inline, url }` value.
pub enum UnstructuredValueCase<'a> {
    /// The `inline` case (case 0): the inner `text` / `binary` value payload.
    Inline(&'a SchemaValue),
    /// The `url` case (case 1): the referenced URL.
    Url(&'a str),
}

/// Interpret a [`SchemaValue`] as a canonical unstructured `variant { inline, url }`
/// value, returning the selected case.
///
/// Used by every consumer that turns an unstructured value into a transport
/// representation (HTTP body / redirect, MCP content / resource link) so the
/// inline-vs-url dispatch lives in exactly one place.
pub fn decode_unstructured_value(
    value: &SchemaValue,
) -> Result<UnstructuredValueCase<'_>, SchemaAdapterError> {
    let SchemaValue::Variant(VariantValuePayload { case, payload }) = value else {
        return Err(SchemaAdapterError::LossySchemaType(
            "expected a variant value for an unstructured text/binary type".into(),
        ));
    };
    match *case {
        INLINE_CASE => {
            let payload = payload.as_deref().ok_or_else(|| {
                SchemaAdapterError::LossySchemaType(
                    "unstructured `inline` value must carry a payload".into(),
                )
            })?;
            Ok(UnstructuredValueCase::Inline(payload))
        }
        URL_CASE => {
            let payload = payload.as_deref().ok_or_else(|| {
                SchemaAdapterError::LossySchemaType(
                    "unstructured `url` value must carry a payload".into(),
                )
            })?;
            let SchemaValue::Url { url } = payload else {
                return Err(SchemaAdapterError::LossySchemaType(
                    "unstructured `url` value payload must be a url".into(),
                ));
            };
            Ok(UnstructuredValueCase::Url(url))
        }
        other => Err(SchemaAdapterError::LossySchemaType(format!(
            "unknown unstructured variant case {other}"
        ))),
    }
}

/// Build a canonical unstructured `inline` value (case 0) wrapping the inner
/// `text` / `binary` value payload.
pub fn unstructured_inline_value(inline: SchemaValue) -> SchemaValue {
    SchemaValue::Variant(VariantValuePayload {
        case: INLINE_CASE,
        payload: Some(Box::new(inline)),
    })
}

/// Build a canonical unstructured `url` value (case 1).
pub fn unstructured_url_value(url: String) -> SchemaValue {
    SchemaValue::Variant(VariantValuePayload {
        case: URL_CASE,
        payload: Some(Box::new(SchemaValue::Url { url })),
    })
}
