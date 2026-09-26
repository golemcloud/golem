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

//! Shared command-line parsing and help for schema-described tools.
//!
//! Arguments are decoded into the canonical input record and checked against tool constraints.
//! Metadata never reads the caller's environment; callers supply explicit argument values.

use crate::schema::tool::canonical::CanonicalSurfaceRef;
use crate::schema::tool::constraints::validate_tool_constraints;
use crate::schema::tool::{
    DuplicateKeyPolicy, FlagShape, FlagSpec, OptionShape, OptionSpec, Repetition, Tool,
};
use crate::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};
use std::collections::HashMap;

/// A request for command help or a validated tool invocation.
pub enum ParsedToolArguments {
    /// Help text for the selected command.
    Help(String),
    /// Canonical command identity and input, ready for a tool transport.
    Invoke {
        /// Canonical subcommand names, excluding the root tool name.
        command_path: Vec<String>,
        /// Input encoded according to the command's canonical input model.
        input: Box<TypedSchemaValue>,
    },
}

#[derive(Clone, Copy)]
enum Surface<'a> {
    Option(&'a OptionSpec),
    Flag(&'a FlagSpec),
    Positional(usize),
    Tail,
}

enum Token<'a> {
    Help,
    TailSeparator {
        verbatim: bool,
    },
    EndOptions,
    LongOption {
        option: &'a OptionSpec,
        attached: Option<String>,
    },
    LongFlag {
        flag: &'a FlagSpec,
        negated: bool,
    },
    ShortOptions,
    Subcommand(usize),
    Positional,
    Tail,
}

struct ParseCursor {
    options_enabled: bool,
    tail_started: bool,
    body_started: bool,
}

impl ParseCursor {
    fn classify<'a>(
        &self,
        tool: &'a Tool,
        node_index: usize,
        token: &str,
        fixed_count: usize,
    ) -> Result<Token<'a>, String> {
        let node = &tool.commands.nodes[node_index];
        // A declared separator can skip optional fixed slots, but not required ones.
        // It remains required even after option parsing has ended.
        if !self.tail_started
            && let Some(spec) = node
                .body
                .as_ref()
                .filter(|body| {
                    body.positionals
                        .fixed
                        .iter()
                        .skip(fixed_count)
                        .all(|p| !p.required || p.default.is_some())
                })
                .and_then(|body| body.positionals.tail.as_ref())
                .filter(|tail| tail.separator.as_deref() == Some(token))
        {
            return Ok(Token::TailSeparator {
                verbatim: spec.verbatim,
            });
        }
        if self.options_enabled {
            let surfaces = effective_named(tool, node_index);
            if let Some(long) = token.strip_prefix("--") {
                let (name, attached) = long
                    .split_once('=')
                    .map_or((long, None), |(name, value)| (name, Some(value)));
                if let Some(option) = surfaces.iter().find_map(|(_, surface)| match surface {
                    Surface::Option(option)
                        if option.long == name
                            || option.aliases.iter().any(|alias| alias == name) =>
                    {
                        Some(*option)
                    }
                    _ => None,
                }) {
                    return Ok(Token::LongOption {
                        option,
                        attached: attached.map(str::to_owned),
                    });
                }
                if let Some((_, flag, negated)) = find_long_flag(&surfaces, name) {
                    if attached.is_some() {
                        return Err(format!("flag --{name} does not take a value"));
                    }
                    return Ok(Token::LongFlag { flag, negated });
                }
            }
            // Declared short forms own their spelling, including digits and punctuation.
            // Numeric-looking tokens become positionals only when no declaration matches.
            if let Some(short) = token.strip_prefix('-').and_then(|rest| rest.chars().next())
                && find_short(&surfaces, short).is_some()
            {
                return Ok(Token::ShortOptions);
            }
            if !self.body_started
                && let Some(child) = child_named(tool, node_index, token)
            {
                return Ok(Token::Subcommand(child));
            }
            if help_available(tool, node_index) && matches!(token, "--help" | "-h") {
                return Ok(Token::Help);
            }
            if token == "--" {
                return Ok(Token::EndOptions);
            }
            if token.starts_with("--") && token.len() > 2 {
                let name = token[2..].split('=').next().unwrap();
                return Err(format!("unknown option --{name}"));
            }
            if token.starts_with('-') && token != "-" && !looks_negative_number(token) {
                return Ok(Token::ShortOptions);
            }
        }
        let body = node
            .body
            .as_ref()
            .ok_or_else(|| format!("expected a subcommand, found {token:?}"))?;
        if !self.tail_started && fixed_count < body.positionals.fixed.len() {
            return Ok(Token::Positional);
        }
        if let Some(spec) = &body.positionals.tail {
            if !self.tail_started
                && let Some(separator) = &spec.separator
            {
                return Err(format!("expected tail separator {separator:?}"));
            }
            return Ok(Token::Tail);
        }
        Err(format!("unexpected argument {token:?}"))
    }
}

/// Parses arguments using a validated tool definition's command-line surface.
///
/// `args` excludes the tool name. Options, subcommands, aliases, positionals, defaults,
/// help, and constraints follow the same rules for native CLI and guest callers.
/// Environment-variable metadata is descriptive and never reads process environment values.
///
/// # Errors
/// Returns a diagnostic for an invalid command line or a value that fails canonical validation.
pub fn parse(tool: &Tool, args: &[String]) -> Result<ParsedToolArguments, String> {
    if tool.commands.nodes.is_empty() {
        return Err("tool metadata has no root command".into());
    }
    let mut node_index = 0;
    let mut path = Vec::new();
    let mut supplied: HashMap<String, Vec<String>> = HashMap::new();
    let mut flags: HashMap<String, (u32, Option<bool>)> = HashMap::new();
    let mut positionals = Vec::new();
    let mut tail = Vec::new();
    let mut cursor = ParseCursor {
        options_enabled: true,
        tail_started: false,
        body_started: false,
    };
    let mut i = 0;

    while i < args.len() {
        let token = &args[i];
        let node = &tool.commands.nodes[node_index];
        match cursor.classify(tool, node_index, token, positionals.len())? {
            Token::Help => {
                return Ok(ParsedToolArguments::Help(render_help(
                    tool, node_index, &path,
                )));
            }
            Token::TailSeparator { verbatim } => {
                cursor.options_enabled &= !verbatim;
                cursor.tail_started = true;
                cursor.body_started = true;
            }
            Token::EndOptions => cursor.options_enabled = false,
            Token::LongOption { option, attached } => {
                let value = option_value(option, attached, args, &mut i)?;
                supplied.entry(option.long.clone()).or_default().push(value);
                cursor.body_started |= node
                    .body
                    .as_ref()
                    .is_some_and(|body| body.options.iter().any(|o| std::ptr::eq(o, option)));
            }
            Token::LongFlag { flag, negated } => {
                apply_flag(&mut flags, flag, negated)?;
                cursor.body_started |= node
                    .body
                    .as_ref()
                    .is_some_and(|body| body.flags.iter().any(|f| std::ptr::eq(f, flag)));
            }
            Token::ShortOptions => {
                let surfaces = effective_named(tool, node_index);
                let chars: Vec<char> = token[1..].chars().collect();
                let mut c = 0;
                while c < chars.len() {
                    let short = chars[c];
                    if help_available(tool, node_index) && short == 'h' {
                        return Ok(ParsedToolArguments::Help(render_help(
                            tool, node_index, &path,
                        )));
                    }
                    let Some(surface) = find_short(&surfaces, short) else {
                        return Err(format!("unknown short option -{short}"));
                    };
                    match surface {
                        Surface::Flag(flag) => {
                            apply_flag(&mut flags, flag, false)?;
                            cursor.body_started |= node.body.as_ref().is_some_and(|body| {
                                body.flags.iter().any(|f| std::ptr::eq(f, flag))
                            });
                        }
                        Surface::Option(option) => {
                            cursor.body_started |= node.body.as_ref().is_some_and(|body| {
                                body.options.iter().any(|o| std::ptr::eq(o, option))
                            });
                            let rest: String = chars[c + 1..].iter().collect();
                            let attached = (!rest.is_empty()).then_some(rest);
                            supplied
                                .entry(option.long.clone())
                                .or_default()
                                .push(option_value(option, attached, args, &mut i)?);
                            break;
                        }
                        _ => unreachable!(),
                    }
                    c += 1;
                }
            }
            Token::Subcommand(child) => {
                node_index = child;
                path.push(tool.commands.nodes[child].name.clone());
            }
            Token::Positional => {
                cursor.body_started = true;
                positionals.push(token.clone());
            }
            Token::Tail => {
                cursor.body_started = true;
                tail.push(token.clone());
            }
        }
        i += 1;
    }

    let node = &tool.commands.nodes[node_index];
    let body = node
        .body
        .as_ref()
        .ok_or_else(|| "a subcommand is required".to_string())?;
    let surfaces = tool.canonical_input_surfaces(node_index);
    let mut values = Vec::with_capacity(surfaces.len());
    for surface_ref in surfaces.iter().copied() {
        let surface = resolve_surface(tool, node_index, surface_ref)?;
        values.push(collect_value(
            tool,
            node_index,
            surface,
            &supplied,
            &flags,
            &positionals,
            &tail,
        )?);
    }
    let model = tool
        .canonical_input_model(node_index)
        .map_err(|e| e.to_string())?;
    let record = SchemaValue::Record { fields: values };
    let decoded = model
        .decode_record(record.clone())
        .map_err(|e| e.to_string())?;
    validate_tool_constraints(tool, node_index, &body.constraints, &surfaces, &decoded)?;
    crate::schema::validation::validate_value(
        &model.record_schema,
        &model.record_schema.root,
        &record,
    )
    .map_err(|errors| format!("invalid tool arguments: {errors:?}"))?;
    Ok(ParsedToolArguments::Invoke {
        command_path: path,
        input: Box::new(TypedSchemaValue::new(model.record_schema, record)),
    })
}

fn effective_named(tool: &Tool, command: usize) -> Vec<((usize, usize), Surface<'_>)> {
    let mut path = Vec::new();
    find_node_path(tool, 0, command, &mut path);
    let mut result = Vec::new();
    for node_index in path {
        let node = &tool.commands.nodes[node_index];
        result.extend(
            node.globals
                .options
                .iter()
                .enumerate()
                .map(|(index, option)| ((node_index, index), Surface::Option(option))),
        );
        result.extend(
            node.globals
                .flags
                .iter()
                .enumerate()
                .map(|(index, flag)| ((node_index, index), Surface::Flag(flag))),
        );
    }
    if let Some(body) = &tool.commands.nodes[command].body {
        result.extend(
            body.options
                .iter()
                .enumerate()
                .map(|(index, option)| ((command, index), Surface::Option(option))),
        );
        result.extend(
            body.flags
                .iter()
                .enumerate()
                .map(|(index, flag)| ((command, index), Surface::Flag(flag))),
        );
    }
    result
}

fn find_node_path(tool: &Tool, current: usize, target: usize, path: &mut Vec<usize>) -> bool {
    path.push(current);
    if current == target {
        return true;
    }
    for child in tool.commands.nodes[current]
        .subcommands
        .iter()
        .filter_map(|index| index.as_usize())
    {
        if find_node_path(tool, child, target, path) {
            return true;
        }
    }
    path.pop();
    false
}

fn resolve_surface(
    tool: &Tool,
    command: usize,
    r: CanonicalSurfaceRef,
) -> Result<Surface<'_>, String> {
    let body = tool.commands.nodes[command]
        .body
        .as_ref()
        .ok_or("command has no body")?;
    Ok(match r {
        CanonicalSurfaceRef::GlobalOption { node, index } => {
            Surface::Option(&tool.commands.nodes[node].globals.options[index])
        }
        CanonicalSurfaceRef::GlobalFlag { node, index } => {
            Surface::Flag(&tool.commands.nodes[node].globals.flags[index])
        }
        CanonicalSurfaceRef::BodyPositional { index } => Surface::Positional(index),
        CanonicalSurfaceRef::BodyTail => Surface::Tail,
        CanonicalSurfaceRef::BodyOption { index } => Surface::Option(&body.options[index]),
        CanonicalSurfaceRef::BodyFlag { index } => Surface::Flag(&body.flags[index]),
    })
}

fn collect_value(
    tool: &Tool,
    command: usize,
    surface: Surface<'_>,
    supplied: &HashMap<String, Vec<String>>,
    flags: &HashMap<String, (u32, Option<bool>)>,
    positionals: &[String],
    tail: &[String],
) -> Result<SchemaValue, String> {
    let graph = &tool.schema;
    match surface {
        Surface::Positional(index) => {
            let p = &tool.commands.nodes[command]
                .body
                .as_ref()
                .unwrap()
                .positionals
                .fixed[index];
            match positionals.get(index) {
                Some(v) => decode(graph, &p.type_, v),
                None if p.default.is_some() => Ok(p.default.clone().unwrap()),
                None if !p.required => omitted(resolve(graph, &p.type_)?),
                None => Err(format!("missing required positional {}", p.name)),
            }
        }
        Surface::Tail => {
            let p = tool.commands.nodes[command]
                .body
                .as_ref()
                .unwrap()
                .positionals
                .tail
                .as_ref()
                .unwrap();
            if tail.len() < p.min as usize || p.max.is_some_and(|m| tail.len() > m as usize) {
                return Err(format!("tail {} has invalid cardinality", p.name));
            }
            Ok(SchemaValue::List {
                elements: tail
                    .iter()
                    .map(|v| decode(graph, &p.item_type, v))
                    .collect::<Result<_, _>>()?,
            })
        }
        Surface::Flag(flag) => {
            let (count, boolean) = flags.get(&flag.long).copied().unwrap_or_default();
            match flag.shape {
                FlagShape::BoolFlag(s) => Ok(SchemaValue::Bool(boolean.unwrap_or(s.default))),
                FlagShape::CountFlag(_) => Ok(SchemaValue::U32(count)),
            }
        }
        Surface::Option(option) => {
            let raw = supplied.get(&option.long).cloned();
            if raw.is_none() {
                if let Some(default) = &option.default {
                    return Ok(default.clone());
                }
                if option.required {
                    return Err(format!("missing required option --{}", option.long));
                }
            }
            collect_option(graph, option, raw.unwrap_or_default())
        }
    }
}

fn collect_option(
    graph: &SchemaGraph,
    option: &OptionSpec,
    raw: Vec<String>,
) -> Result<SchemaValue, String> {
    let repeatable = match &option.shape {
        OptionShape::Scalar(_) | OptionShape::OptionalScalar(_) => false,
        OptionShape::RepeatableList(shape) => !matches!(shape.repetition, Repetition::Delimited(_)),
        OptionShape::RepeatableMap(shape) => !matches!(shape.repetition, Repetition::Delimited(_)),
    };
    if raw.len() > 1 && !repeatable {
        return Err(format!("--{} cannot be repeated", option.long));
    }
    match &option.shape {
        OptionShape::Scalar(ty) => raw
            .last()
            .map(|v| decode(graph, ty, v))
            .unwrap_or_else(|| omitted(resolve(graph, ty)?)),
        OptionShape::OptionalScalar(ty) => match raw.last() {
            Some(value) if value == "\0" => option
                .default
                .clone()
                .ok_or_else(|| format!("--{} has no bare-value default", option.long)),
            Some(value) => decode(graph, ty, value),
            None => omitted(resolve(graph, ty)?),
        },
        OptionShape::RepeatableList(shape) => {
            let parts = expand(&raw, shape.repetition);
            Ok(SchemaValue::List {
                elements: parts
                    .iter()
                    .map(|v| decode(graph, &shape.item_type, v))
                    .collect::<Result<_, _>>()?,
            })
        }
        OptionShape::RepeatableMap(shape) => {
            let SchemaType::Map { key, value, .. } = resolve(graph, &shape.map_type)? else {
                return Err(format!("--{} does not declare a map type", option.long));
            };
            let mut entries: Vec<(SchemaValue, SchemaValue)> = Vec::new();
            for item in expand(&raw, shape.repetition) {
                let (k, v) = item
                    .split_once('=')
                    .ok_or_else(|| format!("--{} expects KEY=VALUE", option.long))?;
                let pair = (decode(graph, key, k)?, decode(graph, value, v)?);
                if let Some(index) = entries.iter().position(|(old, _)| old == &pair.0) {
                    match shape.duplicate_key_policy {
                        DuplicateKeyPolicy::Reject => {
                            return Err(format!("duplicate key for --{}", option.long));
                        }
                        DuplicateKeyPolicy::LastWins => entries[index] = pair,
                    }
                } else {
                    entries.push(pair);
                }
            }
            Ok(SchemaValue::Map { entries })
        }
    }
}

fn option_value(
    option: &OptionSpec,
    attached: Option<String>,
    args: &[String],
    i: &mut usize,
) -> Result<String, String> {
    if let Some(value) = attached {
        return Ok(value);
    }
    if matches!(option.shape, OptionShape::OptionalScalar(_)) {
        return Ok("\0".to_string());
    }
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("--{} requires a value", option.long))
}

fn apply_flag(
    store: &mut HashMap<String, (u32, Option<bool>)>,
    flag: &FlagSpec,
    negated: bool,
) -> Result<(), String> {
    let entry = store.entry(flag.long.clone()).or_default();
    match flag.shape {
        FlagShape::BoolFlag(_) => entry.1 = Some(!negated),
        FlagShape::CountFlag(max) => {
            entry.0 += 1;
            if max.is_some_and(|m| entry.0 > m) {
                return Err(format!("--{} exceeds maximum count", flag.long));
            }
        }
    }
    Ok(())
}

fn find_short<'a>(surfaces: &[((usize, usize), Surface<'a>)], short: char) -> Option<Surface<'a>> {
    surfaces.iter().find_map(|(_, surface)| match surface {
        Surface::Option(option) if option.short == Some(short) => Some(*surface),
        Surface::Flag(flag) if flag.short == Some(short) => Some(*surface),
        _ => None,
    })
}

fn find_long_flag<'a>(
    surfaces: &[((usize, usize), Surface<'a>)],
    name: &str,
) -> Option<((usize, usize), &'a FlagSpec, bool)> {
    surfaces
        .iter()
        .find_map(|(key, s)| match s {
            Surface::Flag(f) if f.long == name || f.aliases.iter().any(|a| a == name) => {
                Some((*key, *f, false))
            }
            _ => None,
        })
        .or_else(|| {
            surfaces.iter().find_map(|(key, s)| match s {
                Surface::Flag(f)
                    if matches!(f.shape, FlagShape::BoolFlag(s) if s.negatable)
                        && name == format!("no-{}", f.long) =>
                {
                    Some((*key, *f, true))
                }
                _ => None,
            })
        })
}

fn decode(graph: &SchemaGraph, ty: &SchemaType, raw: &str) -> Result<SchemaValue, String> {
    let resolved = resolve(graph, ty)?;
    if let SchemaType::Option { inner, .. } = resolved {
        return Ok(SchemaValue::Option {
            inner: Some(Box::new(decode(graph, inner, raw)?)),
        });
    }
    let json = match resolved {
        SchemaType::String { .. }
        | SchemaType::Char { .. }
        | SchemaType::Enum { .. }
        | SchemaType::Path { .. }
        | SchemaType::Url { .. }
        | SchemaType::Datetime { .. }
        | SchemaType::Duration { .. } => serde_json::Value::String(raw.into()),
        SchemaType::Bool { .. } if raw.eq_ignore_ascii_case("true") => {
            serde_json::Value::Bool(true)
        }
        SchemaType::Bool { .. } if raw.eq_ignore_ascii_case("false") => {
            serde_json::Value::Bool(false)
        }
        _ => serde_json::from_str(raw).map_err(|e| format!("invalid value {raw:?}: {e}"))?,
    };
    crate::schema::render::from_untrusted_json_value(graph, ty, &json)
        .map_err(|e| format!("invalid value {raw:?}: {e}"))
}

fn resolve<'a>(graph: &'a SchemaGraph, ty: &'a SchemaType) -> Result<&'a SchemaType, String> {
    graph.resolve_ref(ty).map_err(|e| e.to_string())
}

fn omitted(ty: &SchemaType) -> Result<SchemaValue, String> {
    match ty {
        SchemaType::Option { .. } => Ok(SchemaValue::Option { inner: None }),
        SchemaType::List { .. } => Ok(SchemaValue::List { elements: vec![] }),
        SchemaType::Map { .. } => Ok(SchemaValue::Map { entries: vec![] }),
        _ => Err("optional argument must have an option or collection schema, or a default".into()),
    }
}

fn expand(values: &[String], repetition: Repetition) -> Vec<String> {
    match repetition {
        Repetition::Repeated => values.to_vec(),
        Repetition::Delimited(c) | Repetition::Either(c) => values
            .iter()
            .flat_map(|v| v.split(c).map(str::to_string))
            .collect(),
    }
}

fn child_named(tool: &Tool, node: usize, name: &str) -> Option<usize> {
    tool.commands.nodes[node]
        .subcommands
        .iter()
        .filter_map(|i| i.as_usize())
        .find(|i| {
            tool.commands
                .nodes
                .get(*i)
                .is_some_and(|n| n.name == name || n.aliases.iter().any(|a| a == name))
        })
}

fn looks_negative_number(token: &str) -> bool {
    token[1..].starts_with(|c: char| c.is_ascii_digit() || c == '.')
}

fn help_available(tool: &Tool, node: usize) -> bool {
    if tool.commands.nodes[node]
        .body
        .as_ref()
        .and_then(|body| body.positionals.tail.as_ref())
        .and_then(|tail| tail.separator.as_deref())
        .is_some_and(|separator| matches!(separator, "--help" | "-h"))
    {
        return false;
    }
    !effective_named(tool, node).iter().any(|(_, s)| match s {
        Surface::Option(o) => {
            o.long == "help" || o.aliases.iter().any(|a| a == "help") || o.short == Some('h')
        }
        Surface::Flag(f) => {
            f.long == "help" || f.aliases.iter().any(|a| a == "help") || f.short == Some('h')
        }
        _ => false,
    })
}

fn render_help(tool: &Tool, node: usize, path: &[String]) -> String {
    let command = &tool.commands.nodes[node];
    let mut out = format!("Usage: {}", tool.name().unwrap_or("tool"));
    for part in path {
        out.push(' ');
        out.push_str(part);
    }
    if !effective_named(tool, node).is_empty() {
        out.push_str(" [OPTIONS]");
    }
    if !command.subcommands.is_empty() {
        out.push_str(" [COMMAND]");
    }
    if let Some(body) = &command.body {
        for p in &body.positionals.fixed {
            out.push(' ');
            let name = p.value_name.as_deref().unwrap_or(&p.name);
            if p.required && p.default.is_none() {
                out.push_str(&format!("<{name}>"));
            } else {
                out.push_str(&format!("[{name}]"));
            }
        }
        if let Some(tail) = &body.positionals.tail {
            if let Some(separator) = &tail.separator {
                out.push_str(&format!(" {separator}"));
            }
            out.push_str(&format!(
                " [{}]...",
                tail.value_name.as_deref().unwrap_or(&tail.name)
            ));
        }
    }
    out.push('\n');
    if !command.doc.summary.is_empty() {
        out.push('\n');
        out.push_str(&command.doc.summary);
        out.push('\n');
    }
    if !command.doc.description.is_empty() {
        out.push_str(&command.doc.description);
        out.push('\n');
    }
    if !command.subcommands.is_empty() {
        out.push_str("\nCommands:\n");
        for child in &command.subcommands {
            if let Some(c) = child.as_usize().and_then(|i| tool.commands.nodes.get(i)) {
                out.push_str(&format!("  {:16} {}\n", c.name, c.doc.summary));
            }
        }
    }
    out.push_str("\nOptions:\n");
    for (_, s) in effective_named(tool, node) {
        match s {
            Surface::Option(o) => {
                let short = o.short.map(|s| format!("-{s}, ")).unwrap_or_default();
                let value = o.value_name.as_deref().unwrap_or("VALUE");
                let value = if matches!(o.shape, OptionShape::OptionalScalar(_)) {
                    format!("[={value}]")
                } else {
                    format!(" <{value}>")
                };
                out.push_str(&format!("  {short}--{}{value}  {}", o.long, o.doc.summary));
                if o.required {
                    out.push_str(" [required]");
                }
                if o.default.is_some() {
                    out.push_str(" [has default]");
                }
                if !o.aliases.is_empty() {
                    out.push_str(&format!(" [aliases: {}]", o.aliases.join(", ")));
                }
                out.push('\n');
            }
            Surface::Flag(f) => {
                let short = f.short.map(|s| format!("-{s}, ")).unwrap_or_default();
                out.push_str(&format!("  {short}--{}  {}", f.long, f.doc.summary));
                if let FlagShape::BoolFlag(shape) = f.shape
                    && shape.negatable
                {
                    out.push_str(&format!(" [--no-{}]", f.long));
                }
                if !f.aliases.is_empty() {
                    out.push_str(&format!(" [aliases: {}]", f.aliases.join(", ")));
                }
                out.push('\n');
            }
            _ => {}
        }
    }
    if help_available(tool, node) {
        out.push_str("  -h, --help      Print help (reserved only when undeclared)\n");
    }
    out
}

#[cfg(test)]
mod tests;
