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

//! Hot-path micro-benchmarks for the schema graph and value validator.
//!
//! These exercise the operations that run on every worker invocation:
//! named-ref lookup in the graph, ref-chain resolution, and structural
//! value validation against a schema. Run before/after a change with:
//!
//! ```text
//! cargo bench -p golem-schema --bench schema_hotpath
//! ```

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use golem_schema::schema::graph::{GraphIndex, SchemaGraph, SchemaTypeDef};
use golem_schema::schema::host_managed::find_host_managed_value;
use golem_schema::schema::metadata::{MetadataEnvelope, TypeId};
use golem_schema::schema::schema_type::{
    NamedFieldType, SchemaType, TextRestrictions, VariantCaseType,
};
use golem_schema::schema::schema_value::{
    SchemaValue, SecretValuePayload, TextValuePayload, VariantValuePayload,
};
use golem_schema::schema::validation::value::{PreparedValueValidator, validate_value};

// ── Builders ────────────────────────────────────────────────────────────────

fn field(name: &str, body: SchemaType) -> NamedFieldType {
    NamedFieldType {
        name: name.to_string(),
        body,
        metadata: MetadataEnvelope::default(),
    }
}

fn def(id: &str, body: SchemaType) -> SchemaTypeDef {
    SchemaTypeDef {
        id: TypeId::new(id),
        name: None,
        body,
    }
}

/// A graph with `n` named record defs (`t000`..) plus a recursive `tree` def.
/// The root record references several named defs plus a list of `tree`.
fn wide_graph(n: usize) -> (SchemaGraph, SchemaValue) {
    let mut defs = Vec::with_capacity(n + 1);
    for i in 0..n {
        defs.push(def(
            &format!("t{i:03}"),
            SchemaType::record(vec![
                field("a", SchemaType::u32()),
                field("b", SchemaType::string()),
                field("c", SchemaType::option(SchemaType::u64())),
            ]),
        ));
    }
    // Recursive tree: variant { leaf(s32), node(tuple[ref tree, ref tree]) }
    defs.push(def(
        "tree",
        SchemaType::variant(vec![
            VariantCaseType {
                name: "leaf".to_string(),
                payload: Some(SchemaType::s32()),
                metadata: MetadataEnvelope::default(),
            },
            VariantCaseType {
                name: "node".to_string(),
                payload: Some(SchemaType::tuple(vec![
                    SchemaType::ref_to(TypeId::new("tree")),
                    SchemaType::ref_to(TypeId::new("tree")),
                ])),
                metadata: MetadataEnvelope::default(),
            },
        ]),
    ));

    // Root references the first, middle and last named defs (forcing linear
    // scans deep into the defs vec), plus a list<tree>.
    let root = SchemaType::record(vec![
        field("first", SchemaType::ref_to(TypeId::new("t000"))),
        field(
            "middle",
            SchemaType::ref_to(TypeId::new(format!("t{:03}", n / 2))),
        ),
        field(
            "last",
            SchemaType::ref_to(TypeId::new(format!("t{:03}", n - 1))),
        ),
        field(
            "trees",
            SchemaType::list(SchemaType::ref_to(TypeId::new("tree"))),
        ),
    ]);

    let graph = SchemaGraph {
        defs,
        root: root.clone(),
    };

    let named_record_value = SchemaValue::Record {
        fields: vec![
            SchemaValue::U32(1),
            SchemaValue::String("x".to_string()),
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::U64(2))),
            },
        ],
    };

    // Balanced tree of depth 6.
    fn tree(depth: usize) -> SchemaValue {
        if depth == 0 {
            SchemaValue::Variant(VariantValuePayload {
                case: 0,
                payload: Some(Box::new(SchemaValue::S32(7))),
            })
        } else {
            SchemaValue::Variant(VariantValuePayload {
                case: 1,
                payload: Some(Box::new(SchemaValue::Tuple {
                    elements: vec![tree(depth - 1), tree(depth - 1)],
                })),
            })
        }
    }

    let value = SchemaValue::Record {
        fields: vec![
            named_record_value.clone(),
            named_record_value.clone(),
            named_record_value,
            SchemaValue::List {
                elements: (0..16).map(|_| tree(6)).collect(),
            },
        ],
    };

    (graph, value)
}

/// Flat record with no refs: a baseline for validation that does not hit the
/// graph at all.
fn flat_record() -> (SchemaGraph, SchemaType, SchemaValue) {
    let ty = SchemaType::record(vec![
        field("a", SchemaType::u32()),
        field("b", SchemaType::string()),
        field("c", SchemaType::list(SchemaType::u8())),
        field("d", SchemaType::option(SchemaType::bool())),
    ]);
    let value = SchemaValue::Record {
        fields: vec![
            SchemaValue::U32(1),
            SchemaValue::String("hello world".to_string()),
            SchemaValue::List {
                elements: (0..64).map(SchemaValue::U8).collect(),
            },
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::Bool(true))),
            },
        ],
    };
    (SchemaGraph::anonymous(ty.clone()), ty, value)
}

// ── Benchmarks ───────────────────────────────────────────────────────────────

fn bench_lookup(c: &mut Criterion) {
    let (graph, _) = wide_graph(64);
    let last = TypeId::new("t063");
    // Same-length, same-prefix id that is absent: forces a full scan with
    // realistic per-element comparison cost (unlike a length-mismatched miss
    // that rejects on the first byte).
    let missing = TypeId::new("t999");

    let mut group = c.benchmark_group("graph_lookup");
    group.bench_function("hit_last", |b| {
        b.iter(|| black_box(graph.lookup(black_box(&last))).is_some())
    });
    group.bench_function("miss_same_len", |b| {
        b.iter(|| black_box(graph.lookup(black_box(&missing))).is_some())
    });
    group.finish();
}

fn bench_resolve_ref(c: &mut Criterion) {
    let (graph, _) = wide_graph(64);
    let ref_last = SchemaType::ref_to(TypeId::new("t063"));

    c.bench_function("graph_resolve_ref/hit_last", |b| {
        b.iter(|| black_box(graph.resolve_ref(black_box(&ref_last)).is_ok()))
    });
}

fn bench_validate(c: &mut Criterion) {
    let mut group = c.benchmark_group("validate_value");

    let (flat_graph, flat_ty, flat_val) = flat_record();
    group.bench_function("flat_record", |b| {
        b.iter(|| black_box(validate_value(&flat_graph, &flat_ty, &flat_val).is_ok()))
    });

    let (graph, value) = wide_graph(64);
    let root = graph.root.clone();
    group.bench_function("wide_graph_with_refs_and_recursion", |b| {
        b.iter(|| black_box(validate_value(&graph, &root, &value).is_ok()))
    });
    let prepared = PreparedValueValidator::new(&graph, &root);
    group.bench_function("wide_graph_prepared", |b| {
        b.iter(|| black_box(prepared.validate(&value).is_ok()))
    });

    group.finish();
}

fn bench_graph_index(c: &mut Criterion) {
    let (wide, _) = wide_graph(64);
    let last = TypeId::new("t063");
    let missing = TypeId::new("t999");

    let (small, _) = wide_graph(4);
    let small_last = TypeId::new("t003");

    let mut group = c.benchmark_group("graph_index");

    // One-shot construction cost of the borrowed accelerator over a wide graph
    // (this is paid once per validation/rendering traversal).
    group.bench_function("build_wide", |b| {
        b.iter(|| black_box(GraphIndex::new(black_box(&wide))))
    });

    // Indexed lookups over a wide graph: should be ~O(1) vs the linear
    // `graph_lookup/*` benches above.
    let wide_index = GraphIndex::new(&wide);
    group.bench_function("lookup_hit_last_indexed", |b| {
        b.iter(|| black_box(wide_index.lookup(black_box(&last))).is_some())
    });
    group.bench_function("lookup_miss_indexed", |b| {
        b.iter(|| black_box(wide_index.lookup(black_box(&missing))).is_some())
    });

    // Small graph stays on the linear fallback (below the index threshold):
    // confirms narrow inputs are not penalised by index construction.
    group.bench_function("build_small_linear", |b| {
        b.iter(|| black_box(GraphIndex::new(black_box(&small))))
    });
    let small_index = GraphIndex::new(&small);
    group.bench_function("lookup_small_linear", |b| {
        b.iter(|| black_box(small_index.lookup(black_box(&small_last))).is_some())
    });

    group.finish();
}

fn bench_capability_traversal(c: &mut Criterion) {
    let mut group = c.benchmark_group("capability_traversal");
    for size in [100, 10_000] {
        let plain = SchemaValue::List {
            elements: vec![SchemaValue::U8(7); size],
        };
        let mut late = plain.clone();
        let SchemaValue::List { elements } = &mut late else {
            unreachable!()
        };
        elements.push(SchemaValue::Secret(SecretValuePayload {
            secret_id: uuid::Uuid::nil(),
            config_key: None,
            version: 0,
            resolved_at: chrono::DateTime::UNIX_EPOCH,
            category: None,
        }));
        group.bench_function(format!("no_match/{size}"), |b| {
            b.iter(|| black_box(find_host_managed_value(black_box(&plain))))
        });
        group.bench_function(format!("late_match/{size}"), |b| {
            b.iter(|| black_box(find_host_managed_value(black_box(&late))))
        });
    }
    group.finish();
}

#[cfg(feature = "regex")]
fn bench_constrained_text(c: &mut Criterion) {
    let pattern = "^[a-z]+$";
    let graph = SchemaGraph::anonymous(SchemaType::list(SchemaType::text(TextRestrictions {
        regex: Some(pattern.into()),
        ..Default::default()
    })));
    let mut group = c.benchmark_group("constrained_text");
    for size in [100, 10_000] {
        let value = SchemaValue::List {
            elements: vec![
                SchemaValue::Text(TextValuePayload {
                    text: "asymmetric".into(),
                    language: None,
                });
                size
            ],
        };
        group.bench_function(format!("validate/{size}"), |b| {
            b.iter(|| black_box(validate_value(&graph, &graph.root, black_box(&value))).unwrap())
        });
        let prepared = PreparedValueValidator::new(&graph, &graph.root);
        group.bench_function(format!("prepared/{size}"), |b| {
            b.iter(|| black_box(prepared.validate(black_box(&value))).unwrap())
        });
    }
    group.bench_function("compile_regex", |b| {
        b.iter(|| black_box(regex::Regex::new(black_box(pattern))).unwrap())
    });
    let compiled = regex::Regex::new(pattern).unwrap();
    group.bench_function("match_precompiled", |b| {
        b.iter(|| black_box(compiled.is_match(black_box("asymmetric"))))
    });
    group.finish();
}

#[cfg(not(feature = "regex"))]
fn bench_constrained_text(_: &mut Criterion) {}

criterion_group!(
    benches,
    bench_lookup,
    bench_resolve_ref,
    bench_validate,
    bench_graph_index,
    bench_capability_traversal,
    bench_constrained_text
);
criterion_main!(benches);
