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

use super::schema_graph::{option_wrapper_graph, schema_shapes_match};
use super::validation::{check_graph_closed, check_scalar_graph};
use super::*;
use crate::tool_refinement::{refine_numeric, refine_path, refine_text, refine_url};
use golem_schema::schema::schema_type::{NumericBound, NumericRestrictions};
use golem_schema::schema::{
    BinaryRestrictions, PathDirection, PathKind, PathSpec, PermissionCardSpec, QuantitySpec,
    QuotaTokenSpec, SecretSpec, TextRestrictions,
};
use test_r::test;

fn doc(summary: &str) -> Doc {
    Doc {
        summary: summary.to_string(),
        description: String::new(),
        examples: vec![],
    }
}

fn str_graph() -> SchemaGraph {
    SchemaGraph::anonymous(SchemaType::string())
}
fn u32_graph() -> SchemaGraph {
    SchemaGraph::anonymous(SchemaType::u32())
}

#[test]
fn preparation_checks_once_and_caches_conversion() {
    thread_local! {
        static CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }
    fn counted(graph: &SchemaGraph, position: &str) -> Result<(), ToolBuildError> {
        CHECKS.with(|count| count.set(count.get() + 1));
        check_graph_closed(graph, position)
    }
    let tool = sample_tool();
    let reference = tool.try_to_native_tool().unwrap();
    let prepared = tool
        .prepare_without_literals(ToolSchemaChecks(counted))
        .unwrap();
    assert_eq!(CHECKS.with(|count| count.get()), 4);
    assert_eq!(prepared.native(), &reference);
    assert!(std::ptr::eq(prepared.native(), prepared.native()));
    let clone = prepared.clone();
    assert_eq!(clone.native(), prepared.native());
    #[cfg(feature = "guest")]
    {
        assert_eq!(
            format!("{:?}", clone.wire()),
            format!("{:?}", prepared.extended().try_to_tool().unwrap())
        );
    }
    assert_eq!(CHECKS.with(|count| count.get()), 4);
}

#[test]
fn scalar_preparation_checks_structure_and_rejects_unexpected_literals() {
    let mut tool = sample_tool();
    tool.commands[1].body.as_mut().unwrap().options.clear();
    let reference = tool.try_to_native_tool().unwrap();
    assert_eq!(
        tool.clone()
            .prepare_without_literals(ToolSchemaChecks::scalar())
            .unwrap()
            .native(),
        &reference
    );

    let mut duplicate = tool.clone();
    duplicate.commands[1].body.as_mut().unwrap().flags[0].long = "input".into();
    assert_eq!(
        duplicate
            .clone()
            .prepare_without_literals(ToolSchemaChecks::scalar())
            .unwrap_err()
            .to_string(),
        duplicate.prepare().unwrap_err().to_string(),
    );

    let mut default = tool.clone();
    default.commands[1].body.as_mut().unwrap().positionals.fixed[0].default =
        Some(SchemaValue::String("valid".into()));
    assert!(default.clone().prepare().is_ok());
    assert!(matches!(
        default.prepare_without_literals(ToolSchemaChecks::scalar()),
        Err(ToolBuildError::DefaultTypeMismatch(_))
    ));

    tool.commands[1]
        .body
        .as_mut()
        .unwrap()
        .constraints
        .push(ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
            ExtendedValueIsRef {
                name: "input".into(),
                value: ExtendedValueIsLiteral::Resolved(SchemaValue::String("valid".into())),
            },
        )]));
    assert!(tool.clone().prepare().is_ok());
    assert!(matches!(
        tool.prepare_without_literals(ToolSchemaChecks::scalar()),
        Err(ToolBuildError::UnresolvedValueIsLiteral(_))
    ));
}

#[test]
fn schema_check_selection_uses_resolved_type_and_keeps_dynamic_checks() {
    type Alias = String;
    struct Custom;
    assert!(
        (&ToolSchemaProbe::<Alias>(std::marker::PhantomData))
            .tool_schema_checks()
            .is_none()
    );
    let dynamic = (&ToolSchemaProbe::<Custom>(std::marker::PhantomData))
        .tool_schema_checks()
        .unwrap();
    let invalid = SchemaGraph::anonymous(SchemaType::text(TextRestrictions {
        regex: Some("[".into()),
        ..Default::default()
    }));
    assert_eq!(
        (dynamic.0)(&invalid, "test").unwrap_err().to_string(),
        check_graph_closed(&invalid, "test")
            .unwrap_err()
            .to_string()
    );
    assert!((ToolSchemaChecks::scalar().0)(&invalid, "test").is_err());
}

#[test]
fn scalar_schema_constructors_match_reference_without_calling_fallback() {
    macro_rules! check {
        ($($ty:ty),*) => {$(
            let graph = (&ToolSchemaProbe::<$ty>(std::marker::PhantomData))
                .tool_value_schema(|| panic!("scalar construction must be static"))
                .unwrap();
            assert_eq!(graph, native_tool_value_schema::<$ty>("test").unwrap());
            check_scalar_graph(&graph, "test").unwrap();
        )*};
    }
    check!(
        bool, u8, u16, u32, u64, i8, i16, i32, i64, f32, f64, char, String
    );
    struct Custom;
    let graph = (&ToolSchemaProbe::<Custom>(std::marker::PhantomData))
        .tool_value_schema(|| Ok(str_graph()))
        .unwrap();
    assert_eq!(graph, str_graph());
}

fn sample_tool() -> ExtendedToolType {
    ExtendedToolType {
        version: "0.1.0".to_string(),
        requires_filesystem: false,
        commands: vec![
            ExtendedCommandNode {
                name: "root".to_string(),
                aliases: vec![],
                doc: doc("root"),
                globals: ExtendedGlobals {
                    options: vec![ExtendedOptionSpec {
                        long: "verbose".to_string(),
                        short: None,
                        aliases: vec![],
                        doc: doc("global"),
                        value_name: None,
                        shape: ExtendedOptionShape::Scalar(u32_graph()),
                        default: None,
                        required: false,
                        env_var: None,
                    }],
                    flags: vec![],
                },
                subcommands: vec![1],
                body: None,
            },
            ExtendedCommandNode {
                name: "run".to_string(),
                aliases: vec!["r".to_string()],
                doc: doc("run"),
                globals: ExtendedGlobals::default(),
                subcommands: vec![],
                body: Some(ExtendedCommandBody {
                    positionals: ExtendedPositionals {
                        fixed: vec![ExtendedPositional {
                            name: "input".to_string(),
                            doc: doc("input"),
                            value_name: None,
                            type_: str_graph(),
                            default: None,
                            required: true,
                            accepts_stdio: false,
                        }],
                        tail: None,
                    },
                    options: vec![ExtendedOptionSpec {
                        long: "config".to_string(),
                        short: None,
                        aliases: vec![],
                        doc: doc("config"),
                        value_name: None,
                        shape: ExtendedOptionShape::RepeatableMap(ExtendedRepeatableMapShape {
                            repetition: wire::Repetition::Repeated,
                            map_type: SchemaGraph::anonymous(SchemaType::map(
                                SchemaType::string(),
                                SchemaType::string(),
                            )),
                            duplicate_key_policy: wire::DuplicateKeyPolicy::Reject,
                        }),
                        default: None,
                        required: false,
                        env_var: None,
                    }],
                    flags: vec![FlagSpec {
                        long: "force".to_string(),
                        short: None,
                        aliases: vec![],
                        doc: doc("force"),
                        shape: wire::FlagShape::BoolFlag(wire::BoolFlagShape {
                            default: false,
                            negatable: false,
                        }),
                        env_var: None,
                    }],
                    constraints: vec![],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                    result: Some(ExtendedResultSpec {
                        type_: str_graph(),
                        doc: doc("result"),
                        formatters: vec![ToolFormatter {
                            name: "human".to_string(),
                            doc: doc("human"),
                        }],
                        default_formatter: "human".to_string(),
                    }),
                    errors: vec![],
                    annotations: None,
                    positional_plan: vec![],
                }),
            },
        ],
    }
}

#[test]
fn canonical_input_model_rejects_conflicting_type_ids_across_fields() {
    use golem_schema::schema::{SchemaTypeDef, TypeId};

    // Two canonical input fields carry a definition with the same id but
    // different bodies: the record schema construction must fail rather
    // than silently pick either body.
    let mut tool = sample_tool();
    let body = tool.commands[1].body.as_mut().unwrap();
    body.positionals.fixed[0].type_ = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: TypeId::from("conflicting"),
            name: None,
            body: SchemaType::string(),
        }],
        root: SchemaType::ref_to(TypeId::from("conflicting")),
    };
    body.options[0].shape = ExtendedOptionShape::Scalar(SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: TypeId::from("conflicting"),
            name: None,
            body: SchemaType::u32(),
        }],
        root: SchemaType::ref_to(TypeId::from("conflicting")),
    });

    // The per-field graphs are still the author-provided ones.
    let fields = tool.canonical_input_fields(1);
    let input = fields.iter().find(|f| f.name == "input").unwrap();
    assert_eq!(
        input.schema.root,
        SchemaType::ref_to(TypeId::from("conflicting"))
    );
    assert_eq!(input.schema.defs[0].body, SchemaType::string());
    let config = fields.iter().find(|f| f.name == "config").unwrap();
    assert_eq!(config.schema.defs[0].body, SchemaType::u32());

    assert!(tool.canonical_input_model(1).is_err());
}

#[cfg(feature = "guest")]
#[test]
fn builds_tool_and_orders_fields() {
    let tool = sample_tool();
    let wire = tool.to_tool();
    assert_eq!(wire.commands.nodes.len(), 2);
    assert_eq!(
        wire.commands.nodes[1].body.as_ref().unwrap().options.len(),
        1
    );
    let names: Vec<_> = tool
        .canonical_input_fields(1)
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(names, vec!["verbose", "input", "config", "force"]);
}

#[test]
fn canonical_input_model_builds_record_schema_in_field_order() {
    let tool = sample_tool();
    let model = tool.canonical_input_model(1).unwrap();

    assert_eq!(
        model
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        vec!["verbose", "input", "config", "force"]
    );
    let SchemaType::Record { fields, .. } = &model.record_schema.root else {
        panic!("canonical input schema must be a record")
    };
    assert_eq!(
        fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        vec!["verbose", "input", "config", "force"]
    );
}

#[test]
fn extended_and_native_canonical_input_schemas_match_optional_carriers() {
    let mut tool = sample_tool();
    let body = tool.commands[1].body.as_mut().unwrap();
    body.positionals.fixed[0].required = false;
    let extended = tool.canonical_input_model(1).unwrap();
    let native = tool
        .canonical_projection()
        .canonical_input_model(1)
        .unwrap();
    assert_eq!(extended.record_schema, native.record_schema);

    let SchemaType::Record { fields, .. } = &extended.record_schema.root else {
        panic!("canonical input schema must be a record")
    };
    assert!(matches!(fields[0].body, SchemaType::Option { .. }));
    assert!(matches!(fields[1].body, SchemaType::Option { .. }));
    assert!(matches!(fields[2].body, SchemaType::Map { .. }));
}

#[test]
fn generated_client_input_model_encodes_absent_and_supplied_optional_values() {
    let model = sample_tool().canonical_input_model(1).unwrap();
    for (verbose, expected) in [
        (
            SchemaValue::Option { inner: None },
            SchemaValue::Option { inner: None },
        ),
        (
            SchemaValue::U32(3),
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::U32(3))),
            },
        ),
    ] {
        let input = build_canonical_input(
            &model,
            vec![
                ("verbose", verbose),
                ("input", SchemaValue::String("in.txt".to_string())),
                (
                    "config",
                    SchemaValue::Map {
                        entries: Vec::new(),
                    },
                ),
                ("force", SchemaValue::Bool(false)),
            ],
        )
        .unwrap();
        assert_eq!(input.graph(), &model.record_schema);
        let SchemaValue::Record { fields } = input.value() else {
            panic!("canonical generated-client input must be a record")
        };
        assert_eq!(&fields[0], &expected);
    }
}

#[test]
fn canonical_forwarding_adapts_present_optional_carriers() {
    let bare = u32_graph();
    let optional = option_wrapper_graph(&bare);
    let source = CanonicalInputValue {
        name: "count".to_string(),
        aliases: vec!["format".to_string()],
        short: None,
        schema: optional.clone(),
        value: SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::U32(7))),
        },
    };
    assert_eq!(
        adapt_canonical_input_value(source, "format", &bare).unwrap(),
        SchemaValue::U32(7)
    );

    let source = CanonicalInputValue {
        name: "format".to_string(),
        aliases: vec!["count".to_string()],
        short: None,
        schema: bare,
        value: SchemaValue::U32(7),
    };
    assert_eq!(
        adapt_canonical_input_value(source, "count", &optional).unwrap(),
        SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::U32(7))),
        }
    );
}

#[test]
fn canonical_forwarding_rejects_absent_required_values() {
    let bare = u32_graph();
    let source = CanonicalInputValue {
        name: "count".to_string(),
        aliases: vec!["format".to_string()],
        short: None,
        schema: option_wrapper_graph(&bare),
        value: SchemaValue::Option { inner: None },
    };
    assert_eq!(
        adapt_canonical_input_value(source, "format", &bare).unwrap_err(),
        "canonical tool input field `count` is absent but forwarded field `format` is required"
    );
}

#[test]
fn inherited_prefix_keeps_parent_carrier_when_child_redeclares_field() {
    let bare = u32_graph();
    let optional = option_wrapper_graph(&bare);
    for value in [
        SchemaValue::Option { inner: None },
        SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::U32(7))),
        },
    ] {
        let input = build_canonical_input_with_prefix(
            vec![CanonicalInputField {
                name: "count".to_string(),
                aliases: vec![],
                short: None,
                schema: bare.clone(),
            }],
            &[CanonicalInputValue {
                name: "count".to_string(),
                aliases: vec![],
                short: None,
                schema: optional.clone(),
                value: value.clone(),
            }],
            vec![],
        )
        .unwrap();
        assert_eq!(
            input.value(),
            &SchemaValue::Record {
                fields: vec![value]
            }
        );
        let expected = CanonicalInputModel::from_fields(vec![CanonicalInputField {
            name: "count".to_string(),
            aliases: vec![],
            short: None,
            schema: optional.clone(),
        }])
        .unwrap();
        assert_eq!(input.graph(), &expected.record_schema);
    }
}

#[test]
fn inherited_prefix_keeps_optional_parent_field_absent_from_child() {
    let optional = option_wrapper_graph(&u32_graph());
    for value in [
        SchemaValue::Option { inner: None },
        SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::U32(7))),
        },
    ] {
        let input = build_canonical_input_with_prefix(
            vec![CanonicalInputField {
                name: "query".to_string(),
                aliases: vec![],
                short: None,
                schema: str_graph(),
            }],
            &[CanonicalInputValue {
                name: "verbose".to_string(),
                aliases: vec![],
                short: None,
                schema: optional.clone(),
                value: value.clone(),
            }],
            vec![("query", SchemaValue::String("demo".to_string()))],
        )
        .unwrap();
        let expected = CanonicalInputModel::from_fields(vec![
            CanonicalInputField {
                name: "verbose".to_string(),
                aliases: vec![],
                short: None,
                schema: optional.clone(),
            },
            CanonicalInputField {
                name: "query".to_string(),
                aliases: vec![],
                short: None,
                schema: str_graph(),
            },
        ])
        .unwrap();
        assert_eq!(input.graph(), &expected.record_schema);
        assert_eq!(
            input.value(),
            &SchemaValue::Record {
                fields: vec![value, SchemaValue::String("demo".to_string())]
            }
        );
    }
}

#[test]
fn inherited_prefix_keeps_required_parent_carrier_when_child_is_optional() {
    let required = str_graph();
    let input = build_canonical_input_with_prefix(
        vec![CanonicalInputField {
            name: "format".to_string(),
            aliases: vec![],
            short: None,
            schema: option_wrapper_graph(&required),
        }],
        &[CanonicalInputValue {
            name: "format".to_string(),
            aliases: vec![],
            short: None,
            schema: required.clone(),
            value: SchemaValue::String("json".to_string()),
        }],
        vec![],
    )
    .unwrap();
    let expected = CanonicalInputModel::from_fields(vec![CanonicalInputField {
        name: "format".to_string(),
        aliases: vec![],
        short: None,
        schema: required,
    }])
    .unwrap();
    assert_eq!(input.graph(), &expected.record_schema);
    assert_eq!(
        input.value(),
        &SchemaValue::Record {
            fields: vec![SchemaValue::String("json".to_string())]
        }
    );
}

#[test]
fn canonical_input_model_decodes_positional_record_by_index() {
    let tool = sample_tool();
    let model = tool.canonical_input_model(1).unwrap();
    let decoded = model
        .decode_record(SchemaValue::Record {
            fields: vec![
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::U32(3))),
                },
                SchemaValue::String("in.txt".to_string()),
                SchemaValue::Map { entries: vec![] },
                SchemaValue::Bool(true),
            ],
        })
        .unwrap();

    assert_eq!(
        decoded
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        vec!["verbose", "input", "config", "force"]
    );
    assert_eq!(
        decoded[0].value,
        SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::U32(3))),
        }
    );
    assert_eq!(decoded[1].value, SchemaValue::String("in.txt".to_string()));
    assert_eq!(decoded[3].value, SchemaValue::Bool(true));
}

#[test]
fn canonical_input_model_rejects_non_record_and_wrong_field_count() {
    let tool = sample_tool();
    let model = tool.canonical_input_model(1).unwrap();

    assert_eq!(
        model.decode_record(SchemaValue::String("nope".to_string())),
        Err(CanonicalInputDecodeError::ExpectedRecord)
    );
    assert_eq!(
        model.decode_record(SchemaValue::Record { fields: vec![] }),
        Err(CanonicalInputDecodeError::FieldCountMismatch {
            expected: 4,
            actual: 0,
        })
    );
}

#[test]
fn canonical_input_model_rejects_out_of_bounds_command_index() {
    let tool = sample_tool();

    assert_eq!(
        tool.canonical_input_model(99).unwrap_err(),
        ToolBuildError::CommandIndexOutOfBounds { index: 99, len: 2 }
    );
    assert_eq!(
        tool.canonical_input_record_schema(99).unwrap_err(),
        ToolBuildError::CommandIndexOutOfBounds { index: 99, len: 2 }
    );
    assert_eq!(
        tool.decode_canonical_input_record(99, SchemaValue::Record { fields: vec![] }),
        Err(CanonicalInputDecodeError::Model(
            ToolBuildError::CommandIndexOutOfBounds { index: 99, len: 2 }
        ))
    );
}

#[test]
fn canonical_input_model_validates_synthesized_record_schema() {
    let error = CanonicalInputModel::from_fields(vec![
        CanonicalInputField {
            name: "same".to_string(),
            aliases: Vec::new(),
            short: None,
            schema: str_graph(),
        },
        CanonicalInputField {
            name: "same".to_string(),
            aliases: Vec::new(),
            short: None,
            schema: u32_graph(),
        },
    ])
    .unwrap_err();

    assert!(
        matches!(error, ToolBuildError::IllFormedSchema { .. }),
        "expected synthesized record validation error, got {error:?}"
    );
}

#[test]
fn canonical_input_model_rejects_field_schema_with_duplicate_identical_defs() {
    let id = golem_schema::schema::TypeId::new("dup");
    let graph = SchemaGraph {
        defs: vec![
            golem_schema::schema::SchemaTypeDef {
                id: id.clone(),
                name: Some("first".to_string()),
                body: SchemaType::string(),
            },
            golem_schema::schema::SchemaTypeDef {
                id: id.clone(),
                name: Some("second".to_string()),
                body: SchemaType::string(),
            },
        ],
        root: SchemaType::ref_to(id),
    };

    assert!(golem_schema::schema::validation::validate_graph(&graph).is_err());
    let error = CanonicalInputModel::from_fields(vec![CanonicalInputField {
        name: "field".to_string(),
        aliases: Vec::new(),
        short: None,
        schema: graph,
    }])
    .unwrap_err();

    assert!(
        matches!(error, ToolBuildError::IllFormedSchema { .. }),
        "expected duplicate type id validation error, got {error:?}"
    );
}

/// Builds a two-node tool: a root carrying one inherited global option, and a
/// single `leaf` body whose only option is described by `(long, aliases)`.
/// Used to exercise alias-aware body-vs-inherited-global shadowing on an
/// unnormalized descriptor.
fn tool_with_inherited_global_and_body_option(
    global_long: &str,
    global_aliases: Vec<String>,
    body_long: &str,
    body_aliases: Vec<String>,
) -> ExtendedToolType {
    ExtendedToolType {
        version: "0.1.0".to_string(),
        requires_filesystem: false,
        commands: vec![
            ExtendedCommandNode {
                name: "root".to_string(),
                aliases: vec![],
                doc: doc("root"),
                globals: ExtendedGlobals {
                    options: vec![ExtendedOptionSpec {
                        long: global_long.to_string(),
                        short: None,
                        aliases: global_aliases,
                        doc: doc("global"),
                        value_name: None,
                        shape: ExtendedOptionShape::Scalar(u32_graph()),
                        default: None,
                        required: false,
                        env_var: None,
                    }],
                    flags: vec![],
                },
                subcommands: vec![1],
                body: None,
            },
            ExtendedCommandNode {
                name: "leaf".to_string(),
                aliases: vec![],
                doc: doc("leaf"),
                globals: ExtendedGlobals::default(),
                subcommands: vec![],
                body: Some(ExtendedCommandBody {
                    positionals: ExtendedPositionals::default(),
                    options: vec![ExtendedOptionSpec {
                        long: body_long.to_string(),
                        short: None,
                        aliases: body_aliases,
                        doc: doc("body"),
                        value_name: None,
                        shape: ExtendedOptionShape::Scalar(str_graph()),
                        default: None,
                        required: false,
                        env_var: None,
                    }],
                    flags: vec![],
                    constraints: vec![],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                    result: None,
                    errors: vec![],
                    annotations: None,
                    positional_plan: vec![],
                }),
            },
        ],
    }
}

#[test]
fn canonical_input_fields_body_alias_shadows_inherited_global() {
    // A body option whose ALIAS equals an inherited global's long name
    // shadows that global on an unnormalized descriptor.
    let tool = tool_with_inherited_global_and_body_option(
        "verbose",
        vec![],
        "local",
        vec!["verbose".to_string()],
    );
    let names: Vec<_> = tool
        .canonical_input_fields(1)
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(
        names,
        vec!["local"],
        "an inherited global must be shadowed by a body option aliased to its name"
    );
}

#[test]
fn canonical_input_fields_inherited_global_alias_is_shadowed() {
    // The reverse: an inherited global whose ALIAS equals a body option's
    // long name is shadowed too.
    let tool =
        tool_with_inherited_global_and_body_option("global", vec!["v".to_string()], "v", vec![]);
    let names: Vec<_> = tool
        .canonical_input_fields(1)
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(
        names,
        vec!["v"],
        "an inherited global aliased to a body option's name must be shadowed"
    );
}

#[test]
fn help_contains_names() {
    let help = render_help(&sample_tool(), &["run".to_string()]).unwrap();
    assert!(help.contains("run"));
    assert!(help.contains("--config"));
}

fn dispatcher_child() -> ExtendedToolType {
    ExtendedToolType {
        version: "x".into(),
        requires_filesystem: false,
        commands: vec![
            ExtendedCommandNode {
                name: "child".into(),
                aliases: vec![],
                doc: doc(""),
                globals: ExtendedGlobals::default(),
                subcommands: vec![1],
                body: None,
            },
            ExtendedCommandNode {
                name: "leaf".into(),
                aliases: vec![],
                doc: doc(""),
                globals: ExtendedGlobals::default(),
                subcommands: vec![],
                body: None,
            },
        ],
    }
}

#[test]
fn graft_accepts_root_with_body() {
    // A grafted subtree root may carry its own body (the child trait's
    // implicit-body method). With no parent globals, the body survives
    // unchanged.
    let graft = graft_subtree(
        leaf_tool_with_body(empty_body()),
        "t",
        ExtendedGlobals::default(),
        &[],
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert!(graft[0].body.is_some(), "grafted root keeps its body");
}

#[test]
fn graft_deprojects_child_body_against_parent_globals() {
    // The child root body re-declares `verbose` as a string option, which is
    // compatible with the parent global of the same name; it is de-projected
    // and the parent global is the single source of truth.
    let parent_globals = ExtendedGlobals {
        options: vec![scalar_opt("verbose", None)],
        flags: vec![],
    };
    let mut child_body = empty_body();
    child_body.options = vec![scalar_opt("verbose", None)];
    child_body.options[0].doc = doc("local verbose");
    let graft = graft_subtree(
        leaf_tool_with_body(child_body),
        "t",
        parent_globals,
        &[],
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let body = graft[0].body.as_ref().expect("body preserved");
    assert!(
        !body.options.iter().any(|o| o.long == "verbose"),
        "compatible body option is de-projected onto the parent global"
    );
    assert!(
        graft[0].globals.options.iter().any(|o| o.long == "verbose"),
        "parent global is prepended onto the grafted root globals"
    );
}

#[test]
fn graft_rejects_incompatible_child_body_vs_parent_global() {
    // The child root body declares `verbose` as a string option, but the
    // parent global is a bool flag of the same name: incompatible shapes
    // are an `InheritedGlobalConflict`, reported against the final grafted
    // command name.
    let parent_globals = ExtendedGlobals {
        options: vec![],
        flags: vec![bool_flag("verbose", None)],
    };
    let mut child_body = empty_body();
    child_body.options = vec![scalar_opt("verbose", None)];
    let err = graft_subtree(
        leaf_tool_with_body(child_body),
        "t",
        parent_globals,
        &[],
        None,
        None,
        None,
        None,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ToolBuildError::InheritedGlobalConflict { name, command, .. }
            if name == "verbose" && command == "t"
    ));
}

/// A single-root child whose root carries `globals` (no body). Used to
/// exercise graft-time reconciliation of the grafted root's own globals
/// against `parent_globals`.
fn leaf_tool_with_globals(globals: ExtendedGlobals) -> ExtendedToolType {
    ExtendedToolType {
        version: "0.1.0".to_string(),
        requires_filesystem: false,
        commands: vec![ExtendedCommandNode {
            name: "t".to_string(),
            aliases: vec![],
            doc: doc(""),
            globals,
            subcommands: vec![],
            body: None,
        }],
    }
}

#[test]
fn graft_deprojects_child_root_globals_against_parent_globals() {
    // The child root re-declares `verbose` as its own global (e.g. the
    // standalone child trait propagating it to its own descendants). It is
    // compatible with the parent global of the same name, so it is
    // de-projected and the prepended parent global is the single source.
    let parent_globals = ExtendedGlobals {
        options: vec![],
        flags: vec![bool_flag("verbose", None)],
    };
    let child_globals = ExtendedGlobals {
        options: vec![],
        flags: vec![bool_flag("verbose", None)],
    };
    let graft = graft_subtree(
        leaf_tool_with_globals(child_globals),
        "t",
        parent_globals,
        &[],
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let verbose_count = graft[0]
        .globals
        .flags
        .iter()
        .filter(|f| f.long == "verbose")
        .count();
    assert_eq!(
        verbose_count, 1,
        "compatible child-root global is de-projected; exactly one verbose global remains"
    );
}

#[test]
fn graft_rejects_incompatible_child_root_global_vs_parent_global() {
    // The child root global `verbose` (string option) is incompatible with
    // the parent global `verbose` (bool flag): `InheritedGlobalConflict`.
    let parent_globals = ExtendedGlobals {
        options: vec![],
        flags: vec![bool_flag("verbose", None)],
    };
    let child_globals = ExtendedGlobals {
        options: vec![scalar_opt("verbose", None)],
        flags: vec![],
    };
    let err = graft_subtree(
        leaf_tool_with_globals(child_globals),
        "t",
        parent_globals,
        &[],
        None,
        None,
        None,
        None,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        ToolBuildError::InheritedGlobalConflict { name, command, .. }
            if name == "verbose" && command == "t"
    ));
}

#[test]
fn graft_preserves_local_indices() {
    // The child root stays at index 0; its graft-local subcommand index (1)
    // is unchanged.
    let graft = graft_subtree(
        dispatcher_child(),
        "child",
        ExtendedGlobals::default(),
        &[],
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(graft.len(), 2);
    assert!(graft[0].body.is_none());
    assert_eq!(graft[0].subcommands, vec![1]);

    // Appending at offset N shifts the internal index to N + 1.
    let mut parent = vec![ExtendedCommandNode {
        name: "root".into(),
        aliases: vec![],
        doc: doc(""),
        globals: ExtendedGlobals::default(),
        subcommands: vec![],
        body: None,
    }];
    let offset = append_grafted_subtree(&mut parent, graft);
    assert_eq!(offset, 1);
    assert_eq!(parent[1].subcommands, vec![2]);
    assert_eq!(parent.len(), 3);
}

#[test]
fn graft_enforces_name_rule_and_rejects_annotations() {
    let mismatch = graft_subtree(
        dispatcher_child(),
        "remote",
        ExtendedGlobals::default(),
        &[],
        None,
        None,
        None,
        None,
    )
    .unwrap_err();
    assert!(matches!(
        mismatch,
        ToolBuildError::SubtreeRootNameMismatch { .. }
    ));

    // An explicit override name bypasses the match rule.
    let ok = graft_subtree(
        dispatcher_child(),
        "remote",
        ExtendedGlobals::default(),
        &[],
        Some("remote".into()),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(ok[0].name, "remote");

    let ann = graft_subtree(
        dispatcher_child(),
        "child",
        ExtendedGlobals::default(),
        &[],
        None,
        None,
        None,
        Some(CommandAnnotations {
            read_only: true,
            destructive: false,
            idempotent: false,
            open_world: false,
        }),
    )
    .unwrap_err();
    assert!(matches!(
        ann,
        ToolBuildError::SubtreeAnnotationsUnsupported(_)
    ));
}

#[test]
fn cycle_detection_and_refinements_work() {
    let mut ctx: ToolBuildCtx = ToolBuildCtx::new();
    ctx.push_descriptor("a").unwrap();
    assert!(matches!(
        ctx.push_descriptor("a"),
        Err(ToolBuildError::SubtreeCycle(_))
    ));
    let text = refine_text(SchemaType::string(), Some("x+".into()), Some(1), Some(3)).unwrap();
    assert!(matches!(text, SchemaType::Text { .. }));
    let url = refine_url(
        SchemaType::url(Default::default()),
        Some(vec!["https".into()]),
    )
    .unwrap();
    assert!(matches!(url, SchemaType::Url { .. }));
    let num = refine_numeric(
        SchemaType::u32(),
        Some(NumericBound::Unsigned(1)),
        None,
        Some("ms".into()),
    )
    .unwrap();
    assert_eq!(
        num.numeric_restrictions().unwrap().unit.as_deref(),
        Some("ms")
    );
    let path = refine_path(
        SchemaType::Path {
            spec: PathSpec {
                direction: PathDirection::Input,
                kind: PathKind::File,
                allowed_mime_types: None,
                allowed_extensions: None,
            },
            metadata: Default::default(),
        },
        None,
        None,
        None,
        Some(vec!["wasm".into(), "wat".into()]),
    )
    .unwrap();
    assert!(matches!(
        path,
        SchemaType::Path { spec, .. }
            if spec.allowed_extensions == Some(vec!["wasm".into(), "wat".into()])
    ));

    // Refinements reject schema kinds that cannot carry their restrictions
    // (the runtime backstop for macro-opaque types).
    assert!(matches!(
        refine_numeric(SchemaType::string(), None, None, Some("ms".into())),
        Err(ToolBuildError::RefinementTypeMismatch {
            refinement: "numeric",
            ..
        })
    ));
    assert!(matches!(
        refine_path(SchemaType::string(), None, None, None, None),
        Err(ToolBuildError::RefinementTypeMismatch {
            refinement: "path",
            ..
        })
    ));
    assert!(matches!(
        refine_url(SchemaType::string(), Some(vec!["https".into()])),
        Err(ToolBuildError::RefinementTypeMismatch {
            refinement: "url",
            ..
        })
    ));
    assert!(matches!(
        refine_text(SchemaType::u32(), Some("x+".into()), None, None),
        Err(ToolBuildError::RefinementTypeMismatch {
            refinement: "text",
            ..
        })
    ));
}

#[test]
fn refine_numeric_overlays_existing_restrictions() {
    // A type that already carries min + unit; refining only max must
    // preserve the existing min and unit instead of dropping them.
    let base = refine_numeric(
        SchemaType::u32(),
        Some(NumericBound::Unsigned(10)),
        None,
        Some("items".into()),
    )
    .unwrap();
    let refined = refine_numeric(base, None, Some(NumericBound::Unsigned(20)), None).unwrap();
    let r = refined.numeric_restrictions().unwrap();
    assert_eq!(r.min, Some(NumericBound::Unsigned(10)));
    assert_eq!(r.max, Some(NumericBound::Unsigned(20)));
    assert_eq!(r.unit.as_deref(), Some("items"));
}

#[test]
fn refine_numeric_preserves_unspecified_existing_restrictions() {
    let base = SchemaType::U32 {
        restrictions: NumericRestrictions {
            min: Some(NumericBound::Unsigned(10)),
            max: Some(NumericBound::Unsigned(100)),
            unit: Some("items".to_string()),
        }
        .normalize(),
        metadata: Default::default(),
    };

    let refined = refine_numeric(base, None, Some(NumericBound::Unsigned(200)), None).unwrap();

    let restrictions = refined.numeric_restrictions().unwrap();
    assert_eq!(restrictions.min, Some(NumericBound::Unsigned(10)));
    assert_eq!(restrictions.max, Some(NumericBound::Unsigned(200)));
    assert_eq!(restrictions.unit.as_deref(), Some("items"));
}

#[test]
fn numeric_value_validation_rejects_malformed_restrictions() {
    let ty = SchemaType::U32 {
        restrictions: Some(NumericRestrictions {
            min: Some(NumericBound::Unsigned(10)),
            max: Some(NumericBound::Unsigned(1)),
            unit: None,
        }),
        metadata: Default::default(),
    };
    let graph = SchemaGraph::anonymous(ty.clone());

    validate_value(&graph, &ty, &SchemaValue::U32(5))
        .expect_err("malformed numeric restrictions must not be ignored");
}

fn leaf_tool_with_body(body: ExtendedCommandBody) -> ExtendedToolType {
    ExtendedToolType {
        version: "0.1.0".to_string(),
        requires_filesystem: false,
        commands: vec![ExtendedCommandNode {
            name: "t".to_string(),
            aliases: vec![],
            doc: doc(""),
            globals: ExtendedGlobals::default(),
            subcommands: vec![],
            body: Some(body),
        }],
    }
}

fn empty_body() -> ExtendedCommandBody {
    ExtendedCommandBody {
        positionals: ExtendedPositionals::default(),
        options: vec![],
        flags: vec![],
        constraints: vec![],
        stdin: None,
        stdout: None,
        stderr: None,
        result: None,
        errors: vec![],
        annotations: None,
        positional_plan: vec![],
    }
}

fn map_config_option(constraints: Vec<ExtendedConstraint>) -> ExtendedCommandBody {
    let mut body = empty_body();
    body.options = vec![ExtendedOptionSpec {
        long: "config".to_string(),
        short: None,
        aliases: vec![],
        doc: doc(""),
        value_name: None,
        shape: ExtendedOptionShape::RepeatableMap(ExtendedRepeatableMapShape {
            repetition: wire::Repetition::Repeated,
            map_type: SchemaGraph::anonymous(SchemaType::map(
                SchemaType::string(),
                SchemaType::u32(),
            )),
            duplicate_key_policy: wire::DuplicateKeyPolicy::Reject,
        }),
        default: None,
        required: false,
        env_var: None,
    }];
    body.constraints = constraints;
    body
}

#[test]
fn default_type_mismatch_is_rejected() {
    let mut body = empty_body();
    body.positionals.fixed = vec![ExtendedPositional {
        name: "count".to_string(),
        doc: doc(""),
        value_name: None,
        type_: u32_graph(),
        default: Some(SchemaValue::String("not-a-number".to_string())),
        required: false,
        accepts_stdio: false,
    }];
    let err = leaf_tool_with_body(body).try_to_native_tool().unwrap_err();
    assert!(matches!(err, ToolBuildError::DefaultTypeMismatch(_)));
}

#[test]
fn value_is_resolves_to_map_value_type() {
    // A `value-is` over a repeatable-map names the map's value type (u32).
    let ok = leaf_tool_with_body(map_config_option(vec![ExtendedConstraint::RequiresAll(
        vec![ExtendedRef::ValueIs(ExtendedValueIsRef {
            name: "config".to_string(),
            value: ExtendedValueIsLiteral::Resolved(SchemaValue::U32(1)),
        })],
    )]));
    assert!(ok.try_to_native_tool().is_ok());

    let bad = leaf_tool_with_body(map_config_option(vec![ExtendedConstraint::RequiresAll(
        vec![ExtendedRef::ValueIs(ExtendedValueIsRef {
            name: "config".to_string(),
            value: ExtendedValueIsLiteral::Resolved(SchemaValue::String("x".to_string())),
        })],
    )]));
    assert!(matches!(
        bad.try_to_native_tool().unwrap_err(),
        ToolBuildError::ValueIsTypeMismatch(_)
    ));
}

#[test]
fn unresolved_constraint_ref_is_rejected() {
    let body = map_config_option(vec![ExtendedConstraint::RequiresAll(vec![
        ExtendedRef::Present("missing".to_string()),
    ])]);
    assert!(matches!(
        leaf_tool_with_body(body).try_to_native_tool().unwrap_err(),
        ToolBuildError::UnresolvedConstraintRef(_)
    ));
}

/// Builds a parent-dispatcher + child-leaf tool where the child's body carries
/// `constraints` referencing names that only exist on the parent's globals (a
/// subtree-style composition). The parent declares the given `parent_globals`.
fn deferred_value_is_tree(
    parent_globals: ExtendedGlobals,
    constraints: Vec<ExtendedConstraint>,
) -> ExtendedToolType {
    let mut child_body = empty_body();
    child_body.constraints = constraints;
    tool_with_nodes(vec![
        ExtendedCommandNode {
            name: "root".into(),
            aliases: vec![],
            doc: doc(""),
            globals: parent_globals,
            subcommands: vec![1],
            body: None,
        },
        ExtendedCommandNode {
            name: "leaf".into(),
            aliases: vec![],
            doc: doc(""),
            globals: ExtendedGlobals::default(),
            subcommands: vec![],
            body: Some(child_body),
        },
    ])
}

fn first_value_is(body: &ExtendedCommandBody) -> &ExtendedValueIsRef {
    match &body.constraints[0] {
        ExtendedConstraint::RequiresAll(refs) => match &refs[0] {
            ExtendedRef::ValueIs(v) => v,
            other => panic!("expected a value-is ref, got {other:?}"),
        },
        other => panic!("expected a requires-all constraint, got {other:?}"),
    }
}

#[test]
fn deferred_value_is_resolves_against_ancestor_global() {
    let mut tool = deferred_value_is_tree(
        ExtendedGlobals {
            options: vec![scalar_opt("format", None)],
            flags: vec![],
        },
        vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
            ExtendedValueIsRef {
                name: "format".into(),
                value: ExtendedValueIsLiteral::Deferred(ToolLiteral::Str("json".into())),
            },
        )])],
    );
    normalize_inherited_globals(&mut tool)
        .expect("composition should resolve the deferred literal against the parent global");
    let resolved = first_value_is(tool.commands[1].body.as_ref().unwrap());
    assert!(
        matches!(&resolved.value, ExtendedValueIsLiteral::Resolved(SchemaValue::String(s)) if s == "json"),
        "deferred literal should be resolved to a string, got {:?}",
        resolved.value
    );
    assert!(tool.try_to_native_tool().is_ok());
}

#[test]
fn deferred_value_is_unknown_name_is_rejected() {
    let mut tool = deferred_value_is_tree(
        ExtendedGlobals::default(),
        vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
            ExtendedValueIsRef {
                name: "missing".into(),
                value: ExtendedValueIsLiteral::Deferred(ToolLiteral::Str("json".into())),
            },
        )])],
    );
    // Normalization leaves a name absent from every scope deferred; the
    // dangling reference is reported by validation.
    normalize_inherited_globals(&mut tool).expect("normalization tolerates unknown names");
    assert!(matches!(
        tool.try_to_native_tool().unwrap_err(),
        ToolBuildError::UnresolvedConstraintRef(name) if name == "missing"
    ));
}

#[test]
fn deferred_value_is_incompatible_literal_is_rejected() {
    let mut tool = deferred_value_is_tree(
        ExtendedGlobals {
            options: vec![scalar_opt("format", None)],
            flags: vec![],
        },
        vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
            ExtendedValueIsRef {
                name: "format".into(),
                value: ExtendedValueIsLiteral::Deferred(ToolLiteral::Bool(true)),
            },
        )])],
    );
    assert!(matches!(
        normalize_inherited_globals(&mut tool).unwrap_err(),
        ToolBuildError::ValueIsTypeMismatch(name) if name == "format"
    ));
}

#[test]
fn deferred_value_is_against_ancestor_flag_is_rejected() {
    let mut tool = deferred_value_is_tree(
        ExtendedGlobals {
            options: vec![],
            flags: vec![bool_flag("force", None)],
        },
        vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
            ExtendedValueIsRef {
                name: "force".into(),
                value: ExtendedValueIsLiteral::Deferred(ToolLiteral::Bool(true)),
            },
        )])],
    );
    assert!(matches!(
        normalize_inherited_globals(&mut tool).unwrap_err(),
        ToolBuildError::ValueIsTypeMismatch(name) if name == "force"
    ));
}

#[test]
fn deferred_value_is_unresolved_at_validation_is_rejected() {
    // A deferred literal whose name *is* in scope but was never resolved
    // (composition skipped) is a resolution gap, not a silently accepted ref.
    let mut body = empty_body();
    body.options = vec![scalar_opt("format", None)];
    body.constraints = vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
        ExtendedValueIsRef {
            name: "format".into(),
            value: ExtendedValueIsLiteral::Deferred(ToolLiteral::Str("json".into())),
        },
    )])];
    assert!(matches!(
        validate_tool(&leaf_tool_with_body(body)),
        Err(ToolBuildError::UnresolvedValueIsLiteral(name)) if name == "format"
    ));
}

fn bare_node(name: &str, subcommands: Vec<i32>) -> ExtendedCommandNode {
    ExtendedCommandNode {
        name: name.into(),
        aliases: vec![],
        doc: doc(""),
        globals: ExtendedGlobals::default(),
        subcommands,
        body: None,
    }
}

fn tool_with_nodes(nodes: Vec<ExtendedCommandNode>) -> ExtendedToolType {
    ExtendedToolType {
        version: "0.1.0".into(),
        requires_filesystem: false,
        commands: nodes,
    }
}

fn scalar_opt(long: &str, short: Option<char>) -> ExtendedOptionSpec {
    ExtendedOptionSpec {
        long: long.into(),
        short,
        aliases: vec![],
        doc: doc(""),
        value_name: None,
        shape: ExtendedOptionShape::Scalar(str_graph()),
        default: None,
        required: false,
        env_var: None,
    }
}

fn bool_flag(long: &str, short: Option<char>) -> FlagSpec {
    FlagSpec {
        long: long.into(),
        short,
        aliases: vec![],
        doc: doc(""),
        shape: wire::FlagShape::BoolFlag(wire::BoolFlagShape {
            default: false,
            negatable: false,
        }),
        env_var: None,
    }
}

fn variant_graph() -> SchemaGraph {
    // A single well-formed case so the graph passes well-formedness and the
    // tool-specific "variant in input position" rule is what rejects it.
    SchemaGraph::anonymous(SchemaType::Variant {
        cases: vec![golem_schema::schema::schema_type::VariantCaseType {
            name: "case".into(),
            payload: None,
            metadata: Default::default(),
        }],
        metadata: Default::default(),
    })
}

#[test]
fn empty_tree_is_rejected() {
    assert!(matches!(
        validate_tool(&tool_with_nodes(vec![])),
        Err(ToolBuildError::EmptyCommandTree)
    ));
}

#[test]
fn out_of_bounds_subcommand_is_rejected() {
    assert!(matches!(
        validate_tool(&tool_with_nodes(vec![bare_node("root", vec![5])])),
        Err(ToolBuildError::CommandIndexOutOfBounds { .. })
    ));
    assert!(matches!(
        validate_tool(&tool_with_nodes(vec![bare_node("root", vec![-1])])),
        Err(ToolBuildError::CommandIndexOutOfBounds { .. })
    ));
}

#[test]
fn canonical_input_model_rejects_invalid_subcommand_index_in_reachable_tree() {
    let tool = tool_with_nodes(vec![
        bare_node("root", vec![1, -1]),
        bare_node("child", vec![]),
    ]);

    assert!(matches!(
        validate_tool(&tool),
        Err(ToolBuildError::CommandIndexOutOfBounds { index: -1, len: 2 })
    ));
    assert!(matches!(
        tool.canonical_input_model(1),
        Err(ToolBuildError::CommandIndexOutOfBounds { index: -1, len: 2 })
    ));
    assert!(matches!(
        tool.canonical_input_record_schema(1),
        Err(ToolBuildError::CommandIndexOutOfBounds { index: -1, len: 2 })
    ));
    assert!(matches!(
        tool.decode_canonical_input_record(1, SchemaValue::Record { fields: vec![] }),
        Err(CanonicalInputDecodeError::Model(
            ToolBuildError::CommandIndexOutOfBounds { index: -1, len: 2 }
        ))
    ));
}

#[test]
fn cyclic_tree_is_rejected_without_panicking() {
    let tool = tool_with_nodes(vec![
        bare_node("root", vec![1]),
        bare_node("child", vec![0]),
    ]);
    assert!(matches!(
        validate_tool(&tool),
        Err(ToolBuildError::CommandTreeCycle(_))
    ));
    // Helpers must not panic / infinitely recurse on the malformed tree.
    assert!(render_help(&tool, &[]).is_ok());
    let _ = tool.effective_globals(1);
}

#[test]
fn canonical_input_model_rejects_cycle_on_target_path() {
    let tool = tool_with_nodes(vec![
        bare_node("root", vec![1]),
        bare_node("child", vec![0]),
    ]);

    assert!(matches!(
        tool.canonical_input_model(1),
        Err(ToolBuildError::CommandTreeCycle(_))
    ));
    assert!(matches!(
        tool.decode_canonical_input_record(1, SchemaValue::Record { fields: vec![] }),
        Err(CanonicalInputDecodeError::Model(
            ToolBuildError::CommandTreeCycle(_)
        ))
    ));
}

#[test]
fn shared_subcommand_is_rejected() {
    let tool = tool_with_nodes(vec![
        bare_node("root", vec![1, 2]),
        bare_node("a", vec![3]),
        bare_node("b", vec![3]),
        bare_node("leaf", vec![]),
    ]);
    assert!(matches!(
        validate_tool(&tool),
        Err(ToolBuildError::DuplicateCommandParent(3))
    ));
}

#[test]
fn canonical_input_model_rejects_duplicate_parent_command_path() {
    let mut a = bare_node("a", vec![3]);
    a.globals.options = vec![scalar_opt("from-a", None)];
    let mut b = bare_node("b", vec![3]);
    b.globals.options = vec![scalar_opt("from-b", None)];
    let tool = tool_with_nodes(vec![
        bare_node("root", vec![1, 2]),
        a,
        b,
        bare_node("leaf", vec![]),
    ]);

    assert!(matches!(
        validate_tool(&tool),
        Err(ToolBuildError::DuplicateCommandParent(3))
    ));
    assert!(matches!(
        tool.canonical_input_model(3),
        Err(ToolBuildError::DuplicateCommandParent(3))
    ));
}

#[test]
fn unreachable_node_is_rejected() {
    let tool = tool_with_nodes(vec![bare_node("root", vec![]), bare_node("orphan", vec![])]);
    assert!(matches!(
        validate_tool(&tool),
        Err(ToolBuildError::UnreachableCommandNode(1))
    ));
}

#[test]
fn invalid_identifier_is_rejected() {
    assert!(matches!(
        validate_tool(&tool_with_nodes(vec![bare_node("Root", vec![])])),
        Err(ToolBuildError::InvalidIdentifier { .. })
    ));
}

#[test]
fn body_name_colliding_with_inherited_global_is_rejected() {
    let mut root = bare_node("root", vec![1]);
    root.globals.options = vec![scalar_opt("shared", None)];
    let mut child = bare_node("child", vec![]);
    let mut body = empty_body();
    body.options = vec![scalar_opt("shared", None)];
    child.body = Some(body);
    assert!(matches!(
        validate_tool(&tool_with_nodes(vec![root, child])),
        Err(ToolBuildError::DuplicateName(_))
    ));
}

#[test]
fn duplicate_short_form_is_rejected() {
    let mut body = empty_body();
    body.options = vec![
        scalar_opt("alpha", Some('a')),
        scalar_opt("beta", Some('a')),
    ];
    assert!(matches!(
        validate_tool(&leaf_tool_with_body(body)),
        Err(ToolBuildError::DuplicateShort('a'))
    ));
}

#[test]
fn verbatim_tail_without_separator_is_rejected() {
    let mut body = empty_body();
    body.positionals.tail = Some(ExtendedTailPositional {
        name: "args".into(),
        doc: doc(""),
        value_name: None,
        item_type: str_graph(),
        min: 0,
        max: None,
        separator: None,
        verbatim: true,
        accepts_stdio: false,
    });
    assert!(matches!(
        validate_tool(&leaf_tool_with_body(body)),
        Err(ToolBuildError::VerbatimWithoutSeparator(_))
    ));
}

#[test]
fn variant_in_input_position_is_rejected() {
    let mut body = empty_body();
    body.positionals.fixed = vec![ExtendedPositional {
        name: "choice".into(),
        doc: doc(""),
        value_name: None,
        type_: variant_graph(),
        default: None,
        required: true,
        accepts_stdio: false,
    }];
    assert!(matches!(
        validate_tool(&leaf_tool_with_body(body)),
        Err(ToolBuildError::VariantInInputPosition(_))
    ));
}

#[test]
fn value_is_against_flag_is_rejected() {
    let mut body = empty_body();
    body.flags = vec![bool_flag("force", None)];
    body.constraints = vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
        ExtendedValueIsRef {
            name: "force".into(),
            value: ExtendedValueIsLiteral::Resolved(SchemaValue::Bool(true)),
        },
    )])];
    assert!(matches!(
        validate_tool(&leaf_tool_with_body(body)),
        Err(ToolBuildError::ValueIsTypeMismatch(_))
    ));
}

#[test]
fn repeatable_map_with_non_map_type_is_rejected() {
    let mut body = empty_body();
    body.options = vec![ExtendedOptionSpec {
        long: "config".into(),
        short: None,
        aliases: vec![],
        doc: doc(""),
        value_name: None,
        shape: ExtendedOptionShape::RepeatableMap(ExtendedRepeatableMapShape {
            repetition: wire::Repetition::Repeated,
            map_type: str_graph(),
            duplicate_key_policy: wire::DuplicateKeyPolicy::Reject,
        }),
        default: None,
        required: false,
        env_var: None,
    }];
    assert!(matches!(
        validate_tool(&leaf_tool_with_body(body)),
        Err(ToolBuildError::RepeatableMapTypeNotMap(_))
    ));
}

#[test]
fn deferred_value_is_does_not_mask_repeatable_map_type_error() {
    let mut body = empty_body();
    body.options = vec![ExtendedOptionSpec {
        long: "config".into(),
        short: None,
        aliases: vec![],
        doc: doc(""),
        value_name: None,
        shape: ExtendedOptionShape::RepeatableMap(ExtendedRepeatableMapShape {
            repetition: wire::Repetition::Repeated,
            map_type: str_graph(),
            duplicate_key_policy: wire::DuplicateKeyPolicy::Reject,
        }),
        default: None,
        required: false,
        env_var: None,
    }];
    body.constraints = vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
        ExtendedValueIsRef {
            name: "config".into(),
            value: ExtendedValueIsLiteral::Deferred(ToolLiteral::Int(1)),
        },
    )])];

    let mut tool = leaf_tool_with_body(body);
    normalize_inherited_globals(&mut tool).expect(
        "normalization must not report a value-is mismatch before validation can report the malformed repeatable-map type",
    );
    assert!(matches!(
        validate_tool(&tool),
        Err(ToolBuildError::RepeatableMapTypeNotMap(name)) if name == "config"
    ));
}

fn ref_graph(id: &str) -> SchemaGraph {
    SchemaGraph::anonymous(SchemaType::Ref {
        id: id.into(),
        metadata: Default::default(),
    })
}

#[test]
fn dangling_type_ref_in_positional_is_rejected() {
    let mut body = empty_body();
    body.positionals.fixed = vec![ExtendedPositional {
        name: "thing".into(),
        doc: doc(""),
        value_name: None,
        type_: ref_graph("missing-type"),
        default: None,
        required: true,
        accepts_stdio: false,
    }];
    assert!(matches!(
        validate_tool(&leaf_tool_with_body(body)),
        Err(ToolBuildError::UnresolvedTypeRef { id, .. }) if id == "missing-type"
    ));
}

#[test]
fn dangling_type_ref_in_option_is_rejected() {
    let mut body = empty_body();
    body.options = vec![scalar_opt("name", None)];
    body.options[0].shape = ExtendedOptionShape::Scalar(ref_graph("nope"));
    assert!(matches!(
        validate_tool(&leaf_tool_with_body(body)),
        Err(ToolBuildError::UnresolvedTypeRef { position, id })
            if position == "option --name" && id == "nope"
    ));
}

#[test]
fn deferred_value_is_does_not_mask_dangling_option_type_ref() {
    let mut body = empty_body();
    body.options = vec![scalar_opt("name", None)];
    body.options[0].shape = ExtendedOptionShape::Scalar(ref_graph("nope"));
    body.constraints = vec![ExtendedConstraint::RequiresAll(vec![ExtendedRef::ValueIs(
        ExtendedValueIsRef {
            name: "name".into(),
            value: ExtendedValueIsLiteral::Deferred(ToolLiteral::Str("x".into())),
        },
    )])];

    let mut tool = leaf_tool_with_body(body);
    normalize_inherited_globals(&mut tool).expect(
        "normalization must not report a value-is mismatch before schema validation can report the dangling type ref",
    );
    assert!(matches!(
        validate_tool(&tool),
        Err(ToolBuildError::UnresolvedTypeRef { position, id })
            if position == "option --name" && id == "nope"
    ));
}

#[test]
fn dangling_type_ref_in_definition_body_is_rejected() {
    // The root resolves, but a definition body references a missing id.
    let graph = SchemaGraph {
        defs: vec![golem_schema::schema::SchemaTypeDef {
            id: "rec".into(),
            name: Some("rec".into()),
            body: SchemaType::List {
                element: Box::new(SchemaType::Ref {
                    id: "gone".into(),
                    metadata: Default::default(),
                }),
                metadata: Default::default(),
            },
        }],
        root: SchemaType::Ref {
            id: "rec".into(),
            metadata: Default::default(),
        },
    };
    let mut body = empty_body();
    body.positionals.fixed = vec![ExtendedPositional {
        name: "thing".into(),
        doc: doc(""),
        value_name: None,
        type_: graph,
        default: None,
        required: true,
        accepts_stdio: false,
    }];
    assert!(matches!(
        validate_tool(&leaf_tool_with_body(body)),
        Err(ToolBuildError::UnresolvedTypeRef { id, .. }) if id == "gone"
    ));
}

#[test]
fn ill_formed_numeric_restriction_is_rejected() {
    // A u32 positional whose inline restrictions are unsatisfiable
    // (min > max) must be rejected at tool-build time by well-formedness.
    let mut body = empty_body();
    body.positionals.fixed = vec![ExtendedPositional {
        name: "count".into(),
        doc: doc(""),
        value_name: None,
        type_: SchemaGraph::anonymous(SchemaType::U32 {
            restrictions: Some(golem_schema::schema::schema_type::NumericRestrictions {
                min: Some(NumericBound::Unsigned(10)),
                max: Some(NumericBound::Unsigned(1)),
                unit: None,
            }),
            metadata: Default::default(),
        }),
        default: None,
        required: true,
        accepts_stdio: false,
    }];
    assert!(matches!(
        validate_tool(&leaf_tool_with_body(body)),
        Err(ToolBuildError::IllFormedSchema { position, .. }) if position == "positional count"
    ));
}

#[test]
fn resolvable_type_ref_is_accepted() {
    // A self-contained graph whose root Ref resolves within its own defs
    // must pass the closedness check.
    let graph = SchemaGraph {
        defs: vec![golem_schema::schema::SchemaTypeDef {
            id: "rec".into(),
            name: Some("rec".into()),
            body: SchemaType::string(),
        }],
        root: SchemaType::Ref {
            id: "rec".into(),
            metadata: Default::default(),
        },
    };
    let mut body = empty_body();
    body.positionals.fixed = vec![ExtendedPositional {
        name: "thing".into(),
        doc: doc(""),
        value_name: None,
        type_: graph,
        default: None,
        required: true,
        accepts_stdio: false,
    }];
    assert!(validate_tool(&leaf_tool_with_body(body)).is_ok());
}

#[test]
fn argument_help_finds_global_and_body_args() {
    let tool = sample_tool();
    // `verbose` is a root global visible at the `run` subcommand.
    let global = render_argument_help(&tool, &["run".to_string()], "verbose").unwrap();
    assert!(global.contains("--verbose"));
    assert!(global.contains("global"));
    // `config` is a body option on `run`.
    let body = render_argument_help(&tool, &["run".to_string()], "config").unwrap();
    assert!(body.contains("--config"));
    // Unknown argument is an error, not a panic.
    assert!(matches!(
        render_argument_help(&tool, &["run".to_string()], "nope"),
        Err(ToolBuildError::CommandNotFound(_))
    ));
}

fn shapes_match(a: SchemaType, b: SchemaType) -> bool {
    schema_shapes_match(&SchemaGraph::anonymous(a), &SchemaGraph::anonymous(b))
}

fn quantity(base_unit: &str, allowed_suffixes: &[&str]) -> SchemaType {
    SchemaType::quantity(QuantitySpec {
        base_unit: base_unit.to_string(),
        allowed_suffixes: allowed_suffixes.iter().map(|s| s.to_string()).collect(),
        min: None,
        max: None,
    })
}

#[test]
fn rich_leaf_quantity_identity_is_compared_but_bounds_ignored() {
    // Different base unit: not interchangeable.
    assert!(!shapes_match(quantity("m", &[]), quantity("s", &[])));
    // Same base unit, different accepted unit sets: not interchangeable.
    assert!(!shapes_match(
        quantity("m", &["m", "km"]),
        quantity("m", &["m"])
    ));
    // Same identity, suffix order irrelevant.
    assert!(shapes_match(
        quantity("m", &["m", "km"]),
        quantity("m", &["km", "m"])
    ));
    // Bounds (min/max) are validation restrictions and stay ignored.
    let lo = SchemaType::quantity(QuantitySpec {
        base_unit: "m".to_string(),
        allowed_suffixes: vec![],
        min: None,
        max: None,
    });
    let hi = SchemaType::quantity(QuantitySpec {
        base_unit: "m".to_string(),
        allowed_suffixes: vec![],
        min: Some(golem_schema::schema::QuantityValue {
            mantissa: 0,
            scale: 0,
            unit: "m".to_string(),
        }),
        max: None,
    });
    assert!(shapes_match(lo, hi));
}

#[test]
fn rich_leaf_capability_identity_is_compared() {
    assert!(shapes_match(
        SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::string()),
            category: Some("api-key".into()),
        }),
        SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::string()),
            category: Some("api-key".into()),
        }),
    ));
    assert!(!shapes_match(
        SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::string()),
            category: Some("api-key".into()),
        }),
        SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::string()),
            category: Some("oauth-token".into()),
        }),
    ));
    assert!(!shapes_match(
        SchemaType::quota_token(QuotaTokenSpec {
            resource_name: Some("tokens".into())
        }),
        SchemaType::quota_token(QuotaTokenSpec {
            resource_name: Some("requests".into())
        }),
    ));
    assert!(shapes_match(
        SchemaType::permission_card(PermissionCardSpec { polymorphic: true }),
        SchemaType::permission_card(PermissionCardSpec { polymorphic: true }),
    ));
    assert!(!shapes_match(
        SchemaType::permission_card(PermissionCardSpec { polymorphic: true }),
        SchemaType::permission_card(PermissionCardSpec { polymorphic: false }),
    ));
}

#[test]
fn rich_leaf_secret_inner_identity_is_compared() {
    assert!(!shapes_match(
        SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::string()),
            category: Some("api-key".into()),
        }),
        SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::u64()),
            category: Some("api-key".into()),
        }),
    ));
}

#[test]
fn rich_leaf_text_and_binary_identity_is_compared_but_other_bounds_ignored() {
    let text = |languages: Option<&[&str]>, regex: Option<&str>| {
        SchemaType::text(TextRestrictions {
            languages: languages.map(|ls| ls.iter().map(|s| s.to_string()).collect()),
            min_length: None,
            max_length: None,
            regex: regex.map(|s| s.to_string()),
        })
    };
    // Language set is type identity.
    assert!(!shapes_match(
        text(Some(&["en"]), None),
        text(Some(&["de"]), None)
    ));
    assert!(!shapes_match(text(None, None), text(Some(&["en"]), None)));
    // Regex is a refinable restriction and stays ignored.
    assert!(shapes_match(
        text(Some(&["en"]), Some("a+")),
        text(Some(&["en"]), Some("b+"))
    ));

    let binary = |mime: Option<&[&str]>, max_bytes: Option<u32>| {
        SchemaType::binary(BinaryRestrictions {
            mime_types: mime.map(|ms| ms.iter().map(|s| s.to_string()).collect()),
            min_bytes: None,
            max_bytes,
        })
    };
    assert!(!shapes_match(
        binary(Some(&["image/png"]), None),
        binary(Some(&["image/jpeg"]), None)
    ));
    // Byte bounds are refinable restrictions and stay ignored.
    assert!(shapes_match(
        binary(Some(&["image/png"]), Some(10)),
        binary(Some(&["image/png"]), Some(20))
    ));
}

#[test]
fn rich_leaf_path_direction_and_kind_are_refinable_and_ignored() {
    let path = |direction: PathDirection, kind: PathKind| {
        SchemaType::path(PathSpec {
            direction,
            kind,
            allowed_mime_types: None,
            allowed_extensions: None,
        })
    };
    // direction/kind are `#[arg]`-refinable, so a leaf may re-specify them;
    // de-projection still treats them as the same inherited global.
    assert!(shapes_match(
        path(PathDirection::Input, PathKind::File),
        path(PathDirection::Output, PathKind::Directory)
    ));
}

#[test]
fn plain_string_matches_unrestricted_text_but_not_language_restricted() {
    let unrestricted = SchemaType::text(TextRestrictions {
        languages: None,
        min_length: None,
        max_length: None,
        regex: Some("^x$".to_string()),
    });
    let language_restricted = SchemaType::text(TextRestrictions {
        languages: Some(vec!["en".to_string()]),
        min_length: None,
        max_length: None,
        regex: None,
    });
    // A leaf `String` de-projects onto an inherited refined-`String` (`Text`)
    // global; the regex is a refinable restriction and is ignored.
    assert!(shapes_match(SchemaType::string(), unrestricted.clone()));
    assert!(shapes_match(unrestricted, SchemaType::string()));
    // A language-restricted `Text` is a different Rust type and must conflict.
    assert!(!shapes_match(SchemaType::string(), language_restricted));
}
