// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

//! Pure projection of native tool metadata into MCP exports.
//!
//! Native invocation remains authoritative for constraints, middleware composition,
//! aliases, formatter selection, repetition mode, and exit status. MCP deliberately
//! exposes only typed values and finite stdin/stdout; the runtime buffers each stream
//! to 16 MiB and does not expose formatter or exit-code details.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use golem_common::base_model::component::{ComponentId, ComponentName};
use golem_common::base_model::tool::ToolName;
use golem_common::schema::tool::canonical::CanonicalSurfaceRef;
use golem_common::schema::tool::validation::validate_tool;
use golem_common::schema::tool::{FlagShape, OptionShape, Tool};
use golem_common::schema::{
    FALLBACK_OUTPUT_FIELD_NAME, SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue,
    find_host_managed_type,
};
use golem_schema::schema::render::{
    JsonSchemaConfig, from_untrusted_json_value, to_external_input_json_schema,
    to_external_output_json_schema,
};
use golem_schema::schema::tool::constraints::validate_tool_constraints;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashSet;

const STDIN_FIELD: &str = "_stdin";
const MAX_STDIN: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, desert_rust::BinaryCodec)]
#[desert(evolution())]
pub struct CompiledMcpToolExport {
    pub mcp_name: String,
    pub description: String,
    pub owner_component_id: ComponentId,
    pub owner_component_name: ComponentName,
    pub tool_name: ToolName,
    pub command_path: Vec<String>,
    pub definition: Tool,
    pub input_schema: SchemaGraph,
}

impl CompiledMcpToolExport {
    pub fn command(&self) -> Result<(usize, &golem_common::schema::tool::CommandBody), String> {
        let index = self
            .definition
            .command_index_by_path(&self.command_path)
            .ok_or_else(|| "compiled native tool has an invalid command path".to_string())?;
        Ok((
            index,
            self.definition.commands.nodes[index].body.as_ref().unwrap(),
        ))
    }

    pub fn input_json_schema(&self) -> Result<Map<String, Value>, String> {
        let (index, body) = self.command()?;
        match &self.input_schema.root {
            SchemaType::Record { .. } => {}
            _ => return Err("canonical native tool input is not a record".to_string()),
        }
        let mut value = to_external_input_json_schema(
            &self.input_schema,
            &self.input_schema.root,
            JsonSchemaConfig::WITHOUT_DRAFT_MARKER.include_draft_marker,
        );
        let object = value
            .as_object_mut()
            .ok_or_else(|| "input schema did not render as an object".to_string())?;
        let model = self
            .definition
            .canonical_input_model(index)
            .map_err(|e| e.to_string())?;
        for (surface, field) in self
            .definition
            .canonical_input_surfaces(index)
            .into_iter()
            .zip(&model.fields)
        {
            let required = object
                .entry("required")
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .unwrap();
            required.retain(|name| name.as_str() != Some(&field.name));
            if omitted_value(&self.definition, index, surface).is_none() {
                required.push(json!(field.name));
            }
            if let Some((min, max)) = collection_bounds(&self.definition, index, surface)
                && let Some(property) = object
                    .get_mut("properties")
                    .and_then(Value::as_object_mut)
                    .and_then(|properties| properties.get_mut(&field.name))
                    .and_then(Value::as_object_mut)
            {
                property.insert("minItems".to_string(), Value::from(min));
                if let Some(max) = max {
                    property.insert("maxItems".to_string(), Value::from(max));
                }
            }
            if repeatable_map_required(&self.definition, index, surface)
                && let Some(property) = object
                    .get_mut("properties")
                    .and_then(Value::as_object_mut)
                    .and_then(|properties| properties.get_mut(&field.name))
                    .and_then(Value::as_object_mut)
            {
                property.insert("minProperties".to_string(), Value::from(1));
            }
        }
        if let Some(stdin) = &body.stdin {
            let binary = !stdin.mime.iter().all(|mime| mime.starts_with("text/"));
            let schema = if binary {
                json!({"type":"string","contentEncoding":"base64","description":"Finite stdin (maximum decoded size 16 MiB)."})
            } else {
                json!({"type":"string","description":"Finite text stdin (maximum UTF-8 size 16 MiB)."})
            };
            object
                .entry("properties")
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .unwrap()
                .insert(STDIN_FIELD.to_string(), schema);
            if stdin.required {
                object
                    .entry("required")
                    .or_insert_with(|| Value::Array(Vec::new()))
                    .as_array_mut()
                    .unwrap()
                    .push(Value::String(STDIN_FIELD.to_string()));
            }
        }
        Ok(object.clone())
    }

    pub fn output_json_schema(&self) -> Result<Option<Map<String, Value>>, String> {
        let (_, body) = self.command()?;
        let Some(result) = &body.result else {
            return Ok(None);
        };
        reject_capabilities(&self.definition.schema, &result.type_, "result")?;
        let mut inner =
            to_external_output_json_schema(&self.definition.schema, &result.type_, false);
        if matches!(&result.type_, SchemaType::Record { .. }) {
            return inner
                .as_object()
                .cloned()
                .map(Some)
                .ok_or_else(|| "output schema is not an object".to_string());
        }
        let defs = inner.as_object_mut().and_then(|o| o.remove("$defs"));
        let mut wrapper = json!({"type":"object","properties":{FALLBACK_OUTPUT_FIELD_NAME:inner},"required":[FALLBACK_OUTPUT_FIELD_NAME]});
        if let Some(defs) = defs {
            wrapper
                .as_object_mut()
                .unwrap()
                .insert("$defs".to_string(), defs);
        }
        Ok(wrapper.as_object().cloned())
    }

    pub fn parse_arguments(
        &self,
        mut args: Map<String, Value>,
    ) -> Result<(TypedSchemaValue, Option<Vec<u8>>), String> {
        let (index, body) = self.command()?;
        let stdin = match (body.stdin.as_ref(), args.remove(STDIN_FIELD)) {
            (None, Some(_)) => return Err("unknown argument `_stdin`".to_string()),
            (Some(spec), None) if spec.required => {
                return Err("missing required argument `_stdin`".to_string());
            }
            (_, None) => None,
            (Some(spec), Some(Value::String(value))) => {
                let bytes = if spec.mime.iter().all(|m| m.starts_with("text/")) {
                    value.into_bytes()
                } else {
                    if value.len() > MAX_STDIN.div_ceil(3) * 4 {
                        return Err("stdin exceeds the 16 MiB limit".to_string());
                    }
                    STANDARD
                        .decode(value)
                        .map_err(|e| format!("invalid base64 stdin: {e}"))?
                };
                if bytes.len() > MAX_STDIN {
                    return Err("stdin exceeds the 16 MiB limit".to_string());
                }
                Some(bytes)
            }
            (Some(_), Some(_)) => return Err("`_stdin` must be a string".to_string()),
        };
        let model = self
            .definition
            .canonical_input_model(index)
            .map_err(|e| e.to_string())?;
        if let Some(name) = args
            .keys()
            .find(|name| !model.fields.iter().any(|field| field.name == **name))
        {
            return Err(format!("unknown argument `{name}`"));
        }
        let mut fields = Vec::with_capacity(model.fields.len());
        for (surface, field) in self
            .definition
            .canonical_input_surfaces(index)
            .into_iter()
            .zip(&model.fields)
        {
            fields.push(match args.remove(&field.name) {
                Some(json) => from_untrusted_json_value(&self.input_schema, &field.type_, &json)
                    .map_err(|e| format!("invalid argument `{}`: {e}", field.name))?,
                None => omitted_value(&self.definition, index, surface)
                    .ok_or_else(|| format!("missing required argument `{}`", field.name))?,
            });
        }
        let value = SchemaValue::Record { fields };
        let values = self
            .definition
            .decode_canonical_input_record(index, value.clone())
            .map_err(|e| e.to_string())?;
        validate_collection_values(
            &self.definition,
            index,
            &self.definition.canonical_input_surfaces(index),
            &values,
        )?;
        validate_tool_constraints(
            &self.definition,
            index,
            &body.constraints,
            &self.definition.canonical_input_surfaces(index),
            &values,
        )?;
        Ok((
            TypedSchemaValue::new(self.input_schema.clone(), value),
            stdin,
        ))
    }
}

pub fn compile_native_tool_exports(
    owner_component_id: ComponentId,
    owner_component_name: ComponentName,
    tool_name: ToolName,
    definition: &Tool,
    include: Option<&[String]>,
    exclude: Option<&[String]>,
) -> Result<Vec<CompiledMcpToolExport>, String> {
    if include.is_some() && exclude.is_some() {
        return Err("include and exclude are mutually exclusive".to_string());
    }
    validate_tool(definition).map_err(|e| {
        e.into_iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    if definition.name() != Some(tool_name.as_str()) {
        return Err("registered tool name does not match its root command".to_string());
    }
    let includes = compile_patterns(include)?;
    let excludes = compile_patterns(exclude)?;
    let filter_includes = include.is_some();
    let mut candidates = Vec::new();
    walk_paths(definition, 0, vec![], vec![vec![]], &mut candidates)?;
    let mut names = HashSet::new();
    let mut result = Vec::new();
    for (index, canonical, exposed) in candidates {
        if excludes.iter().any(|p| pattern_matches(p, &canonical)) {
            continue;
        }
        let canonical_included = includes.iter().any(|p| pattern_matches(p, &canonical));
        for alias_path in exposed {
            if filter_includes
                && !canonical_included
                && !includes.iter().any(|p| pattern_matches(p, &alias_path))
            {
                continue;
            }
            if excludes.iter().any(|p| pattern_matches(p, &alias_path)) {
                continue;
            }
            let input_schema = definition
                .canonical_input_record_schema(index)
                .map_err(|e| e.to_string())?;
            if matches!(&input_schema.root, SchemaType::Record { fields, .. } if fields.iter().any(|field| field.name == STDIN_FIELD))
            {
                return Err("native input field `_stdin` is reserved".to_string());
            }
            reject_capabilities(&input_schema, &input_schema.root, "input")?;
            let body = definition.commands.nodes[index].body.as_ref().unwrap();
            if let Some(r) = &body.result {
                reject_capabilities(&definition.schema, &r.type_, "result")?;
            }
            for error in &body.errors {
                if let Some(payload) = &error.payload {
                    reject_capabilities(
                        &definition.schema,
                        payload,
                        &format!("custom error `{}`", error.name),
                    )?;
                }
            }
            let node = &definition.commands.nodes[index];
            let mut description = if node.doc.description.is_empty() {
                node.doc.summary.clone()
            } else {
                node.doc.description.clone()
            };
            if alias_path != canonical {
                description.push_str(&format!(" (alias for {})", canonical.join(" ")));
            }
            if !body.constraints.is_empty() {
                description.push_str(&format!(
                    " Constraints: {}.",
                    serde_json::to_string(&body.constraints).map_err(|error| error.to_string())?
                ));
            }
            if body.stdin.is_some() || body.stdout.is_some() {
                description.push_str(
                    " Streaming is exposed as finite buffered data (16 MiB per direction).",
                );
            }
            for root_name in std::iter::once(tool_name.as_str()).chain(
                definition.commands.nodes[0]
                    .aliases
                    .iter()
                    .map(String::as_str),
            ) {
                let mut all = vec![root_name.to_string()];
                all.extend(alias_path.clone());
                let name = all
                    .iter()
                    .map(|s| s.replace('-', "_").to_lowercase())
                    .collect::<Vec<_>>()
                    .join("_");
                if !names.insert(name.clone()) {
                    return Err(format!("native MCP tool name collision: `{name}`"));
                }
                result.push(CompiledMcpToolExport {
                    mcp_name: name,
                    description: if root_name == tool_name.as_str() {
                        description.clone()
                    } else {
                        format!(
                            "{description} (alias for {} {})",
                            tool_name,
                            canonical.join(" ")
                        )
                    },
                    owner_component_id,
                    owner_component_name: owner_component_name.clone(),
                    tool_name: tool_name.clone(),
                    command_path: canonical.clone(),
                    definition: definition.clone(),
                    input_schema: input_schema.clone(),
                });
            }
        }
    }
    if result.is_empty() {
        return Err("native MCP tool export selects no executable commands".to_string());
    }
    Ok(result)
}

fn reject_capabilities(graph: &SchemaGraph, ty: &SchemaType, where_: &str) -> Result<(), String> {
    if golem_common::schema::agent::contains_stream_in_graph(graph, ty) {
        return Err(format!(
            "typed streams are not supported in native tool {where_}"
        ));
    }
    match find_host_managed_type(graph, ty).map_err(|e| e.to_string())? {
        Some(found) => Err(format!(
            "unsupported capability in native tool {where_}: {} ({})",
            found.kind.kind_name(),
            found.path
        )),
        None => Ok(()),
    }
}

type CommandPaths = (usize, Vec<String>, Vec<Vec<String>>);

fn walk_paths(
    tool: &Tool,
    index: usize,
    canonical: Vec<String>,
    exposed: Vec<Vec<String>>,
    out: &mut Vec<CommandPaths>,
) -> Result<(), String> {
    let node = &tool.commands.nodes[index];
    if node.body.is_some() {
        out.push((index, canonical.clone(), exposed.clone()));
    }
    for child in &node.subcommands {
        let ci = child.as_usize().ok_or("negative command index")?;
        let c = &tool.commands.nodes[ci];
        let mut cp = canonical.clone();
        cp.push(c.name.clone());
        let mut ep = Vec::new();
        for p in &exposed {
            for n in std::iter::once(&c.name).chain(c.aliases.iter()) {
                let mut x = p.clone();
                x.push(n.clone());
                ep.push(x);
            }
        }
        walk_paths(tool, ci, cp, ep, out)?;
    }
    Ok(())
}

#[derive(Clone)]
enum Pat {
    One(String),
    AnyOne,
    AnyDepth,
}
fn compile_patterns(raw: Option<&[String]>) -> Result<Vec<Vec<Pat>>, String> {
    raw.unwrap_or_default()
        .iter()
        .map(|s| {
            if s.is_empty() || s.split_whitespace().collect::<Vec<_>>().join(" ") != *s {
                return Err(format!("malformed native tool path pattern `{s}`"));
            }
            s.split(' ')
                .map(|x| match x {
                    "*" => Ok(Pat::AnyOne),
                    "**" => Ok(Pat::AnyDepth),
                    _ if x.contains('*') => Err(format!("malformed wildcard segment `{x}`")),
                    _ => Ok(Pat::One(x.to_string())),
                })
                .collect()
        })
        .collect()
}
fn pattern_matches(p: &[Pat], path: &[String]) -> bool {
    fn go(p: &[Pat], s: &[String]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(Pat::AnyDepth) => go(&p[1..], s) || (!s.is_empty() && go(p, &s[1..])),
            Some(Pat::AnyOne) => !s.is_empty() && go(&p[1..], &s[1..]),
            Some(Pat::One(x)) => s.first() == Some(x) && go(&p[1..], &s[1..]),
        }
    }
    go(p, path)
}

fn omitted_value(
    tool: &Tool,
    command_index: usize,
    surface: CanonicalSurfaceRef,
) -> Option<SchemaValue> {
    let body = tool.commands.nodes[command_index].body.as_ref()?;
    match surface {
        CanonicalSurfaceRef::GlobalOption { node, index } => {
            option_omitted(&tool.commands.nodes[node].globals.options[index])
        }
        CanonicalSurfaceRef::BodyOption { index } => option_omitted(&body.options[index]),
        CanonicalSurfaceRef::GlobalFlag { node, index } => Some(flag_default(
            &tool.commands.nodes[node].globals.flags[index].shape,
        )),
        CanonicalSurfaceRef::BodyFlag { index } => Some(flag_default(&body.flags[index].shape)),
        CanonicalSurfaceRef::BodyPositional { index } => {
            body.positionals.fixed[index].default.clone().or_else(|| {
                (!body.positionals.fixed[index].required
                    && matches!(
                        body.positionals.fixed[index].type_,
                        SchemaType::Option { .. }
                    ))
                .then(|| SchemaValue::Option { inner: None })
            })
        }
        CanonicalSurfaceRef::BodyTail => body
            .positionals
            .tail
            .as_ref()
            .filter(|tail| tail.min == 0)
            .map(|_| SchemaValue::List {
                elements: Vec::new(),
            }),
    }
}

fn option_omitted(option: &golem_common::schema::tool::OptionSpec) -> Option<SchemaValue> {
    option.default.clone().or_else(|| match option.shape {
        OptionShape::RepeatableList(_) if !option.required => Some(SchemaValue::List {
            elements: Vec::new(),
        }),
        OptionShape::RepeatableMap(_) if !option.required => Some(SchemaValue::Map {
            entries: Vec::new(),
        }),
        OptionShape::Scalar(SchemaType::Option { .. })
        | OptionShape::OptionalScalar(SchemaType::Option { .. })
            if !option.required =>
        {
            Some(SchemaValue::Option { inner: None })
        }
        _ => None,
    })
}

fn collection_bounds(
    tool: &Tool,
    command_index: usize,
    surface: CanonicalSurfaceRef,
) -> Option<(u32, Option<u32>)> {
    let body = tool.commands.nodes[command_index].body.as_ref()?;
    match surface {
        CanonicalSurfaceRef::BodyTail => body
            .positionals
            .tail
            .as_ref()
            .map(|tail| (tail.min, tail.max)),
        CanonicalSurfaceRef::GlobalOption { node, index } => matches!(
            tool.commands.nodes[node].globals.options[index].shape,
            OptionShape::RepeatableList(_)
        )
        .then_some((
            u32::from(tool.commands.nodes[node].globals.options[index].required),
            None,
        )),
        CanonicalSurfaceRef::BodyOption { index } => {
            matches!(body.options[index].shape, OptionShape::RepeatableList(_))
                .then_some((u32::from(body.options[index].required), None))
        }
        _ => None,
    }
}

fn validate_collection_values(
    tool: &Tool,
    command_index: usize,
    surfaces: &[CanonicalSurfaceRef],
    values: &[golem_common::schema::tool::canonical::CanonicalInputValue],
) -> Result<(), String> {
    for (surface, value) in surfaces.iter().copied().zip(values) {
        if repeatable_map_required(tool, command_index, surface)
            && matches!(&value.value, SchemaValue::Map { entries } if entries.is_empty())
        {
            return Err(format!(
                "argument `{}` requires at least 1 value(s)",
                value.name
            ));
        }
        let Some((min, max)) = collection_bounds(tool, command_index, surface) else {
            continue;
        };
        let SchemaValue::List { elements } = &value.value else {
            continue;
        };
        let length = elements.len() as u32;
        if length < min {
            return Err(format!(
                "argument `{}` requires at least {min} value(s)",
                value.name
            ));
        }
        if let Some(max) = max
            && length > max
        {
            return Err(format!(
                "argument `{}` accepts at most {max} value(s)",
                value.name
            ));
        }
    }
    Ok(())
}

fn repeatable_map_required(
    tool: &Tool,
    command_index: usize,
    surface: CanonicalSurfaceRef,
) -> bool {
    let Some(body) = tool.commands.nodes[command_index].body.as_ref() else {
        return false;
    };
    let option = match surface {
        CanonicalSurfaceRef::GlobalOption { node, index } => {
            Some(&tool.commands.nodes[node].globals.options[index])
        }
        CanonicalSurfaceRef::BodyOption { index } => Some(&body.options[index]),
        _ => None,
    };
    option.is_some_and(|option| {
        option.required && matches!(option.shape, OptionShape::RepeatableMap(_))
    })
}

fn flag_default(shape: &FlagShape) -> SchemaValue {
    match shape {
        FlagShape::BoolFlag(shape) => SchemaValue::Bool(shape.default),
        FlagShape::CountFlag(_) => SchemaValue::U32(0),
    }
}

#[cfg(test)]
mod tests;
