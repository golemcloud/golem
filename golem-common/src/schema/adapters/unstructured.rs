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
