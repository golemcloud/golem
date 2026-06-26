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

#[cfg(feature = "full")]
mod binary_codec_tests;
#[cfg(feature = "full")]
mod protobuf_tests;
#[cfg(feature = "full")]
mod schema_derive_tests;
#[cfg(feature = "full")]
mod wit_tests;

/// Deterministic cross-SDK numeric-restriction vectors, shared by the protobuf,
/// WIT, and binary codec round-trip tests. Every entry is already normalized
/// (no `Some(empty)`), so each codec must preserve it exactly. Empty → `None`
/// normalization is covered separately by the per-codec decode-boundary tests.
#[cfg(feature = "full")]
pub(crate) fn golden_numeric_schema_types()
-> Vec<(&'static str, crate::schema::schema_type::SchemaType)> {
    use crate::schema::metadata::MetadataEnvelope;
    use crate::schema::schema_type::{NumericBound, NumericRestrictions, SchemaType};

    fn restr(
        min: Option<NumericBound>,
        max: Option<NumericBound>,
        unit: Option<&str>,
    ) -> Option<NumericRestrictions> {
        NumericRestrictions {
            min,
            max,
            unit: unit.map(|u| u.to_string()),
        }
        .normalize()
    }

    macro_rules! numeric {
        ($variant:ident, $r:expr) => {
            SchemaType::$variant {
                restrictions: $r,
                metadata: MetadataEnvelope::default(),
            }
        };
    }

    let u = NumericBound::Unsigned;
    let s = NumericBound::Signed;
    let f = |v: f64| NumericBound::float(v).expect("finite bound");

    vec![
        // bare u32 (None) — the unconstrained hot-path case
        ("u32 bare", SchemaType::u32()),
        // u32 min=1
        ("u32 min=1", numeric!(U32, restr(Some(u(1)), None, None))),
        (
            "u32 min=1 +unit",
            numeric!(U32, restr(Some(u(1)), None, Some("items"))),
        ),
        // u32 bounds=(0,100)
        (
            "u32 bounds=(0,100)",
            numeric!(U32, restr(Some(u(0)), Some(u(100)), None)),
        ),
        (
            "u32 bounds=(0,100) +unit",
            numeric!(U32, restr(Some(u(0)), Some(u(100)), Some("percent"))),
        ),
        // s64 bounds=(0, i64::MAX)
        (
            "s64 bounds=(0,i64::MAX)",
            numeric!(S64, restr(Some(s(0)), Some(s(i64::MAX)), None)),
        ),
        (
            "s64 bounds=(0,i64::MAX) +unit",
            numeric!(S64, restr(Some(s(0)), Some(s(i64::MAX)), Some("ns"))),
        ),
        // u64 near u64::MAX
        (
            "u64 near u64::MAX",
            numeric!(U64, restr(Some(u(u64::MAX - 1)), Some(u(u64::MAX)), None)),
        ),
        (
            "u64 near u64::MAX +unit",
            numeric!(
                U64,
                restr(Some(u(u64::MAX - 1)), Some(u(u64::MAX)), Some("bytes"))
            ),
        ),
        // f64 min=0.0
        (
            "f64 min=0.0",
            numeric!(F64, restr(Some(f(0.0)), None, None)),
        ),
        (
            "f64 min=0.0 +unit",
            numeric!(F64, restr(Some(f(0.0)), None, Some("seconds"))),
        ),
        // s8 / f32 coverage for the desert variant-payload change
        (
            "s8 bounds=(-1,1)",
            numeric!(S8, restr(Some(s(-1)), Some(s(1)), None)),
        ),
        (
            "f32 max=1.5",
            numeric!(F32, restr(None, Some(f(1.5)), None)),
        ),
    ]
}
