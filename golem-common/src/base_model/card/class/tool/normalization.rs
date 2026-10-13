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

use super::*;
use crate::schema::tool::canonical::CanonicalSurfaceRef;
use crate::schema::tool::{FlagShape, FlagSpec, OptionShape, OptionSpec, Repetition, Tool};

enum Surface<'a> {
    Option(&'a OptionSpec),
    Flag(&'a FlagSpec),
    Negated(&'a FlagSpec),
}

impl Surface<'_> {
    fn name(&self) -> String {
        match self {
            Self::Option(o) => o.long.clone(),
            Self::Flag(f) => f.long.clone(),
            Self::Negated(f) => format!("no-{}", f.long),
        }
    }

    fn matches(&self, name: &str, short: bool) -> bool {
        let (long, aliases, letter) = match self {
            Self::Option(o) => (&o.long, &o.aliases, o.short),
            Self::Flag(f) => (&f.long, &f.aliases, f.short),
            Self::Negated(f) => {
                return !short
                    && (name == format!("no-{}", f.long)
                        || f.aliases.iter().any(|a| name == format!("no-{a}")));
            }
        };
        if short {
            letter.is_some_and(|c| name == c.to_string())
        } else {
            name == long || aliases.iter().any(|a| a == name)
        }
    }
}

impl ToolInvocationPattern {
    /// Without metadata, long names are primary names; short aliases cannot be interpreted.
    pub fn normalize(
        &self,
        definition: Option<&Tool>,
    ) -> Result<CanonicalToolPattern, ToolPatternError> {
        let mut command_path = self.command_path.clone().unwrap_or_default();
        let surfaces = if let Some(tool) = definition {
            let command = tool
                .command_index_by_path(
                    &command_path.iter().map(|p| p.0.clone()).collect::<Vec<_>>(),
                )
                .ok_or_else(|| ToolPatternError::Invalid("unknown command path".into()))?;
            let mut node = 0;
            for segment in &mut command_path {
                node = tool.commands.nodes[node]
                    .subcommands
                    .iter()
                    .filter_map(|n| n.as_usize())
                    .find(|&n| {
                        let candidate = &tool.commands.nodes[n];
                        candidate.name == segment.0 || candidate.aliases.contains(&segment.0)
                    })
                    .expect("command_index_by_path validated the path");
                segment.0 = tool.commands.nodes[node].name.clone();
            }
            let body = tool.commands.nodes[command].body.as_ref().unwrap();
            let mut surfaces = Vec::new();
            for surface in tool.canonical_input_surfaces(command) {
                match surface {
                    CanonicalSurfaceRef::GlobalOption { node, index } => surfaces.push(
                        Surface::Option(&tool.commands.nodes[node].globals.options[index]),
                    ),
                    CanonicalSurfaceRef::BodyOption { index } => {
                        surfaces.push(Surface::Option(&body.options[index]))
                    }
                    CanonicalSurfaceRef::GlobalFlag { node, index } => surfaces.push(
                        Surface::Flag(&tool.commands.nodes[node].globals.flags[index]),
                    ),
                    CanonicalSurfaceRef::BodyFlag { index } => {
                        surfaces.push(Surface::Flag(&body.flags[index]))
                    }
                    _ => {}
                }
            }
            let negated = surfaces.iter().filter_map(|s| match s {
                Surface::Flag(f) if matches!(f.shape, FlagShape::BoolFlag(shape) if shape.negatable) => Some(Surface::Negated(f)),
                _ => None,
            }).collect::<Vec<_>>();
            surfaces.extend(negated);
            Some(surfaces)
        } else {
            None
        };
        let mut result = CanonicalToolPattern {
            command_path,
            flags: vec![],
            positionals: vec![],
        };
        let mut add_flag = |name: &str,
                            short: bool,
                            value: &Option<ToolValuePattern>|
         -> Result<(), ToolPatternError> {
            let surface = match &surfaces {
                Some(surfaces) => {
                    let mut matches = surfaces.iter().filter(|s| s.matches(name, short));
                    let found = matches
                        .next()
                        .ok_or_else(|| ToolPatternError::Invalid(format!("unknown flag {name}")))?;
                    if matches.next().is_some() {
                        return Err(ToolPatternError::Invalid(format!("ambiguous flag {name}")));
                    }
                    Some(found)
                }
                None if short => return Err(ToolPatternError::MissingDefinition(name.into())),
                None => None,
            };
            let name = ToolIdentifier(
                surface
                    .map(|s| s.name())
                    .unwrap_or_else(|| name.to_string()),
            );
            if value.is_some() && matches!(surface, Some(Surface::Flag(_) | Surface::Negated(_))) {
                return Err(ToolPatternError::Invalid(
                    "boolean/count flags do not take value constraints".into(),
                ));
            }
            let separator = match surface {
                Some(Surface::Option(o)) => match &o.shape {
                    OptionShape::RepeatableList(s) => repetition_separator(s.repetition),
                    OptionShape::RepeatableMap(s) => repetition_separator(s.repetition),
                    _ => None,
                },
                _ => None,
            };
            let values = match (separator, value) {
                (Some(separator), Some(value)) => value
                    .split_on_literal(separator)
                    .into_iter()
                    .map(Some)
                    .collect(),
                _ => vec![value.clone()],
            };
            for value in values {
                result.flags.push(CanonicalToolFlag {
                    name: name.clone(),
                    value,
                });
            }
            Ok(())
        };
        for arg in &self.args {
            match arg {
                ToolArgPattern::Positional(value) => result.positionals.push(value.clone()),
                ToolArgPattern::LongFlag { name, value } => add_flag(&name.0, false, value)?,
                ToolArgPattern::ShortFlags { flags, value } => {
                    for (index, flag) in flags.iter().enumerate() {
                        add_flag(
                            &flag.to_string(),
                            true,
                            if index + 1 == flags.len() {
                                value
                            } else {
                                &None
                            },
                        )?;
                    }
                }
            }
        }
        Ok(result)
    }
}

fn repetition_separator(repetition: Repetition) -> Option<char> {
    match repetition {
        Repetition::Repeated => None,
        Repetition::Delimited(c) | Repetition::Either(c) => Some(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tool::*;
    use crate::schema::{SchemaGraph, SchemaType};
    use test_r::test;

    fn definition() -> Tool {
        Tool {
            version: "1".into(),
            requires_filesystem: false,
            schema: SchemaGraph::empty(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: "test".into(),
                    aliases: vec![],
                    doc: Doc::default(),
                    globals: Globals::default(),
                    subcommands: vec![],
                    body: Some(CommandBody {
                        positionals: Positionals::default(),
                        constraints: vec![],
                        stdin: None,
                        stdout: None,
                        stderr: None,
                        result: None,
                        errors: vec![],
                        annotations: None,
                        flags: vec![
                            FlagSpec {
                                long: "recursive".into(),
                                short: Some('r'),
                                aliases: vec!["recurse".into()],
                                doc: Doc::default(),
                                shape: FlagShape::BoolFlag(BoolFlagShape {
                                    default: false,
                                    negatable: true,
                                }),
                                env_var: None,
                            },
                            FlagSpec {
                                long: "force".into(),
                                short: Some('f'),
                                aliases: vec![],
                                doc: Doc::default(),
                                shape: FlagShape::BoolFlag(BoolFlagShape {
                                    default: false,
                                    negatable: false,
                                }),
                                env_var: None,
                            },
                            FlagSpec {
                                long: "verbose".into(),
                                short: Some('v'),
                                aliases: vec![],
                                doc: Doc::default(),
                                shape: FlagShape::CountFlag(None),
                                env_var: None,
                            },
                        ],
                        options: vec![OptionSpec {
                            long: "include".into(),
                            short: Some('I'),
                            aliases: vec!["paths".into()],
                            doc: Doc::default(),
                            value_name: None,
                            shape: OptionShape::RepeatableList(RepeatableListShape {
                                repetition: Repetition::Either(','),
                                item_type: SchemaType::string(),
                            }),
                            default: None,
                            required: false,
                            env_var: None,
                        }],
                    }),
                }],
            },
        }
    }

    #[test]
    fn aliases_are_canonical_before_subsumption() {
        let tool = definition();
        let parse = |s| ToolResourcePattern::parse_with_definition(s, Some(&tool)).unwrap();
        let denial = parse("-rf");
        for resource in [
            "-fr -vv",
            "--force --recurse",
            "--recursive --force --include=extra",
        ] {
            assert!(denial.subsumes(&parse(resource)), "{resource}");
        }
        assert_eq!(
            parse("-I=/workspace/*,/private/**"),
            parse("--include=/workspace/* --paths=/private/**")
        );
        assert!(parse("-vv").subsumes(&parse("--verbose -vv")));
        assert!(!parse("-vv").subsumes(&parse("-v")));
        assert_eq!(parse("--no-recurse"), parse("--no-recursive"));
        assert!(ToolResourcePattern::parse_with_definition("-r", None).is_err());
        assert!(ToolResourcePattern::parse_with_definition("-x", Some(&tool)).is_err());
    }

    #[test]
    fn policy_rendering_preserves_empty_root_and_literal_values() {
        use crate::base_model::card::{PermissionPattern, parse_permission};
        for resource in [
            ".",
            ". -- '' \\* -literal",
            "status --cwd=/workspace/** -- 'two words'",
            "status --value=''",
            ". -- 'trailing '",
            ". --value='trailing '",
            ". -- 'trailing\t'",
            ". --value='trailing\t'",
            ". -- 'trailing\u{2003}'",
            ". --value='trailing\u{2003}'",
            r#"status -- \\ \" \'"#,
        ] {
            let source = format!("tool(a/app/env/component/tool) @ * : invoke : {resource}");
            let parsed = parse_permission(&source).unwrap();
            assert_eq!(parse_permission(&parsed.render().unwrap()).unwrap(), parsed);
            assert_eq!(
                serde_json::from_str::<PermissionPattern>(&serde_json::to_string(&parsed).unwrap())
                    .unwrap(),
                parsed
            );
            if resource == "." {
                assert!(
                    matches!(parsed, PermissionPattern::Tool(p) if matches!(p.resource, ToolResourcePattern::Invocation(ref i) if i.command_path.is_empty()))
                );
            }
        }
    }
}
