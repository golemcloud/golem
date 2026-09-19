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

use crate::schema::graph::{SchemaGraph, reachable_defs};
use crate::schema::metadata::{MetadataEnvelope, Role};
use crate::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, NumericBound, NumericRestrictions, PathDirection,
    PathKind, QuantityValue, SchemaType, TextRestrictions, UrlRestrictions,
};
use crate::schema::validation::validate_graph;
use crate::schema::{FromSchema, FromSchemaError, IntoSchema, SchemaBuilder, SchemaValue, TypeId};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const FINGERPRINT_DOMAIN: &str = "golem-schema-fingerprint";
const FORMAT_VERSION: u64 = 1;

/// BLAKE3-256 digest of the v1 deterministic-CBOR encoding of a stream element schema closure.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
#[serde(transparent)]
pub struct SchemaFingerprintV1(pub [u8; 32]);

impl SchemaFingerprintV1 {
    pub const FORMAT_VERSION: u8 = 1;

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(self) -> String {
        blake3::Hash::from_bytes(self.0).to_hex().to_string()
    }
}

impl std::fmt::Display for SchemaFingerprintV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&blake3::Hash::from_bytes(self.0).to_hex())
    }
}

impl IntoSchema for SchemaFingerprintV1 {
    fn type_id() -> TypeId {
        TypeId::new("golem_schema.SchemaFingerprintV1")
    }

    fn register_in(builder: &mut SchemaBuilder) -> SchemaType {
        <Vec<u8> as IntoSchema>::register_in(builder)
    }

    fn to_value(&self) -> SchemaValue {
        self.0.to_vec().to_value()
    }
}

impl FromSchema for SchemaFingerprintV1 {
    fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
        let bytes = Vec::<u8>::from_value(value)?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|bytes: Vec<u8>| {
            FromSchemaError::custom(format!(
                "schema fingerprint must have 32 bytes, got {}",
                bytes.len()
            ))
        })?;
        Ok(Self(bytes))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SchemaFingerprintError {
    #[error("invalid stream element schema: {0}")]
    InvalidSchema(String),
    #[error("duplicate value `{value}` in set-valued schema field `{field}`")]
    DuplicateSetValue { field: &'static str, value: String },
}

/// Computes the v1 stream element schema fingerprint.
///
/// `None` represents the Component Model stream unit element. The input graph may be the
/// multi-root definition registry of an agent schema; only definitions reachable from `element`
/// are encoded.
pub fn schema_fingerprint_v1(
    graph: &SchemaGraph,
    element: Option<&SchemaType>,
) -> Result<SchemaFingerprintV1, SchemaFingerprintError> {
    let bytes = canonical_schema_bytes_v1(graph, element)?;
    Ok(SchemaFingerprintV1(*blake3::hash(&bytes).as_bytes()))
}

/// Resolves a stream element fingerprint against a pinned multi-root schema registry.
///
/// The returned graph is self-contained: its root is the matching element type and its
/// definitions are the transitive closure reachable from that root. A unit stream element is
/// represented by an empty tuple root.
pub fn resolve_stream_element_schema_v1(
    graph: &SchemaGraph,
    fingerprint: SchemaFingerprintV1,
) -> Result<Option<SchemaGraph>, SchemaFingerprintError> {
    let mut elements = Vec::new();
    collect_stream_elements(&graph.root, &mut elements);
    for definition in &graph.defs {
        collect_stream_elements(&definition.body, &mut elements);
    }
    for element in elements {
        if schema_fingerprint_v1(graph, element)? == fingerprint {
            let root = element.cloned().unwrap_or_else(synthetic_unit);
            return Ok(Some(SchemaGraph {
                defs: reachable_defs(graph, &root),
                root,
            }));
        }
    }
    Ok(None)
}

fn collect_stream_elements<'a>(ty: &'a SchemaType, elements: &mut Vec<Option<&'a SchemaType>>) {
    match ty {
        SchemaType::Record { fields, .. } => {
            for field in fields {
                collect_stream_elements(&field.body, elements);
            }
        }
        SchemaType::Variant { cases, .. } => {
            for case in cases {
                if let Some(payload) = &case.payload {
                    collect_stream_elements(payload, elements);
                }
            }
        }
        SchemaType::Tuple {
            elements: tuple_elements,
            ..
        } => {
            for element in tuple_elements {
                collect_stream_elements(element, elements);
            }
        }
        SchemaType::List { element, .. } | SchemaType::FixedList { element, .. } => {
            collect_stream_elements(element, elements);
        }
        SchemaType::Map { key, value, .. } => {
            collect_stream_elements(key, elements);
            collect_stream_elements(value, elements);
        }
        SchemaType::Option { inner, .. } => collect_stream_elements(inner, elements),
        SchemaType::Result { spec, .. } => {
            if let Some(ok) = &spec.ok {
                collect_stream_elements(ok, elements);
            }
            if let Some(err) = &spec.err {
                collect_stream_elements(err, elements);
            }
        }
        SchemaType::Union { spec, .. } => {
            for branch in &spec.branches {
                collect_stream_elements(&branch.body, elements);
            }
        }
        SchemaType::Future { inner, .. } => {
            if let Some(inner) = inner {
                collect_stream_elements(inner, elements);
            }
        }
        SchemaType::Stream { inner, .. } => {
            elements.push(inner.as_deref());
            if let Some(inner) = inner {
                collect_stream_elements(inner, elements);
            }
        }
        SchemaType::Secret { spec, .. } => collect_stream_elements(&spec.inner, elements),
        SchemaType::Ref { .. }
        | SchemaType::Bool { .. }
        | SchemaType::S8 { .. }
        | SchemaType::S16 { .. }
        | SchemaType::S32 { .. }
        | SchemaType::S64 { .. }
        | SchemaType::U8 { .. }
        | SchemaType::U16 { .. }
        | SchemaType::U32 { .. }
        | SchemaType::U64 { .. }
        | SchemaType::F32 { .. }
        | SchemaType::F64 { .. }
        | SchemaType::Char { .. }
        | SchemaType::String { .. }
        | SchemaType::Enum { .. }
        | SchemaType::Flags { .. }
        | SchemaType::Text { .. }
        | SchemaType::Binary { .. }
        | SchemaType::Path { .. }
        | SchemaType::Url { .. }
        | SchemaType::Datetime { .. }
        | SchemaType::Duration { .. }
        | SchemaType::Quantity { .. }
        | SchemaType::QuotaToken { .. }
        | SchemaType::PermissionCard { .. } => {}
    }
}

fn canonical_schema_bytes_v1(
    graph: &SchemaGraph,
    element: Option<&SchemaType>,
) -> Result<Vec<u8>, SchemaFingerprintError> {
    let root = element.cloned().unwrap_or_else(synthetic_unit);
    let is_synthetic_unit = element.is_none();

    let mut seen = HashSet::new();
    for def in &graph.defs {
        if !seen.insert(def.id.as_str()) {
            return Err(SchemaFingerprintError::InvalidSchema(format!(
                "duplicate type id `{}`",
                def.id
            )));
        }
    }

    let mut defs = reachable_defs(graph, &root);
    defs.sort_by(|left, right| {
        left.id
            .as_str()
            .as_bytes()
            .cmp(right.id.as_str().as_bytes())
    });
    let projected = SchemaGraph {
        defs: defs.clone(),
        root: root.clone(),
    };
    if let Err(errors) = validate_graph(&projected) {
        return Err(SchemaFingerprintError::InvalidSchema(
            errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }

    let mut encoder = CborEncoder::default();
    encoder.array(4);
    encoder.text(FINGERPRINT_DOMAIN);
    encoder.unsigned(FORMAT_VERSION);
    if is_synthetic_unit {
        encoder.array(2);
        encoder.unsigned(0);
        encode_metadata(&mut encoder, &MetadataEnvelope::default())?;
    } else {
        encode_type(&mut encoder, &root)?;
    }
    encoder.array(defs.len());
    for def in defs {
        encoder.array(3);
        encoder.text(def.id.as_str());
        encoder.optional_text(def.name.as_deref());
        encode_type(&mut encoder, &def.body)?;
    }

    Ok(encoder.bytes)
}

fn synthetic_unit() -> SchemaType {
    SchemaType::Tuple {
        elements: Vec::new(),
        metadata: MetadataEnvelope::default(),
    }
}

fn encode_type(
    encoder: &mut CborEncoder,
    schema: &SchemaType,
) -> Result<(), SchemaFingerprintError> {
    match schema {
        SchemaType::Ref { id, metadata } => {
            encoder.array(3);
            encoder.unsigned(1);
            encoder.text(id.as_str());
            encode_metadata(encoder, metadata)?;
        }
        SchemaType::Bool { metadata } => encode_leaf(encoder, 2, metadata)?,
        SchemaType::S8 {
            restrictions,
            metadata,
        } => encode_numeric(encoder, 3, restrictions.as_ref(), metadata)?,
        SchemaType::S16 {
            restrictions,
            metadata,
        } => encode_numeric(encoder, 4, restrictions.as_ref(), metadata)?,
        SchemaType::S32 {
            restrictions,
            metadata,
        } => encode_numeric(encoder, 5, restrictions.as_ref(), metadata)?,
        SchemaType::S64 {
            restrictions,
            metadata,
        } => encode_numeric(encoder, 6, restrictions.as_ref(), metadata)?,
        SchemaType::U8 {
            restrictions,
            metadata,
        } => encode_numeric(encoder, 7, restrictions.as_ref(), metadata)?,
        SchemaType::U16 {
            restrictions,
            metadata,
        } => encode_numeric(encoder, 8, restrictions.as_ref(), metadata)?,
        SchemaType::U32 {
            restrictions,
            metadata,
        } => encode_numeric(encoder, 9, restrictions.as_ref(), metadata)?,
        SchemaType::U64 {
            restrictions,
            metadata,
        } => encode_numeric(encoder, 10, restrictions.as_ref(), metadata)?,
        SchemaType::F32 {
            restrictions,
            metadata,
        } => encode_numeric(encoder, 11, restrictions.as_ref(), metadata)?,
        SchemaType::F64 {
            restrictions,
            metadata,
        } => encode_numeric(encoder, 12, restrictions.as_ref(), metadata)?,
        SchemaType::Char { metadata } => encode_leaf(encoder, 13, metadata)?,
        SchemaType::String { metadata } => encode_leaf(encoder, 14, metadata)?,
        SchemaType::Record { fields, metadata } => {
            encoder.array(3);
            encoder.unsigned(15);
            encoder.array(fields.len());
            for field in fields {
                encoder.array(3);
                encoder.text(&field.name);
                encode_type(encoder, &field.body)?;
                encode_metadata(encoder, &field.metadata)?;
            }
            encode_metadata(encoder, metadata)?;
        }
        SchemaType::Variant { cases, metadata } => {
            encoder.array(3);
            encoder.unsigned(16);
            encoder.array(cases.len());
            for case in cases {
                encoder.array(3);
                encoder.text(&case.name);
                encode_optional_type(encoder, case.payload.as_ref())?;
                encode_metadata(encoder, &case.metadata)?;
            }
            encode_metadata(encoder, metadata)?;
        }
        SchemaType::Enum { cases, metadata } => {
            encode_names(encoder, 17, cases, metadata)?;
        }
        SchemaType::Flags { flags, metadata } => {
            encode_names(encoder, 18, flags, metadata)?;
        }
        SchemaType::Tuple { elements, metadata } => {
            encoder.array(3);
            encoder.unsigned(19);
            encoder.array(elements.len());
            for element in elements {
                encode_type(encoder, element)?;
            }
            encode_metadata(encoder, metadata)?;
        }
        SchemaType::List { element, metadata } => {
            encode_unary(encoder, 20, element, metadata)?;
        }
        SchemaType::FixedList {
            element,
            length,
            metadata,
        } => {
            encoder.array(4);
            encoder.unsigned(21);
            encode_type(encoder, element)?;
            encoder.unsigned(u64::from(*length));
            encode_metadata(encoder, metadata)?;
        }
        SchemaType::Map {
            key,
            value,
            metadata,
        } => {
            encoder.array(4);
            encoder.unsigned(22);
            encode_type(encoder, key)?;
            encode_type(encoder, value)?;
            encode_metadata(encoder, metadata)?;
        }
        SchemaType::Option { inner, metadata } => {
            encode_unary(encoder, 23, inner, metadata)?;
        }
        SchemaType::Result { spec, metadata } => {
            encoder.array(4);
            encoder.unsigned(24);
            encode_optional_type(encoder, spec.ok.as_deref())?;
            encode_optional_type(encoder, spec.err.as_deref())?;
            encode_metadata(encoder, metadata)?;
        }
        SchemaType::Text {
            restrictions,
            metadata,
        } => encode_text(encoder, restrictions, metadata)?,
        SchemaType::Binary {
            restrictions,
            metadata,
        } => encode_binary(encoder, restrictions, metadata)?,
        SchemaType::Path { spec, metadata } => {
            encoder.array(6);
            encoder.unsigned(27);
            encoder.unsigned(match spec.direction {
                PathDirection::Input => 0,
                PathDirection::Output => 1,
                PathDirection::InOut => 2,
            });
            encoder.unsigned(match spec.kind {
                PathKind::File => 0,
                PathKind::Directory => 1,
                PathKind::Any => 2,
            });
            encode_optional_set(
                encoder,
                "path.allowed_mime_types",
                spec.allowed_mime_types.as_ref(),
            )?;
            encode_optional_set(
                encoder,
                "path.allowed_extensions",
                spec.allowed_extensions.as_ref(),
            )?;
            encode_metadata(encoder, metadata)?;
        }
        SchemaType::Url {
            restrictions,
            metadata,
        } => encode_url(encoder, restrictions, metadata)?,
        SchemaType::Datetime { metadata } => encode_leaf(encoder, 29, metadata)?,
        SchemaType::Duration { metadata } => encode_leaf(encoder, 30, metadata)?,
        SchemaType::Quantity { spec, metadata } => {
            encoder.array(6);
            encoder.unsigned(31);
            encoder.text(&spec.base_unit);
            encoder.array(spec.allowed_suffixes.len());
            for suffix in &spec.allowed_suffixes {
                encoder.text(suffix);
            }
            encode_optional_quantity(encoder, spec.min.as_ref());
            encode_optional_quantity(encoder, spec.max.as_ref());
            encode_metadata(encoder, metadata)?;
        }
        SchemaType::Union { spec, metadata } => {
            encoder.array(3);
            encoder.unsigned(32);
            encoder.array(spec.branches.len());
            for branch in &spec.branches {
                encoder.array(4);
                encoder.text(&branch.tag);
                encode_type(encoder, &branch.body)?;
                encode_discriminator(encoder, &branch.discriminator);
                encode_metadata(encoder, &branch.metadata)?;
            }
            encode_metadata(encoder, metadata)?;
        }
        SchemaType::Secret { spec, metadata } => {
            encoder.array(4);
            encoder.unsigned(33);
            encode_type(encoder, &spec.inner)?;
            encoder.optional_text(spec.category.as_deref());
            encode_metadata(encoder, metadata)?;
        }
        SchemaType::QuotaToken { spec, metadata } => {
            encoder.array(3);
            encoder.unsigned(34);
            encoder.optional_text(spec.resource_name.as_deref());
            encode_metadata(encoder, metadata)?;
        }
        SchemaType::Future { inner, metadata } => {
            encoder.array(3);
            encoder.unsigned(35);
            encode_optional_type(encoder, inner.as_deref())?;
            encode_metadata(encoder, metadata)?;
        }
        SchemaType::Stream { inner, metadata } => {
            encoder.array(3);
            encoder.unsigned(36);
            encode_optional_type(encoder, inner.as_deref())?;
            encode_metadata(encoder, metadata)?;
        }
        SchemaType::PermissionCard { spec, metadata } => {
            encoder.array(3);
            encoder.unsigned(37);
            encoder.boolean(spec.polymorphic);
            encode_metadata(encoder, metadata)?;
        }
    }
    Ok(())
}

fn encode_leaf(
    encoder: &mut CborEncoder,
    tag: u64,
    metadata: &MetadataEnvelope,
) -> Result<(), SchemaFingerprintError> {
    encoder.array(2);
    encoder.unsigned(tag);
    encode_metadata(encoder, metadata)
}

fn encode_numeric(
    encoder: &mut CborEncoder,
    tag: u64,
    restrictions: Option<&NumericRestrictions>,
    metadata: &MetadataEnvelope,
) -> Result<(), SchemaFingerprintError> {
    encoder.array(3);
    encoder.unsigned(tag);
    match restrictions {
        None => encoder.null(),
        Some(restrictions) => {
            encoder.array(3);
            encode_optional_bound(encoder, restrictions.min);
            encode_optional_bound(encoder, restrictions.max);
            encoder.optional_text(restrictions.unit.as_deref().filter(|unit| !unit.is_empty()));
        }
    }
    encode_metadata(encoder, metadata)
}

fn encode_optional_bound(encoder: &mut CborEncoder, bound: Option<NumericBound>) {
    match bound {
        None => encoder.null(),
        Some(NumericBound::Signed(value)) => {
            encoder.array(2);
            encoder.unsigned(0);
            encoder.signed(value);
        }
        Some(NumericBound::Unsigned(value)) => {
            encoder.array(2);
            encoder.unsigned(1);
            encoder.unsigned(value);
        }
        Some(NumericBound::FloatBits(bits)) => {
            encoder.array(2);
            encoder.unsigned(2);
            encoder.unsigned(if f64::from_bits(bits) == 0.0 { 0 } else { bits });
        }
    }
}

fn encode_names(
    encoder: &mut CborEncoder,
    tag: u64,
    names: &[String],
    metadata: &MetadataEnvelope,
) -> Result<(), SchemaFingerprintError> {
    encoder.array(3);
    encoder.unsigned(tag);
    encoder.array(names.len());
    for name in names {
        encoder.text(name);
    }
    encode_metadata(encoder, metadata)
}

fn encode_unary(
    encoder: &mut CborEncoder,
    tag: u64,
    inner: &SchemaType,
    metadata: &MetadataEnvelope,
) -> Result<(), SchemaFingerprintError> {
    encoder.array(3);
    encoder.unsigned(tag);
    encode_type(encoder, inner)?;
    encode_metadata(encoder, metadata)
}

fn encode_optional_type(
    encoder: &mut CborEncoder,
    schema: Option<&SchemaType>,
) -> Result<(), SchemaFingerprintError> {
    match schema {
        Some(schema) => encode_type(encoder, schema),
        None => {
            encoder.null();
            Ok(())
        }
    }
}

fn encode_text(
    encoder: &mut CborEncoder,
    restrictions: &TextRestrictions,
    metadata: &MetadataEnvelope,
) -> Result<(), SchemaFingerprintError> {
    encoder.array(6);
    encoder.unsigned(25);
    encode_optional_set(encoder, "text.languages", restrictions.languages.as_ref())?;
    encoder.optional_u32(restrictions.min_length);
    encoder.optional_u32(restrictions.max_length);
    encoder.optional_text(restrictions.regex.as_deref());
    encode_metadata(encoder, metadata)
}

fn encode_binary(
    encoder: &mut CborEncoder,
    restrictions: &BinaryRestrictions,
    metadata: &MetadataEnvelope,
) -> Result<(), SchemaFingerprintError> {
    encoder.array(5);
    encoder.unsigned(26);
    encode_optional_set(
        encoder,
        "binary.mime_types",
        restrictions.mime_types.as_ref(),
    )?;
    encoder.optional_u32(restrictions.min_bytes);
    encoder.optional_u32(restrictions.max_bytes);
    encode_metadata(encoder, metadata)
}

fn encode_url(
    encoder: &mut CborEncoder,
    restrictions: &UrlRestrictions,
    metadata: &MetadataEnvelope,
) -> Result<(), SchemaFingerprintError> {
    encoder.array(4);
    encoder.unsigned(28);
    encode_optional_set(
        encoder,
        "url.allowed_schemes",
        restrictions.allowed_schemes.as_ref(),
    )?;
    encode_optional_set(
        encoder,
        "url.allowed_hosts",
        restrictions.allowed_hosts.as_ref(),
    )?;
    encode_metadata(encoder, metadata)
}

fn encode_optional_set(
    encoder: &mut CborEncoder,
    field: &'static str,
    values: Option<&Vec<String>>,
) -> Result<(), SchemaFingerprintError> {
    let Some(values) = values else {
        encoder.null();
        return Ok(());
    };
    let mut values: Vec<&str> = values.iter().map(String::as_str).collect();
    values.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    for duplicate in values.windows(2) {
        if duplicate[0] == duplicate[1] {
            return Err(SchemaFingerprintError::DuplicateSetValue {
                field,
                value: duplicate[0].to_string(),
            });
        }
    }
    encoder.array(values.len());
    for value in values {
        encoder.text(value);
    }
    Ok(())
}

fn encode_optional_quantity(encoder: &mut CborEncoder, value: Option<&QuantityValue>) {
    match value {
        None => encoder.null(),
        Some(value) => {
            encoder.array(3);
            encoder.signed(value.mantissa);
            encoder.signed(i64::from(value.scale));
            encoder.text(&value.unit);
        }
    }
}

fn encode_discriminator(encoder: &mut CborEncoder, discriminator: &DiscriminatorRule) {
    match discriminator {
        DiscriminatorRule::Prefix { prefix } => {
            encoder.array(2);
            encoder.unsigned(0);
            encoder.text(prefix);
        }
        DiscriminatorRule::Suffix { suffix } => {
            encoder.array(2);
            encoder.unsigned(1);
            encoder.text(suffix);
        }
        DiscriminatorRule::Contains { substring } => {
            encoder.array(2);
            encoder.unsigned(2);
            encoder.text(substring);
        }
        DiscriminatorRule::Regex { regex } => {
            encoder.array(2);
            encoder.unsigned(3);
            encoder.text(regex);
        }
        DiscriminatorRule::FieldEquals(field) => {
            encoder.array(3);
            encoder.unsigned(4);
            encoder.text(&field.field_name);
            encoder.optional_text(field.literal.as_deref());
        }
        DiscriminatorRule::FieldAbsent { field_name } => {
            encoder.array(2);
            encoder.unsigned(5);
            encoder.text(field_name);
        }
    }
}

fn encode_metadata(
    encoder: &mut CborEncoder,
    metadata: &MetadataEnvelope,
) -> Result<(), SchemaFingerprintError> {
    encoder.array(5);
    encoder.optional_text(metadata.doc.as_deref());
    encode_set(encoder, "metadata.aliases", &metadata.aliases)?;
    encoder.array(metadata.examples.len());
    for example in &metadata.examples {
        encoder.text(example);
    }
    encoder.optional_text(metadata.deprecated.as_deref());
    match &metadata.role {
        None => encoder.null(),
        Some(Role::Multimodal) => {
            encoder.array(1);
            encoder.unsigned(0);
        }
        Some(Role::UnstructuredText) => {
            encoder.array(1);
            encoder.unsigned(1);
        }
        Some(Role::UnstructuredBinary) => {
            encoder.array(1);
            encoder.unsigned(2);
        }
        Some(Role::Other(role)) => {
            encoder.array(2);
            encoder.unsigned(3);
            encoder.text(role);
        }
    }
    Ok(())
}

fn encode_set(
    encoder: &mut CborEncoder,
    field: &'static str,
    values: &[String],
) -> Result<(), SchemaFingerprintError> {
    let mut values: Vec<&str> = values.iter().map(String::as_str).collect();
    values.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    for duplicate in values.windows(2) {
        if duplicate[0] == duplicate[1] {
            return Err(SchemaFingerprintError::DuplicateSetValue {
                field,
                value: duplicate[0].to_string(),
            });
        }
    }
    encoder.array(values.len());
    for value in values {
        encoder.text(value);
    }
    Ok(())
}

#[derive(Default)]
struct CborEncoder {
    bytes: Vec<u8>,
}

impl CborEncoder {
    fn unsigned(&mut self, value: u64) {
        self.major(0, value);
    }

    fn signed(&mut self, value: i64) {
        if value >= 0 {
            self.unsigned(value as u64);
        } else {
            self.major(1, (-1i128 - i128::from(value)) as u64);
        }
    }

    fn array(&mut self, length: usize) {
        self.major(4, length as u64);
    }

    fn text(&mut self, value: &str) {
        self.major(3, value.len() as u64);
        self.bytes.extend_from_slice(value.as_bytes());
    }

    fn optional_text(&mut self, value: Option<&str>) {
        match value {
            Some(value) => self.text(value),
            None => self.null(),
        }
    }

    fn optional_u32(&mut self, value: Option<u32>) {
        match value {
            Some(value) => self.unsigned(u64::from(value)),
            None => self.null(),
        }
    }

    fn boolean(&mut self, value: bool) {
        self.bytes.push(if value { 0xf5 } else { 0xf4 });
    }

    fn null(&mut self) {
        self.bytes.push(0xf6);
    }

    fn major(&mut self, major: u8, value: u64) {
        let prefix = major << 5;
        match value {
            0..=23 => self.bytes.push(prefix | value as u8),
            24..=0xff => self.bytes.extend_from_slice(&[prefix | 24, value as u8]),
            0x100..=0xffff => {
                self.bytes.push(prefix | 25);
                self.bytes.extend_from_slice(&(value as u16).to_be_bytes());
            }
            0x1_0000..=0xffff_ffff => {
                self.bytes.push(prefix | 26);
                self.bytes.extend_from_slice(&(value as u32).to_be_bytes());
            }
            _ => {
                self.bytes.push(prefix | 27);
                self.bytes.extend_from_slice(&value.to_be_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SchemaFingerprintError, SchemaFingerprintV1, canonical_schema_bytes_v1,
        resolve_stream_element_schema_v1, schema_fingerprint_v1,
    };
    use crate::schema::{
        MetadataEnvelope, NamedFieldType, PermissionCardSpec, Role, SchemaGraph, SchemaType,
        SchemaTypeDef, TextRestrictions, TypeId,
    };
    use test_r::test;

    #[test]
    fn v1_golden_vectors() {
        let graph = SchemaGraph::empty();
        assert_eq!(canonical_schema_bytes_v1(&graph, None).unwrap().len(), 37);
        assert_eq!(
            schema_fingerprint_v1(&graph, None).unwrap().to_hex(),
            "b50494cf0f33961c703d5f6e6af3d3159e528c4d09c1d801172cdf8f022dcafa"
        );
        assert_eq!(
            canonical_schema_bytes_v1(&graph, Some(&SchemaType::string()))
                .unwrap()
                .len(),
            37
        );
        assert_eq!(
            schema_fingerprint_v1(&graph, Some(&SchemaType::string()))
                .unwrap()
                .to_hex(),
            "61c50c0a3c6ffd63529621ada78afc0d4d8e5fe691f8b0993035f847c660a307"
        );
        assert_eq!(
            canonical_schema_bytes_v1(&graph, Some(&SchemaType::list(SchemaType::string())))
                .unwrap()
                .len(),
            45
        );
        assert_eq!(
            schema_fingerprint_v1(&graph, Some(&SchemaType::list(SchemaType::string())))
                .unwrap()
                .to_hex(),
            "4939707f8ef97e9d4b31b568332eaf5a3011f2be7c358f7546966fadfb9416d4"
        );

        let node_id = TypeId::new("example.node");
        let recursive = SchemaGraph {
            root: SchemaType::ref_to(node_id.clone()),
            defs: vec![SchemaTypeDef {
                id: node_id.clone(),
                name: Some("Node".to_string()),
                body: SchemaType::record(vec![
                    NamedFieldType {
                        name: "value".to_string(),
                        body: SchemaType::string(),
                        metadata: MetadataEnvelope::default(),
                    },
                    NamedFieldType {
                        name: "next".to_string(),
                        body: SchemaType::option(SchemaType::ref_to(node_id)),
                        metadata: MetadataEnvelope::default(),
                    },
                ]),
            }],
        };
        assert_eq!(
            canonical_schema_bytes_v1(&recursive, Some(&recursive.root))
                .unwrap()
                .len(),
            140
        );
        assert_eq!(
            schema_fingerprint_v1(&recursive, Some(&recursive.root))
                .unwrap()
                .to_hex(),
            "3931585d2d02a2b7d5c99e3da1082ac8fe904c535e2700bd45e29a95ff2399fa"
        );

        let constrained = SchemaType::Text {
            restrictions: TextRestrictions {
                languages: Some(vec!["fr".to_string(), "en".to_string()]),
                min_length: Some(1),
                max_length: Some(64),
                regex: Some("^[a-z]+$".to_string()),
            },
            metadata: MetadataEnvelope {
                doc: Some("text".to_string()),
                aliases: vec!["z".to_string(), "a".to_string()],
                examples: vec!["\"hello\"".to_string()],
                deprecated: Some("use-v2".to_string()),
                role: Some(Role::Other("prompt".to_string())),
            },
        };
        let constrained_bytes =
            canonical_schema_bytes_v1(&SchemaGraph::empty(), Some(&constrained)).unwrap();
        let constrained_hex = constrained_bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            constrained_hex,
            "847818676f6c656d2d736368656d612d66696e6765727072696e74018618198262656e626672011840685e5b612d7a5d2b24856474657874826161617a81672268656c6c6f22667573652d763282036670726f6d707480"
        );
        assert_eq!(constrained_bytes.len(), 87);
        assert_eq!(
            blake3::hash(&constrained_bytes).to_hex().as_str(),
            "b985cdb5445862be90e8dca06bbfa9c46b50cf40edc84ed34205bb3a214c5bb0"
        );

        let permission_card = SchemaType::permission_card(PermissionCardSpec { polymorphic: true });
        let permission_card_bytes =
            canonical_schema_bytes_v1(&SchemaGraph::empty(), Some(&permission_card)).unwrap();
        assert_eq!(
            permission_card_bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            "847818676f6c656d2d736368656d612d66696e6765727072696e7401831825f585f68080f6f680"
        );
        assert_eq!(permission_card_bytes.len(), 39);
        assert_eq!(
            blake3::hash(&permission_card_bytes).to_hex().as_str(),
            "b7d3c09af5db4e56b527051f561689f451dfdb21213cd70aabd11618e244da8b"
        );
    }

    #[test]
    fn sorts_set_values_and_rejects_duplicates() {
        let graph = SchemaGraph::empty();
        let left = SchemaType::Text {
            restrictions: crate::schema::TextRestrictions {
                languages: Some(vec!["fr".into(), "en".into()]),
                ..Default::default()
            },
            metadata: MetadataEnvelope::default(),
        };
        let right = SchemaType::Text {
            restrictions: crate::schema::TextRestrictions {
                languages: Some(vec!["en".into(), "fr".into()]),
                ..Default::default()
            },
            metadata: MetadataEnvelope::default(),
        };
        assert_eq!(
            schema_fingerprint_v1(&graph, Some(&left)).unwrap(),
            schema_fingerprint_v1(&graph, Some(&right)).unwrap()
        );

        let duplicate = SchemaType::Text {
            restrictions: crate::schema::TextRestrictions {
                languages: Some(vec!["en".into(), "en".into()]),
                ..Default::default()
            },
            metadata: MetadataEnvelope::default(),
        };
        assert!(matches!(
            schema_fingerprint_v1(&graph, Some(&duplicate)),
            Err(SchemaFingerprintError::DuplicateSetValue { .. })
        ));
    }

    #[test]
    fn resolves_stream_element_fingerprints_to_self_contained_schema_graphs() {
        let node_id = TypeId::new("example.node");
        let node_ref = SchemaType::ref_to(node_id.clone());
        let graph = SchemaGraph {
            root: SchemaType::record(vec![
                NamedFieldType {
                    name: "nodes".to_string(),
                    body: SchemaType::stream(Some(node_ref.clone())),
                    metadata: MetadataEnvelope::default(),
                },
                NamedFieldType {
                    name: "signals".to_string(),
                    body: SchemaType::stream(None),
                    metadata: MetadataEnvelope::default(),
                },
            ]),
            defs: vec![SchemaTypeDef {
                id: node_id.clone(),
                name: Some("Node".to_string()),
                body: SchemaType::record(vec![NamedFieldType {
                    name: "next".to_string(),
                    body: SchemaType::option(SchemaType::ref_to(node_id)),
                    metadata: MetadataEnvelope::default(),
                }]),
            }],
        };

        let node_fingerprint = schema_fingerprint_v1(&graph, Some(&node_ref)).unwrap();
        let resolved = resolve_stream_element_schema_v1(&graph, node_fingerprint)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.root, node_ref);
        assert_eq!(resolved.defs, graph.defs);

        let unit_fingerprint = schema_fingerprint_v1(&graph, None).unwrap();
        let resolved_unit = resolve_stream_element_schema_v1(&graph, unit_fingerprint)
            .unwrap()
            .unwrap();
        assert_eq!(
            resolved_unit,
            SchemaGraph::anonymous(SchemaType::tuple(Vec::new()))
        );

        assert!(
            resolve_stream_element_schema_v1(&graph, SchemaFingerprintV1([0xff; 32]))
                .unwrap()
                .is_none()
        );
    }
}
