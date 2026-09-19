use super::*;
use crate::schema::metadata::MetadataEnvelope;
use crate::schema::schema_type::NamedFieldType;
use crate::schema::tool::{
    BoolFlagShape, CommandAnnotations, CommandIndex, CommandNode, CommandTree, Doc, ErrorKind,
    FlagShape, FlagSpec, Globals, OptionShape, OptionSpec, Positional, Positionals, StreamSpec,
};

test_r::enable!();
use test_r::test;

fn tool(result: SchemaType) -> Tool {
    Tool {
        version: "1".into(),
        schema: SchemaGraph::empty(),
        commands: CommandTree {
            nodes: vec![CommandNode {
                name: "demo".into(),
                aliases: vec![],
                doc: Default::default(),
                globals: Globals::default(),
                subcommands: vec![],
                body: Some(CommandBody {
                    positionals: Positionals::default(),
                    options: vec![],
                    flags: vec![],
                    constraints: vec![],
                    stdin: None,
                    stdout: None,
                    result: Some(super::super::ResultSpec {
                        type_: result,
                        doc: Default::default(),
                        formatters: vec![super::super::Formatter {
                            name: "json".into(),
                            doc: Default::default(),
                        }],
                        default_formatter: "json".into(),
                    }),
                    errors: vec![],
                    annotations: None,
                }),
            }],
        },
    }
}

fn record(names: &[&str]) -> SchemaType {
    SchemaType::record(
        names
            .iter()
            .map(|name| NamedFieldType {
                name: (*name).into(),
                body: SchemaType::string(),
                metadata: MetadataEnvelope::default(),
            })
            .collect(),
    )
}

fn positional(name: &str, type_: SchemaType) -> Positional {
    Positional {
        name: name.into(),
        doc: Default::default(),
        value_name: None,
        type_,
        default: None,
        required: true,
        accepts_stdio: false,
    }
}

fn option(name: &str, type_: SchemaType, default: Option<SchemaValue>) -> OptionSpec {
    OptionSpec {
        long: name.into(),
        short: None,
        aliases: vec![],
        doc: Default::default(),
        value_name: None,
        shape: OptionShape::Scalar(type_),
        default,
        required: false,
        env_var: None,
    }
}

fn flag(name: &str) -> FlagSpec {
    FlagSpec {
        long: name.into(),
        short: None,
        aliases: vec![],
        doc: Default::default(),
        shape: FlagShape::BoolFlag(BoolFlagShape {
            default: false,
            negatable: false,
        }),
        env_var: None,
    }
}

fn body(tool: &mut Tool, index: usize) -> &mut CommandBody {
    tool.commands.nodes[index].body.as_mut().unwrap()
}

#[test]
fn result_record_projection_reorders_positional_values_by_name() {
    let expected = tool(record(&["first", "second"]));
    let inner = tool(record(&["second", "first", "discarded"]));
    let compiled =
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
            .unwrap();
    let plan = compiled.commands[0].result.as_ref().unwrap();
    let ProjectionNode::Record { fields, discard } = &plan.nodes[plan.root] else {
        panic!("record plan expected")
    };
    assert_eq!(
        fields.iter().map(|f| f.source_index).collect::<Vec<_>>(),
        vec![Some(1), Some(0)]
    );
    assert_eq!(discard, &vec![2]);
}

#[test]
fn strict_equality_ignores_named_definition_allocation_ids() {
    let expected = tool(record(&["value"]));
    let inner = tool(record(&["value"]));
    assert!(
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StrictEquality)
            .is_ok()
    );
}

fn recursive_tool(first: &str, second: &str, reverse_defs: bool) -> Tool {
    let first_def = SchemaTypeDef {
        id: TypeId::new(first),
        name: Some("First".into()),
        body: SchemaType::record(vec![NamedFieldType {
            name: "next".into(),
            body: SchemaType::option(SchemaType::ref_to(TypeId::new(second))),
            metadata: Default::default(),
        }]),
    };
    let second_def = SchemaTypeDef {
        id: TypeId::new(second),
        name: Some("Second".into()),
        body: SchemaType::record(vec![NamedFieldType {
            name: "next".into(),
            body: SchemaType::option(SchemaType::ref_to(TypeId::new(first))),
            metadata: Default::default(),
        }]),
    };
    let mut result = tool(SchemaType::ref_to(TypeId::new(first)));
    result.schema.defs = if reverse_defs {
        vec![second_def, first_def]
    } else {
        vec![first_def, second_def]
    };
    result
}

#[test]
fn strict_equality_canonicalizes_reordered_alpha_renamed_recursive_defs() {
    let expected = recursive_tool("left-a", "left-b", false);
    let inner = recursive_tool("right-a", "right-b", true);
    assert!(strictly_equal(&expected, &inner));
}

#[test]
fn strict_equality_canonicalizes_reordered_identical_unused_defs() {
    fn with_unused_defs(first: (&str, &str), second: (&str, &str)) -> Tool {
        let mut result = tool(SchemaType::string());
        result.schema.defs = vec![first, second]
            .into_iter()
            .map(|(id, name)| SchemaTypeDef {
                id: TypeId::new(id),
                name: Some(name.into()),
                body: SchemaType::string(),
            })
            .collect();
        result
    }

    for _ in 0..64 {
        let expected = with_unused_defs(("left-a", "A"), ("left-b", "B"));
        let inner = with_unused_defs(("right-b", "B"), ("right-a", "A"));
        assert!(strictly_equal(&expected, &inner));
    }
}

#[test]
fn strict_equality_ignores_docs_but_preserves_default_literal_strings_matching_ids() {
    let mut expected = recursive_tool("literal-id", "other-id", false);
    expected.commands.nodes[0].doc.summary = "literal-id".into();
    expected.commands.nodes[0]
        .body
        .as_mut()
        .unwrap()
        .positionals
        .fixed
        .push(Positional {
            name: "input".into(),
            doc: Default::default(),
            value_name: None,
            type_: SchemaType::string(),
            default: Some(SchemaValue::String("literal-id".into())),
            required: false,
            accepts_stdio: false,
        });

    let mut inner = recursive_tool("$def0", "$def1", false);
    inner.commands.nodes[0].doc.summary = "$def0".into();
    inner.commands.nodes[0].body.as_mut().unwrap().positionals = expected.commands.nodes[0]
        .body
        .as_ref()
        .unwrap()
        .positionals
        .clone();
    inner.commands.nodes[0]
        .body
        .as_mut()
        .unwrap()
        .positionals
        .fixed[0]
        .default = Some(SchemaValue::String("$def0".into()));

    assert!(!strictly_equal(&expected, &inner));
    inner.commands.nodes[0].doc.summary = expected.commands.nodes[0].doc.summary.clone();
    assert!(!strictly_equal(&expected, &inner));
    inner.commands.nodes[0]
        .body
        .as_mut()
        .unwrap()
        .positionals
        .fixed[0]
        .default = expected.commands.nodes[0]
        .body
        .as_ref()
        .unwrap()
        .positionals
        .fixed[0]
        .default
        .clone();
    inner.commands.nodes[0].doc.summary = "$def0".into();
    assert!(strictly_equal(&expected, &inner));
}

#[test]
fn strict_equality_does_not_ignore_other_descriptor_fields() {
    let expected = tool(SchemaType::string());
    let mut inner = expected.clone();
    inner.commands.nodes[0].body.as_mut().unwrap().annotations = Some(CommandAnnotations {
        read_only: true,
        destructive: false,
        idempotent: false,
        open_world: false,
    });
    assert!(!strictly_equal(&expected, &inner));
}

#[test]
fn nominal_emits_checked_dynamic_plans() {
    let expected = tool(record(&["left"]));
    let inner = tool(record(&["right"]));
    let compiled =
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::Nominal).unwrap();
    assert_eq!(compiled.commands.len(), 1);
    let result = compiled.commands[0].result.as_ref().unwrap();
    assert_eq!(result.nodes, vec![ProjectionNode::DynamicChecked]);
}

#[test]
fn nominal_matches_named_command_structure_and_rejects_a_different_name() {
    let expected = tool(record(&["left"]));
    let mut unrelated = tool(SchemaType::bool());
    unrelated.version = "anything".into();
    assert!(
        compile_tool_compatibility(&expected, &unrelated, ToolCompatibilityMode::Nominal).is_ok()
    );

    unrelated.commands.nodes[0].name = "other".into();
    assert!(
        compile_tool_compatibility(&expected, &unrelated, ToolCompatibilityMode::Nominal).is_err()
    );
}

#[test]
fn strict_equality_ignores_version_and_documentation() {
    let expected = tool(SchemaType::string());
    let mut inner = expected.clone();
    inner.version = "2".into();
    assert!(
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StrictEquality)
            .is_ok()
    );
    inner = expected.clone();
    inner.commands.nodes[0].doc.summary = "changed".into();
    assert!(
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StrictEquality)
            .is_ok()
    );
}

#[test]
fn strict_equality_ignores_result_type_and_record_field_documentation() {
    let expected = tool(record(&["value"]));
    let mut inner = expected.clone();
    let result_type = &mut body(&mut inner, 0).result.as_mut().unwrap().type_;
    result_type.metadata_mut().doc = Some("result documentation".into());
    let SchemaType::Record { fields, .. } = result_type else {
        panic!("record result expected")
    };
    fields[0].metadata.doc = Some("field documentation".into());

    assert!(strictly_equal(&expected, &inner));
}

#[test]
fn strict_equality_preserves_doc_and_version_fields_in_literal_defaults() {
    let literal_type = SchemaType::record(vec![
        NamedFieldType {
            name: "doc".into(),
            body: SchemaType::string(),
            metadata: Default::default(),
        },
        NamedFieldType {
            name: "version".into(),
            body: SchemaType::string(),
            metadata: Default::default(),
        },
    ]);
    let mut expected = tool(SchemaType::string());
    body(&mut expected, 0).options.push(option(
        "config",
        literal_type,
        Some(SchemaValue::Record {
            fields: vec![
                SchemaValue::String("expected doc".into()),
                SchemaValue::String("expected version".into()),
            ],
        }),
    ));
    let mut inner = expected.clone();
    body(&mut inner, 0).options[0].default = Some(SchemaValue::Record {
        fields: vec![
            SchemaValue::String("changed doc".into()),
            SchemaValue::String("expected version".into()),
        ],
    });

    assert!(!strictly_equal(&expected, &inner));
}

#[test]
fn error_vocabulary_flows_from_inner_to_expected() {
    fn error(name: &str) -> ErrorCase {
        ErrorCase {
            name: name.into(),
            doc: Default::default(),
            kind: ErrorKind::RuntimeError,
            exit_code: 1,
            payload: None,
        }
    }
    let mut expected = tool(SchemaType::string());
    expected.commands.nodes[0].body.as_mut().unwrap().errors = vec![error("expected")];
    let missing = tool(SchemaType::string());
    let mut additional = expected.clone();
    additional.commands.nodes[0]
        .body
        .as_mut()
        .unwrap()
        .errors
        .push(error("additional"));
    for mode in [
        ToolCompatibilityMode::StructuralSubtype,
        ToolCompatibilityMode::Nominal,
    ] {
        let compiled = compile_tool_compatibility(&expected, &missing, mode).unwrap();
        assert!(!compiled.commands[0].forward_unknown_errors);
        assert!(compiled.commands[0].errors.is_empty());
        let errors = compile_tool_compatibility(&expected, &additional, mode).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.path.ends_with(".error.additional"))
        );
    }
    assert!(
        compile_tool_compatibility(&expected, &missing, ToolCompatibilityMode::StrictEquality)
            .is_err()
    );
}

#[test]
fn structural_checks_error_semantics_and_reports_discarded_inputs() {
    let mut expected = tool(SchemaType::string());
    let body = expected.commands.nodes[0].body.as_mut().unwrap();
    body.positionals.fixed.push(Positional {
        name: "token".into(),
        doc: Default::default(),
        value_name: None,
        type_: SchemaType::string(),
        default: None,
        required: true,
        accepts_stdio: false,
    });
    body.errors.push(ErrorCase {
        name: "failed".into(),
        doc: Default::default(),
        kind: ErrorKind::RuntimeError,
        exit_code: 1,
        payload: None,
    });
    let mut inner = expected.clone();
    inner.commands.nodes[0]
        .body
        .as_mut()
        .unwrap()
        .positionals
        .fixed
        .clear();
    let compiled =
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
            .unwrap();
    assert_eq!(
        compiled.warnings,
        vec![ToolCompatibilityWarning {
            path: "root".into(),
            name: "token".into(),
        }]
    );

    inner.commands.nodes[0].body.as_mut().unwrap().errors[0].exit_code = 2;
    assert!(
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype,)
            .is_err()
    );
    inner.commands.nodes[0].body.as_mut().unwrap().errors[0].exit_code = 1;
    inner.commands.nodes[0].body.as_mut().unwrap().errors[0].kind = ErrorKind::UsageError;
    assert!(
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype,)
            .is_err()
    );
}

#[test]
fn structural_input_records_are_checked_expected_to_inner() {
    let mut expected = tool(SchemaType::string());
    body(&mut expected, 0)
        .positionals
        .fixed
        .push(positional("request", record(&["kept", "owned-by-outer"])));
    let mut inner = tool(SchemaType::string());
    body(&mut inner, 0)
        .positionals
        .fixed
        .push(positional("request", record(&["kept"])));

    assert!(
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
            .is_ok()
    );

    body(&mut inner, 0).positionals.fixed[0].type_ = record(&["kept", "inner-required"]);
    assert!(
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
            .is_err()
    );
}

#[test]
fn structural_inner_only_optional_and_defaulted_inputs_are_synthesized() {
    let expected = tool(SchemaType::string());
    let mut inner = expected.clone();
    body(&mut inner, 0).options.extend([
        option("optional", SchemaType::option(SchemaType::string()), None),
        option(
            "defaulted",
            SchemaType::string(),
            Some(SchemaValue::String("fallback".into())),
        ),
    ]);

    let compiled =
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
            .unwrap();
    let ProjectionNode::Record { fields, .. } =
        &compiled.commands[0].input.nodes[compiled.commands[0].input.root]
    else {
        panic!("canonical input record projection expected")
    };
    assert_eq!(fields[0].default, Some(SchemaValue::Option { inner: None }));
    assert_eq!(
        fields[1].default,
        Some(SchemaValue::String("fallback".into()))
    );
}

#[test]
fn structural_discards_expected_only_owned_input_and_rejects_required_inner_input() {
    let mut expected = tool(SchemaType::string());
    body(&mut expected, 0)
        .positionals
        .fixed
        .push(positional("outer-owned", SchemaType::string()));
    let inner = tool(SchemaType::string());
    let compiled =
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
            .unwrap();
    assert_eq!(compiled.warnings[0].name, "outer-owned");

    let mut required_inner = inner;
    body(&mut required_inner, 0)
        .positionals
        .fixed
        .push(positional("inner-required", SchemaType::string()));
    assert!(
        compile_tool_compatibility(
            &expected,
            &required_inner,
            ToolCompatibilityMode::StructuralSubtype
        )
        .is_err()
    );
}

#[test]
fn structural_command_matching_includes_inherited_global_inputs() {
    let mut expected = tool(SchemaType::string());
    expected.commands.nodes[0].body = None;
    expected.commands.nodes[0].globals.options.push(option(
        "profile",
        SchemaType::string(),
        Some(SchemaValue::String("dev".into())),
    ));
    expected.commands.nodes[0].subcommands = vec![CommandIndex(1)];
    expected.commands.nodes.push(CommandNode {
        name: "run".into(),
        aliases: vec![],
        doc: Default::default(),
        globals: Globals::default(),
        subcommands: vec![],
        body: tool(SchemaType::string()).commands.nodes.remove(0).body,
    });
    let mut inner = expected.clone();
    inner.commands.nodes[0].globals.options.clear();
    body(&mut inner, 1).options.push(option(
        "profile",
        SchemaType::string(),
        Some(SchemaValue::String("prod".into())),
    ));
    assert!(
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
            .is_ok()
    );

    body(&mut inner, 1).options[0].shape = OptionShape::Scalar(SchemaType::bool());
    assert!(
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
            .is_err()
    );
}

#[test]
fn structural_inner_only_flags_are_canonically_omittable() {
    let expected = tool(SchemaType::string());
    let mut inner = expected.clone();
    body(&mut inner, 0).flags.push(flag("verbose"));
    assert!(
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
            .is_ok()
    );
}

fn recursive_record_tool(position: &str, extra_field: Option<&str>, result: bool) -> Tool {
    let id = TypeId::new("node");
    let mut fields = vec![NamedFieldType {
        name: "next".into(),
        body: SchemaType::option(SchemaType::ref_to(id.clone())),
        metadata: Default::default(),
    }];
    if let Some(name) = extra_field {
        fields.push(NamedFieldType {
            name: name.into(),
            body: SchemaType::string(),
            metadata: Default::default(),
        });
    }
    let mut value = tool(if result {
        SchemaType::ref_to(id.clone())
    } else {
        SchemaType::string()
    });
    value.schema.defs.push(SchemaTypeDef {
        id: id.clone(),
        name: Some("Node".into()),
        body: SchemaType::record(fields),
    });
    if !result {
        body(&mut value, 0)
            .positionals
            .fixed
            .push(positional(position, SchemaType::ref_to(id)));
    }
    value
}

#[test]
fn structural_recursive_input_and_output_projection_are_directional() {
    let expected_input = recursive_record_tool("tree", Some("outer-owned"), false);
    let inner_input = recursive_record_tool("tree", None, false);
    let input = compile_tool_compatibility(
        &expected_input,
        &inner_input,
        ToolCompatibilityMode::StructuralSubtype,
    )
    .unwrap();
    assert!(
        input.commands[0]
            .input
            .nodes
            .iter()
            .any(|node| matches!(node, ProjectionNode::Recursive { .. }))
    );
    assert!(
        compile_tool_compatibility(
            &inner_input,
            &expected_input,
            ToolCompatibilityMode::StructuralSubtype
        )
        .is_err()
    );

    let expected_output = recursive_record_tool("unused", None, true);
    let inner_output = recursive_record_tool("unused", Some("inner-owned"), true);
    let output = compile_tool_compatibility(
        &expected_output,
        &inner_output,
        ToolCompatibilityMode::StructuralSubtype,
    )
    .unwrap();
    assert!(
        output.commands[0]
            .result
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .any(|node| matches!(node, ProjectionNode::Recursive { .. }))
    );
    assert!(
        compile_tool_compatibility(
            &inner_output,
            &expected_output,
            ToolCompatibilityMode::StructuralSubtype
        )
        .is_err()
    );
}

#[test]
fn stream_documentation_is_ignored_in_all_compatibility_modes() {
    let mut expected = tool(SchemaType::string());
    let stream = StreamSpec {
        doc: Doc::default(),
        mime: vec!["application/json".into()],
        required: true,
    };
    body(&mut expected, 0).stdin = Some(stream.clone());
    body(&mut expected, 0).stdout = Some(stream);
    let mut inner = expected.clone();
    body(&mut inner, 0).stdin.as_mut().unwrap().doc.summary = "different input docs".into();
    body(&mut inner, 0).stdout.as_mut().unwrap().doc.summary = "different output docs".into();

    for mode in [
        ToolCompatibilityMode::StrictEquality,
        ToolCompatibilityMode::StructuralSubtype,
        ToolCompatibilityMode::Nominal,
    ] {
        assert!(compile_tool_compatibility(&expected, &inner, mode).is_ok());
    }
}

#[test]
fn stream_mime_and_requiredness_remain_semantic() {
    let mut expected = tool(SchemaType::string());
    body(&mut expected, 0).stdin = Some(StreamSpec {
        doc: Doc::default(),
        mime: vec!["application/json".into()],
        required: true,
    });
    let mut different_mime = expected.clone();
    body(&mut different_mime, 0).stdin.as_mut().unwrap().mime = vec!["text/plain".into()];
    let mut different_requiredness = expected.clone();
    body(&mut different_requiredness, 0)
        .stdin
        .as_mut()
        .unwrap()
        .required = false;

    for mode in [
        ToolCompatibilityMode::StrictEquality,
        ToolCompatibilityMode::StructuralSubtype,
        ToolCompatibilityMode::Nominal,
    ] {
        assert!(compile_tool_compatibility(&expected, &different_mime, mode).is_err());
        assert!(compile_tool_compatibility(&expected, &different_requiredness, mode).is_err());
    }
}

fn diamond_tool() -> Tool {
    let leaf = TypeId::new("leaf");
    let root = TypeId::new("root");
    let mut value = tool(SchemaType::ref_to(root.clone()));
    value.schema.defs = vec![
        SchemaTypeDef {
            id: root,
            name: Some("Root".into()),
            body: SchemaType::record(vec![
                NamedFieldType {
                    name: "left".into(),
                    body: SchemaType::ref_to(leaf.clone()),
                    metadata: Default::default(),
                },
                NamedFieldType {
                    name: "right".into(),
                    body: SchemaType::ref_to(leaf.clone()),
                    metadata: Default::default(),
                },
            ]),
        },
        SchemaTypeDef {
            id: leaf,
            name: Some("Leaf".into()),
            body: record(&["value"]),
        },
    ];
    value
}

#[test]
fn completed_named_type_pairs_are_shared_in_diamond_plans() {
    let expected = diamond_tool();
    let inner = diamond_tool();
    let compiled =
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
            .unwrap();
    let plan = compiled.commands[0].result.as_ref().unwrap();
    let ProjectionNode::Recursive { node: root_body } = plan.nodes[plan.root] else {
        panic!("named root expected")
    };
    let ProjectionNode::Record { fields, .. } = &plan.nodes[root_body] else {
        panic!("root record expected")
    };
    assert_eq!(fields[0].plan, fields[1].plan);
}

fn nested_lists(depth: usize) -> SchemaType {
    (0..depth).fold(SchemaType::string(), |inner, _| SchemaType::list(inner))
}

#[test]
fn projection_depth_limit_accepts_boundary_and_rejects_next_level() {
    let graph = SchemaGraph::empty();
    let mut errors = Vec::new();
    assert!(
        compile_plan(
            &graph,
            &nested_lists(MAX_PROJECTION_DEPTH - 1),
            &graph,
            &nested_lists(MAX_PROJECTION_DEPTH - 1),
            ToolCompatibilityMode::StructuralSubtype,
            "depth",
            &mut errors,
        )
        .is_some()
    );
    assert!(errors.is_empty());

    assert!(
        compile_plan(
            &graph,
            &nested_lists(MAX_PROJECTION_DEPTH),
            &graph,
            &nested_lists(MAX_PROJECTION_DEPTH),
            ToolCompatibilityMode::StructuralSubtype,
            "depth",
            &mut errors,
        )
        .is_none()
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("depth limit"))
    );
}

#[test]
fn projection_node_limit_accepts_boundary_and_rejects_next_node() {
    let graph = SchemaGraph::empty();
    for (elements, succeeds) in [
        (MAX_PROJECTION_NODES - 1, true),
        (MAX_PROJECTION_NODES, false),
    ] {
        let tuple = SchemaType::tuple(vec![SchemaType::string(); elements]);
        let mut errors = Vec::new();
        let plan = compile_plan(
            &graph,
            &tuple,
            &graph,
            &tuple,
            ToolCompatibilityMode::StructuralSubtype,
            "nodes",
            &mut errors,
        );
        assert_eq!(plan.is_some(), succeeds);
        assert_eq!(errors.is_empty(), succeeds);
    }
}

#[test]
fn reordered_enum_and_flags_map_source_indices_to_target_indices() {
    let graph = SchemaGraph::empty();
    for (source, target, expected_flags) in [
        (
            SchemaType::r#enum(vec!["b".into(), "a".into()]),
            SchemaType::r#enum(vec!["a".into(), "c".into(), "b".into()]),
            false,
        ),
        (
            SchemaType::flags(vec!["b".into(), "a".into()]),
            SchemaType::flags(vec!["a".into(), "c".into(), "b".into()]),
            true,
        ),
    ] {
        let mut errors = Vec::new();
        let plan = compile_plan(
            &graph,
            &source,
            &graph,
            &target,
            ToolCompatibilityMode::StructuralSubtype,
            "mapping",
            &mut errors,
        )
        .unwrap();
        match &plan.nodes[plan.root] {
            ProjectionNode::Enum { cases } if !expected_flags => assert_eq!(cases, &[2, 0]),
            ProjectionNode::Flags { flags } if expected_flags => assert_eq!(flags, &[2, 0]),
            node => panic!("unexpected projection node: {node:?}"),
        }
    }
}

struct TestStreams {
    projected: Vec<Option<ProjectionPlan>>,
    discarded: usize,
}

impl ProjectionStreamHandler for TestStreams {
    fn project_stream(
        &mut self,
        stream: crate::schema::SchemaValueStream,
        item_plan: Option<ProjectionPlan>,
    ) -> Result<crate::schema::SchemaValueStream, String> {
        self.projected.push(item_plan);
        Ok(stream)
    }

    fn discard_stream(&mut self, _stream: crate::schema::SchemaValueStream) {
        self.discarded += 1;
    }
}

#[test]
fn evaluator_applies_compiled_reordering_and_discards_values() {
    let expected = tool(record(&["first", "second"]));
    let inner = tool(record(&["second", "first", "discarded"]));
    let compiled =
        compile_tool_compatibility(&expected, &inner, ToolCompatibilityMode::StructuralSubtype)
            .unwrap();
    let plan = compiled.commands[0].result.as_ref().unwrap();
    let mut streams = TestStreams {
        projected: vec![],
        discarded: 0,
    };
    let projected = apply_projection(
        plan,
        SchemaValue::Record {
            fields: vec![
                SchemaValue::String("second".into()),
                SchemaValue::String("first".into()),
                SchemaValue::String("gone".into()),
            ],
        },
        &mut streams,
    )
    .unwrap();
    assert_eq!(
        projected,
        SchemaValue::Record {
            fields: vec![
                SchemaValue::String("first".into()),
                SchemaValue::String("second".into())
            ]
        }
    );
}

#[test]
fn evaluator_remaps_flags_and_rejects_dynamic_mismatch() {
    let source = SchemaType::flags(vec!["b".into(), "a".into()]);
    let target = SchemaType::flags(vec!["a".into(), "c".into(), "b".into()]);
    let mut errors = vec![];
    let plan = compile_plan(
        &SchemaGraph::empty(),
        &source,
        &SchemaGraph::empty(),
        &target,
        ToolCompatibilityMode::StructuralSubtype,
        "flags",
        &mut errors,
    )
    .unwrap();
    let mut streams = TestStreams {
        projected: vec![],
        discarded: 0,
    };
    assert_eq!(
        apply_projection(
            &plan,
            SchemaValue::Flags {
                bits: vec![true, false]
            },
            &mut streams
        )
        .unwrap(),
        SchemaValue::Flags {
            bits: vec![false, false, true]
        }
    );

    let plan = dynamic_plan(
        SchemaGraph::anonymous(SchemaType::string()),
        SchemaGraph::anonymous(SchemaType::u32()),
    );
    assert!(apply_projection(&plan, SchemaValue::String("no".into()), &mut streams).is_err());
}

#[test]
#[cfg(feature = "host")]
fn evaluator_returns_nonidentity_stream_item_plan_and_closes_discarded_stream() {
    let source_item = record(&["kept", "discarded"]);
    let target_item = record(&["kept"]);
    let source = SchemaType::record(vec![
        NamedFieldType {
            name: "kept".into(),
            body: SchemaType::stream(Some(source_item)),
            metadata: Default::default(),
        },
        NamedFieldType {
            name: "discarded".into(),
            body: SchemaType::stream(None),
            metadata: Default::default(),
        },
    ]);
    let target = SchemaType::record(vec![NamedFieldType {
        name: "kept".into(),
        body: SchemaType::stream(Some(target_item)),
        metadata: Default::default(),
    }]);
    let mut errors = vec![];
    let plan = compile_plan(
        &SchemaGraph::empty(),
        &source,
        &SchemaGraph::empty(),
        &target,
        ToolCompatibilityMode::StructuralSubtype,
        "stream",
        &mut errors,
    )
    .unwrap();
    let retained = crate::schema::SchemaValueStream::from_host_endpoint(1_u8);
    let discarded = crate::schema::SchemaValueStream::from_host_endpoint(2_u8);
    let mut streams = TestStreams {
        projected: vec![],
        discarded: 0,
    };
    apply_projection(
        &plan,
        SchemaValue::Record {
            fields: vec![
                SchemaValue::Stream(retained),
                SchemaValue::Stream(discarded),
            ],
        },
        &mut streams,
    )
    .unwrap();
    assert_eq!(streams.projected.len(), 1);
    assert!(streams.projected[0].is_some());
    assert_eq!(streams.discarded, 1);
}

#[test]
#[cfg(feature = "host")]
fn equivalent_stream_projection_preserves_endpoint_without_calling_handler() {
    let stream_type = SchemaType::stream(Some(record(&["value"])));
    let mut errors = vec![];
    let plan = compile_plan(
        &SchemaGraph::empty(),
        &stream_type,
        &SchemaGraph::empty(),
        &stream_type,
        ToolCompatibilityMode::StructuralSubtype,
        "stream",
        &mut errors,
    )
    .unwrap();
    assert!(matches!(plan.nodes[plan.root], ProjectionNode::Identity));

    let endpoint = crate::schema::SchemaValueStream::from_host_endpoint(7_u8);
    let cell = endpoint.cell_id();
    let mut streams = TestStreams {
        projected: vec![],
        discarded: 0,
    };
    let SchemaValue::Stream(output) =
        apply_projection(&plan, SchemaValue::Stream(endpoint), &mut streams).unwrap()
    else {
        panic!("stream result expected")
    };
    assert_eq!(output.cell_id(), cell);
    assert!(streams.projected.is_empty());
}

#[cfg(feature = "host")]
struct FailingStreams {
    project_calls: usize,
    discarded_endpoints: Vec<u8>,
}

#[cfg(feature = "host")]
impl ProjectionStreamHandler for FailingStreams {
    fn project_stream(
        &mut self,
        stream: crate::schema::SchemaValueStream,
        _item_plan: Option<ProjectionPlan>,
    ) -> Result<crate::schema::SchemaValueStream, String> {
        self.project_calls += 1;
        if self.project_calls == 2 {
            self.discarded_endpoints
                .push(stream.take_host_endpoint::<u8>()?);
            Err("relay failed".into())
        } else {
            Ok(stream)
        }
    }

    fn discard_stream(&mut self, stream: crate::schema::SchemaValueStream) {
        self.discarded_endpoints
            .push(stream.take_host_endpoint::<u8>().unwrap());
    }
}

#[test]
#[cfg(feature = "host")]
fn later_stream_failure_discards_transferred_output_and_unvisited_sibling() {
    let source_item = record(&["kept", "discarded"]);
    let target_item = record(&["kept"]);
    let source_stream = SchemaType::stream(Some(source_item));
    let target_stream = SchemaType::stream(Some(target_item));
    let source = SchemaType::tuple(vec![
        source_stream.clone(),
        source_stream.clone(),
        source_stream,
    ]);
    let target = SchemaType::tuple(vec![
        target_stream.clone(),
        target_stream.clone(),
        target_stream,
    ]);
    let mut errors = vec![];
    let plan = compile_plan(
        &SchemaGraph::empty(),
        &source,
        &SchemaGraph::empty(),
        &target,
        ToolCompatibilityMode::StructuralSubtype,
        "streams",
        &mut errors,
    )
    .unwrap();
    let mut streams = FailingStreams {
        project_calls: 0,
        discarded_endpoints: vec![],
    };
    let result = apply_projection(
        &plan,
        SchemaValue::Tuple {
            elements: [1_u8, 2, 3]
                .into_iter()
                .map(|id| {
                    SchemaValue::Stream(crate::schema::SchemaValueStream::from_host_endpoint(id))
                })
                .collect(),
        },
        &mut streams,
    );
    assert!(result.is_err());
    assert_eq!(streams.project_calls, 2);
    assert_eq!(streams.discarded_endpoints, vec![2, 1, 3]);
}

#[test]
#[cfg(feature = "host")]
fn invalid_record_discard_index_errors_and_disposes_input_stream() {
    let stream_type = SchemaType::record(vec![NamedFieldType {
        name: "value".into(),
        body: SchemaType::stream(None),
        metadata: Default::default(),
    }]);
    let plan = ProjectionPlan {
        source_schema: SchemaGraph::anonymous(stream_type.clone()),
        target_schema: SchemaGraph::anonymous(stream_type),
        nodes: vec![ProjectionNode::Record {
            fields: vec![],
            discard: vec![1],
        }],
        root: 0,
    };
    let mut streams = FailingStreams {
        project_calls: 0,
        discarded_endpoints: vec![],
    };
    assert!(
        apply_projection(
            &plan,
            SchemaValue::Record {
                fields: vec![SchemaValue::Stream(
                    crate::schema::SchemaValueStream::from_host_endpoint(9_u8)
                )],
            },
            &mut streams,
        )
        .is_err()
    );
    assert_eq!(streams.discarded_endpoints, vec![9]);
}

#[test]
#[cfg(feature = "host")]
fn boundary_validation_failures_dispose_streams() {
    let source_failure = ProjectionPlan {
        source_schema: SchemaGraph::anonymous(SchemaType::string()),
        target_schema: SchemaGraph::anonymous(SchemaType::string()),
        nodes: vec![ProjectionNode::Identity],
        root: 0,
    };
    let target_failure = ProjectionPlan {
        source_schema: SchemaGraph::anonymous(SchemaType::stream(None)),
        target_schema: SchemaGraph::anonymous(SchemaType::string()),
        nodes: vec![ProjectionNode::Identity],
        root: 0,
    };
    let mut streams = FailingStreams {
        project_calls: 0,
        discarded_endpoints: vec![],
    };

    let source_error = apply_projection(
        &source_failure,
        SchemaValue::Stream(crate::schema::SchemaValueStream::from_host_endpoint(10_u8)),
        &mut streams,
    )
    .unwrap_err();
    let target_error = apply_projection(
        &target_failure,
        SchemaValue::Stream(crate::schema::SchemaValueStream::from_host_endpoint(11_u8)),
        &mut streams,
    )
    .unwrap_err();

    assert_eq!(source_error.path, "source");
    assert_eq!(target_error.path, "target");
    assert_eq!(streams.discarded_endpoints, vec![10, 11]);
}

#[test]
#[cfg(feature = "host")]
fn provisional_dynamic_checked_rejection_discards_input_stream() {
    let plan = ProjectionPlan {
        source_schema: SchemaGraph::anonymous(SchemaType::stream(None)),
        target_schema: SchemaGraph::anonymous(SchemaType::string()),
        nodes: vec![ProjectionNode::DynamicChecked],
        root: 0,
    };
    let mut streams = FailingStreams {
        project_calls: 0,
        discarded_endpoints: vec![],
    };

    assert!(
        apply_projection(
            &plan,
            SchemaValue::Stream(crate::schema::SchemaValueStream::from_host_endpoint(12_u8)),
            &mut streams,
        )
        .is_err()
    );
    assert_eq!(streams.discarded_endpoints, vec![12]);
}

#[test]
#[cfg(feature = "host")]
fn provisional_malformed_stream_plan_discards_input_stream() {
    let plan = ProjectionPlan {
        source_schema: SchemaGraph::anonymous(SchemaType::stream(None)),
        target_schema: SchemaGraph::anonymous(SchemaType::stream(Some(SchemaType::string()))),
        nodes: vec![ProjectionNode::Stream { item: Some(0) }],
        root: 0,
    };
    let mut streams = FailingStreams {
        project_calls: 0,
        discarded_endpoints: vec![],
    };

    assert!(
        apply_projection(
            &plan,
            SchemaValue::Stream(crate::schema::SchemaValueStream::from_host_endpoint(13_u8)),
            &mut streams,
        )
        .is_err()
    );
    assert_eq!(streams.discarded_endpoints, vec![13]);
}

#[test]
#[cfg(feature = "host")]
fn malformed_nodes_discard_all_still_owned_streams_once() {
    let stream = SchemaType::stream(None);
    let record = SchemaType::record(vec![
        NamedFieldType {
            name: "first".into(),
            body: stream.clone(),
            metadata: Default::default(),
        },
        NamedFieldType {
            name: "second".into(),
            body: stream.clone(),
            metadata: Default::default(),
        },
    ]);
    let plans = [
        ProjectionPlan {
            source_schema: SchemaGraph::anonymous(stream.clone()),
            target_schema: SchemaGraph::anonymous(stream.clone()),
            nodes: vec![],
            root: 0,
        },
        ProjectionPlan {
            source_schema: SchemaGraph::anonymous(record.clone()),
            target_schema: SchemaGraph::anonymous(SchemaType::record(vec![])),
            nodes: vec![ProjectionNode::Record {
                fields: vec![],
                discard: vec![0, 0],
            }],
            root: 0,
        },
    ];
    let values = [
        SchemaValue::Stream(crate::schema::SchemaValueStream::from_host_endpoint(14_u8)),
        SchemaValue::Record {
            fields: [15_u8, 16]
                .map(|id| {
                    SchemaValue::Stream(crate::schema::SchemaValueStream::from_host_endpoint(id))
                })
                .into(),
        },
    ];
    let mut streams = FailingStreams {
        project_calls: 0,
        discarded_endpoints: vec![],
    };

    for (plan, value) in plans.iter().zip(values) {
        assert!(apply_projection(plan, value, &mut streams).is_err());
    }
    assert_eq!(streams.discarded_endpoints, vec![14, 15, 16]);
}

#[test]
#[cfg(feature = "host")]
fn map_key_failure_also_discards_its_unprocessed_value() {
    let stream = SchemaType::stream(None);
    let plan = ProjectionPlan {
        source_schema: SchemaGraph::anonymous(SchemaType::map(stream.clone(), stream.clone())),
        target_schema: SchemaGraph::anonymous(SchemaType::map(SchemaType::string(), stream)),
        nodes: vec![
            ProjectionNode::DynamicChecked,
            ProjectionNode::Identity,
            ProjectionNode::Map { key: 0, value: 1 },
        ],
        root: 2,
    };
    let mut streams = FailingStreams {
        project_calls: 0,
        discarded_endpoints: vec![],
    };
    let value = SchemaValue::Map {
        entries: vec![(
            SchemaValue::Stream(crate::schema::SchemaValueStream::from_host_endpoint(17_u8)),
            SchemaValue::Stream(crate::schema::SchemaValueStream::from_host_endpoint(18_u8)),
        )],
    };

    assert!(apply_projection(&plan, value, &mut streams).is_err());
    assert_eq!(streams.discarded_endpoints, vec![17, 18]);
}

#[cfg(feature = "host")]
struct NestedDiscardStreams {
    discarded: Vec<u8>,
}

#[cfg(feature = "host")]
impl ProjectionStreamHandler for NestedDiscardStreams {
    fn project_stream(
        &mut self,
        stream: crate::schema::SchemaValueStream,
        _item_plan: Option<ProjectionPlan>,
    ) -> Result<crate::schema::SchemaValueStream, String> {
        Ok(stream)
    }

    fn discard_stream(&mut self, stream: crate::schema::SchemaValueStream) {
        self.discarded
            .push(stream.take_host_endpoint::<u8>().unwrap());
    }
}

#[test]
#[cfg(feature = "host")]
fn source_rejection_discards_nested_and_sibling_streams() {
    let plan = ProjectionPlan {
        source_schema: SchemaGraph::anonymous(SchemaType::string()),
        target_schema: SchemaGraph::anonymous(SchemaType::string()),
        nodes: vec![ProjectionNode::Identity],
        root: 0,
    };
    let mut streams = NestedDiscardStreams { discarded: vec![] };
    let value = SchemaValue::Tuple {
        elements: vec![
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::Tuple {
                    elements: [21_u8, 22]
                        .map(|id| {
                            SchemaValue::Stream(
                                crate::schema::SchemaValueStream::from_host_endpoint(id),
                            )
                        })
                        .into(),
                })),
            },
            SchemaValue::Stream(crate::schema::SchemaValueStream::from_host_endpoint(23_u8)),
        ],
    };

    assert!(apply_projection(&plan, value, &mut streams).is_err());
    assert_eq!(streams.discarded, vec![21, 22, 23]);
}
