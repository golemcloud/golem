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

test_r::enable!();

#[path = "../../../../test-components/agent-sdk-rust/src/config.rs"]
mod guest;

#[test_r::sequential]
mod tests {
    use super::guest;
    use golem_rust::agentic::{
        AgentTypeName, Config, ConfigSchema, ExtendedAgentType, IntoRpcConfigParam,
        get_agent_type_by_name,
    };
    use golem_rust::schema::render::{from_untrusted_json_value, to_json_value};
    use golem_rust::schema::validation::is_equivalent_cross_graph;
    use golem_rust::schema::wit::direct::{self, FromWire, IntoWire, WireSchema};
    use golem_rust::schema::{SchemaGraph, SchemaType};
    use serde::Deserialize;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use test_r::test;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Corpus {
        version: u32,
        schemas: BTreeMap<String, Fixture>,
        static_cases: Vec<StaticCase>,
    }

    #[derive(Deserialize)]
    struct Fixture {
        graph: SchemaGraph,
        declarations: Vec<Declaration>,
    }

    #[derive(Deserialize)]
    struct Declaration {
        source: String,
        path: Vec<String>,
        value_type: SchemaType,
        default_value: Option<String>,
    }

    #[derive(Deserialize)]
    struct StaticCase {
        id: String,
        schema: String,
        expected: Option<Value>,
    }

    fn corpus() -> Corpus {
        let corpus: Corpus = serde_json::from_str(include_str!(
            "../../../../test-data/config-conformance/v1.json"
        ))
        .unwrap();
        assert_eq!(corpus.version, 1);
        corpus
    }

    fn check_metadata<T: ConfigSchema>(fixture: &Fixture, agent: &str) {
        let native = Config::<T>::config_entries();
        let wire = get_agent_type_by_name(&AgentTypeName(agent.to_string())).unwrap();
        let reflected = ExtendedAgentType::from_agent_type(wire).unwrap();
        for declarations in [&native, &reflected.config] {
            assert_eq!(declarations.len(), fixture.declarations.len(), "{agent}");
            for expected in &fixture.declarations {
                let actual = declarations
                    .iter()
                    .find(|entry| entry.path == expected.path)
                    .unwrap_or_else(|| panic!("{agent}: missing {:?}", expected.path));
                assert_eq!(expected.source, "Local");
                assert!(matches!(
                    actual.source,
                    golem_rust::golem_agentic::golem::agent::common::AgentConfigSource::Local
                ));
                assert!(
                    is_equivalent_cross_graph(
                        &actual.value_type,
                        &actual.value_type.root,
                        &fixture.graph,
                        &expected.value_type,
                    ),
                    "{agent}: {:?}",
                    expected.path
                );
                let parse = |value: &Option<String>| {
                    value
                        .as_deref()
                        .map(serde_json::from_str::<Value>)
                        .transpose()
                        .unwrap()
                };
                assert_eq!(
                    parse(&actual.default_value),
                    parse(&expected.default_value),
                    "{agent}: {:?}",
                    expected.path
                );
            }
        }
    }

    #[test]
    fn guest_derived_metadata_matches_independent_corpus() {
        let corpus = corpus();
        check_metadata::<guest::StaticDefaultFields>(
            &corpus.schemas["reference"],
            "StaticDefaultsAgent",
        );
        check_metadata::<guest::StaticDeliveryFields>(
            &corpus.schemas["recordUnion"],
            "StaticDeliveryAgent",
        );
        check_metadata::<guest::StaticRecursiveFields>(
            &corpus.schemas["recursive"],
            "StaticRecursiveAgent",
        );
        check_metadata::<guest::StaticNamedFields>(
            &corpus.schemas["canonicalName"],
            "StaticNamedAgent",
        );
        check_metadata::<guest::StaticRequiredFields>(
            &corpus.schemas["required"],
            "StaticRequiredAgent",
        );
        check_metadata::<guest::StaticOptionalFields>(
            &corpus.schemas["optionalDefault"],
            "StaticOptionalAgent",
        );
        check_metadata::<guest::StaticEmptyFields>(
            &corpus.schemas["emptyValues"],
            "StaticEmptyAgent",
        );
    }

    fn round_trip<T: FromWire + IntoWire + WireSchema>(json: &Value) {
        let graph = golem_rust::decode_schema_graph(&direct::schema::<T>()).unwrap();
        let value = from_untrusted_json_value(&graph, &graph.root, json).unwrap();
        let typed = direct::decode::<T>(golem_rust::encode_schema_value(&value).unwrap()).unwrap();
        let value = golem_rust::decode_schema_value(direct::encode(&typed).unwrap()).unwrap();
        assert_eq!(&to_json_value(&graph, &graph.root, &value).unwrap(), json);
    }

    #[test]
    fn corpus_resolved_values_decode_in_real_guest_types() {
        for case in corpus().static_cases {
            let Some(expected) = case.expected else {
                continue;
            };
            match case.schema.as_str() {
                "reference" => {
                    round_trip::<guest::StaticDefaultSettings>(&expected["settings"]);
                    round_trip::<guest::StaticDefaultChoice>(&expected["choice"]);
                    round_trip::<(Option<i32>, Option<i32>, String)>(&expected["slots"]);
                    round_trip::<Option<String>>(&expected["omitted"]);
                }
                "recordUnion" => round_trip::<guest::StaticDelivery>(&expected["delivery"]),
                "recursive" => round_trip::<guest::StaticNode>(&expected["tree"]),
                "canonicalName" => round_trip::<u32>(&expected["maxChars"]),
                "optionalDefault" => round_trip::<Option<String>>(&expected["optional"]),
                "emptyValues" => {
                    round_trip::<bool>(&expected["enabled"]);
                    round_trip::<Vec<String>>(&expected["names"]);
                    round_trip::<String>(&expected["label"]);
                }
                other => panic!("{}: uncovered schema {other}", case.id),
            }
        }
    }

    #[test]
    fn creation_values_preserve_names_null_and_positional_absence() {
        let fields = guest::StaticDefaultFieldsRpc {
            settings: Some(guest::StaticDefaultSettings {
                label: "creation".into(),
                count: 0,
            }),
            slots: Some((Some(11), None, "third".into())),
            omitted: Some(None),
            ..Default::default()
        }
        .into_rpc_param(&[]);
        assert_eq!(
            fields
                .iter()
                .map(|field| field.path.clone())
                .collect::<Vec<_>>(),
            [vec!["settings"], vec!["slots"], vec!["omitted"]]
        );
        for (field, expected) in fields.into_iter().zip([
            json!({"label":"creation","count":0}),
            json!([11, null, "third"]),
            Value::Null,
        ]) {
            let graph = golem_rust::decode_schema_graph(&field.value.graph).unwrap();
            let value = golem_rust::decode_schema_value(field.value.value).unwrap();
            assert_eq!(
                to_json_value(&graph, &graph.root, &value).unwrap(),
                expected
            );
        }
        let named = guest::StaticNamedFieldsRpc { max_chars: Some(0) }
            .into_rpc_param(&["Outer.Exact".into()]);
        assert_eq!(named[0].path, ["Outer.Exact", "maxChars"]);
        let tree = guest::StaticRecursiveFieldsRpc {
            tree: Some(guest::StaticNode {
                display_name: "move-only".into(),
                next: None,
            }),
        }
        .into_rpc_param(&[]);
        assert_eq!(tree[0].path, ["tree"]);
    }

    #[expect(
        dead_code,
        reason = "This fixture inspects metadata without invoking the host"
    )]
    #[derive(golem_rust::ConfigSchema)]
    #[config_schema(rename_all = "camelCase")]
    struct OptionalFields {
        no_default: Option<String>,
        #[config_schema(rename = "ExplicitNull", default = "null")]
        explicit_null: Option<String>,
    }

    #[expect(
        dead_code,
        reason = "This fixture inspects metadata without invoking the host"
    )]
    #[derive(golem_rust::ConfigSchema)]
    struct NestedFields {
        #[config_schema(nested, rename = "CanonicalParent")]
        settings: OptionalFields,
    }

    #[test]
    fn optional_default_is_distinct_from_no_default() {
        let entries = Config::<NestedFields>::config_entries();
        assert_eq!(entries[0].path, ["CanonicalParent", "noDefault"]);
        assert_eq!(entries[0].default_value, None);
        assert_eq!(entries[1].path, ["CanonicalParent", "ExplicitNull"]);
        assert_eq!(entries[1].default_value.as_deref(), Some("null"));
        let rpc = NestedFieldsRpc {
            settings: OptionalFieldsRpc {
                no_default: None,
                explicit_null: Some(None),
            },
        }
        .into_rpc_param(&[]);
        assert_eq!(rpc.len(), 1);
        assert_eq!(rpc[0].path, ["CanonicalParent", "ExplicitNull"]);
    }

    #[test]
    #[should_panic(expected = "invalid code default for config field count")]
    fn invalid_code_default_prevents_metadata_export() {
        #[expect(
            dead_code,
            reason = "An invalid default prevents this config from loading"
        )]
        #[derive(golem_rust::ConfigSchema)]
        struct InvalidFields {
            #[config_schema(default = r#""not-a-number""#)]
            count: u32,
        }
        Config::<InvalidFields>::config_entries();
    }

    #[test]
    fn defaults_reject_noncanonical_or_capability_data() {
        #[derive(golem_rust::ConfigSchema)]
        struct TaggedChoice {
            #[config_schema(default = r#"{"Text":"selected:value"}"#)]
            _value: guest::StaticDefaultChoice,
        }

        #[derive(golem_rust::ConfigSchema)]
        struct CompactedSlots {
            #[config_schema(default = r#"[11,"third"]"#)]
            _value: (Option<i32>, Option<i32>, String),
        }

        #[derive(golem_rust::ConfigSchema)]
        struct CapabilityDefault {
            #[config_schema(default = "null")]
            _value: Option<golem_rust::secrets::GuestSecretHandle>,
        }

        for describe in [
            Config::<TaggedChoice>::config_entries,
            Config::<CompactedSlots>::config_entries,
            Config::<CapabilityDefault>::config_entries,
        ] {
            assert!(std::panic::catch_unwind(describe).is_err());
        }
    }
}
