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

use super::validation::{ToolValidationError, validate_tool};
use super::wit::{decode_tool, encode_tool, wire};
use super::*;
use crate::schema::graph::{SchemaGraph, SchemaTypeDef};
use crate::schema::metadata::TypeId;
use crate::schema::proptest_strategies::{schema_graph_strategy, schema_value_strategy};
use crate::schema::schema_type::{NamedFieldType, SchemaType, VariantCaseType};
use crate::schema::schema_value::SchemaValue;
use proptest::prelude::*;
use test_r::test;

// --- builders ---

/// Root command node with no body and no subcommands.
fn root(name: &str) -> CommandNode {
    CommandNode {
        name: name.to_string(),
        aliases: Vec::new(),
        doc: Doc::default(),
        globals: Globals::default(),
        subcommands: Vec::new(),
        body: None,
    }
}

fn empty_body() -> CommandBody {
    CommandBody {
        positionals: Positionals::default(),
        options: Vec::new(),
        flags: Vec::new(),
        constraints: Vec::new(),
        stdin: None,
        stdout: None,
        result: None,
        errors: Vec::new(),
        annotations: None,
    }
}

/// A required scalar option carrying the given value type.
fn scalar_option(long: &str, ty: SchemaType) -> OptionSpec {
    OptionSpec {
        long: long.to_string(),
        short: None,
        aliases: Vec::new(),
        doc: Doc::default(),
        value_name: None,
        shape: OptionShape::Scalar(ty),
        default: None,
        required: false,
        env_var: None,
    }
}

fn bool_flag(long: &str) -> FlagSpec {
    FlagSpec {
        long: long.to_string(),
        short: None,
        aliases: Vec::new(),
        doc: Doc::default(),
        shape: FlagShape::BoolFlag(BoolFlagShape {
            default: false,
            negatable: false,
        }),
        env_var: None,
    }
}

fn record_field(name: &str, body: SchemaType) -> NamedFieldType {
    NamedFieldType {
        name: name.to_string(),
        body,
        metadata: Default::default(),
    }
}

fn variant_case(name: &str, payload: Option<SchemaType>) -> VariantCaseType {
    VariantCaseType {
        name: name.to_string(),
        payload,
        metadata: Default::default(),
    }
}

/// A single-command tool with one root command and an empty schema graph.
fn tool_with_root(node: CommandNode) -> Tool {
    Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree { nodes: vec![node] },
        schema: SchemaGraph::empty(),
    }
}

// --- round-trip ---

/// A tool exercising the command-body constructs plus a named schema-graph
/// definition referenced from an input position, used to confirm the native
/// <-> wire conversion is lossless. It is not required to satisfy the
/// producer-side invariants.
fn kitchen_sink_tool() -> Tool {
    let mut schema = SchemaGraph::empty();
    schema.defs = vec![SchemaTypeDef {
        id: TypeId::new("color"),
        name: Some("Color".to_string()),
        body: SchemaType::r#enum(vec!["red".to_string(), "green".to_string()]),
    }];
    let color_ref = SchemaType::ref_to(TypeId::new("color"));

    let mut body = empty_body();
    body.positionals = Positionals {
        fixed: vec![Positional {
            name: "input".to_string(),
            doc: Doc::default(),
            value_name: Some("INPUT".to_string()),
            type_: color_ref.clone(),
            default: Some(SchemaValue::Enum { case: 0 }),
            required: true,
        }],
        tail: Some(TailPositional {
            name: "rest".to_string(),
            doc: Doc::default(),
            value_name: None,
            item_type: SchemaType::string(),
            min: 0,
            max: Some(3),
            separator: Some("--".to_string()),
            verbatim: true,
        }),
    };
    body.options = vec![
        OptionSpec {
            long: "level".to_string(),
            short: Some('l'),
            aliases: vec!["lvl".to_string()],
            doc: Doc::default(),
            value_name: None,
            shape: OptionShape::OptionalScalar(SchemaType::s64()),
            default: Some(SchemaValue::S64(2)),
            required: false,
            env_var: Some("LEVEL".to_string()),
        },
        OptionSpec {
            long: "inc".to_string(),
            short: None,
            aliases: Vec::new(),
            doc: Doc::default(),
            value_name: None,
            shape: OptionShape::Repeatable(RepeatableShape {
                repetition: Repetition::Either(','),
                type_: SchemaType::string(),
            }),
            default: None,
            required: false,
            env_var: None,
        },
    ];
    body.flags = vec![
        bool_flag("verbose"),
        FlagSpec {
            long: "count".to_string(),
            short: Some('c'),
            aliases: Vec::new(),
            doc: Doc::default(),
            shape: FlagShape::CountFlag(Some(3)),
            env_var: None,
        },
    ];
    body.constraints = vec![
        Constraint::RequiresAll(vec![Ref::Present("level".to_string())]),
        Constraint::MutexGroups(vec![RefGroup {
            refs: vec![Ref::ValueIs(ValueIsRef {
                name: "level".to_string(),
                value: SchemaValue::S64(5),
            })],
        }]),
        Constraint::Implies(ImpliesC {
            lhs_quant: Quantifier::All,
            lhs: vec![Ref::Present("verbose".to_string())],
            rhs_quant: Quantifier::Any,
            rhs: vec![Ref::Present("level".to_string())],
        }),
        Constraint::Forbids(ForbidsC {
            lhs_quant: Quantifier::Any,
            lhs: vec![Ref::Present("count".to_string())],
            rhs: vec![Ref::Present("verbose".to_string())],
        }),
    ];
    body.result = Some(ResultSpec {
        type_: SchemaType::record(vec![record_field("field", SchemaType::string())]),
        doc: Doc::default(),
        formatters: vec![Formatter {
            name: "json".to_string(),
            doc: Doc::default(),
        }],
        default_formatter: "json".to_string(),
    });
    body.errors = vec![ErrorCase {
        name: "boom".to_string(),
        doc: Doc {
            summary: "boom".to_string(),
            description: "kaboom".to_string(),
            examples: vec![Example {
                title: "t".to_string(),
                body: "b".to_string(),
            }],
        },
        kind: ErrorKind::RuntimeError,
        exit_code: 2,
        payload: Some(SchemaType::string()),
    }];
    body.annotations = Some(CommandAnnotations {
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    });
    body.stdin = Some(StreamSpec {
        doc: Doc::default(),
        mime: vec!["text/plain".to_string()],
        required: false,
    });
    body.stdout = Some(StreamSpec {
        doc: Doc::default(),
        mime: Vec::new(),
        required: true,
    });

    let mut node = root("tool");
    node.aliases = vec!["t".to_string()];
    node.globals = Globals {
        options: vec![scalar_option("config", color_ref)],
        flags: vec![bool_flag("debug")],
    };
    node.subcommands = vec![CommandIndex(1)];
    node.body = Some(body);

    let child = root("sub");

    Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree {
            nodes: vec![node, child],
        },
        schema,
    }
}

#[test]
fn native_wire_round_trip_is_lossless() {
    let tool = kitchen_sink_tool();
    let wire = encode_tool(&tool).expect("native -> wire should succeed");
    let back = decode_tool(&wire).expect("wire -> native should succeed");
    assert_eq!(tool, back);
}

// --- validation: happy path ---

#[test]
fn minimal_tool_is_valid() {
    let tool = tool_with_root(root("tool"));
    assert_eq!(validate_tool(&tool), Ok(()));
}

#[test]
fn rich_tool_without_input_variant_is_valid() {
    // A variant type is legal in an output (result) position; only inputs must
    // avoid it. Scalar inputs and a value-is literal that matches its type
    // exercise a rich-but-legal tool.
    let mut body = empty_body();
    body.options = vec![scalar_option("name", SchemaType::string())];
    body.flags = vec![bool_flag("verbose")];
    body.result = Some(ResultSpec {
        type_: SchemaType::variant(vec![variant_case("ok", None)]),
        doc: Doc::default(),
        formatters: vec![Formatter {
            name: "text".to_string(),
            doc: Doc::default(),
        }],
        default_formatter: "text".to_string(),
    });
    body.constraints = vec![Constraint::RequiresAll(vec![Ref::ValueIs(ValueIsRef {
        name: "name".to_string(),
        value: SchemaValue::String("x".to_string()),
    })])];

    let mut node = root("tool");
    node.body = Some(body);

    let tool = tool_with_root(node);
    assert_eq!(validate_tool(&tool), Ok(()));
}

// --- validation: failures ---

#[test]
fn empty_command_tree_is_rejected() {
    let tool = Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree { nodes: Vec::new() },
        schema: SchemaGraph::empty(),
    };
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.contains(&ToolValidationError::EmptyCommandTree));
}

#[test]
fn invalid_identifier_is_rejected() {
    let tool = tool_with_root(root("Tool")); // uppercase is illegal
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::InvalidIdentifier { value, .. } if value == "Tool"
    )));
}

#[test]
fn duplicate_subcommand_alias_is_rejected() {
    let mut parent = root("tool");
    parent.subcommands = vec![CommandIndex(1), CommandIndex(2)];
    let mut a = root("build");
    a.aliases = vec!["b".to_string()];
    let mut b = root("bundle");
    b.aliases = vec!["b".to_string()]; // collides with build's alias
    let tool = Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree {
            nodes: vec![parent, a, b],
        },
        schema: SchemaGraph::empty(),
    };
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::DuplicateSubcommandName { name, .. } if name == "b"
    )));
}

#[test]
fn body_name_colliding_with_inherited_global_is_rejected() {
    let mut node = root("tool");
    node.globals = Globals {
        options: vec![scalar_option("shared", SchemaType::string())],
        flags: Vec::new(),
    };
    let mut body = empty_body();
    body.flags = vec![bool_flag("shared")]; // collides with the global option
    node.body = Some(body);

    let tool = tool_with_root(node);
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::DuplicateName { name, .. } if name == "shared"
    )));
}

#[test]
fn unresolved_constraint_ref_is_rejected() {
    let mut body = empty_body();
    body.options = vec![scalar_option("name", SchemaType::string())];
    body.constraints = vec![Constraint::RequiresAll(vec![Ref::Present(
        "missing".to_string(),
    )])];
    let mut node = root("tool");
    node.body = Some(body);
    let tool = tool_with_root(node);
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::UnresolvedConstraintRef { name, .. } if name == "missing"
    )));
}

#[test]
fn value_is_type_mismatch_is_rejected() {
    let mut body = empty_body();
    body.options = vec![scalar_option("count", SchemaType::s64())]; // declared int
    body.constraints = vec![Constraint::RequiresAll(vec![Ref::ValueIs(ValueIsRef {
        name: "count".to_string(),
        value: SchemaValue::String("not-an-int".to_string()),
    })])];
    let mut node = root("tool");
    node.body = Some(body);
    let tool = tool_with_root(node);
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::ValueIsTypeMismatch { name, .. } if name == "count"
    )));
}

#[test]
fn value_is_into_list_element_is_accepted() {
    // option "tags" is list<string>; a bare string literal matches an element.
    let mut body = empty_body();
    body.options = vec![scalar_option(
        "tags",
        SchemaType::list(SchemaType::string()),
    )];
    body.constraints = vec![Constraint::RequiresAll(vec![Ref::ValueIs(ValueIsRef {
        name: "tags".to_string(),
        value: SchemaValue::String("x".to_string()),
    })])];
    let mut node = root("tool");
    node.body = Some(body);
    let tool = tool_with_root(node);
    assert_eq!(validate_tool(&tool), Ok(()));
}

#[test]
fn value_is_into_optional_list_element_is_accepted() {
    // option "tags" is option<list<string>>; a bare string literal matches an
    // element after peeling the option wrapper.
    let mut body = empty_body();
    body.options = vec![scalar_option(
        "tags",
        SchemaType::option(SchemaType::list(SchemaType::string())),
    )];
    body.constraints = vec![Constraint::RequiresAll(vec![Ref::ValueIs(ValueIsRef {
        name: "tags".to_string(),
        value: SchemaValue::String("x".to_string()),
    })])];
    let mut node = root("tool");
    node.body = Some(body);
    let tool = tool_with_root(node);
    assert_eq!(validate_tool(&tool), Ok(()));
}

#[test]
fn value_is_record_missing_field_is_rejected() {
    // option "rec" is record { a: string, b: string }; a record literal with
    // only one field does not validate against it.
    let mut body = empty_body();
    body.options = vec![scalar_option(
        "rec",
        SchemaType::record(vec![
            record_field("a", SchemaType::string()),
            record_field("b", SchemaType::string()),
        ]),
    )];
    body.constraints = vec![Constraint::RequiresAll(vec![Ref::ValueIs(ValueIsRef {
        name: "rec".to_string(),
        value: SchemaValue::Record {
            fields: vec![SchemaValue::String("x".to_string())],
        },
    })])];
    let mut node = root("tool");
    node.body = Some(body);
    let tool = tool_with_root(node);
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::ValueIsTypeMismatch { name, .. } if name == "rec"
    )));
}

#[test]
fn unresolved_default_formatter_is_rejected() {
    let mut body = empty_body();
    body.result = Some(ResultSpec {
        type_: SchemaType::string(),
        doc: Doc::default(),
        formatters: vec![Formatter {
            name: "json".to_string(),
            doc: Doc::default(),
        }],
        default_formatter: "yaml".to_string(),
    });
    let mut node = root("tool");
    node.body = Some(body);
    let tool = tool_with_root(node);
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::UnresolvedDefaultFormatter { formatter, .. } if formatter == "yaml"
    )));
}

#[test]
fn verbatim_tail_without_separator_is_rejected() {
    let mut body = empty_body();
    body.positionals = Positionals {
        fixed: Vec::new(),
        tail: Some(TailPositional {
            name: "rest".to_string(),
            doc: Doc::default(),
            value_name: None,
            item_type: SchemaType::string(),
            min: 0,
            max: None,
            separator: None,
            verbatim: true,
        }),
    };
    let mut node = root("tool");
    node.body = Some(body);
    let tool = tool_with_root(node);
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::VerbatimWithoutSeparator { positional, .. } if positional == "rest"
    )));
}

#[test]
fn variant_in_input_position_is_rejected() {
    // A variant used directly as a positional input type.
    let mut body = empty_body();
    body.positionals = Positionals {
        fixed: vec![Positional {
            name: "arg".to_string(),
            doc: Doc::default(),
            value_name: None,
            type_: SchemaType::variant(vec![variant_case("ok", None)]),
            default: None,
            required: true,
        }],
        tail: None,
    };
    let mut node = root("tool");
    node.body = Some(body);
    let tool = tool_with_root(node);
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::VariantInInputPosition { position, .. } if position == "arg"
    )));
}

#[test]
fn variant_reachable_through_body_option_is_rejected() {
    let mut body = empty_body();
    body.options = vec![scalar_option(
        "out",
        SchemaType::variant(vec![variant_case("ok", None)]),
    )];
    let mut node = root("tool");
    node.body = Some(body);
    let tool = tool_with_root(node);
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::VariantInInputPosition { position, .. } if position == "out"
    )));
}

#[test]
fn variant_reachable_through_global_option_is_rejected() {
    let mut node = root("tool");
    node.globals = Globals {
        options: vec![scalar_option(
            "opt",
            SchemaType::variant(vec![variant_case("ok", None)]),
        )],
        flags: Vec::new(),
    };
    let tool = tool_with_root(node);
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::VariantInInputPosition { position, .. } if position == "opt"
    )));
}

#[test]
fn variant_reachable_through_named_ref_is_rejected() {
    // The input type is a named reference whose definition is (transitively) a
    // variant; ref resolution must still detect it.
    let mut schema = SchemaGraph::empty();
    schema.defs = vec![SchemaTypeDef {
        id: TypeId::new("payload"),
        name: None,
        body: SchemaType::record(vec![record_field(
            "inner",
            SchemaType::variant(vec![variant_case("ok", None)]),
        )]),
    }];
    let mut body = empty_body();
    body.options = vec![scalar_option(
        "out",
        SchemaType::ref_to(TypeId::new("payload")),
    )];
    let mut node = root("tool");
    node.body = Some(body);
    let tool = Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree { nodes: vec![node] },
        schema,
    };
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::VariantInInputPosition { position, .. } if position == "out"
    )));
}

#[test]
fn scalar_default_type_mismatch_is_rejected() {
    let mut count = scalar_option("count", SchemaType::s64());
    count.default = Some(SchemaValue::String("nope".to_string()));
    let mut body = empty_body();
    body.options = vec![count];
    let mut node = root("tool");
    node.body = Some(body);
    let tool = tool_with_root(node);
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::DefaultTypeMismatch { name, .. } if name == "count"
    )));
}

#[test]
fn positional_default_type_mismatch_is_rejected() {
    let mut body = empty_body();
    body.positionals = Positionals {
        fixed: vec![Positional {
            name: "n".to_string(),
            doc: Doc::default(),
            value_name: None,
            type_: SchemaType::s64(),
            default: Some(SchemaValue::String("nope".to_string())),
            required: false,
        }],
        tail: None,
    };
    let mut node = root("tool");
    node.body = Some(body);
    let tool = tool_with_root(node);
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::DefaultTypeMismatch { name, .. } if name == "n"
    )));
}

#[test]
fn repeatable_scalar_default_is_rejected() {
    // A repeatable option's default must be a list of element values, not a
    // bare element.
    let mut inc = scalar_option("inc", SchemaType::string());
    inc.shape = OptionShape::Repeatable(RepeatableShape {
        repetition: Repetition::Repeated,
        type_: SchemaType::string(),
    });
    inc.default = Some(SchemaValue::String("x".to_string()));
    let mut body = empty_body();
    body.options = vec![inc];
    let mut node = root("tool");
    node.body = Some(body);
    let tool = tool_with_root(node);
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::DefaultTypeMismatch { name, .. } if name == "inc"
    )));
}

#[test]
fn repeatable_list_default_is_accepted() {
    let mut inc = scalar_option("inc", SchemaType::string());
    inc.shape = OptionShape::Repeatable(RepeatableShape {
        repetition: Repetition::Repeated,
        type_: SchemaType::string(),
    });
    inc.default = Some(SchemaValue::List {
        elements: vec![SchemaValue::String("x".to_string())],
    });
    let mut body = empty_body();
    body.options = vec![inc];
    let mut node = root("tool");
    node.body = Some(body);
    let tool = tool_with_root(node);
    assert_eq!(validate_tool(&tool), Ok(()));
}

#[test]
fn dangling_type_ref_is_rejected() {
    // An option referencing a type id that has no definition in the graph.
    let mut body = empty_body();
    body.options = vec![scalar_option(
        "out",
        SchemaType::ref_to(TypeId::new("missing")),
    )];
    let mut node = root("tool");
    node.body = Some(body);
    let tool = tool_with_root(node); // empty schema graph -> ref is dangling
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::UnresolvedTypeRef { id, .. } if id == "missing"
    )));
}

#[test]
fn dangling_ref_in_definition_is_rejected() {
    // A definition body references a type id that is not in the graph; this is
    // caught even though no command position references the definition.
    let mut schema = SchemaGraph::empty();
    schema.defs = vec![SchemaTypeDef {
        id: TypeId::new("outer"),
        name: None,
        body: SchemaType::list(SchemaType::ref_to(TypeId::new("inner-missing"))),
    }];
    let tool = Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree {
            nodes: vec![root("tool")],
        },
        schema,
    };
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::UnresolvedTypeRef { id, .. } if id == "inner-missing"
    )));
}

#[test]
fn out_of_bounds_command_index_is_rejected() {
    let mut node = root("tool");
    node.subcommands = vec![CommandIndex(9)];
    let tool = tool_with_root(node);
    let errors = validate_tool(&tool).unwrap_err();
    assert!(errors.iter().any(|e| matches!(
        e,
        ToolValidationError::CommandIndexOutOfBounds { index: 9, .. }
    )));
}

#[test]
fn duplicate_global_short_across_levels_is_rejected() {
    let mut alpha = scalar_option("alpha", SchemaType::string());
    alpha.short = Some('a');
    let mut apex = scalar_option("apex", SchemaType::string());
    apex.short = Some('a');

    let mut parent = root("tool");
    parent.globals = Globals {
        options: vec![alpha],
        flags: Vec::new(),
    };
    parent.subcommands = vec![CommandIndex(1)];
    let mut child = root("sub");
    child.globals = Globals {
        options: vec![apex],
        flags: Vec::new(),
    };

    let tool = Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree {
            nodes: vec![parent, child],
        },
        schema: SchemaGraph::empty(),
    };
    let errors = validate_tool(&tool).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ToolValidationError::DuplicateShort { short: 'a', .. }))
    );
}

#[test]
fn orphan_command_node_is_rejected() {
    let parent = root("tool"); // no subcommands -> node 1 is unreachable
    let orphan = root("orphan");
    let tool = Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree {
            nodes: vec![parent, orphan],
        },
        schema: SchemaGraph::empty(),
    };
    let errors = validate_tool(&tool).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ToolValidationError::UnreachableCommandNode { index: 1 }))
    );
}

#[test]
fn command_tree_cycle_is_rejected() {
    // 0 -> 1 -> 0 forms a cycle.
    let mut a = root("tool");
    a.subcommands = vec![CommandIndex(1)];
    let mut b = root("sub");
    b.subcommands = vec![CommandIndex(0)];
    let tool = Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree { nodes: vec![a, b] },
        schema: SchemaGraph::empty(),
    };
    let errors = validate_tool(&tool).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ToolValidationError::CommandTreeCycle { .. }))
    );
}

#[test]
fn shared_subcommand_is_rejected() {
    // 0 -> {1, 2}; both 1 and 2 reference 3, giving node 3 two parents.
    let mut r = root("tool");
    r.subcommands = vec![CommandIndex(1), CommandIndex(2)];
    let mut a = root("alpha");
    a.subcommands = vec![CommandIndex(3)];
    let mut b = root("beta");
    b.subcommands = vec![CommandIndex(3)];
    let c = root("gamma");
    let tool = Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree {
            nodes: vec![r, a, b, c],
        },
        schema: SchemaGraph::empty(),
    };
    let errors = validate_tool(&tool).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ToolValidationError::DuplicateCommandParent { index: 3 }))
    );
}

#[test]
fn identifier_grammar() {
    assert!(validation::is_valid_identifier("build"));
    assert!(validation::is_valid_identifier("build-all"));
    assert!(validation::is_valid_identifier("x1"));
    assert!(!validation::is_valid_identifier("Build"));
    assert!(!validation::is_valid_identifier("1build"));
    assert!(!validation::is_valid_identifier("build-"));
    assert!(!validation::is_valid_identifier("build--all"));
    assert!(!validation::is_valid_identifier(""));
    assert!(!validation::is_valid_identifier("snake_case"));
}

// ============================================================
// property-based round-trip strategies
// ============================================================

fn arb_ident() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_-]{0,7}".prop_map(|s| s)
}

fn arb_text() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 _.-]{0,12}".prop_map(|s| s)
}

fn arb_char() -> impl Strategy<Value = char> {
    prop::char::range('!', '~')
}

/// Self-contained `SchemaType`s (primitives plus a few composites, no `Ref`s),
/// so the embedding `Tool` needs no shared `defs`. The `defs`/`Ref` path is
/// covered separately by [`tool_with_defs_round_trip`].
fn arb_schema_type() -> impl Strategy<Value = SchemaType> {
    let leaf = prop_oneof![
        Just(SchemaType::string()),
        Just(SchemaType::bool()),
        Just(SchemaType::s32()),
        Just(SchemaType::s64()),
        Just(SchemaType::u32()),
        Just(SchemaType::f64()),
        prop::collection::vec(arb_ident(), 1..4).prop_map(SchemaType::r#enum),
    ];
    leaf.prop_recursive(3, 24, 3, |inner| {
        prop_oneof![
            inner.clone().prop_map(SchemaType::list),
            inner.clone().prop_map(SchemaType::option),
            prop::collection::vec((arb_ident(), inner.clone()), 0..3).prop_map(|fields| {
                SchemaType::record(
                    fields
                        .into_iter()
                        .map(|(name, body)| NamedFieldType {
                            name,
                            body,
                            metadata: Default::default(),
                        })
                        .collect(),
                )
            }),
        ]
    })
}

fn arb_example() -> impl Strategy<Value = Example> {
    (arb_text(), arb_text()).prop_map(|(title, body)| Example { title, body })
}

fn arb_doc() -> impl Strategy<Value = Doc> {
    (
        arb_text(),
        arb_text(),
        prop::collection::vec(arb_example(), 0..3),
    )
        .prop_map(|(summary, description, examples)| Doc {
            summary,
            description,
            examples,
        })
}

fn arb_bool_flag_shape() -> impl Strategy<Value = BoolFlagShape> {
    (any::<bool>(), any::<bool>())
        .prop_map(|(default, negatable)| BoolFlagShape { default, negatable })
}

fn arb_flag_shape() -> impl Strategy<Value = FlagShape> {
    prop_oneof![
        arb_bool_flag_shape().prop_map(FlagShape::BoolFlag),
        prop::option::of(any::<u32>()).prop_map(FlagShape::CountFlag),
    ]
}

fn arb_flag_spec() -> impl Strategy<Value = FlagSpec> {
    (
        arb_ident(),
        prop::option::of(arb_char()),
        prop::collection::vec(arb_ident(), 0..3),
        arb_doc(),
        arb_flag_shape(),
        prop::option::of(arb_ident()),
    )
        .prop_map(|(long, short, aliases, doc, shape, env_var)| FlagSpec {
            long,
            short,
            aliases,
            doc,
            shape,
            env_var,
        })
}

fn arb_repetition() -> impl Strategy<Value = Repetition> {
    prop_oneof![
        Just(Repetition::Repeated),
        arb_char().prop_map(Repetition::Delimited),
        arb_char().prop_map(Repetition::Either),
    ]
}

fn arb_quantifier() -> impl Strategy<Value = Quantifier> {
    prop_oneof![Just(Quantifier::All), Just(Quantifier::Any)]
}

fn arb_stream_spec() -> impl Strategy<Value = StreamSpec> {
    (
        arb_doc(),
        prop::collection::vec(arb_ident(), 0..3),
        any::<bool>(),
    )
        .prop_map(|(doc, mime, required)| StreamSpec {
            doc,
            mime,
            required,
        })
}

fn arb_formatter() -> impl Strategy<Value = Formatter> {
    (arb_ident(), arb_doc()).prop_map(|(name, doc)| Formatter { name, doc })
}

fn arb_error_kind() -> impl Strategy<Value = ErrorKind> {
    prop_oneof![Just(ErrorKind::UsageError), Just(ErrorKind::RuntimeError)]
}

fn arb_command_annotations() -> impl Strategy<Value = CommandAnnotations> {
    (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()).prop_map(
        |(read_only, destructive, idempotent, open_world)| CommandAnnotations {
            read_only,
            destructive,
            idempotent,
            open_world,
        },
    )
}

fn arb_option_shape() -> impl Strategy<Value = OptionShape> {
    prop_oneof![
        arb_schema_type().prop_map(OptionShape::Scalar),
        arb_schema_type().prop_map(OptionShape::OptionalScalar),
        (arb_repetition(), arb_schema_type()).prop_map(|(repetition, type_)| {
            OptionShape::Repeatable(RepeatableShape { repetition, type_ })
        }),
    ]
}

fn arb_option_spec() -> impl Strategy<Value = OptionSpec> {
    (
        arb_ident(),
        prop::option::of(arb_char()),
        prop::collection::vec(arb_ident(), 0..3),
        arb_doc(),
        prop::option::of(arb_ident()),
        arb_option_shape(),
        prop::option::of(schema_value_strategy()),
        any::<bool>(),
        prop::option::of(arb_ident()),
    )
        .prop_map(
            |(long, short, aliases, doc, value_name, shape, default, required, env_var)| {
                OptionSpec {
                    long,
                    short,
                    aliases,
                    doc,
                    value_name,
                    shape,
                    default,
                    required,
                    env_var,
                }
            },
        )
}

fn arb_positional() -> impl Strategy<Value = Positional> {
    (
        arb_ident(),
        arb_doc(),
        prop::option::of(arb_ident()),
        arb_schema_type(),
        prop::option::of(schema_value_strategy()),
        any::<bool>(),
    )
        .prop_map(
            |(name, doc, value_name, type_, default, required)| Positional {
                name,
                doc,
                value_name,
                type_,
                default,
                required,
            },
        )
}

fn arb_tail_positional() -> impl Strategy<Value = TailPositional> {
    (
        arb_ident(),
        arb_doc(),
        prop::option::of(arb_ident()),
        arb_schema_type(),
        any::<u32>(),
        prop::option::of(any::<u32>()),
        prop::option::of(arb_ident()),
        any::<bool>(),
    )
        .prop_map(
            |(name, doc, value_name, item_type, min, max, separator, verbatim)| TailPositional {
                name,
                doc,
                value_name,
                item_type,
                min,
                max,
                separator,
                verbatim,
            },
        )
}

fn arb_positionals() -> impl Strategy<Value = Positionals> {
    (
        prop::collection::vec(arb_positional(), 0..3),
        prop::option::of(arb_tail_positional()),
    )
        .prop_map(|(fixed, tail)| Positionals { fixed, tail })
}

fn arb_value_is_ref() -> impl Strategy<Value = ValueIsRef> {
    (arb_ident(), schema_value_strategy()).prop_map(|(name, value)| ValueIsRef { name, value })
}

fn arb_ref() -> impl Strategy<Value = Ref> {
    prop_oneof![
        arb_ident().prop_map(Ref::Present),
        arb_value_is_ref().prop_map(Ref::ValueIs),
    ]
}

fn arb_refs() -> impl Strategy<Value = Vec<Ref>> {
    prop::collection::vec(arb_ref(), 0..3)
}

fn arb_ref_group() -> impl Strategy<Value = RefGroup> {
    arb_refs().prop_map(|refs| RefGroup { refs })
}

fn arb_constraint() -> impl Strategy<Value = Constraint> {
    prop_oneof![
        arb_refs().prop_map(Constraint::RequiresAll),
        arb_refs().prop_map(Constraint::AllOrNone),
        arb_refs().prop_map(Constraint::RequiresAny),
        prop::collection::vec(arb_ref_group(), 0..3).prop_map(Constraint::MutexGroups),
        (arb_quantifier(), arb_refs(), arb_quantifier(), arb_refs()).prop_map(
            |(lhs_quant, lhs, rhs_quant, rhs)| Constraint::Implies(ImpliesC {
                lhs_quant,
                lhs,
                rhs_quant,
                rhs,
            })
        ),
        (arb_quantifier(), arb_refs(), arb_refs()).prop_map(|(lhs_quant, lhs, rhs)| {
            Constraint::Forbids(ForbidsC {
                lhs_quant,
                lhs,
                rhs,
            })
        }),
    ]
}

fn arb_result_spec() -> impl Strategy<Value = ResultSpec> {
    (
        arb_schema_type(),
        arb_doc(),
        prop::collection::vec(arb_formatter(), 0..3),
        arb_ident(),
    )
        .prop_map(|(type_, doc, formatters, default_formatter)| ResultSpec {
            type_,
            doc,
            formatters,
            default_formatter,
        })
}

fn arb_error_case() -> impl Strategy<Value = ErrorCase> {
    (
        arb_ident(),
        arb_doc(),
        arb_error_kind(),
        any::<u8>(),
        prop::option::of(arb_schema_type()),
    )
        .prop_map(|(name, doc, kind, exit_code, payload)| ErrorCase {
            name,
            doc,
            kind,
            exit_code,
            payload,
        })
}

fn arb_globals() -> impl Strategy<Value = Globals> {
    (
        prop::collection::vec(arb_option_spec(), 0..3),
        prop::collection::vec(arb_flag_spec(), 0..3),
    )
        .prop_map(|(options, flags)| Globals { options, flags })
}

fn arb_command_body() -> impl Strategy<Value = CommandBody> {
    (
        arb_positionals(),
        prop::collection::vec(arb_option_spec(), 0..2),
        prop::collection::vec(arb_flag_spec(), 0..2),
        prop::collection::vec(arb_constraint(), 0..2),
        prop::option::of(arb_stream_spec()),
        prop::option::of(arb_stream_spec()),
        prop::option::of(arb_result_spec()),
        prop::collection::vec(arb_error_case(), 0..2),
        prop::option::of(arb_command_annotations()),
    )
        .prop_map(
            |(
                positionals,
                options,
                flags,
                constraints,
                stdin,
                stdout,
                result,
                errors,
                annotations,
            )| {
                CommandBody {
                    positionals,
                    options,
                    flags,
                    constraints,
                    stdin,
                    stdout,
                    result,
                    errors,
                    annotations,
                }
            },
        )
}

fn arb_command_node() -> impl Strategy<Value = CommandNode> {
    (
        arb_ident(),
        prop::collection::vec(arb_ident(), 0..3),
        arb_doc(),
        arb_globals(),
        prop::collection::vec(any::<i32>().prop_map(CommandIndex), 0..3),
        prop::option::of(arb_command_body()),
    )
        .prop_map(
            |(name, aliases, doc, globals, subcommands, body)| CommandNode {
                name,
                aliases,
                doc,
                globals,
                subcommands,
                body,
            },
        )
}

fn arb_command_tree() -> impl Strategy<Value = CommandTree> {
    prop::collection::vec(arb_command_node(), 1..4).prop_map(|nodes| CommandTree { nodes })
}

fn arb_tool() -> impl Strategy<Value = Tool> {
    (arb_text(), arb_command_tree()).prop_map(|(version, commands)| Tool {
        version,
        commands,
        schema: SchemaGraph::empty(),
    })
}

// ============================================================
// property-based round-trip tests
// ============================================================
//
// The `From`-based, context-free conversions round-trip directly. The
// graph-folding / value-bearing conversions are exercised by embedding an
// arbitrary value in a minimal `Tool` and round-tripping through the public
// `encode_tool` / `decode_tool` boundary.

/// Round-trip a tool through the wire form.
fn rt(tool: &Tool) -> Tool {
    let wire = encode_tool(tool).expect("native -> wire");
    decode_tool(&wire).expect("wire -> native")
}

/// A single-root tool whose body is produced by `f`.
fn body_with(f: impl FnOnce(&mut CommandBody)) -> Tool {
    let mut body = empty_body();
    f(&mut body);
    let mut node = root("root");
    node.body = Some(body);
    tool_with_root(node)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn doc_round_trip(d in arb_doc()) {
        prop_assert_eq!(Doc::from(&wire::Doc::from(&d)), d);
    }

    #[test]
    fn example_round_trip(e in arb_example()) {
        prop_assert_eq!(Example::from(&wire::Example::from(&e)), e);
    }

    #[test]
    fn flag_spec_round_trip(f in arb_flag_spec()) {
        prop_assert_eq!(FlagSpec::from(&wire::FlagSpec::from(&f)), f);
    }

    #[test]
    fn flag_shape_round_trip(s in arb_flag_shape()) {
        prop_assert_eq!(FlagShape::from(&wire::FlagShape::from(&s)), s);
    }

    #[test]
    fn bool_flag_shape_round_trip(s in arb_bool_flag_shape()) {
        prop_assert_eq!(BoolFlagShape::from(&wire::BoolFlagShape::from(&s)), s);
    }

    #[test]
    fn repetition_round_trip(r in arb_repetition()) {
        prop_assert_eq!(Repetition::from(&wire::Repetition::from(&r)), r);
    }

    #[test]
    fn quantifier_round_trip(q in arb_quantifier()) {
        prop_assert_eq!(Quantifier::from(&wire::Quantifier::from(&q)), q);
    }

    #[test]
    fn stream_spec_round_trip(s in arb_stream_spec()) {
        prop_assert_eq!(StreamSpec::from(&wire::StreamSpec::from(&s)), s);
    }

    #[test]
    fn formatter_round_trip(f in arb_formatter()) {
        prop_assert_eq!(Formatter::from(&wire::Formatter::from(&f)), f);
    }

    #[test]
    fn error_kind_round_trip(k in arb_error_kind()) {
        prop_assert_eq!(ErrorKind::from(&wire::ErrorKind::from(&k)), k);
    }

    #[test]
    fn command_annotations_round_trip(a in arb_command_annotations()) {
        prop_assert_eq!(CommandAnnotations::from(&wire::CommandAnnotations::from(&a)), a);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn option_spec_round_trip(o in arb_option_spec()) {
        let tool = body_with(move |b| b.options = vec![o]);
        prop_assert_eq!(rt(&tool), tool);
    }

    #[test]
    fn positional_round_trip(p in arb_positional()) {
        let tool = body_with(move |b| b.positionals.fixed = vec![p]);
        prop_assert_eq!(rt(&tool), tool);
    }

    #[test]
    fn tail_positional_round_trip(t in arb_tail_positional()) {
        let tool = body_with(move |b| b.positionals.tail = Some(t));
        prop_assert_eq!(rt(&tool), tool);
    }

    #[test]
    fn positionals_round_trip(p in arb_positionals()) {
        let tool = body_with(move |b| b.positionals = p);
        prop_assert_eq!(rt(&tool), tool);
    }

    #[test]
    fn option_shape_round_trip(shape in arb_option_shape()) {
        let opt = OptionSpec {
            long: "opt".to_string(),
            short: None,
            aliases: Vec::new(),
            doc: Doc::default(),
            value_name: None,
            shape,
            default: None,
            required: false,
            env_var: None,
        };
        let tool = body_with(move |b| b.options = vec![opt]);
        prop_assert_eq!(rt(&tool), tool);
    }

    #[test]
    fn result_spec_round_trip(r in arb_result_spec()) {
        let tool = body_with(move |b| b.result = Some(r));
        prop_assert_eq!(rt(&tool), tool);
    }

    #[test]
    fn error_case_round_trip(e in arb_error_case()) {
        let tool = body_with(move |b| b.errors = vec![e]);
        prop_assert_eq!(rt(&tool), tool);
    }

    #[test]
    fn constraint_round_trip(c in arb_constraint()) {
        let tool = body_with(move |b| b.constraints = vec![c]);
        prop_assert_eq!(rt(&tool), tool);
    }

    #[test]
    fn command_body_round_trip(body in arb_command_body()) {
        let tool = body_with(move |b| *b = body);
        prop_assert_eq!(rt(&tool), tool);
    }

    #[test]
    fn globals_round_trip(g in arb_globals()) {
        let mut node = root("root");
        node.globals = g;
        let tool = tool_with_root(node);
        prop_assert_eq!(rt(&tool), tool);
    }

    #[test]
    fn command_node_round_trip(n in arb_command_node()) {
        let tool = tool_with_root(n);
        prop_assert_eq!(rt(&tool), tool);
    }

    #[test]
    fn command_tree_round_trip(ct in arb_command_tree()) {
        let tool = Tool {
            version: "1.0.0".to_string(),
            commands: ct,
            schema: SchemaGraph::empty(),
        };
        prop_assert_eq!(rt(&tool), tool);
    }

    #[test]
    fn tool_round_trip(tool in arb_tool()) {
        prop_assert_eq!(rt(&tool), tool);
    }

    /// Exercises the shared `defs` / `Ref` path: an arbitrary set of named
    /// definitions plus a position that references the first one.
    #[test]
    fn tool_with_defs_round_trip(graph in schema_graph_strategy()) {
        let mut schema = SchemaGraph::empty();
        schema.defs = graph.defs.clone();
        let type_ = graph
            .defs
            .first()
            .map(|d| SchemaType::ref_to(d.id.clone()))
            .unwrap_or_else(SchemaType::string);
        let mut body = empty_body();
        body.positionals.fixed = vec![Positional {
            name: "input".to_string(),
            doc: Doc::default(),
            value_name: None,
            type_,
            default: None,
            required: true,
        }];
        let mut node = root("root");
        node.body = Some(body);
        let tool = Tool {
            version: "1.0.0".to_string(),
            commands: CommandTree { nodes: vec![node] },
            schema,
        };
        prop_assert_eq!(rt(&tool), tool);
    }
}
