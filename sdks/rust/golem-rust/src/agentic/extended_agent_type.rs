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

use crate::agentic::{AutoInjectedParamType, schema_graph_root};
use crate::golem_agentic::golem::agent::common::AgentConfigDeclaration;
use crate::golem_agentic::golem::agent::common::{
    AgentConfigSource, AgentConstructor, AgentDependency, AgentMethod, AgentMode, AgentType,
    AutoInjectedKind, FieldSource, HttpEndpointDetails, HttpMountDetails, InputSchema, NamedField,
    OutputSchema, ReadOnlyConfig, Snapshotting,
};
use crate::schema::wit::GraphEncoder;
use crate::schema::{MetadataEnvelope, SchemaGraph, merge_agent_graphs};
use std::collections::HashSet;

// An enriched representation is a different view of the agent type that will include
// extra details such as auto-injected schemas such as Principal
// The agent registry will hold this information for the generated `initiate` , `invoke` and rpc calls
// to look up so that these parameters can be injected automatically by the platform
#[derive(Clone)]
pub struct ExtendedAgentType {
    pub type_name: String,
    pub description: String,
    pub source_language: String,
    pub constructor: ExtendedAgentConstructor,
    pub methods: Vec<EnrichedAgentMethod>,
    pub dependencies: Vec<AgentDependency>,
    pub mode: AgentMode,
    pub http_mount: Option<HttpMountDetails>,
    pub snapshotting: Snapshotting,
    pub config: Vec<ExtendedAgentConfigDeclaration>,
    /// Maps from a name-sorted method index to the original index in `methods`.
    /// Built at registration time to allow O(1) index-based lookup on the hot path
    /// while preserving user-declared method order in `methods`.
    pub sorted_method_indices: Vec<usize>,
}

#[derive(Clone)]
pub struct ExtendedAgentConfigDeclaration {
    pub source: AgentConfigSource,
    pub path: Vec<String>,
    pub value_type: SchemaGraph,
}

impl ExtendedAgentType {
    pub fn principal_params_in_constructor(&self) -> HashSet<String> {
        let mut principal_params = HashSet::new();

        for (name, schema) in &self.constructor.input_schema {
            if let EnrichedParameterSchema::AutoInject(AutoInjectedParamType::Principal) = schema {
                principal_params.insert(name.clone());
            }
        }

        principal_params
    }

    pub fn to_agent_type(&self) -> AgentType {
        let agent_schema = build_agent_schema(self).expect("failed to build agent schema");
        AgentType {
            type_name: self.type_name.clone(),
            description: self.description.clone(),
            source_language: self.source_language.clone(),
            schema: agent_schema.schema,
            constructor: self
                .constructor
                .to_agent_constructor(agent_schema.constructor_input),
            methods: self
                .methods
                .iter()
                .zip(
                    agent_schema
                        .method_inputs
                        .into_iter()
                        .zip(agent_schema.method_outputs),
                )
                .map(|(method, (input, output))| method.to_agent_method(input, output))
                .collect(),
            dependencies: self.dependencies.clone(),
            mode: self.mode,
            http_mount: self.http_mount.clone(),
            snapshotting: self.snapshotting,
            config: self
                .config
                .iter()
                .zip(agent_schema.config_roots)
                .map(|(config, value_type)| AgentConfigDeclaration {
                    source: config.source,
                    path: config.path.clone(),
                    value_type,
                })
                .collect(),
        }
    }
}

#[derive(Clone)]
pub struct EnrichedAgentMethod {
    pub name: String,
    pub description: String,
    pub http_endpoint: Vec<HttpEndpointDetails>,
    pub prompt_hint: Option<String>,
    pub input_schema: Vec<(String, EnrichedParameterSchema)>,
    pub output_schema: Vec<(String, SchemaGraph)>,
    pub read_only: Option<ReadOnlyConfig>,
}

impl EnrichedAgentMethod {
    pub fn to_agent_method(
        &self,
        input_schema: InputSchema,
        output_schema: OutputSchema,
    ) -> AgentMethod {
        AgentMethod {
            name: self.name.clone(),
            description: self.description.clone(),
            http_endpoint: self.http_endpoint.clone(),
            prompt_hint: self.prompt_hint.clone(),
            input_schema,
            output_schema,
            read_only: self.read_only.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ExtendedAgentConstructor {
    pub name: Option<String>,
    pub description: String,
    pub prompt_hint: Option<String>,
    pub input_schema: Vec<(String, EnrichedParameterSchema)>,
}

impl ExtendedAgentConstructor {
    pub fn to_agent_constructor(&self, input_schema: InputSchema) -> AgentConstructor {
        AgentConstructor {
            name: self.name.clone(),
            description: self.description.clone(),
            prompt_hint: self.prompt_hint.clone(),
            input_schema,
        }
    }
}

#[derive(Clone)]
pub enum EnrichedParameterSchema {
    Value(SchemaGraph),
    AutoInject(AutoInjectedParamType),
}

struct AgentSchemaRoots {
    schema: crate::schema::wit::wire::SchemaGraph,
    constructor_input: InputSchema,
    method_inputs: Vec<InputSchema>,
    method_outputs: Vec<OutputSchema>,
    config_roots: Vec<crate::schema::wit::wire::TypeNodeIndex>,
}

fn build_agent_schema(
    agent_type: &ExtendedAgentType,
) -> Result<AgentSchemaRoots, crate::schema::wit::EncodeError> {
    let graph = merge_agent_graphs(collect_schema_graphs(agent_type))
        .expect("failed to merge agent schema graphs");
    let mut encoder = GraphEncoder::new(&graph.defs)?;

    let constructor_input =
        encode_input_schema(&mut encoder, &agent_type.constructor.input_schema)?;
    let mut method_inputs = Vec::with_capacity(agent_type.methods.len());
    let mut method_outputs = Vec::with_capacity(agent_type.methods.len());

    for method in &agent_type.methods {
        method_inputs.push(encode_input_schema(&mut encoder, &method.input_schema)?);
        method_outputs.push(encode_output_schema(&mut encoder, &method.output_schema)?);
    }

    let mut config_roots = Vec::with_capacity(agent_type.config.len());
    for config in &agent_type.config {
        config_roots.push(encoder.encode_type(&schema_graph_root(&config.value_type))?);
    }

    Ok(AgentSchemaRoots {
        schema: encoder.finish(),
        constructor_input,
        method_inputs,
        method_outputs,
        config_roots,
    })
}

fn collect_schema_graphs(agent_type: &ExtendedAgentType) -> Vec<SchemaGraph> {
    let mut graphs = Vec::new();
    collect_data_schema_graphs(&agent_type.constructor.input_schema, &mut graphs);
    for method in &agent_type.methods {
        collect_data_schema_graphs(&method.input_schema, &mut graphs);
        collect_result_schema_graphs(&method.output_schema, &mut graphs);
    }
    for config in &agent_type.config {
        collect_schema_graph(&config.value_type, &mut graphs);
    }
    graphs
}

fn collect_data_schema_graphs(
    schema: &[(String, EnrichedParameterSchema)],
    graphs: &mut Vec<SchemaGraph>,
) {
    for (_, schema) in schema {
        if let EnrichedParameterSchema::Value(schema) = schema {
            collect_schema_graph(schema, graphs);
        }
    }
}

fn collect_result_schema_graphs(schema: &[(String, SchemaGraph)], graphs: &mut Vec<SchemaGraph>) {
    for (_, schema) in schema {
        collect_schema_graph(schema, graphs);
    }
}

fn collect_schema_graph(schema: &SchemaGraph, graphs: &mut Vec<SchemaGraph>) {
    graphs.push(schema.clone());
}

fn encode_input_schema(
    encoder: &mut GraphEncoder,
    input_schema: &[(String, EnrichedParameterSchema)],
) -> Result<InputSchema, crate::schema::wit::EncodeError> {
    let fields = input_schema
        .iter()
        .map(|(name, schema)| {
            let (source, typ) = match schema {
                EnrichedParameterSchema::Value(schema) => (
                    FieldSource::UserSupplied,
                    encoder.encode_type(&schema_graph_root(schema))?,
                ),
                EnrichedParameterSchema::AutoInject(AutoInjectedParamType::Principal) => (
                    FieldSource::AutoInjected(AutoInjectedKind::Principal),
                    encoder.encode_type(&crate::schema::SchemaType::record(vec![]))?,
                ),
            };
            Ok(NamedField {
                name: name.clone(),
                source,
                schema: typ,
                metadata: crate::schema::wit::encode_metadata(&MetadataEnvelope::default()),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(InputSchema::Parameters(fields))
}

fn encode_output_schema(
    encoder: &mut GraphEncoder,
    output_schema: &[(String, SchemaGraph)],
) -> Result<OutputSchema, crate::schema::wit::EncodeError> {
    match output_schema {
        fields if fields.is_empty() => Ok(OutputSchema::Unit),
        fields if fields.len() == 1 => Ok(OutputSchema::Single(
            encoder.encode_type(&schema_graph_root(&fields[0].1))?,
        )),
        fields => Ok(OutputSchema::Single(
            encoder.encode_type(&crate::schema::SchemaType::record(
                fields
                    .iter()
                    .map(|(name, schema)| crate::schema::NamedFieldType {
                        name: name.clone(),
                        body: schema_graph_root(schema),
                        metadata: MetadataEnvelope::default(),
                    })
                    .collect(),
            ))?,
        )),
    }
}
