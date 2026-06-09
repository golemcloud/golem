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

use golem_common::schema::adapters::{
    analysed_type_to_schema_graph, schema_value_to_value, value_and_type_to_typed_schema_value,
};
use golem_common::schema::render::json_value::{from_json_value, to_json_value};
use golem_wasm::Value;
use golem_wasm::ValueAndType;
use golem_wasm::analysis::AnalysedType;

/// Parse the JSON of a single component-model element using the canonical
/// schema-layer JSON value codec (`from_json_value`) rather than the legacy
/// `golem_wasm::json::ValueAndTypeJsonExtensions` codec.
///
/// The MCP tool/resource JSON Schema is advertised by the same schema-layer
/// renderer, so parsing through this codec keeps the advertised shape and the
/// accepted shape in agreement (e.g. `char` as a one-character string,
/// payload-less variants as a bare case name, `option<T>` record fields as
/// required). The legacy `AnalysedType` is projected into the schema layer via
/// [`analysed_type_to_schema_graph`]; the resulting [`SchemaValue`] is lowered
/// back to a component-model [`Value`] for the (still legacy) worker boundary.
pub(crate) fn parse_component_model_value(
    json: &serde_json::Value,
    element_type: &AnalysedType,
) -> Result<Value, String> {
    let graph = analysed_type_to_schema_graph(element_type)
        .map_err(|e| format!("unsupported element type: {e}"))?;
    let schema_value = from_json_value(&graph, &graph.root, json).map_err(|e| e.to_string())?;
    schema_value_to_value(&graph, &graph.root, &schema_value).map_err(|e| e.to_string())
}

/// Render a component-model response value to JSON using the canonical
/// schema-layer JSON value codec (`to_json_value`), so the produced JSON
/// matches the advertised MCP output schema.
pub(crate) fn component_model_value_to_json(
    vat: &ValueAndType,
) -> Result<serde_json::Value, String> {
    let typed = value_and_type_to_typed_schema_value(vat).map_err(|e| e.to_string())?;
    to_json_value(typed.graph(), typed.root_type(), typed.value()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod codec_tests {
    //! Regression tests for the MCP value codec swap: parsing and rendering of
    //! component-model values must agree with the schema-layer JSON Schema the
    //! MCP tools advertise. These cover the three shapes where the old
    //! `ValueAndTypeJsonExtensions` codec disagreed with the advertised schema:
    //! `char`, payload-less variants, and `option<T>` record fields.

    use super::{component_model_value_to_json, parse_component_model_value};
    use golem_wasm::ValueAndType;
    use golem_wasm::analysis::NameOptionTypePair;
    use golem_wasm::analysis::analysed_type::{chr, field, option, record, str, variant};
    use serde_json::json;
    use test_r::test;

    fn round_trip(ty: &golem_wasm::analysis::AnalysedType, json: serde_json::Value) {
        let value = parse_component_model_value(&json, ty)
            .unwrap_or_else(|e| panic!("parse failed for {json}: {e}"));
        let back = component_model_value_to_json(&ValueAndType::new(value, ty.clone()))
            .unwrap_or_else(|e| panic!("render failed: {e}"));
        assert_eq!(back, json, "round-trip changed the JSON shape");
    }

    #[test]
    fn char_is_a_one_character_string() {
        // Advertised as a one-character JSON string; accepted and round-trips.
        round_trip(&chr(), json!("A"));
        // The legacy code-point number form is now rejected.
        assert!(
            parse_component_model_value(&json!(65), &chr()).is_err(),
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
            parse_component_model_value(&json!({"none": null}), &ty).is_err(),
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
            parse_component_model_value(&json!({}), &ty).is_err(),
            "omitted option<T> record field must be rejected"
        );
    }
}
