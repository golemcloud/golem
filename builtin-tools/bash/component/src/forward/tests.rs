//! What the check of each built-in tool definition must accept and refuse.

use golem_schema::{
    SchemaType,
    tool::{CommandIndex, Doc, FlagShape, FlagSpec, StreamSpec, TailPositional, Tool},
};

/// A definition as the host gives it to a tool that has the built-in tool bound.
fn definition(json: &str) -> Tool {
    serde_json::from_str(json).unwrap()
}

/// A type that `type_` is not.
fn another(type_: &SchemaType) -> SchemaType {
    match type_ {
        SchemaType::Bool { .. } => SchemaType::string(),
        _ => SchemaType::bool(),
    }
}

/// `tool` after each single change that makes it another tool, with a name for the change.
fn changes(tool: &Tool) -> Vec<(String, Tool)> {
    let mut changes = Vec::new();
    let mut change = |name: String, change: &dyn Fn(&mut Tool)| {
        let mut changed = tool.clone();
        change(&mut changed);
        changes.push((name, changed));
    };
    let stream = || {
        Some(StreamSpec {
            doc: Doc::default(),
            mime: vec![],
            required: false,
        })
    };
    change("no filesystem".into(), &|tool| {
        tool.requires_filesystem = false;
    });
    change("a second command".into(), &|tool| {
        let command = tool.commands.nodes[0].clone();
        tool.commands.nodes.push(command);
    });
    change("a subcommand".into(), &|tool| {
        tool.commands.nodes[0].subcommands.push(CommandIndex(0));
    });
    change("no body".into(), &|tool| tool.commands.nodes[0].body = None);
    let body = tool.commands.nodes[0].body.as_ref().unwrap();
    change("the other read-only annotation".into(), &|tool| {
        let notes = tool.commands.nodes[0].body.as_mut().unwrap();
        let notes = notes.annotations.as_mut().unwrap();
        notes.read_only = !notes.read_only;
    });
    change("no annotations".into(), &|tool| {
        tool.commands.nodes[0].body.as_mut().unwrap().annotations = None;
    });
    change("a standard input".into(), &|tool| {
        tool.commands.nodes[0].body.as_mut().unwrap().stdin = stream();
    });
    change("a standard output".into(), &|tool| {
        tool.commands.nodes[0].body.as_mut().unwrap().stdout = stream();
    });
    change("a standard error".into(), &|tool| {
        tool.commands.nodes[0].body.as_mut().unwrap().stderr = stream();
    });
    change("no result".into(), &|tool| {
        tool.commands.nodes[0].body.as_mut().unwrap().result = None;
    });
    for index in 0..body.positionals.fixed.len() {
        let name = &body.positionals.fixed[index].name;
        change(format!("argument {name} with another name"), &|tool| {
            let body = tool.commands.nodes[0].body.as_mut().unwrap();
            body.positionals.fixed[index].name = "other".into();
        });
        change(format!("argument {name} required or not"), &|tool| {
            let body = tool.commands.nodes[0].body.as_mut().unwrap();
            let argument = &mut body.positionals.fixed[index];
            argument.required = !argument.required;
        });
        change(format!("argument {name} with another type"), &|tool| {
            let body = tool.commands.nodes[0].body.as_mut().unwrap();
            let argument = &mut body.positionals.fixed[index];
            argument.type_ = another(&argument.type_);
        });
        change(format!("no argument {name}"), &|tool| {
            let body = tool.commands.nodes[0].body.as_mut().unwrap();
            body.positionals.fixed.remove(index);
        });
    }
    change("one more argument".into(), &|tool| {
        let body = tool.commands.nodes[0].body.as_mut().unwrap();
        let argument = body.positionals.fixed[0].clone();
        body.positionals.fixed.push(argument);
    });
    change("a tail argument".into(), &|tool| {
        let body = tool.commands.nodes[0].body.as_mut().unwrap();
        body.positionals.tail = Some(TailPositional {
            name: "more".into(),
            doc: Doc::default(),
            value_name: None,
            item_type: SchemaType::string(),
            min: 0,
            max: None,
            separator: None,
            verbatim: false,
            accepts_stdio: false,
        });
    });
    for index in 0..body.flags.len() {
        let name = &body.flags[index].long;
        change(format!("flag {name} with another name"), &|tool| {
            let body = tool.commands.nodes[0].body.as_mut().unwrap();
            body.flags[index].long = "other".into();
        });
        change(format!("no flag {name}"), &|tool| {
            tool.commands.nodes[0]
                .body
                .as_mut()
                .unwrap()
                .flags
                .remove(index);
        });
    }
    change("one more flag".into(), &|tool| {
        tool.commands.nodes[0]
            .body
            .as_mut()
            .unwrap()
            .flags
            .push(FlagSpec {
                long: "verbose".into(),
                short: None,
                aliases: vec![],
                doc: Doc::default(),
                shape: FlagShape::CountFlag(None),
                env_var: None,
            });
    });
    for (index, shared) in tool.schema.defs.iter().enumerate() {
        let of = shared.name.clone().unwrap_or_default();
        match &shared.body {
            SchemaType::Record { fields, .. } => {
                for field in 0..fields.len() {
                    let name = &fields[field].name;
                    change(format!("{of}.{name} with another name"), &|tool| {
                        let SchemaType::Record { fields, .. } = &mut tool.schema.defs[index].body
                        else {
                            unreachable!()
                        };
                        fields[field].name = "other".into();
                    });
                    change(format!("{of}.{name} with another type"), &|tool| {
                        let SchemaType::Record { fields, .. } = &mut tool.schema.defs[index].body
                        else {
                            unreachable!()
                        };
                        fields[field].body = another(&fields[field].body);
                    });
                    change(format!("{of} without {name}"), &|tool| {
                        let SchemaType::Record { fields, .. } = &mut tool.schema.defs[index].body
                        else {
                            unreachable!()
                        };
                        fields.remove(field);
                    });
                }
            }
            SchemaType::Enum { cases, .. } => {
                for case in 0..cases.len() {
                    let name = &cases[case];
                    change(format!("{of}.{name} with another name"), &|tool| {
                        let SchemaType::Enum { cases, .. } = &mut tool.schema.defs[index].body
                        else {
                            unreachable!()
                        };
                        cases[case] = "Other".into();
                    });
                }
                change(format!("{of} without its last case"), &|tool| {
                    let SchemaType::Enum { cases, .. } = &mut tool.schema.defs[index].body else {
                        unreachable!()
                    };
                    cases.pop();
                });
            }
            SchemaType::Variant { cases, .. } => {
                for case in 0..cases.len() {
                    let name = &cases[case].name;
                    change(format!("{of}.{name} with another name"), &|tool| {
                        let SchemaType::Variant { cases, .. } = &mut tool.schema.defs[index].body
                        else {
                            unreachable!()
                        };
                        cases[case].name = "Other".into();
                    });
                    change(format!("{of}.{name} with another payload"), &|tool| {
                        let SchemaType::Variant { cases, .. } = &mut tool.schema.defs[index].body
                        else {
                            unreachable!()
                        };
                        cases[case].payload = match &cases[case].payload {
                            Some(payload) if cases[case].name != "Other" => Some(another(payload)),
                            Some(_) => None,
                            None => Some(SchemaType::bool()),
                        };
                    });
                }
                change(format!("{of} without its last case"), &|tool| {
                    let SchemaType::Variant { cases, .. } = &mut tool.schema.defs[index].body
                    else {
                        unreachable!()
                    };
                    cases.pop();
                });
            }
            other => panic!("{of} has a type that this test does not change: {other:?}"),
        }
    }
    changes
}

/// Asserts that `accepts` takes the built-in definition `json`, and that it refuses the
/// definition after each single change.
pub(super) fn assert_accepts_only(json: &str, accepts: fn(&Tool) -> bool) {
    let tool = definition(json);
    assert!(accepts(&tool), "the built-in definition is refused");
    let changes = changes(&tool);
    assert!(changes.len() > 20, "{} changes", changes.len());
    let accepted: Vec<String> = changes
        .into_iter()
        .filter(|(_, changed)| accepts(changed))
        .map(|(name, _)| name)
        .collect();
    assert!(accepted.is_empty(), "accepted with: {accepted:?}");
}
