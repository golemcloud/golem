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

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use golem_common::model::agent::structural_format::{
    format_structural, normalize_structural, parse_structural,
};
use golem_common::model::agent::{
    BinaryReference, BinarySource, BinaryType, ComponentModelElementSchema,
    ComponentModelElementValue, DataSchema, DataValue, ElementSchema, ElementValue, ElementValues,
    NamedElementSchema, NamedElementSchemas, NamedElementValue, NamedElementValues, TextDescriptor,
    TextReference, TextSource, TextType, UnstructuredBinaryElementValue,
    UnstructuredTextElementValue,
};
use golem_wasm::analysis::analysed_type::{
    case, f64, field, flags, list, option, record, result, str, tuple, u32, u8, variant,
};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{Value, ValueAndType};

// ── Schema/data helpers ─────────────────────────────────────────────────────

fn tuple_schema(elements: Vec<(&str, ElementSchema)>) -> DataSchema {
    DataSchema::Tuple(NamedElementSchemas {
        elements: elements
            .into_iter()
            .map(|(name, schema)| NamedElementSchema {
                name: name.to_string(),
                schema,
            })
            .collect(),
    })
}

fn multimodal_schema(elements: Vec<(&str, ElementSchema)>) -> DataSchema {
    DataSchema::Multimodal(NamedElementSchemas {
        elements: elements
            .into_iter()
            .map(|(name, schema)| NamedElementSchema {
                name: name.to_string(),
                schema,
            })
            .collect(),
    })
}

fn cm(typ: AnalysedType) -> ElementSchema {
    ElementSchema::ComponentModel(ComponentModelElementSchema { element_type: typ })
}

fn text_elem_schema() -> ElementSchema {
    ElementSchema::UnstructuredText(TextDescriptor { restrictions: None })
}

fn binary_elem_schema() -> ElementSchema {
    ElementSchema::UnstructuredBinary(golem_common::model::agent::BinaryDescriptor {
        restrictions: None,
    })
}

// ── Benchmark data constructors ─────────────────────────────────────────────

/// Simple: single u32 field
fn simple_data_and_schema() -> (DataValue, DataSchema) {
    let schema = tuple_schema(vec![("x", cm(u32()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::U32(42), u32()),
        })],
    });
    (data, schema)
}

/// Medium: record with mixed fields (u32, string, option, list of u8)
fn medium_data_and_schema() -> (DataValue, DataSchema) {
    let typ = record(vec![
        golem_wasm::analysis::analysed_type::field("id", u32()),
        golem_wasm::analysis::analysed_type::field("name", str()),
        golem_wasm::analysis::analysed_type::field("tag", option(str())),
        golem_wasm::analysis::analysed_type::field("scores", list(u8())),
    ]);
    let schema = tuple_schema(vec![("r", cm(typ.clone()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Record(vec![
                    Value::U32(12345),
                    Value::String("benchmark-test".into()),
                    Value::Option(Some(Box::new(Value::String("tagged".into())))),
                    Value::List(vec![
                        Value::U8(10),
                        Value::U8(20),
                        Value::U8(30),
                        Value::U8(40),
                        Value::U8(50),
                    ]),
                ]),
                typ,
            ),
        })],
    });
    (data, schema)
}

/// Complex: nested variant, result, flags, tuple, list of records
fn complex_data_and_schema() -> (DataValue, DataSchema) {
    let inner_record = record(vec![
        golem_wasm::analysis::analysed_type::field("a", u32()),
        golem_wasm::analysis::analysed_type::field("b", str()),
    ]);
    let typ = tuple(vec![
        variant(vec![case("ok", u32()), case("error", str())]),
        result(u32(), str()),
        flags(&["read", "write", "execute"]),
        list(inner_record),
        option(f64()),
    ]);
    let schema = tuple_schema(vec![("t", cm(typ.clone()))]);
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(
                Value::Tuple(vec![
                    Value::Variant {
                        case_idx: 1,
                        case_value: Some(Box::new(Value::String("something went wrong".into()))),
                    },
                    Value::Result(Ok(Some(Box::new(Value::U32(200))))),
                    Value::Flags(vec![true, false, true]),
                    Value::List(vec![
                        Value::Record(vec![Value::U32(1), Value::String("first".into())]),
                        Value::Record(vec![Value::U32(2), Value::String("second".into())]),
                        Value::Record(vec![Value::U32(3), Value::String("third".into())]),
                    ]),
                    Value::Option(Some(Box::new(Value::F64(1.23456789)))),
                ]),
                typ,
            ),
        })],
    });
    (data, schema)
}

/// Multimodal: mixed CM + text + binary elements
fn multimodal_data_and_schema() -> (DataValue, DataSchema) {
    let schema = multimodal_schema(vec![
        (
            "structured",
            cm(record(vec![
                golem_wasm::analysis::analysed_type::field("id", u32()),
                golem_wasm::analysis::analysed_type::field("name", str()),
            ])),
        ),
        ("text", text_elem_schema()),
        ("binary", binary_elem_schema()),
    ]);
    let data = DataValue::Multimodal(NamedElementValues {
        elements: vec![
            NamedElementValue {
                name: "structured".to_string(),
                value: ElementValue::ComponentModel(ComponentModelElementValue {
                    value: ValueAndType::new(
                        Value::Record(vec![Value::U32(99), Value::String("bench".into())]),
                        record(vec![
                            golem_wasm::analysis::analysed_type::field("id", u32()),
                            golem_wasm::analysis::analysed_type::field("name", str()),
                        ]),
                    ),
                }),
                schema_index: 0,
            },
            NamedElementValue {
                name: "text".to_string(),
                value: ElementValue::UnstructuredText(UnstructuredTextElementValue {
                    value: TextReference::Inline(TextSource {
                        data: "Hello, this is a benchmark text element with some content."
                            .to_string(),
                        text_type: Some(TextType {
                            language_code: "en".to_string(),
                        }),
                    }),
                    descriptor: TextDescriptor { restrictions: None },
                }),
                schema_index: 1,
            },
            NamedElementValue {
                name: "binary".to_string(),
                value: ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue {
                    value: BinaryReference::Inline(BinarySource {
                        data: vec![0u8; 256],
                        binary_type: BinaryType {
                            mime_type: "application/octet-stream".to_string(),
                        },
                    }),
                    descriptor: golem_common::model::agent::BinaryDescriptor { restrictions: None },
                }),
                schema_index: 2,
            },
        ],
    });
    (data, schema)
}

/// Large list: 1000 records for throughput measurement
fn large_list_data_and_schema() -> (DataValue, DataSchema) {
    let inner = record(vec![
        golem_wasm::analysis::analysed_type::field("idx", u32()),
        golem_wasm::analysis::analysed_type::field("label", str()),
    ]);
    let typ = list(inner.clone());
    let schema = tuple_schema(vec![("items", cm(typ.clone()))]);
    let items: Vec<Value> = (0..1000)
        .map(|i| Value::Record(vec![Value::U32(i), Value::String(format!("item-{i}"))]))
        .collect();
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::List(items), typ),
        })],
    });
    (data, schema)
}

/// Deep nesting: option nested 30 levels deep — tests recursion depth overhead
fn deep_nesting_data_and_schema() -> (DataValue, DataSchema) {
    let mut typ = u32();
    for _ in 0..30 {
        typ = option(typ);
    }
    let schema = tuple_schema(vec![("nested", cm(typ.clone()))]);
    let mut val: Value = Value::U32(7);
    for _ in 0..30 {
        val = Value::Option(Some(Box::new(val)));
    }
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(val, typ),
        })],
    });
    (data, schema)
}

/// Escaped strings: record with a field full of escape sequences
fn escaped_strings_data_and_schema() -> (DataValue, DataSchema) {
    let typ = record(vec![field("text", str())]);
    let schema = tuple_schema(vec![("r", cm(typ.clone()))]);
    let nasty = "line1\\nline2\\ttab\\\"quoted\\\"back\\\\slash \u{00e9}\u{1f600} end";
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::Record(vec![Value::String(nasty.into())]), typ),
        })],
    });
    (data, schema)
}

/// Large binary: 64 KB inline binary — tests base64 encode/decode dominance
fn large_binary_data_and_schema() -> (DataValue, DataSchema) {
    let schema = multimodal_schema(vec![("blob", binary_elem_schema())]);
    let data = DataValue::Multimodal(NamedElementValues {
        elements: vec![NamedElementValue {
            name: "blob".to_string(),
            schema_index: 0,
            value: ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue {
                value: BinaryReference::Inline(BinarySource {
                    data: vec![0xABu8; 65536],
                    binary_type: BinaryType {
                        mime_type: "application/octet-stream".to_string(),
                    },
                }),
                descriptor: golem_common::model::agent::BinaryDescriptor { restrictions: None },
            }),
        }],
    });
    (data, schema)
}

/// Many flags: 128 flag names with a dense pattern — tests flag formatting/parsing
fn many_flags_data_and_schema() -> (DataValue, DataSchema) {
    let names: Vec<String> = (0..128).map(|i| format!("flag_{i}")).collect();
    let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let typ = flags(&name_refs);
    let schema = tuple_schema(vec![("f", cm(typ.clone()))]);
    // Set every other flag to true
    let bits: Vec<bool> = (0..128).map(|i| i % 2 == 0).collect();
    let data = DataValue::Tuple(ElementValues {
        elements: vec![ElementValue::ComponentModel(ComponentModelElementValue {
            value: ValueAndType::new(Value::Flags(bits), typ),
        })],
    });
    (data, schema)
}

/// Normalize fast-path: a string that is already normalized (no extra whitespace)
fn normalize_already_normal_string() -> String {
    let (data, _schema) = complex_data_and_schema();
    format_structural(&data).unwrap()
}

// ── Benchmarks ──────────────────────────────────────────────────────────────

fn bench_format(c: &mut Criterion) {
    let mut group = c.benchmark_group("format");

    let cases: Vec<(&str, DataValue, DataSchema)> = vec![
        {
            let (d, s) = simple_data_and_schema();
            ("simple", d, s)
        },
        {
            let (d, s) = medium_data_and_schema();
            ("medium", d, s)
        },
        {
            let (d, s) = complex_data_and_schema();
            ("complex", d, s)
        },
        {
            let (d, s) = multimodal_data_and_schema();
            ("multimodal", d, s)
        },
        {
            let (d, s) = large_list_data_and_schema();
            ("large_list_1000", d, s)
        },
        {
            let (d, s) = deep_nesting_data_and_schema();
            ("deep_nesting", d, s)
        },
        {
            let (d, s) = escaped_strings_data_and_schema();
            ("escaped_strings", d, s)
        },
        {
            let (d, s) = large_binary_data_and_schema();
            ("large_binary", d, s)
        },
        {
            let (d, s) = many_flags_data_and_schema();
            ("many_flags", d, s)
        },
    ];

    for (name, data, _schema) in &cases {
        group.bench_with_input(BenchmarkId::new("structural", name), data, |b, d| {
            b.iter(|| format_structural(black_box(d)).unwrap());
        });
    }
    group.finish();
}

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");

    let cases: Vec<(&str, DataValue, DataSchema)> = vec![
        {
            let (d, s) = simple_data_and_schema();
            ("simple", d, s)
        },
        {
            let (d, s) = medium_data_and_schema();
            ("medium", d, s)
        },
        {
            let (d, s) = complex_data_and_schema();
            ("complex", d, s)
        },
        {
            let (d, s) = multimodal_data_and_schema();
            ("multimodal", d, s)
        },
        {
            let (d, s) = large_list_data_and_schema();
            ("large_list_1000", d, s)
        },
        {
            let (d, s) = deep_nesting_data_and_schema();
            ("deep_nesting", d, s)
        },
        {
            let (d, s) = escaped_strings_data_and_schema();
            ("escaped_strings", d, s)
        },
        {
            let (d, s) = large_binary_data_and_schema();
            ("large_binary", d, s)
        },
        {
            let (d, s) = many_flags_data_and_schema();
            ("many_flags", d, s)
        },
    ];

    for (name, data, schema) in &cases {
        let formatted = format_structural(data).unwrap();
        group.bench_with_input(
            BenchmarkId::new("structural", name),
            &(formatted, schema),
            |b, (s, schema)| {
                b.iter(|| parse_structural(black_box(s), black_box(schema)).unwrap());
            },
        );
    }
    group.finish();
}

fn bench_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("roundtrip");

    let cases: Vec<(&str, DataValue, DataSchema)> = vec![
        {
            let (d, s) = simple_data_and_schema();
            ("simple", d, s)
        },
        {
            let (d, s) = medium_data_and_schema();
            ("medium", d, s)
        },
        {
            let (d, s) = complex_data_and_schema();
            ("complex", d, s)
        },
        {
            let (d, s) = deep_nesting_data_and_schema();
            ("deep_nesting", d, s)
        },
        {
            let (d, s) = escaped_strings_data_and_schema();
            ("escaped_strings", d, s)
        },
        {
            let (d, s) = large_binary_data_and_schema();
            ("large_binary", d, s)
        },
        {
            let (d, s) = many_flags_data_and_schema();
            ("many_flags", d, s)
        },
    ];

    for (name, data, schema) in &cases {
        group.bench_with_input(
            BenchmarkId::new("structural", name),
            &(data, schema),
            |b, (d, s)| {
                b.iter(|| {
                    let formatted = format_structural(black_box(d)).unwrap();
                    parse_structural(black_box(&formatted), black_box(s)).unwrap()
                });
            },
        );
    }
    group.finish();
}

fn bench_normalize(c: &mut Criterion) {
    let mut group = c.benchmark_group("normalize");

    let (data, _schema) = complex_data_and_schema();
    let formatted = format_structural(&data).unwrap();

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

    let already_normal = normalize_already_normal_string();
    group.bench_function("already_normal", |b| {
        b.iter(|| normalize_structural(black_box(&already_normal)));
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
