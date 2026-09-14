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

//! Platform agent-schema projections over the shared schema renderer.

use crate::schema::agent::{FieldSource, InputSchema, OutputSchema};
use crate::schema::{MetadataEnvelope, NamedFieldType, SchemaGraph, SchemaType};
use golem_schema::schema::render::json_schema::{
    JsonSchemaConfig, to_external_input_json_schema, to_external_output_json_schema,
};
use serde_json::Value;

/// Render an agent input schema as an object containing only user-supplied fields.
pub fn input_schema_to_json_schema(
    graph: &SchemaGraph,
    input: &InputSchema,
    config: JsonSchemaConfig,
) -> Value {
    let InputSchema::Parameters(fields) = input;
    let record_fields = fields
        .iter()
        .filter(|field| matches!(field.source, FieldSource::UserSupplied))
        .map(|field| NamedFieldType {
            name: field.name.clone(),
            body: field.schema.clone(),
            metadata: field.metadata.clone(),
        })
        .collect();
    let record = SchemaType::Record {
        fields: record_fields,
        metadata: MetadataEnvelope::default(),
    };
    to_external_input_json_schema(graph, &record, config.include_draft_marker)
}

/// Render an agent output schema, returning `None` for unit output.
pub fn output_schema_to_json_schema(
    graph: &SchemaGraph,
    output: &OutputSchema,
    config: JsonSchemaConfig,
) -> Option<Value> {
    match output {
        OutputSchema::Unit => None,
        OutputSchema::Single(ty) => Some(to_external_output_json_schema(
            graph,
            ty,
            config.include_draft_marker,
        )),
    }
}
