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

//! Temporary adapter layer between the older type / value model
//! (`AnalysedType`, `Value`, `ValueAndType`, `DataSchema`, `ElementSchema`,
//! `AgentType`, …) and the new schema layer (`SchemaType`, `SchemaValue`,
//! `TypedSchemaValue`, `InputSchema`, `OutputSchema`, `AgentTypeSchema`).
//!
//! The new schema layer is a strict superset of the older form. Forward
//! conversion is total for the shared subset and fails on resource handles
//! (which the schema layer explicitly excludes). Reverse conversion is
//! partial: rich scalars, unions, capabilities, recursive cycles, and the
//! `FixedList` / `Map` collections have no counterpart in `AnalysedType` and
//! return [`SchemaAdapterError::LossySchemaType`].
//!
//! All adapter functions take or return owned values; they do not mutate
//! their inputs. The split into submodules mirrors the input shape:
//!
//! - [`analysed_type`] – `AnalysedType` ↔ `SchemaType` (inline + graph)
//! - [`value`] – `Value` / `ValueAndType` ↔ `SchemaValue` / `TypedSchemaValue`
//! - [`element_schema`] – `ElementSchema` ↔ `SchemaType`
//! - [`data_schema`] – `DataSchema` ↔ `InputSchema` / `OutputSchema`
//! - [`agent`] – `AgentType` and friends ↔ `AgentTypeSchema` and friends

pub mod agent;
pub mod analysed_type;
pub mod data_schema;
pub mod element_schema;
pub mod error;
pub mod unstructured;
pub mod untyped;
pub mod value;

pub use agent::{
    agent_constructor_to_schema, agent_dependency_to_schema, agent_method_to_schema,
    agent_type_to_schema, legacy_data_value_to_typed_schema_value,
    legacy_parsed_agent_id_to_schema, schema_agent_constructor_to_legacy,
    schema_agent_dependency_to_legacy, schema_agent_method_to_legacy, schema_agent_type_to_legacy,
};
pub use analysed_type::{
    SchemaGraphBuilder, analysed_type_to_schema_graph, analysed_type_to_schema_type_inline,
    schema_graph_to_analysed_type, schema_type_to_analysed_type,
};
pub use data_schema::{
    FALLBACK_OUTPUT_FIELD_NAME, MULTIMODAL_PARTS_FIELD_NAME, data_schema_to_input_schema,
    data_schema_to_output_schema, input_schema_to_data_schema, is_multimodal_schema_type,
    multimodal_variant_cases, output_schema_to_data_schema,
};
pub use element_schema::{element_schema_to_schema_type, schema_type_to_element_schema};
pub use error::{SchemaAdapterError, legacy_type_id, resolve_ref};
pub use unstructured::{
    INLINE_CASE, URL_CASE, UnstructuredKind, UnstructuredOutput, UnstructuredPayloadKind,
    UnstructuredValueCase, binary_body_restrictions, decode_unstructured_output,
    decode_unstructured_value, is_unstructured_variant, text_body_restrictions,
    unstructured_binary_restrictions, unstructured_binary_schema_type, unstructured_inline_value,
    unstructured_kind, unstructured_or_raw_kind, unstructured_text_restrictions,
    unstructured_text_schema_type, unstructured_url_value, wrap_unstructured_inline_for_schema,
};
pub use untyped::{
    input_value_to_typed_schema_value, json_data_value_to_input_value,
    json_data_value_to_legacy_data_value, json_input_schema_value_to_typed_schema_value,
    json_schema_value_to_input_value, json_schema_value_to_typed_schema_value,
    legacy_data_value_to_json, output_json_to_legacy_data_value,
    output_value_to_typed_schema_value, schema_output_value_to_legacy_data_value,
    typed_input_to_untyped_data_value, typed_output_value_to_untyped_data_value,
    typed_schema_value_to_untyped_data_value, untyped_data_value_to_input_value,
    untyped_data_value_to_typed_input, untyped_data_value_to_typed_schema_output,
};
pub use value::{
    schema_value_to_value, typed_schema_value_to_value_and_type,
    value_and_type_to_typed_schema_value, value_to_schema_value,
};

#[cfg(test)]
mod tests;
