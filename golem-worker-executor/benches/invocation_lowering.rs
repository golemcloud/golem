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

//! End-to-end micro-benchmark for the worker-executor invocation lowering
//! path (`lower_invocation`), which on the real hot path resolves the agent
//! type, classifies the method, validates the input against the method schema,
//! and encodes the input to the guest wire tree.
//!
//! Run before/after a change with:
//!
//! ```text
//! cargo bench -p golem-worker-executor --bench invocation_lowering
//! ```

use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
use golem_common::base_model::Empty;
use golem_common::base_model::IdempotencyKey;
use golem_common::base_model::agent::{AgentMode, AgentTypeName, Principal, Snapshotting};
use golem_common::base_model::component_metadata::KnownExports;
use golem_common::model::AgentInvocation;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::schema::{
    AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, AutoInjectedKind, InputSchema,
    MetadataEnvelope, NamedField, OutputSchema, SchemaGraph, SchemaType, SchemaTypeDef,
    SchemaValue, TypeId, TypedSchemaValue,
};
use golem_worker_executor::worker::invocation::lower_invocation;
use std::collections::BTreeMap;

const METHOD_NAME: &str = "do-work";
const AGENT_TYPE: &str = "bench-agent";

fn def(id: &str, body: SchemaType) -> SchemaTypeDef {
    SchemaTypeDef {
        id: TypeId::new(id),
        name: None,
        body,
    }
}

fn shared_graph(n: usize) -> SchemaGraph {
    let mut defs = Vec::with_capacity(n);
    for i in 0..n {
        defs.push(def(
            &format!("t{i:03}"),
            SchemaType::record(vec![
                golem_common::schema::NamedFieldType {
                    name: "a".to_string(),
                    body: SchemaType::u32(),
                    metadata: MetadataEnvelope::default(),
                },
                golem_common::schema::NamedFieldType {
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

fn representative_agent_type() -> AgentTypeSchema {
    let method_fields = vec![
        NamedField::user_supplied("count", SchemaType::u32()),
        NamedField::user_supplied("label", SchemaType::string()),
        NamedField::user_supplied("items", SchemaType::list(SchemaType::u8())),
        NamedField::user_supplied("maybe", SchemaType::option(SchemaType::bool())),
        NamedField::user_supplied("first", SchemaType::ref_to(TypeId::new("t000"))),
        NamedField::user_supplied("last", SchemaType::ref_to(TypeId::new("t031"))),
        NamedField::auto_injected(
            "principal",
            AutoInjectedKind::Principal,
            SchemaType::string(),
        ),
    ];

    AgentTypeSchema {
        type_name: AgentTypeName(AGENT_TYPE.to_string()),
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
            name: METHOD_NAME.to_string(),
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

fn component_metadata() -> ComponentMetadata {
    ComponentMetadata::from_parts(
        KnownExports::default(),
        Vec::new(),
        None,
        None,
        vec![representative_agent_type()],
        BTreeMap::new(),
    )
}

fn parsed_agent_id() -> ParsedAgentId {
    let parameters = TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::record(vec![
            golem_common::schema::NamedFieldType {
                name: "seed".to_string(),
                body: SchemaType::u64(),
                metadata: MetadataEnvelope::default(),
            },
        ])),
        SchemaValue::Record {
            fields: vec![SchemaValue::U64(1)],
        },
    );
    ParsedAgentId::new(AgentTypeName(AGENT_TYPE.to_string()), parameters, None)
}

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

fn agent_method_invocation() -> AgentInvocation {
    AgentInvocation::AgentMethod {
        idempotency_key: IdempotencyKey::new("bench-key".to_string()),
        method_name: METHOD_NAME.to_string(),
        input: method_input_value(),
        invocation_context: InvocationContextStack::fresh(),
        principal: Principal::anonymous(),
    }
}

fn bench_lower_invocation(c: &mut Criterion) {
    let metadata = component_metadata();
    let agent_id = parsed_agent_id();
    let invocation = agent_method_invocation();

    c.bench_function("lower_invocation/agent_method", |b| {
        b.iter_batched(
            || invocation.clone(),
            |invocation| {
                black_box(
                    lower_invocation(invocation, black_box(&metadata), Some(black_box(&agent_id)))
                        .is_ok(),
                )
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(benches, bench_lower_invocation);
criterion_main!(benches);
