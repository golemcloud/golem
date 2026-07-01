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

//! Hot-path microbenchmarks for generated typed tool clients.
//!
//! Run with:
//!
//! ```text
//! cd sdks/rust && cargo bench -p golem-rust --features export_golem_agentic --bench tool_client_bench
//! ```

use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use golem_rust::agentic::{
    CanonicalInputField, CanonicalInputModel, Schema as AgenticSchema, StructuredSchema,
};
use golem_rust::{FromSchema, IntoSchema, SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};

#[derive(Clone, Debug, IntoSchema, FromSchema)]
struct CommitResult {
    commit_id: String,
    changed_paths: Vec<String>,
    warnings: Vec<String>,
}

fn schema_graph_for<T: AgenticSchema>() -> SchemaGraph {
    match <T as AgenticSchema>::get_type() {
        StructuredSchema::Default(graph) => graph,
        StructuredSchema::AutoInject(_) => panic!("tool benchmark type must have a value schema"),
    }
}

fn encode_output<T>(value: T) -> golem_rust::schema::wit::wire::TypedSchemaValue
where
    T: AgenticSchema,
{
    let typed = TypedSchemaValue::new(
        schema_graph_for::<T>(),
        <T as AgenticSchema>::to_schema_value(value).expect("output value must encode"),
    );
    golem_rust::encode_typed_schema_value(&typed).expect("output wire value must encode")
}

fn tool_round_trip_uncached<O>(
    input_fields: Vec<CanonicalInputField>,
    input_values: Vec<SchemaValue>,
    output: &golem_rust::schema::wit::wire::TypedSchemaValue,
) -> O
where
    O: AgenticSchema,
{
    let model =
        CanonicalInputModel::from_fields(input_fields).expect("input model must build from fields");
    let input = TypedSchemaValue::new(
        model.record_schema.clone(),
        SchemaValue::Record {
            fields: input_values,
        },
    );
    let input =
        golem_rust::encode_typed_schema_value(&input).expect("input wire value must encode");
    let input =
        golem_rust::decode_typed_schema_value(&input).expect("input wire value must decode");
    let (_, input_value) = input.into_parts();
    let _decoded_input = model
        .decode_record(input_value)
        .expect("input canonical record must decode");

    let output =
        golem_rust::decode_typed_schema_value(output).expect("output wire value must decode");
    let (_, output_value) = output.into_parts();
    O::from_schema_value(
        output_value,
        StructuredSchema::Default(schema_graph_for::<O>()),
    )
    .expect("output schema value must decode")
}

fn tool_round_trip_cached<O>(
    input_model: &CanonicalInputModel,
    input_values: Vec<SchemaValue>,
    output_graph: &SchemaGraph,
    output: &golem_rust::schema::wit::wire::TypedSchemaValue,
) -> O
where
    O: AgenticSchema,
{
    let input = TypedSchemaValue::new(
        input_model.record_schema.clone(),
        SchemaValue::Record {
            fields: input_values,
        },
    );
    let input =
        golem_rust::encode_typed_schema_value(&input).expect("input wire value must encode");
    let input =
        golem_rust::decode_typed_schema_value(&input).expect("input wire value must decode");
    let (_, input_value) = input.into_parts();
    let _decoded_input = input_model
        .decode_record(input_value)
        .expect("input canonical record must decode");

    let output =
        golem_rust::decode_typed_schema_value(output).expect("output wire value must decode");
    let (_, output_value) = output.into_parts();
    O::from_schema_value(
        output_value,
        StructuredSchema::Default(output_graph.clone()),
    )
    .expect("output schema value must decode")
}

fn field(name: &str, schema: SchemaGraph) -> CanonicalInputField {
    CanonicalInputField {
        name: name.to_string(),
        aliases: Vec::new(),
        schema,
    }
}

fn grep_fields() -> Vec<CanonicalInputField> {
    vec![
        field("pattern", schema_graph_for::<String>()),
        field("path", schema_graph_for::<String>()),
        field("ignore-case", schema_graph_for::<bool>()),
        field("context", schema_graph_for::<Option<u64>>()),
    ]
}

fn grep_values() -> Vec<SchemaValue> {
    vec![
        SchemaValue::String("todo|fixme".to_string()),
        SchemaValue::String("src".to_string()),
        SchemaValue::Bool(true),
        SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::U64(2))),
        },
    ]
}

fn bench_grep_shape(c: &mut Criterion) {
    let output = encode_output::<Vec<String>>(vec![
        "src/lib.rs:42:TODO: tighten this".to_string(),
        "src/tool.rs:7:FIXME: remove workaround".to_string(),
    ]);
    let input_model = CanonicalInputModel::from_fields(grep_fields())
        .expect("grep-shaped input model must build");
    let output_graph = schema_graph_for::<Vec<String>>();

    let mut group = c.benchmark_group("tool_client_round_trip/grep_shape");
    group.bench_function("uncached_graphs", |b| {
        b.iter_batched(
            grep_values,
            |values| {
                black_box(tool_round_trip_uncached::<Vec<String>>(
                    black_box(grep_fields()),
                    black_box(values),
                    black_box(&output),
                ))
            },
            BatchSize::SmallInput,
        )
    });
    group.bench_function("cached_graphs", |b| {
        b.iter_batched(
            grep_values,
            |values| {
                black_box(tool_round_trip_cached::<Vec<String>>(
                    black_box(&input_model),
                    black_box(values),
                    black_box(&output_graph),
                    black_box(&output),
                ))
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

fn git_commit_fields() -> Vec<CanonicalInputField> {
    vec![
        field("message", schema_graph_for::<String>()),
        field("paths", schema_graph_for::<Vec<String>>()),
        field("author", schema_graph_for::<Option<String>>()),
        field("all", schema_graph_for::<bool>()),
        field("amend", schema_graph_for::<bool>()),
        field("signoff", schema_graph_for::<bool>()),
        field("trailers", schema_graph_for::<Vec<String>>()),
        field("cleanup", schema_graph_for::<Option<String>>()),
        field("gpg-sign", schema_graph_for::<Option<String>>()),
        field("verbose", schema_graph_for::<u64>()),
    ]
}

fn git_commit_values() -> Vec<SchemaValue> {
    vec![
        SchemaValue::String("Implement typed tool client benchmarks".to_string()),
        SchemaValue::List {
            elements: vec![
                SchemaValue::String("sdks/rust/golem-rust/Cargo.toml".to_string()),
                SchemaValue::String(
                    "sdks/rust/golem-rust/benches/tool_client_bench.rs".to_string(),
                ),
            ],
        },
        SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::String(
                "Golem Bot <bot@golem.cloud>".to_string(),
            ))),
        },
        SchemaValue::Bool(false),
        SchemaValue::Bool(false),
        SchemaValue::Bool(true),
        SchemaValue::List {
            elements: vec![
                SchemaValue::String("Refs: #3534".to_string()),
                SchemaValue::String("Benchmark: typed-tool-client".to_string()),
            ],
        },
        SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::String("strip".to_string()))),
        },
        SchemaValue::Option { inner: None },
        SchemaValue::U64(2),
    ]
}

fn bench_git_commit_shape(c: &mut Criterion) {
    let output = encode_output(CommitResult {
        commit_id: "0123456789abcdef".to_string(),
        changed_paths: vec![
            "sdks/rust/golem-rust/Cargo.toml".to_string(),
            "sdks/rust/golem-rust/benches/tool_client_bench.rs".to_string(),
        ],
        warnings: vec!["working tree has untracked files".to_string()],
    });
    let input_model = CanonicalInputModel::from_fields(git_commit_fields())
        .expect("git-commit-shaped input model must build");
    let output_graph = schema_graph_for::<CommitResult>();

    let mut group = c.benchmark_group("tool_client_round_trip/git_commit_shape");
    group.bench_function("uncached_graphs", |b| {
        b.iter_batched(
            git_commit_values,
            |values| {
                black_box(tool_round_trip_uncached::<CommitResult>(
                    black_box(git_commit_fields()),
                    black_box(values),
                    black_box(&output),
                ))
            },
            BatchSize::SmallInput,
        )
    });
    group.bench_function("cached_graphs", |b| {
        b.iter_batched(
            git_commit_values,
            |values| {
                black_box(tool_round_trip_cached::<CommitResult>(
                    black_box(&input_model),
                    black_box(values),
                    black_box(&output_graph),
                    black_box(&output),
                ))
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

fn string_graph() -> SchemaGraph {
    SchemaGraph::anonymous(SchemaType::string())
}

pub(crate) fn affine_handle_fields() -> Vec<CanonicalInputField> {
    vec![
        field("secret", string_graph()),
        field("path", string_graph()),
        field("label", string_graph()),
    ]
}

pub(crate) fn affine_handle_fresh_values() -> Vec<SchemaValue> {
    vec![
        // Native benchmarks cannot forge a guest-owned secret handle, so this
        // uses the opaque handle identifier shape that crosses the generated
        // client surface. It still exercises the important affine rule here:
        // only graphs are cached; values are fresh per call and never stored in
        // the cached model.
        SchemaValue::String("<fresh opaque secret handle>".to_string()),
        SchemaValue::String("/prod/db/password".to_string()),
        SchemaValue::String("database-password".to_string()),
    ]
}

fn affine_round_trip_uncached(
    values: Vec<SchemaValue>,
) -> Vec<golem_rust::agentic::CanonicalInputValue> {
    let model = CanonicalInputModel::from_fields(affine_handle_fields())
        .expect("affine-shaped input model must build");
    let input = TypedSchemaValue::new(
        model.record_schema.clone(),
        SchemaValue::Record { fields: values },
    );
    let input = golem_rust::encode_typed_schema_value(&input)
        .expect("affine-shaped input wire value must encode");
    let input = golem_rust::decode_typed_schema_value(&input)
        .expect("affine-shaped input wire value must decode");
    let (_, input_value) = input.into_parts();
    model
        .decode_record(input_value)
        .expect("affine-shaped input canonical record must decode")
}

fn affine_round_trip_cached(
    model: &CanonicalInputModel,
    values: Vec<SchemaValue>,
) -> Vec<golem_rust::agentic::CanonicalInputValue> {
    let input = TypedSchemaValue::new(
        model.record_schema.clone(),
        SchemaValue::Record { fields: values },
    );
    let input = golem_rust::encode_typed_schema_value(&input)
        .expect("affine-shaped input wire value must encode");
    let input = golem_rust::decode_typed_schema_value(&input)
        .expect("affine-shaped input wire value must decode");
    let (_, input_value) = input.into_parts();
    model
        .decode_record(input_value)
        .expect("affine-shaped input canonical record must decode")
}

fn bench_affine_handle_shape(c: &mut Criterion) {
    let cached = CanonicalInputModel::from_fields(affine_handle_fields())
        .expect("affine-shaped input model must build");

    let mut group = c.benchmark_group("tool_client_round_trip/affine_handle_shape");
    group.bench_function("uncached_graphs_fresh_values", |b| {
        b.iter_batched(
            affine_handle_fresh_values,
            |values| black_box(affine_round_trip_uncached(black_box(values))),
            BatchSize::SmallInput,
        )
    });
    group.bench_function("cached_graphs_fresh_values", |b| {
        b.iter_batched(
            affine_handle_fresh_values,
            |values| {
                black_box(affine_round_trip_cached(
                    black_box(&cached),
                    black_box(values),
                ))
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_grep_shape,
    bench_git_commit_shape,
    bench_affine_handle_shape
);
criterion_main!(benches);
