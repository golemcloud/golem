// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::agentic::extended_tool_type::ToolBuildError;
use crate::schema::schema_type::NumericBound;
use crate::schema::{MetadataEnvelope, PathDirection, PathKind, SchemaType, TextRestrictions};

/// Applies text refinements (`regex`/`min_length`/`max_length`) to a text-backed
/// schema. A plain `String` is promoted to `Text` (it has nowhere to store
/// restrictions); a `Text` overlays only the authored fields. Any other schema
/// kind is rejected with [`ToolBuildError::RefinementTypeMismatch`] rather than
/// silently rewritten — this is the runtime backstop for types the macro cannot
/// classify syntactically (e.g. a `type Alias = SomeRecord`).
pub fn refine_text(
    base: SchemaType,
    regex: Option<String>,
    min_len: Option<u32>,
    max_len: Option<u32>,
) -> Result<SchemaType, ToolBuildError> {
    let metadata = base.metadata().clone();
    let mut restrictions = match base {
        SchemaType::Text { restrictions, .. } => restrictions,
        SchemaType::String { .. } => TextRestrictions::default(),
        other => {
            return Err(ToolBuildError::RefinementTypeMismatch {
                refinement: "text",
                actual: schema_kind_name(&other),
            });
        }
    };
    if regex.is_some() {
        restrictions.regex = regex;
    }
    if min_len.is_some() {
        restrictions.min_length = min_len;
    }
    if max_len.is_some() {
        restrictions.max_length = max_len;
    }
    Ok(SchemaType::Text {
        restrictions,
        metadata,
    })
}

/// Applies path refinements to a `Path` schema type. `accepts-stdio` is not a
/// property of the schema type; it lives on the positional that carries the
/// path, so it is set on the command argument rather than here. A non-`Path`
/// schema is rejected (no `String`→`Path` coercion).
pub fn refine_path(
    base: SchemaType,
    direction: Option<PathDirection>,
    kind: Option<PathKind>,
    mime: Option<Vec<String>>,
) -> Result<SchemaType, ToolBuildError> {
    let metadata = base.metadata().clone();
    let mut spec = match base {
        SchemaType::Path { spec, .. } => spec,
        other => {
            return Err(ToolBuildError::RefinementTypeMismatch {
                refinement: "path",
                actual: schema_kind_name(&other),
            });
        }
    };
    if let Some(direction) = direction {
        spec.direction = direction;
    }
    if let Some(kind) = kind {
        spec.kind = kind;
    }
    if mime.is_some() {
        spec.allowed_mime_types = mime;
    }
    Ok(SchemaType::Path { spec, metadata })
}

/// Applies url refinements to a `Url` schema type. A non-`Url` schema is rejected
/// (no `String`→`Url` coercion).
pub fn refine_url(
    base: SchemaType,
    schemes: Option<Vec<String>>,
) -> Result<SchemaType, ToolBuildError> {
    let metadata = base.metadata().clone();
    let mut restrictions = match base {
        SchemaType::Url { restrictions, .. } => restrictions,
        other => {
            return Err(ToolBuildError::RefinementTypeMismatch {
                refinement: "url",
                actual: schema_kind_name(&other),
            });
        }
    };
    if schemes.is_some() {
        restrictions.allowed_schemes = schemes;
    }
    Ok(SchemaType::Url {
        restrictions,
        metadata,
    })
}

/// Applies numeric refinements (`min`/`max`/`unit`) to one of the ten numeric
/// primitive schema variants, preserving the exact variant and overlaying only
/// the authored fields onto any existing restrictions. A non-numeric schema is
/// rejected rather than silently dropping the restrictions.
pub fn refine_numeric(
    base: SchemaType,
    min: Option<NumericBound>,
    max: Option<NumericBound>,
    unit: Option<String>,
) -> Result<SchemaType, ToolBuildError> {
    let metadata = base.metadata().clone();
    // Overlay only the specified fields onto any restrictions the base type
    // already carries (consistent with `refine_text`/`refine_path`/`refine_url`),
    // so refining one field never silently drops the others.
    let mut restrictions = base.numeric_restrictions().cloned().unwrap_or_default();
    if min.is_some() {
        restrictions.min = min;
    }
    if max.is_some() {
        restrictions.max = max;
    }
    if unit.is_some() {
        restrictions.unit = unit;
    }
    let restrictions = restrictions.normalize();
    let refined = match base {
        SchemaType::S8 { .. } => SchemaType::S8 {
            restrictions,
            metadata,
        },
        SchemaType::S16 { .. } => SchemaType::S16 {
            restrictions,
            metadata,
        },
        SchemaType::S32 { .. } => SchemaType::S32 {
            restrictions,
            metadata,
        },
        SchemaType::S64 { .. } => SchemaType::S64 {
            restrictions,
            metadata,
        },
        SchemaType::U8 { .. } => SchemaType::U8 {
            restrictions,
            metadata,
        },
        SchemaType::U16 { .. } => SchemaType::U16 {
            restrictions,
            metadata,
        },
        SchemaType::U32 { .. } => SchemaType::U32 {
            restrictions,
            metadata,
        },
        SchemaType::U64 { .. } => SchemaType::U64 {
            restrictions,
            metadata,
        },
        SchemaType::F32 { .. } => SchemaType::F32 {
            restrictions,
            metadata,
        },
        SchemaType::F64 { .. } => SchemaType::F64 {
            restrictions,
            metadata,
        },
        other => {
            return Err(ToolBuildError::RefinementTypeMismatch {
                refinement: "numeric",
                actual: schema_kind_name(&other),
            });
        }
    };
    Ok(refined)
}

/// A short, stable name for a schema kind, used in
/// [`ToolBuildError::RefinementTypeMismatch`] messages. Exhaustive so adding a
/// `SchemaType` variant forces an update here.
fn schema_kind_name(ty: &SchemaType) -> &'static str {
    match ty {
        SchemaType::Ref { .. } => "ref",
        SchemaType::Bool { .. } => "bool",
        SchemaType::S8 { .. }
        | SchemaType::S16 { .. }
        | SchemaType::S32 { .. }
        | SchemaType::S64 { .. }
        | SchemaType::U8 { .. }
        | SchemaType::U16 { .. }
        | SchemaType::U32 { .. }
        | SchemaType::U64 { .. }
        | SchemaType::F32 { .. }
        | SchemaType::F64 { .. } => "numeric",
        SchemaType::Char { .. } => "char",
        SchemaType::String { .. } => "string",
        SchemaType::Text { .. } => "text",
        SchemaType::Path { .. } => "path",
        SchemaType::Url { .. } => "url",
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
        SchemaType::Binary { .. } => "binary",
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

pub fn empty_metadata() -> MetadataEnvelope {
    MetadataEnvelope::default()
}

/// Converts a concrete Rust numeric value into the matching [`NumericBound`]
/// family for its representation. Used by the tool macro to lower
/// `#[arg(min = …, max = …, bounds = (…, …))]` literals to bounds whose family
/// (`Signed`/`Unsigned`/`FloatBits`) matches the argument's numeric type,
/// without the macro having to classify the representation itself.
///
/// Fallible because a float bound literal can be `NaN`/`inf`, which must surface
/// as a [`ToolBuildError`] from the descriptor build rather than panicking.
pub trait IntoNumericBound {
    fn into_numeric_bound(self) -> Result<NumericBound, ToolBuildError>;
}

macro_rules! impl_into_numeric_bound_signed {
    ($($t:ty),*) => {
        $(impl IntoNumericBound for $t {
            fn into_numeric_bound(self) -> Result<NumericBound, ToolBuildError> {
                Ok(NumericBound::Signed(self as i64))
            }
        })*
    };
}
macro_rules! impl_into_numeric_bound_unsigned {
    ($($t:ty),*) => {
        $(impl IntoNumericBound for $t {
            fn into_numeric_bound(self) -> Result<NumericBound, ToolBuildError> {
                Ok(NumericBound::Unsigned(self as u64))
            }
        })*
    };
}
macro_rules! impl_into_numeric_bound_float {
    ($($t:ty),*) => {
        $(impl IntoNumericBound for $t {
            fn into_numeric_bound(self) -> Result<NumericBound, ToolBuildError> {
                NumericBound::float(self as f64)
                    .map_err(|e| ToolBuildError::InvalidNumericBound(e.to_string()))
            }
        })*
    };
}

// `usize` is the only platform-width integer with a schema value mapping
// (modeled as `u64`); `isize` has no `IntoSchema`/`FromSchema`, so it is not a
// usable tool value type and intentionally has no bound conversion here.
impl_into_numeric_bound_signed!(i8, i16, i32, i64);
impl_into_numeric_bound_unsigned!(u8, u16, u32, u64, usize);
impl_into_numeric_bound_float!(f32, f64);
