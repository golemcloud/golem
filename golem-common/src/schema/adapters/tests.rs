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

//! Tests for the schema adapter layer.
//!
//! Coverage:
//!
//! - Unit tests for the explicit "lossy" cases (rich scalars, unions,
//!   capabilities, maps, fixed lists, recursive cycles, handles).
//! - Property-based round-trip tests over the shared legacy-compatible
//!   subset of types and values: `AnalysedType` → `SchemaType` → `AnalysedType`
//!   and `Value` → `SchemaValue` → `Value` are identities.

use test_r::test;

use golem_wasm::analysis::analysed_type::{field, record, s32, str, u32};
use golem_wasm::analysis::proptest_strategies::arb_type_and_value;
use golem_wasm::ValueAndType;
use proptest::proptest;

use crate::base_model::agent::{
    AgentConstructor, AgentMethod, AgentType, AgentTypeName, BinaryDescriptor, BinaryType,
    ComponentModelElementSchema, DataSchema, ElementSchema, NamedElementSchema,
    NamedElementSchemas, TextDescriptor, TextType,
};
use crate::schema::adapters::analysed_type::{
    analysed_type_to_schema_graph, analysed_type_to_schema_type_inline,
    schema_graph_to_analysed_type, schema_type_to_analysed_type,
};
use crate::schema::adapters::data_schema::{
    data_schema_to_input_schema, data_schema_to_output_schema, input_schema_to_data_schema,
    output_schema_to_data_schema,
};
use crate::schema::adapters::element_schema::{
    element_schema_to_schema_type, schema_type_to_element_schema,
};
use crate::schema::adapters::error::{SchemaAdapterError, legacy_type_id};
use crate::schema::adapters::value::{
    schema_value_to_value, typed_schema_value_to_value_and_type,
    value_and_type_to_typed_schema_value, value_to_schema_value,
};
use crate::schema::agent::{FieldSource, InputSchema, NamedField, OutputSchema};
use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::{Role, TypeId};
use crate::schema::schema_type::{
    BinaryRestrictions, NamedFieldType, PathDirection, PathKind, PathSpec, QuantitySpec,
    QuotaTokenSpec, SchemaType, SecretSpec, TextRestrictions, UnionSpec, UrlRestrictions,
};
use crate::schema::schema_value::{
    BinaryValuePayload, QuotaTokenValuePayload, SchemaValue, SecretValuePayload, TextValuePayload,
};

// --------------------------------------------------------------------------
// TypeId normalisation
// --------------------------------------------------------------------------

#[test]
fn legacy_type_id_dots_only() {
    let id = legacy_type_id(Some("a.b"), Some("c")).unwrap().unwrap();
    assert_eq!(id, TypeId::new("a.b.c"));
}

#[test]
fn legacy_type_id_normalises_double_colons() {
    let id = legacy_type_id(Some("a::b"), Some("c")).unwrap().unwrap();
    assert_eq!(id, TypeId::new("a.b.c"));
    let id = legacy_type_id(Some("a"), Some("b::c")).unwrap().unwrap();
    assert_eq!(id, TypeId::new("a.b.c"));
}

#[test]
fn legacy_type_id_bare_name() {
    let id = legacy_type_id(None, Some("x")).unwrap().unwrap();
    assert_eq!(id, TypeId::new("x"));
}

#[test]
fn legacy_type_id_owner_without_name_is_error() {
    let err = legacy_type_id(Some("a"), None).unwrap_err();
    assert!(matches!(
        err,
        SchemaAdapterError::UnsupportedLegacyMetadata(_)
    ));
}

#[test]
fn legacy_type_id_none() {
    assert!(legacy_type_id(None, None).unwrap().is_none());
}

// --------------------------------------------------------------------------
// AnalysedType → SchemaType: unsupported / lossy
// --------------------------------------------------------------------------

#[test]
fn schema_type_to_analysed_type_rejects_rich_scalars() {
    let cases = vec![
        SchemaType::text(TextRestrictions::default()),
        SchemaType::binary(BinaryRestrictions::default()),
        SchemaType::path(PathSpec {
            direction: PathDirection::InOut,
            kind: PathKind::Any,
            allowed_mime_types: None,
            allowed_extensions: None,
        }),
        SchemaType::url(UrlRestrictions::default()),
        SchemaType::datetime(),
        SchemaType::duration(),
        SchemaType::quantity(QuantitySpec {
            base_unit: "B".into(),
            allowed_suffixes: vec![],
            min: None,
            max: None,
        }),
        SchemaType::union(UnionSpec { branches: vec![] }),
        SchemaType::secret(SecretSpec::default()),
        SchemaType::quota_token(QuotaTokenSpec::default()),
        SchemaType::map(SchemaType::string(), SchemaType::s32()),
        SchemaType::fixed_list(SchemaType::s32(), 3),
    ];
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    for ty in cases {
        let err = schema_type_to_analysed_type(&graph, &ty).unwrap_err();
        assert!(
            matches!(err, SchemaAdapterError::LossySchemaType(_)),
            "expected LossySchemaType for {ty:?}, got {err:?}"
        );
    }
}

#[test]
fn schema_type_to_analysed_type_rejects_recursive_cycles() {
    let id = TypeId::new("rec");
    let graph = SchemaGraph {
        defs: vec![crate::schema::graph::SchemaTypeDef {
            id: id.clone(),
            name: None,
            body: SchemaType::record(vec![NamedFieldType {
                name: "self".into(),
                body: SchemaType::ref_to(id.clone()),
                metadata: Default::default(),
            }]),
        }],
        root: SchemaType::ref_to(id.clone()),
    };
    let err = schema_graph_to_analysed_type(&graph).unwrap_err();
    assert!(matches!(err, SchemaAdapterError::RecursiveRef(got) if got == id));
}

#[test]
fn schema_type_to_analysed_type_dangling_ref() {
    let id = TypeId::new("missing");
    let graph = SchemaGraph::anonymous(SchemaType::ref_to(id.clone()));
    let err = schema_graph_to_analysed_type(&graph).unwrap_err();
    assert!(matches!(err, SchemaAdapterError::DanglingRef(got) if got == id));
}

// --------------------------------------------------------------------------
// Value → SchemaValue: lossy cases
// --------------------------------------------------------------------------

#[test]
fn schema_value_to_value_rejects_rich_payloads() {
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let lossy = vec![
        (
            SchemaType::text(TextRestrictions::default()),
            SchemaValue::Text(TextValuePayload {
                text: "hi".into(),
                language: None,
            }),
        ),
        (
            SchemaType::binary(BinaryRestrictions::default()),
            SchemaValue::Binary(BinaryValuePayload {
                bytes: vec![1],
                mime_type: None,
            }),
        ),
        (
            SchemaType::secret(SecretSpec::default()),
            SchemaValue::Secret(SecretValuePayload {
                secret_ref: "x".into(),
            }),
        ),
        (
            SchemaType::quota_token(QuotaTokenSpec::default()),
            SchemaValue::QuotaToken(QuotaTokenValuePayload {
                environment_id: uuid::Uuid::nil(),
                resource_name: "rss".into(),
                expected_use: 0,
                last_credit: 0,
                last_credit_at: chrono::Utc::now(),
            }),
        ),
    ];
    for (ty, val) in lossy {
        let err = schema_value_to_value(&graph, &ty, &val).unwrap_err();
        assert!(
            matches!(err, SchemaAdapterError::LossySchemaType(_)),
            "expected LossySchemaType for {ty:?}/{val:?}, got {err:?}"
        );
    }
}

// --------------------------------------------------------------------------
// ElementSchema round-trip
// --------------------------------------------------------------------------

#[test]
fn element_schema_unstructured_text_round_trip() {
    let leg = ElementSchema::UnstructuredText(TextDescriptor {
        restrictions: Some(vec![TextType {
            language_code: "en-US".into(),
        }]),
    });
    let schema = element_schema_to_schema_type(&leg).unwrap();
    let graph = SchemaGraph::anonymous(schema.clone());
    let back = schema_type_to_element_schema(&graph, &schema).unwrap();
    assert_eq!(leg, back);
}

#[test]
fn element_schema_unstructured_binary_round_trip() {
    let leg = ElementSchema::UnstructuredBinary(BinaryDescriptor {
        restrictions: Some(vec![BinaryType {
            mime_type: "application/json".into(),
        }]),
    });
    let schema = element_schema_to_schema_type(&leg).unwrap();
    let graph = SchemaGraph::anonymous(schema.clone());
    let back = schema_type_to_element_schema(&graph, &schema).unwrap();
    assert_eq!(leg, back);
}

#[test]
fn element_schema_component_model_round_trip() {
    let leg = ElementSchema::ComponentModel(ComponentModelElementSchema {
        element_type: record(vec![field("x", s32()), field("y", str())]),
    });
    let schema = element_schema_to_schema_type(&leg).unwrap();
    let graph = SchemaGraph::anonymous(schema.clone());
    let back = schema_type_to_element_schema(&graph, &schema).unwrap();
    assert_eq!(leg, back);
}

#[test]
fn element_schema_with_constraints_is_lossy() {
    let restricted = SchemaType::text(TextRestrictions {
        languages: None,
        min_length: Some(1),
        max_length: None,
        regex: None,
    });
    let graph = SchemaGraph::anonymous(restricted.clone());
    let err = schema_type_to_element_schema(&graph, &restricted).unwrap_err();
    assert!(matches!(err, SchemaAdapterError::LossySchemaType(_)));
}

// --------------------------------------------------------------------------
// DataSchema round-trip
// --------------------------------------------------------------------------

#[test]
fn data_schema_tuple_input_round_trip() {
    let ds = DataSchema::Tuple(NamedElementSchemas {
        elements: vec![
            NamedElementSchema {
                name: "a".into(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: s32(),
                }),
            },
            NamedElementSchema {
                name: "b".into(),
                schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
            },
        ],
    });
    let input = data_schema_to_input_schema(&ds).unwrap();
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let back = input_schema_to_data_schema(&graph, &input).unwrap();
    assert_eq!(ds, back);
}

#[test]
fn data_schema_multimodal_to_input_is_error() {
    let ds = DataSchema::Multimodal(NamedElementSchemas { elements: vec![] });
    let err = data_schema_to_input_schema(&ds).unwrap_err();
    assert!(matches!(err, SchemaAdapterError::LossySchemaType(_)));
}

#[test]
fn data_schema_auto_injected_field_does_not_round_trip() {
    let input = InputSchema::Parameters(vec![NamedField {
        name: "principal".into(),
        source: FieldSource::AutoInjected(crate::schema::agent::AutoInjectedKind::Principal),
        schema: SchemaType::string(),
        metadata: Default::default(),
    }]);
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let err = input_schema_to_data_schema(&graph, &input).unwrap_err();
    assert!(matches!(err, SchemaAdapterError::LossySchemaType(_)));
}

#[test]
fn data_schema_tuple_output_empty_round_trip() {
    let ds = DataSchema::Tuple(NamedElementSchemas { elements: vec![] });
    let output = data_schema_to_output_schema(&ds).unwrap();
    assert!(matches!(output, OutputSchema::Unit));
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let back = output_schema_to_data_schema(&graph, &output).unwrap();
    assert_eq!(ds, back);
}

#[test]
fn data_schema_tuple_output_single_round_trip() {
    let ds = DataSchema::Tuple(NamedElementSchemas {
        elements: vec![NamedElementSchema {
            name: "value".into(),
            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                element_type: s32(),
            }),
        }],
    });
    let output = data_schema_to_output_schema(&ds).unwrap();
    // Single element collapses into the inline body (anonymous), so the
    // round-trip uses the fallback name `value`.
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let back = output_schema_to_data_schema(&graph, &output).unwrap();
    assert_eq!(ds, back);
}

#[test]
fn data_schema_tuple_output_multi_round_trip() {
    let ds = DataSchema::Tuple(NamedElementSchemas {
        elements: vec![
            NamedElementSchema {
                name: "a".into(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: s32(),
                }),
            },
            NamedElementSchema {
                name: "b".into(),
                schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
            },
        ],
    });
    let output = data_schema_to_output_schema(&ds).unwrap();
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let back = output_schema_to_data_schema(&graph, &output).unwrap();
    assert_eq!(ds, back);
}

#[test]
fn data_schema_multimodal_output_round_trip() {
    let ds = DataSchema::Multimodal(NamedElementSchemas {
        elements: vec![
            NamedElementSchema {
                name: "text".into(),
                schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
            },
            NamedElementSchema {
                name: "binary".into(),
                schema: ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions: None }),
            },
        ],
    });
    let output = data_schema_to_output_schema(&ds).unwrap();
    // Forward should produce `Single(list<union<...> with Role::Multimodal>)`.
    match &output {
        OutputSchema::Single(SchemaType::List { element, .. }) => match element.as_ref() {
            SchemaType::Union { metadata, .. } => {
                assert_eq!(metadata.role, Some(Role::Multimodal));
            }
            other => panic!("expected list element to be Union, got {other:?}"),
        },
        other => panic!("expected Single(List(...)), got {other:?}"),
    }
    let graph = SchemaGraph::anonymous(SchemaType::bool());
    let back = output_schema_to_data_schema(&graph, &output).unwrap();
    assert_eq!(ds, back);
}

// --------------------------------------------------------------------------
// Agent-level adapters
// --------------------------------------------------------------------------

#[test]
fn agent_type_round_trip() {
    use crate::base_model::Empty;
    use crate::base_model::agent::{AgentMode, Snapshotting};
    use crate::schema::adapters::agent::{agent_type_to_schema, schema_agent_type_to_legacy};

    let ty = AgentType {
        type_name: AgentTypeName("weather-agent".into()),
        description: "Reports weather".into(),
        source_language: "rust".into(),
        constructor: AgentConstructor {
            name: Some("ctor".into()),
            description: "ctor".into(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "city".into(),
                    schema: ElementSchema::UnstructuredText(TextDescriptor { restrictions: None }),
                }],
            }),
        },
        methods: vec![AgentMethod {
            name: "forecast".into(),
            description: "forecast".into(),
            prompt_hint: None,
            input_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![NamedElementSchema {
                    name: "days".into(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: u32(),
                    }),
                }],
            }),
            // Multi-element output preserves the field names through the
            // round-trip (single-element forms collapse into an anonymous
            // body and rehydrate with the fallback `value` name).
            output_schema: DataSchema::Tuple(NamedElementSchemas {
                elements: vec![
                    NamedElementSchema {
                        name: "report".into(),
                        schema: ElementSchema::UnstructuredText(TextDescriptor {
                            restrictions: None,
                        }),
                    },
                    NamedElementSchema {
                        name: "summary".into(),
                        schema: ElementSchema::UnstructuredText(TextDescriptor {
                            restrictions: None,
                        }),
                    },
                ],
            }),
            http_endpoint: vec![],
            read_only: None,
        }],
        dependencies: vec![],
        mode: AgentMode::Ephemeral,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    };

    let schema = agent_type_to_schema(&ty).unwrap();
    let back = schema_agent_type_to_legacy(&schema).unwrap();
    assert_eq!(ty, back);
}

/// Refs in a parent constructor / method body resolve against
/// [`AgentTypeSchema::schema`] during the schema → legacy conversion.
#[test]
fn agent_type_reverse_resolves_parent_refs() {
    use crate::base_model::Empty;
    use crate::base_model::agent::{AgentMode, Snapshotting};
    use crate::schema::adapters::agent::schema_agent_type_to_legacy;
    use crate::schema::agent::{
        AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, NamedField,
        OutputSchema,
    };
    use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
    use crate::schema::metadata::TypeId;
    use crate::schema::schema_type::SchemaType;

    let user_id = TypeId::new("a.b.UserId");
    let agent = AgentTypeSchema {
        type_name: AgentTypeName("dir".into()),
        description: "x".into(),
        source_language: String::new(),
        schema: SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: user_id.clone(),
                name: Some("UserId".into()),
                body: SchemaType::string(),
            }],
            root: SchemaType::Record {
                fields: vec![],
                metadata: Default::default(),
            },
        },
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                "id",
                SchemaType::ref_to(user_id.clone()),
            )]),
        },
        methods: vec![AgentMethodSchema {
            name: "lookup".into(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                "id",
                SchemaType::ref_to(user_id.clone()),
            )]),
            output_schema: OutputSchema::Single(SchemaType::ref_to(user_id.clone())),
            http_endpoint: vec![],
            read_only: None,
        }],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    };

    // Should succeed — refs resolve via the agent's graph.
    let legacy = schema_agent_type_to_legacy(&agent).expect("ref resolution should succeed");

    // Constructor `id` parameter resolves to whatever the data-schema
    // adapter chooses for a `SchemaType::String` (the structural details
    // are covered by element-schema tests; here we only need to confirm
    // that the ref was resolved at all).
    let ctor_elements = match &legacy.constructor.input_schema {
        DataSchema::Tuple(elements) => &elements.elements,
        _ => panic!("expected tuple"),
    };
    assert_eq!(ctor_elements.len(), 1);
    assert_eq!(ctor_elements[0].name, "id");

    // Method output single ref → wrapped in a synthetic tuple element named "value".
    let out_elements = match &legacy.methods[0].output_schema {
        DataSchema::Tuple(elements) => &elements.elements,
        _ => panic!("expected tuple"),
    };
    assert_eq!(out_elements.len(), 1);
    assert_eq!(out_elements[0].name, "value");
}

/// A dependency's refs resolve against the dependency's own graph, not
/// the parent agent's.
#[test]
fn agent_dependency_reverse_resolves_dep_refs() {
    use crate::base_model::Empty;
    use crate::base_model::agent::{AgentMode, Snapshotting};
    use crate::schema::adapters::agent::schema_agent_type_to_legacy;
    use crate::schema::agent::{
        AgentConstructorSchema, AgentDependencySchema, AgentTypeSchema, InputSchema, NamedField,
    };
    use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
    use crate::schema::metadata::TypeId;
    use crate::schema::schema_type::SchemaType;

    let dep_local = TypeId::new("dep.Local");
    let parent_only = TypeId::new("parent.Only");

    let agent = AgentTypeSchema {
        type_name: AgentTypeName("parent".into()),
        description: String::new(),
        source_language: String::new(),
        schema: SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: parent_only.clone(),
                name: None,
                body: SchemaType::string(),
            }],
            root: SchemaType::Record {
                fields: vec![],
                metadata: Default::default(),
            },
        },
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
        },
        methods: vec![],
        dependencies: vec![AgentDependencySchema {
            type_name: "dep".into(),
            description: None,
            schema: SchemaGraph {
                defs: vec![SchemaTypeDef {
                    id: dep_local.clone(),
                    name: None,
                    body: SchemaType::u32(),
                }],
                root: SchemaType::Record {
                    fields: vec![],
                    metadata: Default::default(),
                },
            },
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                    "n",
                    SchemaType::ref_to(dep_local.clone()),
                )]),
            },
            methods: vec![],
        }],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    };

    let legacy = schema_agent_type_to_legacy(&agent).expect("dep ref resolution should succeed");
    let dep_ctor_elements = match &legacy.dependencies[0].constructor.input_schema {
        DataSchema::Tuple(elements) => &elements.elements,
        _ => panic!("expected tuple"),
    };
    assert_eq!(dep_ctor_elements[0].name, "n");
}

/// A ref to a type missing from the agent graph is rejected by the
/// reverse adapter with a clear error.
#[test]
fn agent_type_reverse_rejects_unresolvable_ref() {
    use crate::base_model::Empty;
    use crate::base_model::agent::{AgentMode, Snapshotting};
    use crate::schema::adapters::agent::schema_agent_type_to_legacy;
    use crate::schema::agent::{
        AgentConstructorSchema, AgentTypeSchema, InputSchema, NamedField,
    };
    use crate::schema::graph::SchemaGraph;
    use crate::schema::metadata::TypeId;
    use crate::schema::schema_type::SchemaType;

    let agent = AgentTypeSchema {
        type_name: AgentTypeName("dir".into()),
        description: String::new(),
        source_language: String::new(),
        schema: SchemaGraph::empty(),
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                "id",
                SchemaType::ref_to(TypeId::new("missing")),
            )]),
        },
        methods: vec![],
        dependencies: vec![],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    };

    let err = schema_agent_type_to_legacy(&agent).unwrap_err();
    assert!(
        matches!(err, SchemaAdapterError::DanglingRef(ref id) if id == &TypeId::new("missing")),
        "expected DanglingRef(missing), got: {err:?}"
    );
}

/// A dependency cannot resolve a ref that lives only in the parent
/// agent's graph: it should fail with `DanglingRef`, enforcing the
/// "no cross-agent registry" rule.
#[test]
fn agent_dependency_cannot_see_parent_refs() {
    use crate::base_model::Empty;
    use crate::base_model::agent::{AgentMode, Snapshotting};
    use crate::schema::adapters::agent::schema_agent_type_to_legacy;
    use crate::schema::agent::{
        AgentConstructorSchema, AgentDependencySchema, AgentTypeSchema, InputSchema, NamedField,
    };
    use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
    use crate::schema::metadata::TypeId;
    use crate::schema::schema_type::SchemaType;

    let parent_only = TypeId::new("parent.Only");

    let agent = AgentTypeSchema {
        type_name: AgentTypeName("parent".into()),
        description: String::new(),
        source_language: String::new(),
        schema: SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: parent_only.clone(),
                name: None,
                body: SchemaType::string(),
            }],
            root: SchemaType::Record {
                fields: vec![],
                metadata: Default::default(),
            },
        },
        constructor: AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
        },
        methods: vec![],
        dependencies: vec![AgentDependencySchema {
            type_name: "dep".into(),
            description: None,
            schema: SchemaGraph::empty(), // dep has no defs of its own
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![NamedField::user_supplied(
                    "x",
                    SchemaType::ref_to(parent_only.clone()),
                )]),
            },
            methods: vec![],
        }],
        mode: AgentMode::Durable,
        http_mount: None,
        snapshotting: Snapshotting::Disabled(Empty {}),
        config: vec![],
    };

    let err = schema_agent_type_to_legacy(&agent).unwrap_err();
    assert!(
        matches!(err, SchemaAdapterError::DanglingRef(ref id) if id == &parent_only),
        "expected DanglingRef({parent_only}) — dependencies must not see parent defs; got: {err:?}"
    );
}

// --------------------------------------------------------------------------
// Property-based round-trip tests over the shared legacy subset
// --------------------------------------------------------------------------

proptest! {
    /// `AnalysedType` → inline `SchemaType` → `AnalysedType` is an identity
    /// for the legacy-compatible subset (no `name`/`owner` on composites,
    /// no `Handle`).
    #[test]
    fn proptest_analysed_type_inline_round_trip(
        pair in arb_type_and_value()
    ) {
        let (ty, _value) = pair;
        let schema = analysed_type_to_schema_type_inline(&ty).expect("forward type conv");
        let graph = SchemaGraph::anonymous(schema.clone());
        let back = schema_type_to_analysed_type(&graph, &schema).expect("reverse type conv");
        proptest::prop_assert_eq!(strip_names(ty), back);
    }

    /// `Value` + `AnalysedType` → `SchemaValue` → `Value` is an identity
    /// for the legacy-compatible subset.
    #[test]
    fn proptest_value_round_trip(
        pair in arb_type_and_value()
    ) {
        let (ty, value) = pair;
        let schema = analysed_type_to_schema_type_inline(&ty).expect("forward type conv");
        let graph = SchemaGraph::anonymous(schema.clone());
        let sv = value_to_schema_value(&value, &ty).expect("forward value conv");
        let back = schema_value_to_value(&graph, &schema, &sv).expect("reverse value conv");
        proptest::prop_assert_eq!(value, back);
    }

    /// `ValueAndType` → `TypedSchemaValue` → `ValueAndType` is an identity
    /// for the legacy-compatible subset.
    #[test]
    fn proptest_value_and_type_round_trip(
        pair in arb_type_and_value()
    ) {
        let (ty, value) = pair;
        let vat = ValueAndType { value, typ: ty };
        let tsv = value_and_type_to_typed_schema_value(&vat).expect("forward conv");
        let back = typed_schema_value_to_value_and_type(&tsv).expect("reverse conv");
        // Strip `name`/`owner` from the type side of the original because
        // the inline strategy never sets them, but the graph reverse may
        // attach defaults.
        proptest::prop_assert_eq!(strip_value_and_type_names(vat), back);
    }

    /// Named composites with `owner` / `name` survive the graph-aware
    /// round-trip: the schema graph re-attaches the legacy display name on
    /// reverse.
    #[test]
    fn proptest_analysed_type_graph_round_trip(
        pair in arb_type_and_value()
    ) {
        let (ty, _value) = pair;
        let graph = analysed_type_to_schema_graph(&ty).expect("forward graph conv");
        let back = schema_graph_to_analysed_type(&graph).expect("reverse graph conv");
        proptest::prop_assert_eq!(strip_names(ty), back);
    }
}

// --------------------------------------------------------------------------
// helpers
// --------------------------------------------------------------------------

/// Strip `name` / `owner` from every composite layer of an `AnalysedType`
/// (used because the inline adapter drops these and the strategy never
/// produces them anyway, but to be defensive).
fn strip_names(ty: golem_wasm::analysis::AnalysedType) -> golem_wasm::analysis::AnalysedType {
    use golem_wasm::analysis::AnalysedType::*;
    match ty {
        Bool(_) | S8(_) | S16(_) | S32(_) | S64(_) | U8(_) | U16(_) | U32(_) | U64(_) | F32(_)
        | F64(_) | Chr(_) | Str(_) => ty,
        List(mut t) => {
            t.name = None;
            t.owner = None;
            t.inner = Box::new(strip_names(*t.inner));
            List(t)
        }
        Tuple(mut t) => {
            t.name = None;
            t.owner = None;
            t.items = t.items.into_iter().map(strip_names).collect();
            Tuple(t)
        }
        Record(mut t) => {
            t.name = None;
            t.owner = None;
            t.fields = t
                .fields
                .into_iter()
                .map(|p| golem_wasm::analysis::NameTypePair {
                    name: p.name,
                    typ: strip_names(p.typ),
                })
                .collect();
            Record(t)
        }
        Variant(mut t) => {
            t.name = None;
            t.owner = None;
            t.cases = t
                .cases
                .into_iter()
                .map(|c| golem_wasm::analysis::NameOptionTypePair {
                    name: c.name,
                    typ: c.typ.map(strip_names),
                })
                .collect();
            Variant(t)
        }
        Enum(mut t) => {
            t.name = None;
            t.owner = None;
            Enum(t)
        }
        Flags(mut t) => {
            t.name = None;
            t.owner = None;
            Flags(t)
        }
        Option(mut t) => {
            t.name = None;
            t.owner = None;
            t.inner = Box::new(strip_names(*t.inner));
            Option(t)
        }
        Result(mut t) => {
            t.name = None;
            t.owner = None;
            t.ok = t.ok.map(|b| Box::new(strip_names(*b)));
            t.err = t.err.map(|b| Box::new(strip_names(*b)));
            Result(t)
        }
        Handle(h) => Handle(h),
    }
}

fn strip_value_and_type_names(vat: ValueAndType) -> ValueAndType {
    ValueAndType {
        value: vat.value,
        typ: strip_names(vat.typ),
    }
}

