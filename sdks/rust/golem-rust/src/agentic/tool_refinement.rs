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

use crate::schema::schema_type::NumericBound;
use crate::schema::{
    MetadataEnvelope, PathDirection, PathKind, PathSpec, SchemaType, TextRestrictions,
    UrlRestrictions,
};

pub fn refine_text(
    base: SchemaType,
    regex: Option<String>,
    min_len: Option<u32>,
    max_len: Option<u32>,
) -> SchemaType {
    let metadata = base.metadata().clone();
    let mut restrictions = match base {
        SchemaType::Text { restrictions, .. } => restrictions,
        _ => TextRestrictions::default(),
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
    SchemaType::Text {
        restrictions,
        metadata,
    }
}

/// Applies path refinements to a `Path` schema type. `accepts-stdio` is not a
/// property of the schema type; it lives on the positional that carries the
/// path, so it is set on the command argument rather than here.
pub fn refine_path(
    base: SchemaType,
    direction: Option<PathDirection>,
    kind: Option<PathKind>,
    mime: Option<Vec<String>>,
) -> SchemaType {
    let metadata = base.metadata().clone();
    let mut spec = match base {
        SchemaType::Path { spec, .. } => spec,
        _ => PathSpec {
            direction: PathDirection::InOut,
            kind: PathKind::Any,
            allowed_mime_types: None,
            allowed_extensions: None,
        },
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
    SchemaType::Path { spec, metadata }
}

pub fn refine_url(base: SchemaType, schemes: Option<Vec<String>>) -> SchemaType {
    let metadata = base.metadata().clone();
    let mut restrictions = match base {
        SchemaType::Url { restrictions, .. } => restrictions,
        _ => UrlRestrictions::default(),
    };
    if schemes.is_some() {
        restrictions.allowed_schemes = schemes;
    }
    SchemaType::Url {
        restrictions,
        metadata,
    }
}

pub fn refine_numeric(
    base: SchemaType,
    min: Option<NumericBound>,
    max: Option<NumericBound>,
    unit: Option<String>,
) -> SchemaType {
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
    match base {
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
        other => other.with_metadata(metadata),
    }
}

pub fn empty_metadata() -> MetadataEnvelope {
    MetadataEnvelope::default()
}
