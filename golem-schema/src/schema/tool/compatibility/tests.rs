use super::*;
use crate::schema::metadata::MetadataEnvelope;
use crate::schema::schema_type::NamedFieldType;
use crate::schema::tool::{
    BoolFlagShape, CommandAnnotations, CommandIndex, CommandNode, CommandTree, ErrorKind,
    FlagShape, FlagSpec, Globals, OptionShape, OptionSpec, Positional, Positionals,
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
fn structural_requires_expected_error_vocabulary_but_allows_additions() {
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
    assert!(
        compile_tool_compatibility(
            &expected,
            &missing,
            ToolCompatibilityMode::StructuralSubtype
        )
        .is_err()
    );

    let mut additional = expected.clone();
    additional.commands.nodes[0]
        .body
        .as_mut()
        .unwrap()
        .errors
        .push(error("additional"));
    assert!(
        compile_tool_compatibility(
            &expected,
            &additional,
            ToolCompatibilityMode::StructuralSubtype
        )
        .is_ok()
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
