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

//! Hot-path micro-benchmarks for the agent invocation lowering path.
//!
//! These mirror the per-invocation costs in the worker executor:
//!  - cloning the resolved [`AgentTypeSchema`] (incl. its full graph),
//!  - rebuilding the method-input record type and validating the input,
//!  - converting REST/JSON inputs into a typed schema value.
//!
//! Run before/after a change with:
//!
//! ```text
//! cargo bench -p golem-common --bench invocation_hotpath
//! ```

use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use golem_common::base_model::Empty;
use golem_common::base_model::agent::{AgentMode, AgentTypeName, Snapshotting};
use golem_common::schema::agent::{
    json_input_schema_value_to_typed_schema_value, typed_schema_value_with_projected_defs,
};
use golem_common::schema::validation::validate_value;
use golem_common::schema::validation::value::validate_record_fields;
use golem_common::schema::{
    AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, AutoInjectedKind, FieldSource,
    InputSchema, MetadataEnvelope, NamedField, NamedFieldType, OutputSchema, SchemaGraph,
    SchemaType, SchemaTypeDef, SchemaValue, TypeId, TypedSchemaValue,
};

// ── Builders ────────────────────────────────────────────────────────────────

fn def(id: &str, body: SchemaType) -> SchemaTypeDef {
    SchemaTypeDef {
        id: TypeId::new(id),
        name: None,
        body,
    }
}

/// A graph with `n` named record defs, used as the agent's shared `defs`
/// registry so that ref lookups during validation perform a realistic linear
/// scan.
fn shared_graph(n: usize) -> SchemaGraph {
    let mut defs = Vec::with_capacity(n);
    for i in 0..n {
        defs.push(def(
            &format!("t{i:03}"),
            SchemaType::record(vec![
                NamedFieldType {
                    name: "a".to_string(),
                    body: SchemaType::u32(),
                    metadata: MetadataEnvelope::default(),
                },
                NamedFieldType {
                    name: "b".to_string(),
                    body: SchemaType::string(),
                    metadata: MetadataEnvelope::default(),
                },
            ]),
        ));
    }
    SchemaGraph {
        defs,
        root: SchemaType::record(Vec::new()),
    }
}

/// A representative agent type: a non-trivial shared graph plus one method
/// whose input parameters mix primitives, a list, an option, and references
/// into the shared graph.
fn representative_agent_type() -> AgentTypeSchema {
    let method_fields = vec![
        NamedField::user_supplied("count", SchemaType::u32()),
        NamedField::user_supplied("label", SchemaType::string()),
        NamedField::user_supplied("items", SchemaType::list(SchemaType::u8())),
        NamedField::user_supplied("maybe", SchemaType::option(SchemaType::bool())),
        NamedField::user_supplied("first", SchemaType::ref_to(TypeId::new("t000"))),
        NamedField::user_supplied("last", SchemaType::ref_to(TypeId::new("t031"))),
        // Host-injected field: excluded from caller input arity and validation,
        // exercising the `FieldSource::UserSupplied` filter in the hot path.
        NamedField::auto_injected(
            "principal",
            AutoInjectedKind::Principal,
            SchemaType::string(),
        ),
    ];

    AgentTypeSchema {
        type_name: AgentTypeName("bench-agent".to_string()),
        description: "benchmark agent".to_string(),
        source_language: "rust".to_string(),
        schema: shared_graph(32),
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                "seed",
                SchemaType::u64(),
            )]),
        },
        methods: vec![AgentMethodSchema {
            name: "do-work".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(method_fields),
            output_schema: OutputSchema::Unit,
            http_endpoint: Vec::new(),
            read_only: None,
        }],
        dependencies: Vec::new(),
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: Vec::new(),
    }
}

/// The input value matching the representative method's parameter list.
fn method_input_value() -> SchemaValue {
    let named_record = SchemaValue::Record {
        fields: vec![SchemaValue::U32(1), SchemaValue::String("x".to_string())],
    };
    SchemaValue::Record {
        fields: vec![
            SchemaValue::U32(7),
            SchemaValue::String("hello".to_string()),
            SchemaValue::List {
                elements: (0..32).map(SchemaValue::U8).collect(),
            },
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::Bool(true))),
            },
            named_record.clone(),
            named_record,
        ],
    }
}

/// Mirror of `validate_schema_input_against_method_schema` in the worker
/// executor: it rebuilds a record `SchemaType` from the method's user-supplied
/// fields (cloning each field schema + metadata) and validates the input.
fn validate_method_input(agent_type: &AgentTypeSchema, input: &SchemaValue) -> bool {
    let method = &agent_type.methods[0];
    let user_fields: Vec<_> = method
        .input_schema
        .fields()
        .iter()
        .filter(|field| matches!(field.source, FieldSource::UserSupplied))
        .collect();
    let record_type = SchemaType::record(
        user_fields
            .iter()
            .map(|field| NamedFieldType {
                name: field.name.clone(),
                body: field.schema.clone(),
                metadata: field.metadata.clone(),
            })
            .collect(),
    );
    validate_value(&agent_type.schema, &record_type, input).is_ok()
}

/// Opt5 borrowed-field validation: validates the method's user-supplied fields
/// against the input record values directly, with no temporary record
/// `SchemaType` and no per-field schema/metadata clones.
fn validate_method_input_borrowed(agent_type: &AgentTypeSchema, input: &SchemaValue) -> bool {
    let method = &agent_type.methods[0];
    let SchemaValue::Record { fields: values } = input else {
        return false;
    };
    let user_fields: Vec<_> = method
        .input_schema
        .fields()
        .iter()
        .filter(|field| matches!(field.source, FieldSource::UserSupplied))
        .collect();
    validate_record_fields(
        &agent_type.schema,
        user_fields.iter().map(|f| (f.name.as_str(), &f.schema)),
        values,
    )
    .is_ok()
}

/// A value matching a `Ref(tNNN)` field: each shared def `tNNN` is
/// `record { a: u32, b: string }`.
fn ref_field_value() -> SchemaValue {
    SchemaValue::Record {
        fields: vec![SchemaValue::U32(1), SchemaValue::String("x".to_string())],
    }
}

/// One named input-conversion scenario: a 32-def agent graph plus an input
/// schema + matching on-wire JSON value. The cases vary how many of the graph's
/// defs are transitively reachable from the synthesized input record (the
/// quantity Option 7's projection avoids cloning).
struct JsonInputCase {
    name: &'static str,
    graph: SchemaGraph,
    input_schema: InputSchema,
    json: serde_json::Value,
}

fn json_input_cases() -> Vec<JsonInputCase> {
    let mk = |name: &'static str, input_schema: InputSchema, value: SchemaValue| JsonInputCase {
        name,
        graph: shared_graph(32),
        input_schema,
        json: serde_json::to_value(value).unwrap(),
    };

    let all_refs = {
        let fields: Vec<NamedField> = (0..32)
            .map(|i| {
                NamedField::user_supplied(
                    format!("f{i:03}"),
                    SchemaType::ref_to(TypeId::new(format!("t{i:03}"))),
                )
            })
            .collect();
        let values: Vec<SchemaValue> = (0..32).map(|_| ref_field_value()).collect();
        mk(
            "all_refs_32_defs",
            InputSchema::Parameters(fields),
            SchemaValue::Record { fields: values },
        )
    };

    let large_value = {
        let big: Vec<SchemaValue> = (0..512).map(SchemaValue::U64).collect();
        mk(
            "large_value_no_refs_32_defs",
            InputSchema::Parameters(vec![NamedField::user_supplied(
                "items",
                SchemaType::list(SchemaType::u64()),
            )]),
            SchemaValue::Record {
                fields: vec![SchemaValue::List { elements: big }],
            },
        )
    };

    vec![
        // No refs: 32 defs reachable from the input record → 0 kept.
        mk(
            "primitive_no_refs_32_defs",
            InputSchema::Parameters(vec![NamedField::user_supplied("seed", SchemaType::u64())]),
            SchemaValue::Record {
                fields: vec![SchemaValue::U64(123)],
            },
        ),
        // One ref into the wide graph: 32 → 1 kept.
        mk(
            "single_ref_sparse_32_defs",
            InputSchema::Parameters(vec![NamedField::user_supplied(
                "first",
                SchemaType::ref_to(TypeId::new("t031")),
            )]),
            SchemaValue::Record {
                fields: vec![ref_field_value()],
            },
        ),
        // Two refs: 32 → 2 kept.
        mk(
            "two_refs_sparse_32_defs",
            InputSchema::Parameters(vec![
                NamedField::user_supplied("first", SchemaType::ref_to(TypeId::new("t000"))),
                NamedField::user_supplied("last", SchemaType::ref_to(TypeId::new("t031"))),
            ]),
            SchemaValue::Record {
                fields: vec![ref_field_value(), ref_field_value()],
            },
        ),
        // Every def reachable: 32 → 32 kept. Projection regression guard.
        all_refs,
        // Large value, no refs: isolates the removed redundant `value.clone()`.
        large_value,
    ]
}

// ── Benchmarks ───────────────────────────────────────────────────────────────

fn bench_agent_type_clone(c: &mut Criterion) {
    let agent_type = representative_agent_type();
    c.bench_function("agent_type_schema_clone", |b| {
        b.iter(|| black_box(black_box(&agent_type).clone()))
    });
}

fn bench_validate_method_input(c: &mut Criterion) {
    let agent_type = representative_agent_type();
    let input = method_input_value();
    let mut group = c.benchmark_group("validate_method_input");
    // Old path: rebuild a temp record `SchemaType`, cloning each field schema +
    // metadata per call.
    group.bench_function("via_temp_record", |b| {
        b.iter(|| {
            black_box(validate_method_input(
                black_box(&agent_type),
                black_box(&input),
            ))
        })
    });
    // Opt5 path: validate borrowed fields directly, no temp record / clones.
    group.bench_function("borrowed_fields", |b| {
        b.iter(|| {
            black_box(validate_method_input_borrowed(
                black_box(&agent_type),
                black_box(&input),
            ))
        })
    });
    group.finish();
}

/// Agent config validation (A1): mirror the worker-executor config path, which
/// for each config entry validates the stored value against the entry's
/// declared `value_type`, resolving any `Ref` through the agent's shared `defs`.
///
/// `via_temp_graph` reproduces the pre-A1 code: per entry it materializes a
/// temporary `SchemaGraph` by cloning the agent's full `defs` and setting the
/// entry's `value_type` as `root`. `borrowed_graph` is the A1 form: it passes
/// the agent's existing graph plus the borrowed `value_type` directly, with no
/// per-entry `defs` clone.
fn config_entries(n: usize) -> Vec<(SchemaType, SchemaValue)> {
    (0..n)
        .map(|i| {
            (
                SchemaType::ref_to(TypeId::new(format!("t{:03}", i % 32))),
                ref_field_value(),
            )
        })
        .collect()
}

/// Extracting the value out of a constructor-parameters `TypedSchemaValue`
/// (A2): the worker-start init path needs only the inner `SchemaValue`.
/// `full_clone_into_parts` reproduces the pre-A2 `parameters.clone().into_parts().1`,
/// which deep-clones the whole graph (`defs` + `root`) just to drop it.
/// `value_only_clone` is the A2 form: clone only the inner value.
fn representative_typed_value() -> TypedSchemaValue {
    let graph = SchemaGraph {
        defs: shared_graph(32).defs,
        root: SchemaType::record(vec![
            NamedFieldType {
                name: "first".to_string(),
                body: SchemaType::ref_to(TypeId::new("t000")),
                metadata: MetadataEnvelope::default(),
            },
            NamedFieldType {
                name: "last".to_string(),
                body: SchemaType::ref_to(TypeId::new("t031")),
                metadata: MetadataEnvelope::default(),
            },
        ]),
    };
    TypedSchemaValue::new(
        graph,
        SchemaValue::Record {
            fields: vec![ref_field_value(), ref_field_value()],
        },
    )
}

fn bench_typed_value_extract(c: &mut Criterion) {
    let tv = representative_typed_value();
    let mut group = c.benchmark_group("typed_value_extract");
    group.bench_function("full_clone_into_parts", |b| {
        b.iter(|| black_box(black_box(&tv).clone().into_parts().1))
    });
    group.bench_function("value_only_clone", |b| {
        b.iter(|| black_box(black_box(&tv).value().clone()))
    });
    group.finish();
}

/// Constructing a single-root carrier from an already-validated value (A4/A5/
/// agent_config site 2): `typed_constructor_parameters`, `enrich_with_type`, and
/// `parse_worker_creation_agent_config` previously cloned the agent's whole
/// `defs` registry and overwrote `root`. `via_full_defs_clone` reproduces that;
/// `via_projection` is the new `typed_schema_value_with_projected_defs`, which
/// keeps only the defs reachable from the carrier's root.
fn bench_typed_value_construction(c: &mut Criterion) {
    let graph = shared_graph(32);
    let root = SchemaType::record(vec![
        NamedFieldType {
            name: "first".to_string(),
            body: SchemaType::ref_to(TypeId::new("t000")),
            metadata: MetadataEnvelope::default(),
        },
        NamedFieldType {
            name: "last".to_string(),
            body: SchemaType::ref_to(TypeId::new("t031")),
            metadata: MetadataEnvelope::default(),
        },
    ]);
    let value = SchemaValue::Record {
        fields: vec![ref_field_value(), ref_field_value()],
    };
    let mut group = c.benchmark_group("typed_value_construction");
    group.bench_function("via_full_defs_clone", |b| {
        b.iter(|| {
            let g = SchemaGraph {
                defs: graph.defs.clone(),
                root: root.clone(),
            };
            black_box(TypedSchemaValue::new(g, value.clone()))
        })
    });
    group.bench_function("via_projection", |b| {
        b.iter(|| {
            black_box(typed_schema_value_with_projected_defs(
                black_box(&graph),
                root.clone(),
                value.clone(),
            ))
        })
    });
    group.finish();
}

fn bench_config_validation(c: &mut Criterion) {
    let graph = shared_graph(32);
    let entries = config_entries(16);
    let mut group = c.benchmark_group("config_validation");
    group.bench_function("via_temp_graph", |b| {
        b.iter(|| {
            for (value_type, value) in &entries {
                let temp = SchemaGraph {
                    defs: graph.defs.clone(),
                    root: value_type.clone(),
                };
                black_box(validate_value(&temp, &temp.root, value).is_ok());
            }
        })
    });
    group.bench_function("borrowed_graph", |b| {
        b.iter(|| {
            for (value_type, value) in &entries {
                black_box(validate_value(black_box(&graph), value_type, value).is_ok());
            }
        })
    });
    group.finish();
}

fn bench_json_input_conversion(c: &mut Criterion) {
    let cases = json_input_cases();
    let mut group = c.benchmark_group("json_input_conversion");
    for case in &cases {
        let graph = &case.graph;
        let input_schema = &case.input_schema;
        let json = &case.json;
        // Per-case clone-only baseline (JSON sizes differ across cases) so the
        // JSON clone in `iter_batched` setup can be subtracted from the
        // conversion measurement.
        group.bench_function(format!("{}/clone_baseline", case.name), |b| {
            b.iter(|| black_box(black_box(json).clone()))
        });
        group.bench_function(format!("{}/convert", case.name), |b| {
            b.iter_batched(
                || json.clone(),
                |json| {
                    black_box(
                        json_input_schema_value_to_typed_schema_value(json, graph, input_schema)
                            .unwrap(),
                    )
                },
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_agent_type_clone,
    bench_validate_method_input,
    bench_typed_value_extract,
    bench_typed_value_construction,
    bench_config_validation,
    bench_json_input_conversion,
);
criterion_main!(benches);
