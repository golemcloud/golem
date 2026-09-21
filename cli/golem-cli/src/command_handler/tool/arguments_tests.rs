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

use super::arguments::{ParsedToolArguments, parse};
use golem_common::schema::tool::*;
use golem_common::schema::{SchemaGraph, SchemaType, SchemaValue};
use test_r::test;

fn body() -> CommandBody {
    CommandBody {
        positionals: Positionals::default(),
        options: vec![],
        flags: vec![],
        constraints: vec![],
        stdin: None,
        stdout: None,
        result: None,
        errors: vec![],
        annotations: None,
    }
}

fn tool(body: CommandBody) -> Tool {
    Tool {
        version: "1.0.0".into(),
        schema: SchemaGraph::empty(),
        commands: CommandTree {
            nodes: vec![CommandNode {
                name: "example".into(),
                aliases: vec![],
                doc: Doc::default(),
                globals: Globals::default(),
                subcommands: vec![],
                body: Some(body),
            }],
        },
    }
}

fn positional(name: &str, type_: SchemaType) -> Positional {
    Positional {
        name: name.into(),
        type_,
        doc: Doc::default(),
        value_name: None,
        default: None,
        required: true,
        accepts_stdio: false,
    }
}

fn option(name: &str, shape: OptionShape) -> OptionSpec {
    OptionSpec {
        long: name.into(),
        short: None,
        aliases: vec![],
        doc: Doc::default(),
        value_name: None,
        shape,
        default: None,
        required: false,
        env_var: None,
    }
}

fn flag(name: &str, short: char, shape: FlagShape) -> FlagSpec {
    FlagSpec {
        long: name.into(),
        short: Some(short),
        aliases: vec![],
        doc: Doc::default(),
        shape,
        env_var: None,
    }
}

fn parsed(tool: &Tool, argv: &[&str]) -> Result<ParsedToolArguments, String> {
    parse(
        tool,
        &argv.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    )
}

fn values(tool: &Tool, argv: &[&str]) -> Vec<SchemaValue> {
    let ParsedToolArguments::Invoke { input, .. } = parsed(tool, argv).unwrap() else {
        panic!("unexpected help")
    };
    let (_, SchemaValue::Record { fields }) = input.into_parts() else {
        panic!("not a record")
    };
    fields
}

#[test]
fn schema_directed_scalars_optional_strings_and_rejections() {
    let mut b = body();
    b.positionals.fixed = vec![
        positional("text", SchemaType::string()),
        positional("number", SchemaType::s32()),
    ];
    b.options.push(option(
        "label",
        OptionShape::Scalar(SchemaType::option(SchemaType::string())),
    ));
    let t = tool(b);
    assert_eq!(
        values(&t, &["hello world", "-17", "--label", "null"]),
        vec![
            SchemaValue::String("hello world".into()),
            SchemaValue::S32(-17),
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::String("null".into())))
            },
        ]
    );
    assert_eq!(
        values(&t, &["x", "8"])[2],
        SchemaValue::Option { inner: None }
    );
    for args in [
        vec!["x"],
        vec!["x", "2147483648"],
        vec!["x", "2", "--unknown"],
        vec!["x", "2", "--label", "a", "--label", "b"],
    ] {
        assert!(parsed(&t, &args).is_err(), "accepted {args:?}");
    }
}

#[test]
fn subcommands_aliases_inherited_globals_and_help() {
    let mut t = tool(body());
    t.commands.nodes[0].body = None;
    t.commands.nodes[0].subcommands = vec![CommandIndex(1)];
    t.commands.nodes[0]
        .globals
        .flags
        .push(flag("verbose", 'v', FlagShape::CountFlag(Some(3))));
    let mut child = tool(body()).commands.nodes.remove(0);
    child.name = "repeat".into();
    child.aliases = vec!["r".into()];
    child
        .body
        .as_mut()
        .unwrap()
        .positionals
        .fixed
        .push(positional("value", SchemaType::string()));
    t.commands.nodes.push(child);
    let ParsedToolArguments::Invoke {
        command_path,
        input,
    } = parsed(&t, &["-vv", "r", "word"]).unwrap()
    else {
        panic!()
    };
    assert_eq!(command_path, ["repeat"]);
    assert_eq!(
        input.into_parts().1,
        SchemaValue::Record {
            fields: vec![SchemaValue::U32(2), SchemaValue::String("word".into())]
        }
    );
    for args in [vec!["--help"], vec!["repeat", "--help"]] {
        let ParsedToolArguments::Help(text) = parsed(&t, &args).unwrap() else {
            panic!()
        };
        assert!(text.contains("repeat"));
        assert!(text.contains("verbose"));
    }
    assert!(parsed(&t, &["repeat"]).is_err());
    assert!(parsed(&t, &["-vvvv", "r", "word"]).is_err());
}

#[test]
fn end_of_options_disables_help_and_subcommand_recognition() {
    let mut b = body();
    b.positionals
        .fixed
        .push(positional("value", SchemaType::string()));
    let mut t = tool(b);
    let mut child = tool(body()).commands.nodes.remove(0);
    child.name = "sub".into();
    t.commands.nodes[0].subcommands = vec![CommandIndex(1)];
    t.commands.nodes.push(child);
    assert_eq!(
        values(&t, &["--", "--help"]),
        vec![SchemaValue::String("--help".into())]
    );
    assert_eq!(
        values(&t, &["--", "sub"]),
        vec![SchemaValue::String("sub".into())]
    );
}

#[test]
fn end_of_options_preserves_fixed_values_and_requires_tail_separator() {
    let mut b = body();
    b.positionals
        .fixed
        .push(positional("value", SchemaType::string()));
    b.positionals.tail = Some(TailPositional {
        name: "rest".into(),
        doc: Doc::default(),
        value_name: None,
        item_type: SchemaType::string(),
        min: 0,
        max: None,
        separator: Some("tail".into()),
        verbatim: true,
        accepts_stdio: false,
    });
    let t = tool(b);
    assert_eq!(
        values(&t, &["--", "tail"]),
        vec![
            SchemaValue::String("tail".into()),
            SchemaValue::List { elements: vec![] }
        ]
    );
    assert_eq!(
        values(&t, &["--", "tail", "tail", "--help", "tail"]),
        vec![
            SchemaValue::String("tail".into()),
            SchemaValue::List {
                elements: vec![
                    SchemaValue::String("--help".into()),
                    SchemaValue::String("tail".into()),
                ]
            }
        ]
    );
    assert!(parsed(&t, &["--", "tail", "--help"]).is_err());
}

#[test]
fn tail_separator_is_not_recognized_before_fixed_positionals_are_filled() {
    let mut b = body();
    b.positionals
        .fixed
        .push(positional("value", SchemaType::string()));
    b.positionals.tail = Some(TailPositional {
        name: "rest".into(),
        doc: Doc::default(),
        value_name: None,
        item_type: SchemaType::string(),
        min: 1,
        max: None,
        separator: Some("tail".into()),
        verbatim: true,
        accepts_stdio: false,
    });
    let mut t = tool(b);

    assert_eq!(
        values(&t, &["tail", "tail", "item"]),
        vec![
            SchemaValue::String("tail".into()),
            SchemaValue::List {
                elements: vec![SchemaValue::String("item".into())],
            },
        ]
    );
    t.commands.nodes[0].body.as_mut().unwrap().positionals.fixed[0].default =
        Some(SchemaValue::String("fallback".into()));
    assert_eq!(
        values(&t, &["tail", "item"]),
        vec![
            SchemaValue::String("fallback".into()),
            SchemaValue::List {
                elements: vec![SchemaValue::String("item".into())]
            },
        ]
    );
}

#[test]
fn list_and_map_repetition_honor_metadata() {
    let mut b = body();
    b.options.push(option(
        "tag",
        OptionShape::RepeatableList(RepeatableListShape {
            repetition: Repetition::Either(','),
            item_type: SchemaType::string(),
        }),
    ));
    b.options.push(option(
        "set",
        OptionShape::RepeatableMap(RepeatableMapShape {
            repetition: Repetition::Repeated,
            map_type: SchemaType::map(SchemaType::string(), SchemaType::u32()),
            duplicate_key_policy: DuplicateKeyPolicy::Reject,
        }),
    ));
    let mut t = tool(b);
    assert_eq!(
        values(&t, &["--tag=a,b", "--tag", "c", "--set", "x=7"]),
        vec![
            SchemaValue::List {
                elements: vec![
                    SchemaValue::String("a".into()),
                    SchemaValue::String("b".into()),
                    SchemaValue::String("c".into())
                ]
            },
            SchemaValue::Map {
                entries: vec![(SchemaValue::String("x".into()), SchemaValue::U32(7))]
            },
        ]
    );
    assert!(parsed(&t, &["--set", "x=1", "--set", "x=2"]).is_err());
    if let OptionShape::RepeatableMap(shape) =
        &mut t.commands.nodes[0].body.as_mut().unwrap().options[1].shape
    {
        shape.duplicate_key_policy = DuplicateKeyPolicy::LastWins;
    }
    assert_eq!(
        values(&t, &["--set=x=1", "--set=x=9"])[1],
        SchemaValue::Map {
            entries: vec![(SchemaValue::String("x".into()), SchemaValue::U32(9))]
        }
    );
    if let OptionShape::RepeatableList(shape) =
        &mut t.commands.nodes[0].body.as_mut().unwrap().options[0].shape
    {
        shape.repetition = Repetition::Delimited(',');
    }
    assert!(parsed(&t, &["--tag=a", "--tag=b"]).is_err());
}

#[test]
fn defaults_bare_optional_values_and_negation() {
    let mut b = body();
    let mut o = option("color", OptionShape::OptionalScalar(SchemaType::string()));
    o.default = Some(SchemaValue::String("auto".into()));
    o.short = Some('c');
    o.aliases = vec!["colour".into()];
    b.options.push(o);
    b.flags.push(flag(
        "enabled",
        'e',
        FlagShape::BoolFlag(BoolFlagShape {
            default: true,
            negatable: true,
        }),
    ));
    let t = tool(b);
    assert_eq!(
        values(&t, &[]),
        vec![SchemaValue::String("auto".into()), SchemaValue::Bool(true)]
    );
    assert_eq!(
        values(&t, &["--color", "--no-enabled"]),
        vec![SchemaValue::String("auto".into()), SchemaValue::Bool(false)]
    );
    assert_eq!(
        values(&t, &["--colour=never"])[0],
        SchemaValue::String("never".into())
    );
    assert_eq!(
        values(&t, &["-calways"])[0],
        SchemaValue::String("always".into())
    );
}

#[test]
fn literal_flag_names_and_aliases_win_over_generated_negation() {
    for reversed in [false, true] {
        for alias in [false, true] {
            let mut b = body();
            b.flags.push(flag(
                "foo",
                'f',
                FlagShape::BoolFlag(BoolFlagShape {
                    default: true,
                    negatable: true,
                }),
            ));
            let mut literal = flag(
                if alias { "other" } else { "no-foo" },
                'n',
                FlagShape::CountFlag(None),
            );
            if alias {
                literal.aliases.push("no-foo".into());
            }
            b.flags.push(literal);
            let mut expected = vec![SchemaValue::Bool(true), SchemaValue::U32(1)];
            if reversed {
                b.flags.reverse();
                expected.reverse();
            }
            assert_eq!(values(&tool(b), &["--no-foo"]), expected);
        }
    }
}

#[test]
fn tail_separator_verbatim_and_cardinality() {
    let mut b = body();
    b.positionals.tail = Some(TailPositional {
        name: "files".into(),
        doc: Doc::default(),
        value_name: None,
        item_type: SchemaType::string(),
        min: 1,
        max: Some(2),
        separator: Some("::".into()),
        verbatim: true,
        accepts_stdio: false,
    });
    let t = tool(b);
    assert_eq!(
        values(&t, &["::", "--help"]),
        vec![SchemaValue::List {
            elements: vec![SchemaValue::String("--help".into())]
        }]
    );
    for args in [vec![], vec!["file"], vec!["::", "a", "b", "c"]] {
        assert!(parsed(&t, &args).is_err());
    }
}

#[test]
fn declared_separators_take_precedence_over_generated_syntax() {
    for separator in ["--help", "-h", "--", "tail", "::"] {
        for verbatim in [false, true] {
            let mut b = body();
            b.positionals.tail = Some(TailPositional {
                name: "files".into(),
                doc: Doc::default(),
                value_name: None,
                item_type: SchemaType::string(),
                min: 1,
                max: None,
                separator: Some(separator.into()),
                verbatim,
                accepts_stdio: false,
            });
            b.flags
                .push(flag("verbose", 'v', FlagShape::CountFlag(None)));
            let t = tool(b);
            let items = if verbatim {
                vec!["-v", "item"]
            } else {
                vec!["item"]
            };
            assert_eq!(
                values(&t, &[separator, "-v", "item"]),
                vec![
                    SchemaValue::List {
                        elements: items
                            .into_iter()
                            .map(|s| SchemaValue::String(s.into()))
                            .collect()
                    },
                    SchemaValue::U32(if verbatim { 0 } else { 1 }),
                ],
                "separator={separator:?}, verbatim={verbatim}",
            );
        }
    }
}

#[test]
fn declared_help_option_and_its_value_are_not_generated_help() {
    let mut b = body();
    let mut help = option(
        "manual",
        OptionShape::Scalar(SchemaType::option(SchemaType::string())),
    );
    help.aliases.push("help".into());
    help.short = Some('h');
    b.options.push(help);
    b.flags
        .push(flag("verbose", 'v', FlagShape::CountFlag(None)));
    let t = tool(b);
    for argv in [vec!["--help", "--help", "-v"], vec!["-vh--help"]] {
        assert_eq!(
            values(&t, &argv),
            vec![
                SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::String("--help".into())))
                },
                SchemaValue::U32(1),
            ]
        );
    }
}

#[test]
fn declared_numeric_short_flag_takes_precedence_over_negative_positional_syntax() {
    for short in ('0'..='9').chain(['.', '-', 'λ']) {
        let mut b = body();
        b.flags
            .push(flag("count", short, FlagShape::CountFlag(None)));
        let t = tool(b);
        assert_eq!(
            values(&t, &[&format!("-{short}")]),
            vec![SchemaValue::U32(1)]
        );
        assert_eq!(
            values(&t, &[&format!("-{short}{short}")]),
            vec![SchemaValue::U32(2)]
        );
    }
}

#[test]
fn numeric_short_declarations_and_negative_values_are_unambiguous() {
    let mut b = body();
    b.positionals
        .fixed
        .push(positional("value", SchemaType::s32()));
    b.flags.push(flag("one", '1', FlagShape::CountFlag(None)));
    let t = tool(b);
    for (args, value, count) in [
        (vec!["-2"], -2, 0),
        (vec!["-1", "-2"], -2, 1),
        (vec!["--", "-1"], -1, 0),
        (vec!["-11", "--", "-1"], -1, 2),
    ] {
        assert_eq!(
            values(&t, &args),
            vec![SchemaValue::S32(value), SchemaValue::U32(count)]
        );
    }
    assert!(parsed(&t, &["-1"]).is_err());
    assert!(matches!(
        parsed(&t, &["-1h"]).unwrap(),
        ParsedToolArguments::Help(_)
    ));

    let mut t = t;
    let mut factor = option("factor", OptionShape::Scalar(SchemaType::s32()));
    factor.short = Some('2');
    factor.required = true;
    t.commands.nodes[0]
        .body
        .as_mut()
        .unwrap()
        .options
        .push(factor);
    for args in [
        vec!["-12-7", "-3"],
        vec!["-1", "-2", "-7", "-3"],
        vec!["-1", "--factor=-7", "-3"],
    ] {
        assert_eq!(
            values(&t, &args),
            vec![
                SchemaValue::S32(-3),
                SchemaValue::S32(-7),
                SchemaValue::U32(1)
            ]
        );
    }
    assert_eq!(
        values(&t, &["--factor", "-2", "-3"]),
        vec![
            SchemaValue::S32(-3),
            SchemaValue::S32(-2),
            SchemaValue::U32(0)
        ]
    );
}

#[test]
fn constraints_reject_conflicting_flags() {
    let mut b = body();
    b.flags = vec![
        flag(
            "left",
            'l',
            FlagShape::BoolFlag(BoolFlagShape {
                default: false,
                negatable: false,
            }),
        ),
        flag(
            "right",
            'r',
            FlagShape::BoolFlag(BoolFlagShape {
                default: false,
                negatable: false,
            }),
        ),
    ];
    b.constraints.push(Constraint::Forbids(ForbidsC {
        lhs_quant: Quantifier::All,
        lhs: vec![Ref::Present("left".into())],
        rhs: vec![Ref::Present("right".into())],
    }));
    let t = tool(b);
    assert!(parsed(&t, &["--left"]).is_ok());
    assert!(parsed(&t, &["-lr"]).is_err());
}

#[test]
fn tool_metadata_cannot_read_the_callers_environment() {
    let mut b = body();
    let mut o = option(
        "value",
        OptionShape::Scalar(SchemaType::option(SchemaType::string())),
    );
    o.env_var = Some("PATH".into());
    b.options.push(o);
    let mut f = flag("verbose", 'v', FlagShape::CountFlag(Some(3)));
    f.env_var = Some("PATH".into());
    b.flags.push(f);
    let t = tool(b);
    assert_eq!(
        values(&t, &[]),
        vec![SchemaValue::Option { inner: None }, SchemaValue::U32(0)]
    );
    assert_eq!(
        values(&t, &["--value", "explicit", "-v"]),
        vec![
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::String("explicit".into())))
            },
            SchemaValue::U32(1),
        ]
    );
    let ParsedToolArguments::Help(help) = parsed(&t, &["--help"]).unwrap() else {
        panic!()
    };
    assert!(!help.contains("PATH"));
}
