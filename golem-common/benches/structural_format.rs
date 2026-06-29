// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use golem_common::model::agent::structural_format::{
    format_structural_typed, normalize_structural, parse_structural_typed,
};
use golem_common::schema::{
    BinaryRestrictions, BinaryValuePayload, MetadataEnvelope, NamedFieldType, ResultSpec,
    ResultValuePayload, SchemaGraph, SchemaType, SchemaValue, TextRestrictions, TextValuePayload,
    TypedSchemaValue, VariantCaseType, VariantValuePayload,
};

// ── Schema/value helpers ────────────────────────────────────────────────────

fn field(name: &str, body: SchemaType) -> NamedFieldType {
    NamedFieldType {
        name: name.to_string(),
        body,
        metadata: MetadataEnvelope::default(),
    }
}

fn case(name: &str, payload: Option<SchemaType>) -> VariantCaseType {
    VariantCaseType {
        name: name.to_string(),
        payload,
        metadata: MetadataEnvelope::default(),
    }
}

/// Wrap a single root type + value into a positional parameter record.
fn typed(root: SchemaType, value: SchemaValue) -> TypedSchemaValue {
    TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(vec![field("p0", root)])),
        SchemaValue::Record {
            fields: vec![value],
        },
    )
}

// ── Benchmark data constructors ─────────────────────────────────────────────

/// Simple: single u32 field
fn simple() -> TypedSchemaValue {
    typed(SchemaType::u32(), SchemaValue::U32(42))
}

/// Medium: record with mixed fields (u32, string, option, list of u8)
fn medium() -> TypedSchemaValue {
    let root = SchemaType::record(vec![
        field("id", SchemaType::u32()),
        field("name", SchemaType::string()),
        field("tag", SchemaType::option(SchemaType::string())),
        field("scores", SchemaType::list(SchemaType::u8())),
    ]);
    let value = SchemaValue::Record {
        fields: vec![
            SchemaValue::U32(12345),
            SchemaValue::String("benchmark-test".into()),
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::String("tagged".into()))),
            },
            SchemaValue::List {
                elements: vec![
                    SchemaValue::U8(10),
                    SchemaValue::U8(20),
                    SchemaValue::U8(30),
                    SchemaValue::U8(40),
                    SchemaValue::U8(50),
                ],
            },
        ],
    };
    typed(root, value)
}

/// Complex: nested variant, result, flags, tuple, list of records
fn complex() -> TypedSchemaValue {
    let inner_record = SchemaType::record(vec![
        field("a", SchemaType::u32()),
        field("b", SchemaType::string()),
    ]);
    let root = SchemaType::tuple(vec![
        SchemaType::variant(vec![
            case("ok", Some(SchemaType::u32())),
            case("error", Some(SchemaType::string())),
        ]),
        SchemaType::result(ResultSpec {
            ok: Some(Box::new(SchemaType::u32())),
            err: Some(Box::new(SchemaType::string())),
        }),
        SchemaType::flags(vec!["read".into(), "write".into(), "execute".into()]),
        SchemaType::list(inner_record),
        SchemaType::option(SchemaType::f64()),
    ]);
    let value = SchemaValue::Tuple {
        elements: vec![
            SchemaValue::Variant(VariantValuePayload {
                case: 1,
                payload: Some(Box::new(SchemaValue::String("something went wrong".into()))),
            }),
            SchemaValue::Result(ResultValuePayload::Ok {
                value: Some(Box::new(SchemaValue::U32(200))),
            }),
            SchemaValue::Flags {
                bits: vec![true, false, true],
            },
            SchemaValue::List {
                elements: vec![
                    SchemaValue::Record {
                        fields: vec![SchemaValue::U32(1), SchemaValue::String("first".into())],
                    },
                    SchemaValue::Record {
                        fields: vec![SchemaValue::U32(2), SchemaValue::String("second".into())],
                    },
                    SchemaValue::Record {
                        fields: vec![SchemaValue::U32(3), SchemaValue::String("third".into())],
                    },
                ],
            },
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::F64(1.23456789))),
            },
        ],
    };
    typed(root, value)
}

/// Large list: 1000 records for throughput measurement
fn large_list() -> TypedSchemaValue {
    let inner = SchemaType::record(vec![
        field("idx", SchemaType::u32()),
        field("label", SchemaType::string()),
    ]);
    let root = SchemaType::list(inner);
    let elements: Vec<SchemaValue> = (0..1000)
        .map(|i| SchemaValue::Record {
            fields: vec![
                SchemaValue::U32(i),
                SchemaValue::String(format!("item-{i}")),
            ],
        })
        .collect();
    typed(root, SchemaValue::List { elements })
}

/// Deep nesting: option nested 30 levels deep — tests recursion depth overhead
fn deep_nesting() -> TypedSchemaValue {
    let mut root = SchemaType::u32();
    for _ in 0..30 {
        root = SchemaType::option(root);
    }
    let mut value = SchemaValue::U32(7);
    for _ in 0..30 {
        value = SchemaValue::Option {
            inner: Some(Box::new(value)),
        };
    }
    typed(root, value)
}

/// Escaped strings: record with a field full of escape sequences
fn escaped_strings() -> TypedSchemaValue {
    let root = SchemaType::record(vec![field("text", SchemaType::string())]);
    let nasty = "line1\\nline2\\ttab\\\"quoted\\\"back\\\\slash \u{00e9}\u{1f600} end";
    let value = SchemaValue::Record {
        fields: vec![SchemaValue::String(nasty.into())],
    };
    typed(root, value)
}

/// Large binary: 64 KB inline binary — tests base64 encode/decode dominance
fn large_binary() -> TypedSchemaValue {
    typed(
        SchemaType::binary(BinaryRestrictions::default()),
        SchemaValue::Binary(BinaryValuePayload {
            bytes: vec![0xABu8; 65536],
            mime_type: Some("application/octet-stream".to_string()),
        }),
    )
}

/// Many flags: 128 flag names with a dense pattern — tests flag formatting/parsing
fn many_flags() -> TypedSchemaValue {
    let names: Vec<String> = (0..128).map(|i| format!("flag_{i}")).collect();
    let root = SchemaType::flags(names);
    let bits: Vec<bool> = (0..128).map(|i| i % 2 == 0).collect();
    typed(root, SchemaValue::Flags { bits })
}

/// Multimodal: text + binary
fn multimodal() -> TypedSchemaValue {
    let root = SchemaType::record(vec![
        field("text", SchemaType::text(TextRestrictions::default())),
        field("binary", SchemaType::binary(BinaryRestrictions::default())),
    ]);
    let value = SchemaValue::Record {
        fields: vec![
            SchemaValue::Text(TextValuePayload {
                text: "Hello, this is a benchmark text element with some content.".to_string(),
                language: Some("en".to_string()),
            }),
            SchemaValue::Binary(BinaryValuePayload {
                bytes: vec![0u8; 256],
                mime_type: Some("application/octet-stream".to_string()),
            }),
        ],
    };
    typed(root, value)
}

fn all_cases() -> Vec<(&'static str, TypedSchemaValue)> {
    vec![
        ("simple", simple()),
        ("medium", medium()),
        ("complex", complex()),
        ("multimodal", multimodal()),
        ("large_list_1000", large_list()),
        ("deep_nesting", deep_nesting()),
        ("escaped_strings", escaped_strings()),
        ("large_binary", large_binary()),
        ("many_flags", many_flags()),
    ]
}

// ── Benchmarks ──────────────────────────────────────────────────────────────

fn bench_format(c: &mut Criterion) {
    let mut group = c.benchmark_group("format");
    for (name, value) in &all_cases() {
        group.bench_with_input(BenchmarkId::new("structural", name), value, |b, v| {
            b.iter(|| format_structural_typed(black_box(v)).unwrap());
        });
    }
    group.finish();
}

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");
    for (name, value) in &all_cases() {
        let formatted = format_structural_typed(value).unwrap();
        group.bench_with_input(
            BenchmarkId::new("structural", name),
            &(formatted, value),
            |b, (s, v)| {
                b.iter(|| parse_structural_typed(black_box(s), v.graph(), v.root_type()).unwrap());
            },
        );
    }
    group.finish();
}

fn bench_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("roundtrip");
    for (name, value) in &all_cases() {
        group.bench_with_input(BenchmarkId::new("structural", name), value, |b, v| {
            b.iter(|| {
                let formatted = format_structural_typed(black_box(v)).unwrap();
                parse_structural_typed(black_box(&formatted), v.graph(), v.root_type()).unwrap()
            });
        });
    }
    group.finish();
}

fn bench_normalize(c: &mut Criterion) {
    let mut group = c.benchmark_group("normalize");

    let formatted = format_structural_typed(&complex()).unwrap();

    // Add whitespace to simulate user input
    let with_spaces = formatted
        .chars()
        .flat_map(|c| {
            if c == ',' {
                vec![' ', ',', ' ']
            } else if c == '(' {
                vec!['(', ' ']
            } else {
                vec![c]
            }
        })
        .collect::<String>();

    group.bench_function("complex_with_spaces", |b| {
        b.iter(|| normalize_structural(black_box(&with_spaces)));
    });

    let large_input = with_spaces.repeat(10);
    group.bench_function("large_repeated", |b| {
        b.iter(|| normalize_structural(black_box(&large_input)));
    });

    group.bench_function("already_normal", |b| {
        b.iter(|| normalize_structural(black_box(&formatted)));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_format,
    bench_parse,
    bench_roundtrip,
    bench_normalize
);
criterion_main!(benches);
