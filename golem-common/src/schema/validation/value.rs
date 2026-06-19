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

//! Structural validation that a [`SchemaValue`] matches a given
//! [`SchemaType`] inside a [`SchemaGraph`].

use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::{
    BinaryRestrictions, DiscriminatorRule, PathSpec, QuantitySpec, QuantityValue, QuotaTokenSpec,
    SchemaType, SecretSpec, TextRestrictions, UnionBranch, UrlRestrictions,
};
use crate::schema::schema_value::{
    BinaryValuePayload, QuotaTokenValuePayload, ResultValuePayload, SchemaValue,
    SecretValuePayload, TextValuePayload,
};
use std::collections::HashSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter, Write};

/// A path segment inside a [`SchemaValue`] tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValuePathSegment {
    Field(String),
    Index(usize),
    VariantPayload,
    OptionInner,
    ResultOk,
    ResultErr,
    UnionBody,
    MapKey(usize),
    MapValue(usize),
}

impl Display for ValuePathSegment {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ValuePathSegment::Field(n) => write!(f, ".field({n:?})"),
            ValuePathSegment::Index(i) => write!(f, ".index({i})"),
            ValuePathSegment::VariantPayload => f.write_str(".variant_payload"),
            ValuePathSegment::OptionInner => f.write_str(".option_inner"),
            ValuePathSegment::ResultOk => f.write_str(".ok"),
            ValuePathSegment::ResultErr => f.write_str(".err"),
            ValuePathSegment::UnionBody => f.write_str(".union_body"),
            ValuePathSegment::MapKey(i) => write!(f, ".map_key({i})"),
            ValuePathSegment::MapValue(i) => write!(f, ".map_value({i})"),
        }
    }
}

/// Path through a [`SchemaValue`] tree to a specific failing node.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ValuePath {
    segments: Vec<ValuePathSegment>,
}

impl ValuePath {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn segments(&self) -> &[ValuePathSegment] {
        &self.segments
    }

    fn push(&mut self, segment: ValuePathSegment) {
        self.segments.push(segment);
    }

    fn pop(&mut self) {
        self.segments.pop();
    }

    fn snapshot(&self) -> Self {
        Self {
            segments: self.segments.clone(),
        }
    }
}

impl Display for ValuePath {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for s in &self.segments {
            write!(f, "{s}")?;
        }
        Ok(())
    }
}

/// All errors raised by [`validate_value`].
#[derive(Clone, Debug, PartialEq)]
pub enum ValueError {
    ShapeMismatch {
        path: ValuePath,
        expected: String,
        found: String,
    },
    VariantCaseOutOfRange {
        path: ValuePath,
        case: u32,
        case_count: usize,
    },
    EnumCaseOutOfRange {
        path: ValuePath,
        case: u32,
        case_count: usize,
    },
    RecordArityMismatch {
        path: ValuePath,
        expected: usize,
        found: usize,
    },
    TupleArityMismatch {
        path: ValuePath,
        expected: usize,
        found: usize,
    },
    FlagsArityMismatch {
        path: ValuePath,
        expected: usize,
        found: usize,
    },
    FixedListLengthMismatch {
        path: ValuePath,
        expected: u32,
        found: usize,
    },
    DanglingRef {
        path: ValuePath,
        type_id: TypeId,
    },
    /// A chain of named references at one value position resolves back
    /// to itself (e.g. `A = Ref<A>` or a mutual cycle `A → B → A`) before
    /// reaching a structural shape, so no finite value can ever satisfy
    /// it.
    RecursiveRef {
        path: ValuePath,
        type_id: TypeId,
    },
    UnionUnknownTag {
        path: ValuePath,
        tag: String,
    },
    UnionDiscriminatorMismatch {
        path: ValuePath,
        tag: String,
    },
    VariantPayloadPresenceMismatch {
        path: ValuePath,
        expected_some: bool,
    },
    ResultPayloadPresenceMismatch {
        path: ValuePath,
        expected_some: bool,
        side: ResultSide,
    },
    OptionInnerPresenceMismatch {
        path: ValuePath,
    },
    TextLanguageNotAllowed {
        path: ValuePath,
        language: String,
    },
    TextTooShort {
        path: ValuePath,
        min: u32,
        found: usize,
    },
    TextTooLong {
        path: ValuePath,
        max: u32,
        found: usize,
    },
    TextRegexMismatch {
        path: ValuePath,
        regex: String,
    },
    BinaryMimeNotAllowed {
        path: ValuePath,
        mime_type: String,
    },
    BinaryTooSmall {
        path: ValuePath,
        min: u32,
        found: usize,
    },
    BinaryTooLarge {
        path: ValuePath,
        max: u32,
        found: usize,
    },
    PathEmpty {
        path: ValuePath,
    },
    PathExtensionNotAllowed {
        path: ValuePath,
        extension: String,
    },
    UrlEmpty {
        path: ValuePath,
    },
    UrlInvalid {
        path: ValuePath,
        url: String,
        reason: String,
    },
    UrlSchemeNotAllowed {
        path: ValuePath,
        scheme: String,
    },
    UrlHostNotAllowed {
        path: ValuePath,
        host: String,
    },
    UrlHostMissing {
        path: ValuePath,
    },
    QuantityUnitNotAllowed {
        path: ValuePath,
        unit: String,
    },
    QuantityOutOfRange {
        path: ValuePath,
        reason: String,
    },
    SecretRefEmpty {
        path: ValuePath,
    },
    QuotaTokenResourceMismatch {
        path: ValuePath,
        expected: String,
        found: String,
    },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResultSide {
    Ok,
    Err,
}

impl Display for ValueError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ValueError::ShapeMismatch {
                path,
                expected,
                found,
            } => write!(
                f,
                "shape mismatch at {path}: expected {expected}, found {found}"
            ),
            ValueError::VariantCaseOutOfRange {
                path,
                case,
                case_count,
            } => write!(
                f,
                "variant case index {case} at {path} is out of range (case count: {case_count})"
            ),
            ValueError::EnumCaseOutOfRange {
                path,
                case,
                case_count,
            } => write!(
                f,
                "enum case index {case} at {path} is out of range (case count: {case_count})"
            ),
            ValueError::RecordArityMismatch {
                path,
                expected,
                found,
            } => write!(
                f,
                "record at {path} has {found} field(s), expected {expected}"
            ),
            ValueError::TupleArityMismatch {
                path,
                expected,
                found,
            } => write!(
                f,
                "tuple at {path} has {found} element(s), expected {expected}"
            ),
            ValueError::FlagsArityMismatch {
                path,
                expected,
                found,
            } => write!(
                f,
                "flags value at {path} has {found} bit(s), expected {expected}"
            ),
            ValueError::FixedListLengthMismatch {
                path,
                expected,
                found,
            } => write!(
                f,
                "fixed-list at {path} has {found} element(s), expected {expected}"
            ),
            ValueError::DanglingRef { path, type_id } => write!(
                f,
                "dangling ref `{type_id}` at {path} (no such named definition)"
            ),
            ValueError::RecursiveRef { path, type_id } => write!(
                f,
                "ref chain at {path} loops back to `{type_id}` without reaching a structural shape"
            ),
            ValueError::UnionUnknownTag { path, tag } => write!(
                f,
                "union value at {path} carries tag `{tag}` that does not match any branch"
            ),
            ValueError::UnionDiscriminatorMismatch { path, tag } => write!(
                f,
                "union value at {path} does not satisfy branch `{tag}` discriminator"
            ),
            ValueError::VariantPayloadPresenceMismatch {
                path,
                expected_some,
            } => {
                let mut s = String::new();
                let _ = write!(s, "variant payload presence mismatch at {path}: ");
                if *expected_some {
                    let _ = write!(s, "schema expects a payload, value carries none");
                } else {
                    let _ = write!(s, "schema expects no payload, value carries one");
                }
                f.write_str(&s)
            }
            ValueError::ResultPayloadPresenceMismatch {
                path,
                expected_some,
                side,
            } => {
                let side = match side {
                    ResultSide::Ok => "ok",
                    ResultSide::Err => "err",
                };
                let mut s = String::new();
                let _ = write!(s, "result {side} payload presence mismatch at {path}: ");
                if *expected_some {
                    let _ = write!(s, "schema expects a payload, value carries none");
                } else {
                    let _ = write!(s, "schema expects no payload, value carries one");
                }
                f.write_str(&s)
            }
            ValueError::OptionInnerPresenceMismatch { path } => {
                write!(f, "option-value presence inconsistent at {path}")
            }
            ValueError::TextLanguageNotAllowed { path, language } => write!(
                f,
                "text value at {path} carries language `{language}` not in the allow-list"
            ),
            ValueError::TextTooShort { path, min, found } => write!(
                f,
                "text value at {path} has {found} char(s), below min-length {min}"
            ),
            ValueError::TextTooLong { path, max, found } => write!(
                f,
                "text value at {path} has {found} char(s), above max-length {max}"
            ),
            ValueError::TextRegexMismatch { path, regex } => write!(
                f,
                "text value at {path} does not match required regex `{regex}`"
            ),
            ValueError::BinaryMimeNotAllowed { path, mime_type } => write!(
                f,
                "binary value at {path} carries mime-type `{mime_type}` not in the allow-list"
            ),
            ValueError::BinaryTooSmall { path, min, found } => write!(
                f,
                "binary value at {path} has {found} byte(s), below min-bytes {min}"
            ),
            ValueError::BinaryTooLarge { path, max, found } => write!(
                f,
                "binary value at {path} has {found} byte(s), above max-bytes {max}"
            ),
            ValueError::PathEmpty { path } => write!(f, "path value at {path} is empty"),
            ValueError::PathExtensionNotAllowed { path, extension } => write!(
                f,
                "path value at {path} has extension `{extension}` not in the allow-list"
            ),
            ValueError::UrlEmpty { path } => write!(f, "url value at {path} is empty"),
            ValueError::UrlInvalid { path, url, reason } => write!(
                f,
                "url value at {path} (`{url}`) is not a valid URL: {reason}"
            ),
            ValueError::UrlSchemeNotAllowed { path, scheme } => write!(
                f,
                "url value at {path} has scheme `{scheme}` not in the allow-list"
            ),
            ValueError::UrlHostNotAllowed { path, host } => write!(
                f,
                "url value at {path} has host `{host}` not in the allow-list"
            ),
            ValueError::UrlHostMissing { path } => write!(
                f,
                "url value at {path} has no host (allow-list requires one)"
            ),
            ValueError::QuantityUnitNotAllowed { path, unit } => write!(
                f,
                "quantity value at {path} has unit `{unit}` which is not allowed"
            ),
            ValueError::QuantityOutOfRange { path, reason } => {
                write!(f, "quantity value at {path} is out of range ({reason})")
            }
            ValueError::SecretRefEmpty { path } => {
                write!(f, "secret value at {path} has an empty `secret_ref`")
            }
            ValueError::QuotaTokenResourceMismatch {
                path,
                expected,
                found,
            } => write!(
                f,
                "quota-token value at {path} expected resource `{expected}`, found `{found}`"
            ),
        }
    }
}

impl Error for ValueError {}

/// Validate that `value` structurally conforms to `ty` (in the context of
/// `graph`).
pub fn validate_value(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
) -> Result<(), Vec<ValueError>> {
    let mut errors = Vec::new();
    let mut path = ValuePath::new();
    check(graph, ty, value, &mut path, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn shape_name(value: &SchemaValue) -> &'static str {
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
    }
}

fn type_name(ty: &SchemaType) -> &'static str {
    match ty {
        SchemaType::Ref { .. } => "ref",
        SchemaType::Bool { .. } => "bool",
        SchemaType::S8 { .. } => "s8",
        SchemaType::S16 { .. } => "s16",
        SchemaType::S32 { .. } => "s32",
        SchemaType::S64 { .. } => "s64",
        SchemaType::U8 { .. } => "u8",
        SchemaType::U16 { .. } => "u16",
        SchemaType::U32 { .. } => "u32",
        SchemaType::U64 { .. } => "u64",
        SchemaType::F32 { .. } => "f32",
        SchemaType::F64 { .. } => "f64",
        SchemaType::Char { .. } => "char",
        SchemaType::String { .. } => "string",
        SchemaType::Record { .. } => "record",
        SchemaType::Variant { .. } => "variant",
        SchemaType::Enum { .. } => "enum",
        SchemaType::Flags { .. } => "flags",
        SchemaType::Tuple { .. } => "tuple",
        SchemaType::List { .. } => "list",
        SchemaType::FixedList { .. } => "fixed-list",
        SchemaType::Map { .. } => "map",
        SchemaType::Option { .. } => "option",
        SchemaType::Result { .. } => "result",
        SchemaType::Text { .. } => "text",
        SchemaType::Binary { .. } => "binary",
        SchemaType::Path { .. } => "path",
        SchemaType::Url { .. } => "url",
        SchemaType::Datetime { .. } => "datetime",
        SchemaType::Duration { .. } => "duration",
        SchemaType::Quantity { .. } => "quantity",
        SchemaType::Union { .. } => "union",
        SchemaType::Secret { .. } => "secret",
        SchemaType::QuotaToken { .. } => "quota-token",
        SchemaType::Future { .. } => "future",
        SchemaType::Stream { .. } => "stream",
    }
}

fn shape_mismatch(
    path: &ValuePath,
    ty: &SchemaType,
    value: &SchemaValue,
    errors: &mut Vec<ValueError>,
) {
    errors.push(ValueError::ShapeMismatch {
        path: path.snapshot(),
        expected: type_name(ty).to_string(),
        found: shape_name(value).to_string(),
    });
}

/// Resolve a chain of [`SchemaType::Ref`]s at the *current* value node into
/// a non-`Ref` schema. Cycle protection is local to this resolution loop
/// so it only guards pure alias cycles like `A = Ref<A>` or
/// `A = Ref<B>, B = Ref<A>` that can never reach a structural shape.
/// Recursive types that go through a value-shrinking constructor (record,
/// list, option, …) are fully validated at every level because each
/// value-tree step gets a fresh resolution scope.
///
/// Returns `Some(ty)` with a non-`Ref` schema on success, or `None` after
/// pushing a [`ValueError::DanglingRef`] or [`ValueError::RecursiveRef`]
/// on failure.
fn resolve_refs_at_value_node<'a>(
    graph: &'a SchemaGraph,
    mut ty: &'a SchemaType,
    path: &ValuePath,
    errors: &mut Vec<ValueError>,
) -> Option<&'a SchemaType> {
    let mut seen: HashSet<TypeId> = HashSet::new();
    loop {
        match ty {
            SchemaType::Ref { id, .. } => {
                if !seen.insert(id.clone()) {
                    errors.push(ValueError::RecursiveRef {
                        path: path.snapshot(),
                        type_id: id.clone(),
                    });
                    return None;
                }
                match graph.lookup(id) {
                    Some(def) => {
                        ty = &def.body;
                    }
                    None => {
                        errors.push(ValueError::DanglingRef {
                            path: path.snapshot(),
                            type_id: id.clone(),
                        });
                        return None;
                    }
                }
            }
            other => return Some(other),
        }
    }
}

fn check(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &SchemaValue,
    path: &mut ValuePath,
    errors: &mut Vec<ValueError>,
) {
    // Resolve any chain of named refs at this exact value position. The
    // resolution scope is local to this call: as soon as we descend into
    // a value-shrinking constructor (record field, list element, etc.)
    // a fresh scope is established, so recursive types backed by finite
    // values are validated at every level.
    let Some(ty) = resolve_refs_at_value_node(graph, ty, path, errors) else {
        return;
    };

    match (ty, value) {
        (SchemaType::Bool { .. }, SchemaValue::Bool(_)) => {}
        (SchemaType::S8 { .. }, SchemaValue::S8(_)) => {}
        (SchemaType::S16 { .. }, SchemaValue::S16(_)) => {}
        (SchemaType::S32 { .. }, SchemaValue::S32(_)) => {}
        (SchemaType::S64 { .. }, SchemaValue::S64(_)) => {}
        (SchemaType::U8 { .. }, SchemaValue::U8(_)) => {}
        (SchemaType::U16 { .. }, SchemaValue::U16(_)) => {}
        (SchemaType::U32 { .. }, SchemaValue::U32(_)) => {}
        (SchemaType::U64 { .. }, SchemaValue::U64(_)) => {}
        (SchemaType::F32 { .. }, SchemaValue::F32(_)) => {}
        (SchemaType::F64 { .. }, SchemaValue::F64(_)) => {}
        (SchemaType::Char { .. }, SchemaValue::Char(_)) => {}
        (SchemaType::String { .. }, SchemaValue::String(_)) => {}

        (SchemaType::Text { restrictions, .. }, SchemaValue::Text(payload)) => {
            check_text(restrictions, payload, path, errors);
        }
        (SchemaType::Binary { restrictions, .. }, SchemaValue::Binary(payload)) => {
            check_binary(restrictions, payload, path, errors);
        }
        (SchemaType::Path { spec, .. }, SchemaValue::Path { path: p }) => {
            check_path(spec, p.as_str(), path, errors);
        }
        (SchemaType::Url { restrictions, .. }, SchemaValue::Url { url }) => {
            check_url(restrictions, url.as_str(), path, errors);
        }
        (SchemaType::Datetime { .. }, SchemaValue::Datetime { .. }) => {}
        (SchemaType::Duration { .. }, SchemaValue::Duration(_)) => {}
        (SchemaType::Quantity { spec, .. }, SchemaValue::Quantity(value)) => {
            check_quantity(spec, value, path, errors);
        }
        (SchemaType::Secret { spec, .. }, SchemaValue::Secret(payload)) => {
            check_secret(spec, payload, path, errors);
        }
        (SchemaType::QuotaToken { spec, .. }, SchemaValue::QuotaToken(payload)) => {
            check_quota_token(spec, payload, path, errors);
        }

        (SchemaType::Record { fields, .. }, SchemaValue::Record { fields: vs }) => {
            if fields.len() != vs.len() {
                errors.push(ValueError::RecordArityMismatch {
                    path: path.snapshot(),
                    expected: fields.len(),
                    found: vs.len(),
                });
                return;
            }
            for (field, v) in fields.iter().zip(vs.iter()) {
                path.push(ValuePathSegment::Field(field.name.clone()));
                check(graph, &field.body, v, path, errors);
                path.pop();
            }
        }

        (SchemaType::Variant { cases, .. }, SchemaValue::Variant(vp)) => {
            let idx = vp.case as usize;
            if idx >= cases.len() {
                errors.push(ValueError::VariantCaseOutOfRange {
                    path: path.snapshot(),
                    case: vp.case,
                    case_count: cases.len(),
                });
                return;
            }
            let case = &cases[idx];
            match (&case.payload, &vp.payload) {
                (Some(case_ty), Some(payload)) => {
                    path.push(ValuePathSegment::VariantPayload);
                    check(graph, case_ty, payload, path, errors);
                    path.pop();
                }
                (None, None) => {}
                (Some(_), None) => errors.push(ValueError::VariantPayloadPresenceMismatch {
                    path: path.snapshot(),
                    expected_some: true,
                }),
                (None, Some(_)) => errors.push(ValueError::VariantPayloadPresenceMismatch {
                    path: path.snapshot(),
                    expected_some: false,
                }),
            }
        }

        (SchemaType::Enum { cases, .. }, SchemaValue::Enum { case }) => {
            if (*case as usize) >= cases.len() {
                errors.push(ValueError::EnumCaseOutOfRange {
                    path: path.snapshot(),
                    case: *case,
                    case_count: cases.len(),
                });
            }
        }

        (SchemaType::Flags { flags, .. }, SchemaValue::Flags { bits }) => {
            if flags.len() != bits.len() {
                errors.push(ValueError::FlagsArityMismatch {
                    path: path.snapshot(),
                    expected: flags.len(),
                    found: bits.len(),
                });
            }
        }

        (SchemaType::Tuple { elements, .. }, SchemaValue::Tuple { elements: vs }) => {
            if elements.len() != vs.len() {
                errors.push(ValueError::TupleArityMismatch {
                    path: path.snapshot(),
                    expected: elements.len(),
                    found: vs.len(),
                });
                return;
            }
            for (i, (t, v)) in elements.iter().zip(vs.iter()).enumerate() {
                path.push(ValuePathSegment::Index(i));
                check(graph, t, v, path, errors);
                path.pop();
            }
        }

        (SchemaType::List { element, .. }, SchemaValue::List { elements }) => {
            for (i, v) in elements.iter().enumerate() {
                path.push(ValuePathSegment::Index(i));
                check(graph, element, v, path, errors);
                path.pop();
            }
        }

        (
            SchemaType::FixedList {
                element, length, ..
            },
            SchemaValue::FixedList { elements },
        ) => {
            if elements.len() != *length as usize {
                errors.push(ValueError::FixedListLengthMismatch {
                    path: path.snapshot(),
                    expected: *length,
                    found: elements.len(),
                });
                return;
            }
            for (i, v) in elements.iter().enumerate() {
                path.push(ValuePathSegment::Index(i));
                check(graph, element, v, path, errors);
                path.pop();
            }
        }

        (
            SchemaType::Map {
                key, value: vty, ..
            },
            SchemaValue::Map { entries },
        ) => {
            for (i, (k, v)) in entries.iter().enumerate() {
                path.push(ValuePathSegment::MapKey(i));
                check(graph, key, k, path, errors);
                path.pop();
                path.push(ValuePathSegment::MapValue(i));
                check(graph, vty, v, path, errors);
                path.pop();
            }
        }

        (SchemaType::Option { inner, .. }, SchemaValue::Option { inner: v }) => {
            if let Some(v) = v {
                path.push(ValuePathSegment::OptionInner);
                check(graph, inner, v, path, errors);
                path.pop();
            }
        }

        (SchemaType::Result { spec, .. }, SchemaValue::Result(vp)) => match vp {
            ResultValuePayload::Ok { value: v } => match (&spec.ok, v) {
                (Some(t), Some(v)) => {
                    path.push(ValuePathSegment::ResultOk);
                    check(graph, t, v, path, errors);
                    path.pop();
                }
                (None, None) => {}
                (Some(_), None) => errors.push(ValueError::ResultPayloadPresenceMismatch {
                    path: path.snapshot(),
                    expected_some: true,
                    side: ResultSide::Ok,
                }),
                (None, Some(_)) => errors.push(ValueError::ResultPayloadPresenceMismatch {
                    path: path.snapshot(),
                    expected_some: false,
                    side: ResultSide::Ok,
                }),
            },
            ResultValuePayload::Err { value: v } => match (&spec.err, v) {
                (Some(t), Some(v)) => {
                    path.push(ValuePathSegment::ResultErr);
                    check(graph, t, v, path, errors);
                    path.pop();
                }
                (None, None) => {}
                (Some(_), None) => errors.push(ValueError::ResultPayloadPresenceMismatch {
                    path: path.snapshot(),
                    expected_some: true,
                    side: ResultSide::Err,
                }),
                (None, Some(_)) => errors.push(ValueError::ResultPayloadPresenceMismatch {
                    path: path.snapshot(),
                    expected_some: false,
                    side: ResultSide::Err,
                }),
            },
        },

        (SchemaType::Union { spec, .. }, SchemaValue::Union(vp)) => {
            let branch = spec.branches.iter().find(|b| b.tag == vp.tag);
            match branch {
                None => errors.push(ValueError::UnionUnknownTag {
                    path: path.snapshot(),
                    tag: vp.tag.clone(),
                }),
                Some(branch) => {
                    path.push(ValuePathSegment::UnionBody);
                    let mut sub_errors = Vec::new();
                    check(graph, &branch.body, &vp.body, path, &mut sub_errors);
                    errors.extend(sub_errors);
                    if !discriminator_matches(graph, branch, &vp.body) {
                        errors.push(ValueError::UnionDiscriminatorMismatch {
                            path: path.snapshot(),
                            tag: vp.tag.clone(),
                        });
                    }
                    path.pop();
                }
            }
        }

        (ty, v) => shape_mismatch(path, ty, v, errors),
    }
}

fn check_text(
    restrictions: &TextRestrictions,
    payload: &TextValuePayload,
    path: &mut ValuePath,
    errors: &mut Vec<ValueError>,
) {
    // A missing language is always allowed; an empty allow-list is treated as
    // unrestricted. Only a *present* language outside a *non-empty* allow-list
    // is rejected.
    if let (Some(allowed), Some(lang)) = (&restrictions.languages, &payload.language)
        && !allowed.is_empty()
        && !allowed.iter().any(|a| a == lang)
    {
        errors.push(ValueError::TextLanguageNotAllowed {
            path: path.snapshot(),
            language: lang.clone(),
        });
    }
    let char_len = payload.text.chars().count();
    if let Some(min) = restrictions.min_length
        && (char_len as u64) < (min as u64)
    {
        errors.push(ValueError::TextTooShort {
            path: path.snapshot(),
            min,
            found: char_len,
        });
    }
    if let Some(max) = restrictions.max_length
        && (char_len as u64) > (max as u64)
    {
        errors.push(ValueError::TextTooLong {
            path: path.snapshot(),
            max,
            found: char_len,
        });
    }
    if let Some(regex) = &restrictions.regex
        && let Ok(compiled) = regex::Regex::new(regex.as_str())
        && !compiled.is_match(payload.text.as_str())
    {
        errors.push(ValueError::TextRegexMismatch {
            path: path.snapshot(),
            regex: regex.clone(),
        });
    }
}

fn check_binary(
    restrictions: &BinaryRestrictions,
    payload: &BinaryValuePayload,
    path: &mut ValuePath,
    errors: &mut Vec<ValueError>,
) {
    // A missing MIME type is always allowed; an empty allow-list is treated as
    // unrestricted. Only a *present* MIME type outside a *non-empty* allow-list
    // is rejected.
    if let (Some(allowed), Some(mime)) = (&restrictions.mime_types, &payload.mime_type)
        && !allowed.is_empty()
        && !allowed.iter().any(|a| a == mime)
    {
        errors.push(ValueError::BinaryMimeNotAllowed {
            path: path.snapshot(),
            mime_type: mime.clone(),
        });
    }
    let len = payload.bytes.len();
    if let Some(min) = restrictions.min_bytes
        && (len as u64) < (min as u64)
    {
        errors.push(ValueError::BinaryTooSmall {
            path: path.snapshot(),
            min,
            found: len,
        });
    }
    if let Some(max) = restrictions.max_bytes
        && (len as u64) > (max as u64)
    {
        errors.push(ValueError::BinaryTooLarge {
            path: path.snapshot(),
            max,
            found: len,
        });
    }
}

fn check_path(spec: &PathSpec, p: &str, path: &mut ValuePath, errors: &mut Vec<ValueError>) {
    if p.is_empty() {
        errors.push(ValueError::PathEmpty {
            path: path.snapshot(),
        });
        return;
    }
    if let Some(allowed_exts) = &spec.allowed_extensions
        && let Some(ext) = file_extension(p)
        && !allowed_exts.iter().any(|a| a == ext)
    {
        errors.push(ValueError::PathExtensionNotAllowed {
            path: path.snapshot(),
            extension: ext.to_string(),
        });
    }
    // `allowed_mime_types` on `Path` is not enforceable by value validation
    // because `SchemaValue::Path` carries only a path string. MIME
    // restrictions must be enforced at protocol/adaptation layers that
    // carry or derive MIME metadata (e.g. an HTTP `Content-Type` header,
    // a multimodal envelope, a content-typed canonical-encoding layer).
    // The validator must not read the filesystem or sniff content.
}

fn check_url(
    spec: &UrlRestrictions,
    url: &str,
    path: &mut ValuePath,
    errors: &mut Vec<ValueError>,
) {
    if url.is_empty() {
        errors.push(ValueError::UrlEmpty {
            path: path.snapshot(),
        });
        return;
    }
    let parsed = match url::Url::parse(url) {
        Ok(parsed) => parsed,
        Err(err) => {
            errors.push(ValueError::UrlInvalid {
                path: path.snapshot(),
                url: url.to_string(),
                reason: err.to_string(),
            });
            return;
        }
    };
    if let Some(allowed) = &spec.allowed_schemes
        && !allowed
            .iter()
            .any(|scheme| scheme.eq_ignore_ascii_case(parsed.scheme()))
    {
        errors.push(ValueError::UrlSchemeNotAllowed {
            path: path.snapshot(),
            scheme: parsed.scheme().to_string(),
        });
    }
    if let Some(allowed) = &spec.allowed_hosts {
        match parsed.host_str() {
            Some(host)
                if allowed
                    .iter()
                    .any(|allowed_host| allowed_host.eq_ignore_ascii_case(host)) => {}
            Some(host) => errors.push(ValueError::UrlHostNotAllowed {
                path: path.snapshot(),
                host: host.to_string(),
            }),
            None => errors.push(ValueError::UrlHostMissing {
                path: path.snapshot(),
            }),
        }
    }
}

fn check_quantity(
    spec: &QuantitySpec,
    value: &QuantityValue,
    path: &mut ValuePath,
    errors: &mut Vec<ValueError>,
) {
    let unit_ok = if spec.allowed_suffixes.is_empty() {
        value.unit == spec.base_unit
    } else {
        spec.allowed_suffixes.iter().any(|s| s == &value.unit)
    };
    if !unit_ok {
        errors.push(ValueError::QuantityUnitNotAllowed {
            path: path.snapshot(),
            unit: value.unit.clone(),
        });
        // Range checks below assume canonical unit comparison; bail when the
        // unit is unrecognised.
        return;
    }

    if let Some(min) = &spec.min {
        match quantity_le_checked(min, value) {
            Some(true) => {}
            Some(false) => errors.push(ValueError::QuantityOutOfRange {
                path: path.snapshot(),
                reason: format!("value is below min `{}`", render_quantity(min)),
            }),
            None => errors.push(ValueError::QuantityOutOfRange {
                path: path.snapshot(),
                reason: "overflow".to_string(),
            }),
        }
    }
    if let Some(max) = &spec.max {
        match quantity_le_checked(value, max) {
            Some(true) => {}
            Some(false) => errors.push(ValueError::QuantityOutOfRange {
                path: path.snapshot(),
                reason: format!("value is above max `{}`", render_quantity(max)),
            }),
            None => errors.push(ValueError::QuantityOutOfRange {
                path: path.snapshot(),
                reason: "overflow".to_string(),
            }),
        }
    }
}

fn check_secret(
    _spec: &SecretSpec,
    payload: &SecretValuePayload,
    path: &mut ValuePath,
    errors: &mut Vec<ValueError>,
) {
    if payload.secret_ref.is_empty() {
        errors.push(ValueError::SecretRefEmpty {
            path: path.snapshot(),
        });
    }
}

fn check_quota_token(
    spec: &QuotaTokenSpec,
    payload: &QuotaTokenValuePayload,
    path: &mut ValuePath,
    errors: &mut Vec<ValueError>,
) {
    if let Some(expected) = &spec.resource_name
        && expected != &payload.resource_name
    {
        errors.push(ValueError::QuotaTokenResourceMismatch {
            path: path.snapshot(),
            expected: expected.clone(),
            found: payload.resource_name.clone(),
        });
    }
}

fn discriminator_matches(graph: &SchemaGraph, branch: &UnionBranch, body: &SchemaValue) -> bool {
    match &branch.discriminator {
        DiscriminatorRule::Prefix { prefix } => string_view(graph, &branch.body, body)
            .map(|s| s.starts_with(prefix.as_str()))
            .unwrap_or(false),
        DiscriminatorRule::Suffix { suffix } => string_view(graph, &branch.body, body)
            .map(|s| s.ends_with(suffix.as_str()))
            .unwrap_or(false),
        DiscriminatorRule::Contains { substring } => string_view(graph, &branch.body, body)
            .map(|s| s.contains(substring.as_str()))
            .unwrap_or(false),
        DiscriminatorRule::Regex { regex } => {
            let Some(s) = string_view(graph, &branch.body, body) else {
                return false;
            };
            match regex::Regex::new(regex.as_str()) {
                Ok(compiled) => compiled.is_match(s),
                Err(_) => false,
            }
        }
        DiscriminatorRule::FieldEquals(field_disc) => {
            let Some(record) = record_view(graph, &branch.body, body) else {
                return false;
            };
            let pos = record
                .field_names
                .iter()
                .position(|n| n == &field_disc.field_name);
            let Some(pos) = pos else {
                return false;
            };
            match &field_disc.literal {
                None => true,
                Some(lit) => {
                    let v = &record.values[pos];
                    matches!(v, SchemaValue::String(s) if s == lit)
                        || matches!(v, SchemaValue::Text(t) if &t.text == lit)
                        || matches!(v, SchemaValue::Url { url } if url == lit)
                        || matches!(v, SchemaValue::Path { path } if path == lit)
                }
            }
        }
        DiscriminatorRule::FieldAbsent { field_name } => {
            let Some(record) = record_view(graph, &branch.body, body) else {
                return false;
            };
            !record.field_names.iter().any(|n| n == field_name)
        }
    }
}

fn string_view<'a>(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &'a SchemaValue,
) -> Option<&'a str> {
    match (resolve(graph, ty), value) {
        (Some(_), SchemaValue::String(s)) => Some(s.as_str()),
        (Some(_), SchemaValue::Text(t)) => Some(t.text.as_str()),
        (Some(_), SchemaValue::Url { url }) => Some(url.as_str()),
        (Some(_), SchemaValue::Path { path }) => Some(path.as_str()),
        _ => None,
    }
}

struct RecordView<'a> {
    field_names: Vec<String>,
    values: &'a [SchemaValue],
}

fn record_view<'a>(
    graph: &SchemaGraph,
    ty: &SchemaType,
    value: &'a SchemaValue,
) -> Option<RecordView<'a>> {
    let resolved = resolve_record(graph, ty)?;
    match value {
        SchemaValue::Record { fields } if fields.len() == resolved.len() => Some(RecordView {
            field_names: resolved,
            values: fields.as_slice(),
        }),
        _ => None,
    }
}

fn resolve<'a>(graph: &'a SchemaGraph, ty: &'a SchemaType) -> Option<&'a SchemaType> {
    let mut current = ty;
    let mut visited: Vec<&TypeId> = Vec::new();
    loop {
        match current {
            SchemaType::Ref { id, .. } => {
                if visited.contains(&id) {
                    return None;
                }
                visited.push(id);
                match graph.lookup(id) {
                    Some(def) => current = &def.body,
                    None => return None,
                }
            }
            other => return Some(other),
        }
    }
}

fn resolve_record(graph: &SchemaGraph, ty: &SchemaType) -> Option<Vec<String>> {
    let resolved = resolve(graph, ty)?;
    match resolved {
        SchemaType::Record { fields, .. } => Some(fields.iter().map(|f| f.name.clone()).collect()),
        _ => None,
    }
}

fn file_extension(p: &str) -> Option<&str> {
    let name = p.rsplit('/').next()?;
    let (_, ext) = name.rsplit_once('.')?;
    if ext.is_empty() { None } else { Some(ext) }
}

fn render_quantity(q: &QuantityValue) -> String {
    format!("{}*10^(-{}) {}", q.mantissa, q.scale, q.unit)
}

fn quantity_le_checked(a: &QuantityValue, b: &QuantityValue) -> Option<bool> {
    let common = a.scale.max(b.scale);
    let a_shift = (common - a.scale).max(0) as u32;
    let b_shift = (common - b.scale).max(0) as u32;
    let ten: i128 = 10;
    let a_factor = ten.checked_pow(a_shift)?;
    let b_factor = ten.checked_pow(b_shift)?;
    let a_canon = (a.mantissa as i128).checked_mul(a_factor)?;
    let b_canon = (b.mantissa as i128).checked_mul(b_factor)?;
    Some(a_canon <= b_canon)
}
