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

mod agent_method_input;
pub(crate) mod constructor_param_extraction;
mod multimodal_params_extraction;
pub mod resource;
#[cfg(test)]
pub(crate) mod test_support;
pub mod tool;

use golem_common::schema::agent::{FieldSource, InputSchema};
use golem_common::schema::canonical;
use golem_common::schema::graph::{SchemaGraph, TypedSchemaValue};
use golem_common::schema::schema_type::{
    BinaryRestrictions, NamedFieldType, SchemaType, TextRestrictions,
};
use golem_common::schema::schema_value::SchemaValue;

/// Build the agent-id constructor parameters as a self-contained
/// [`TypedSchemaValue`] from the user-supplied constructor fields and their
/// already-extracted positional values.
///
/// The record root is built over the **user-supplied** constructor fields only
/// (auto-injected fields are filled in by the host and are not part of the MCP
/// agent-id encoding), carrying the agent's `graph.defs` so any
/// [`SchemaType::Ref`] in a field schema still resolves. `values` must be in the
/// same user-supplied field order as
/// [`extract_constructor_input_values`](constructor_param_extraction::extract_constructor_input_values).
pub(crate) fn build_constructor_parameters(
    graph: &SchemaGraph,
    input: &InputSchema,
    values: Vec<SchemaValue>,
) -> TypedSchemaValue {
    let fields = input
        .fields()
        .iter()
        .filter(|f| matches!(f.source, FieldSource::UserSupplied))
        .map(|f| NamedFieldType {
            name: f.name.clone(),
            body: f.schema.clone(),
            metadata: f.metadata.clone(),
        })
        .collect();
    let mut graph = graph.clone();
    graph.root = SchemaType::record(fields);
    TypedSchemaValue::new(graph, SchemaValue::Record { fields: values })
}

/// Parse an unstructured-text element from its canonical JSON envelope
/// `{ "text": "...", "language"?: "..." }` directly into a [`SchemaValue::Text`]
/// and apply the schema's language restrictions.
///
/// The envelope shape is parsed by the shared `golem-common` canonical Text
/// codec — the same shape advertised by the MCP tool/resource schema renderer —
/// so the advertised schema and the accepted JSON stay in agreement.
pub(crate) fn schema_text_value_from_json(
    value_json: &serde_json::Value,
    restrictions: &TextRestrictions,
) -> Result<SchemaValue, String> {
    let payload = canonical::text::from_json(value_json).map_err(|e| e.to_string())?;

    if let Some(code) = &payload.language
        && let Some(allowed) = &restrictions.languages
        && !allowed.is_empty()
        && !allowed.iter().any(|l| l == code)
    {
        return Err(format!(
            "language code '{}' is not allowed. Expected one of: {}",
            code,
            allowed.join(", ")
        ));
    }

    Ok(SchemaValue::Text(payload))
}

/// Parse an unstructured-binary element from its canonical JSON envelope
/// `{ "bytes": "<base64url-no-pad>", "mime_type"?: "..." }` directly into a
/// [`SchemaValue::Binary`] and apply the schema's MIME restrictions.
///
/// The envelope shape is parsed by the shared `golem-common` canonical Binary
/// codec — the same shape advertised by the MCP tool/resource schema renderer —
/// so the advertised schema and the accepted JSON stay in agreement. The
/// canonical encoding is URL-safe base64 without padding.
pub(crate) fn schema_binary_value_from_json(
    value_json: &serde_json::Value,
    restrictions: &BinaryRestrictions,
) -> Result<SchemaValue, String> {
    let payload = canonical::binary::from_json(value_json).map_err(|e| e.to_string())?;

    if let Some(allowed) = &restrictions.mime_types
        && !allowed.is_empty()
    {
        match &payload.mime_type {
            Some(mime) if allowed.iter().any(|m| m == mime) => {}
            Some(mime) => {
                return Err(format!(
                    "MIME type '{}' is not allowed. Expected one of: {}",
                    mime,
                    allowed.join(", ")
                ));
            }
            None => {
                return Err(format!(
                    "MIME type is required. Expected one of: {}",
                    allowed.join(", ")
                ));
            }
        }
    }

    Ok(SchemaValue::Binary(payload))
}

#[cfg(test)]
mod codec_tests {
    //! Regression tests for the MCP value codec: parsing and rendering of
    //! component-model values must agree with the schema-layer JSON Schema the
    //! MCP tools advertise. These cover the three shapes where the old
    //! `ValueAndTypeJsonExtensions` codec disagreed with the advertised schema:
    //! `char`, payload-less variants, and `option<T>` record fields. They now
    //! drive the shared schema-layer JSON codec (`from_json_value` /
    //! `to_json_value`) directly, projecting the `AnalysedType` into a graph via
    //! [`analysed_type_to_schema_graph`].

    use golem_common::schema::adapters::analysed_type_to_schema_graph;
    use golem_common::schema::render::json_value::{from_json_value, to_json_value};
    use golem_wasm::analysis::NameOptionTypePair;
    use golem_wasm::analysis::analysed_type::{chr, field, option, record, str, variant};
    use serde_json::json;
    use test_r::test;

    fn round_trip(ty: &golem_wasm::analysis::AnalysedType, json: serde_json::Value) {
        let graph = analysed_type_to_schema_graph(ty).expect("graph");
        let value = from_json_value(&graph, &graph.root, &json)
            .unwrap_or_else(|e| panic!("parse failed for {json}: {e}"));
        let back = to_json_value(&graph, &graph.root, &value)
            .unwrap_or_else(|e| panic!("render failed: {e}"));
        assert_eq!(back, json, "round-trip changed the JSON shape");
    }

    fn parse_fails(ty: &golem_wasm::analysis::AnalysedType, json: serde_json::Value) -> bool {
        let graph = analysed_type_to_schema_graph(ty).expect("graph");
        from_json_value(&graph, &graph.root, &json).is_err()
    }

    #[test]
    fn char_is_a_one_character_string() {
        // Advertised as a one-character JSON string; accepted and round-trips.
        round_trip(&chr(), json!("A"));
        // The legacy code-point number form is now rejected.
        assert!(
            parse_fails(&chr(), json!(65)),
            "numeric char code point must be rejected"
        );
    }

    #[test]
    fn payloadless_variant_is_a_bare_case_name() {
        let ty = variant(vec![
            NameOptionTypePair {
                name: "none".to_string(),
                typ: None,
            },
            NameOptionTypePair {
                name: "some".to_string(),
                typ: Some(str()),
            },
        ]);
        // Payload-less case is a bare string constant; payload case is tagged.
        round_trip(&ty, json!("none"));
        round_trip(&ty, json!({"some": "x"}));
        // The legacy `{ "none": null }` form is now rejected.
        assert!(
            parse_fails(&ty, json!({"none": null})),
            "tagged-null form for a payload-less case must be rejected"
        );
    }

    #[test]
    fn option_record_field_is_required() {
        let ty = record(vec![field("inner", option(str()))]);
        // The advertised schema marks the field required; an explicit null is
        // accepted and round-trips.
        round_trip(&ty, json!({"inner": null}));
        round_trip(&ty, json!({"inner": "x"}));
        // Omitting the field is rejected (schema and runtime now agree).
        assert!(
            parse_fails(&ty, json!({})),
            "omitted option<T> record field must be rejected"
        );
    }
}
