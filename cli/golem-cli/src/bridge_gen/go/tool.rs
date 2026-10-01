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

//! Go guest tool client generator.
//!
//! A generated tool client declares the tool with `golem.DefineToolClient`,
//! under an identity type of its own, and each command with the same field-bound spec a Go tool is written with,
//! mirroring the published metadata. The guest SDK then builds the canonical
//! input record, invokes through tool RPC and decodes the result exactly as for
//! a tool declared in Go, so the generator emits no codec:
//!
//! ```go
//! res, err := vcs.Commit.Call(func(a *vcs.CommitArgs) { a.Message = "fix" })
//! ```

use super::go::{
    go_string, lower_first, to_exported_ident, to_field_ident, to_unexported_ident, unique_idents,
    unique_idents_with_reserved,
};
use super::go_writer::GoWriter;
use super::{GOLEM_PKG, GoBridgeGenerator, case_idents};
use crate::bridge_gen::tool_bridge_client_directory_name;
use crate::bridge_gen::tool_common::{command_path, idx_to_usize, synthetic_agent_type};
use crate::fs;
use anyhow::{Context, anyhow};
use camino::Utf8Path;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::schema_value::SchemaValue;
use golem_common::schema::tool::canonical::CanonicalSurfaceRef;
use golem_common::schema::tool::{CommandNode, ErrorKind, FlagShape, OptionShape, Tool};
use std::collections::BTreeMap;

/// Generates a Go guest TOOL client module.
pub struct GoToolBridgeGenerator {
    tool: Tool,
    tool_name: String,
    inner: GoBridgeGenerator,
    names: ToolNames,
}

/// The package-level names a tool client declares. They share Go's single
/// package namespace with the generated types, so they are reserved before any
/// type is named.
struct ToolNames {
    /// The tool's identity type.
    marker: String,
    /// Per node with a body: the command's variable.
    commands: BTreeMap<usize, String>,
    /// Per node with a body: the argument struct.
    args: BTreeMap<usize, String>,
    /// Per node with globals: the globals struct.
    globals: BTreeMap<usize, String>,
    /// Per non-root node that has subcommands: the unexported group variable.
    groups: BTreeMap<usize, String>,
    /// The error variables, in declaration order.
    errors: Vec<ErrorVar>,
    /// Per (command, case name): the variable its case is matched with.
    error_of: BTreeMap<(usize, String), String>,
}

struct ErrorVar {
    ident: String,
    name: String,
    kind: ErrorKind,
    exit_code: u8,
    payload: Option<SchemaType>,
    summary: String,
}

const TOOL_VAR: &str = "Tool";

impl GoToolBridgeGenerator {
    pub fn new(tool: Tool, target_path: &Utf8Path, _testing: bool) -> anyhow::Result<Self> {
        let tool_name = tool
            .commands
            .nodes
            .first()
            .map(|node| node.name.clone())
            .ok_or_else(|| anyhow!("tool command tree must contain a root command"))?;
        let names = ToolNames::new(&tool)?;
        let synthetic = synthetic_agent_type(&tool, &tool_name)?;
        let inner = GoBridgeGenerator::new_tool_guest(
            synthetic,
            target_path,
            tool_bridge_client_directory_name(&tool_name),
            names.reserved(),
        )?;
        Ok(Self {
            tool,
            tool_name,
            inner,
            names,
        })
    }

    pub fn generate(&mut self) -> anyhow::Result<()> {
        if !self.inner.target_path.exists() {
            fs::create_dir_all(&self.inner.target_path)?;
        }
        self.inner.write_file("go.mod", self.inner.go_mod()?)?;
        self.inner
            .write_file("types.go", self.inner.types_file()?)?;
        self.inner
            .write_file("registry.go", self.inner.registry_file()?)?;
        self.inner.write_file("client.go", self.client_file()?)?;
        Ok(())
    }

    fn node(&self, index: usize) -> anyhow::Result<&CommandNode> {
        self.tool
            .commands
            .nodes
            .get(index)
            .with_context(|| format!("missing command node {index}"))
    }

    fn client_file(&self) -> anyhow::Result<String> {
        let mut writer = GoWriter::new();
        writer.import(GOLEM_PKG);

        let root = self.node(0)?;
        let summary = root.doc.summary.trim();
        writer.doc(&if summary.is_empty() {
            format!("{TOOL_VAR} is the {} tool.", self.tool_name)
        } else {
            format!(
                "{TOOL_VAR} is the {} tool: {}",
                self.tool_name,
                lower_first(summary)
            )
        });
        let marker = &self.names.marker;
        writer.doc(&format!(
            "{marker} identifies the {} tool, so its commands and errors cannot be used for another.",
            self.tool_name
        ));
        writer.line(format!("type {marker} struct{{}}"));
        writer.blank();
        writer.line(format!(
            "var {TOOL_VAR} = golem.DefineToolClient[{marker}]({})",
            go_string(&self.tool_name)
        ));
        writer.blank();

        for error in &self.names.errors {
            self.write_error(error, &mut writer)?;
        }
        self.write_node(0, &mut writer)?;
        Ok(writer.finish(&self.inner.package_name()))
    }

    /// The Go expression a node's children are declared on.
    fn parent_expr(&self, index: usize) -> String {
        if index == 0 {
            TOOL_VAR.to_string()
        } else {
            self.names.groups[&index].clone()
        }
    }

    fn write_node(&self, index: usize, writer: &mut GoWriter) -> anyhow::Result<()> {
        let node = self.node(index)?.clone();
        if index != 0 && !node.subcommands.is_empty() {
            let parent = self.parent_of(index)?;
            writer.line(format!(
                "var {} = {}.Group({})",
                self.names.groups[&index],
                self.parent_expr(parent),
                go_string(&node.name)
            ));
            writer.blank();
        }
        if let Some(globals) = self.names.globals.get(&index) {
            self.write_globals(index, globals, writer)?;
        }
        if node.body.is_some() {
            self.write_command(index, writer)?;
        } else if index != 0 && node.subcommands.is_empty() {
            let parent = self.parent_of(index)?;
            writer.line(format!(
                "var _ = {}.Group({})",
                self.parent_expr(parent),
                go_string(&node.name)
            ));
            writer.blank();
        }
        for child in &node.subcommands {
            self.write_node(idx_to_usize(*child)?, writer)?;
        }
        Ok(())
    }

    fn parent_of(&self, index: usize) -> anyhow::Result<usize> {
        for (parent, node) in self.tool.commands.nodes.iter().enumerate() {
            for child in &node.subcommands {
                if idx_to_usize(*child)? == index {
                    return Ok(parent);
                }
            }
        }
        Err(anyhow!("command node {index} has no parent"))
    }

    fn write_globals(&self, index: usize, name: &str, writer: &mut GoWriter) -> anyhow::Result<()> {
        let node = self.node(index)?;
        let mut surfaces = Vec::new();
        for i in 0..node.globals.options.len() {
            surfaces.push(CanonicalSurfaceRef::GlobalOption {
                node: index,
                index: i,
            });
        }
        for i in 0..node.globals.flags.len() {
            surfaces.push(CanonicalSurfaceRef::GlobalFlag {
                node: index,
                index: i,
            });
        }
        let fields = self.fields(index, &surfaces, &[], writer)?;

        writer.doc(&format!(
            "{name} holds the global options of {}, which every command below it inherits.",
            self.label(index)?
        ));
        write_struct(name, &[], &fields, false, writer);
        writer.line(format!(
            "var _ = {}.Globals[{name}](func(g *{name}, s *golem.ToolGlobalsSpec) {{",
            self.parent_expr(index)
        ));
        writer.indent();
        for field in &fields {
            writer.line(format!("s.{}", field.binding("g")));
        }
        writer.dedent();
        writer.line("})");
        writer.blank();
        Ok(())
    }

    fn write_command(&self, index: usize, writer: &mut GoWriter) -> anyhow::Result<()> {
        let node = self.node(index)?.clone();
        let body = node.body.as_ref().context("command without a body")?;
        let var = &self.names.commands[&index];
        let args = &self.names.args[&index];

        let embedded = self.ancestor_globals(index)?;
        let surfaces = self
            .tool
            .canonical_input_surfaces(index)
            .into_iter()
            .filter(|s| {
                !matches!(
                    s,
                    CanonicalSurfaceRef::GlobalOption { .. }
                        | CanonicalSurfaceRef::GlobalFlag { .. }
                )
            })
            .collect::<Vec<_>>();
        let reserved = embedded.iter().map(String::as_str).collect::<Vec<_>>();
        let mut fields = self.fields(index, &surfaces, &reserved, writer)?;
        if let Some(stdin) = &body.stdin {
            writer.import("io");
            let mut taken = reserved.clone();
            taken.extend(fields.iter().map(|f| f.ident.as_str()));
            let ident = unique_idents_with_reserved(vec!["Stdin".to_string()], &taken)
                .pop()
                .expect("one identifier");
            fields.push(Field {
                ident: ident.clone(),
                typ: "io.Reader".to_string(),
                binding: format!(
                    "Stdin(&{{}}.{ident}){}",
                    if stdin.required { "" } else { ".Optional()" }
                ),
            });
        }

        let result = match &body.result {
            Some(result) => self.inner.render(&result.type_, writer)?,
            None => "golem.Unit".to_string(),
        };
        let args_type = if embedded.is_empty() && fields.is_empty() {
            "golem.Unit".to_string()
        } else {
            writer.doc(&format!(
                "{args} holds the arguments of {}.",
                self.label(index)?
            ));
            write_struct(args, &embedded, &fields, true, writer);
            args.clone()
        };

        let summary = node.doc.summary.trim();
        writer.doc(&if summary.is_empty() {
            format!("{var} runs {}.", self.label(index)?)
        } else {
            format!("{var} {}", lower_first(summary))
        });
        let (constructor, name_arg) = match (
            index == 0 || !node.subcommands.is_empty(),
            body.stdout.is_some(),
        ) {
            (true, false) => ("Body", String::new()),
            (true, true) => ("StdoutBody", String::new()),
            (false, false) => ("Command", format!("{}, ", go_string(&node.name))),
            (false, true) => ("StdoutCommand", format!("{}, ", go_string(&node.name))),
        };
        let owner = if index == 0 || !node.subcommands.is_empty() {
            self.parent_expr(index)
        } else {
            self.parent_expr(self.parent_of(index)?)
        };
        let raises = body
            .errors
            .iter()
            .map(|case| self.names.error_of[&(index, case.name.clone())].clone())
            .collect::<Vec<_>>();
        if fields.is_empty() && raises.is_empty() {
            writer.line(format!(
                "var {var} = {owner}.{constructor}[{args_type}, {result}]({name_arg}nil)"
            ));
        } else {
            writer.line(format!(
                "var {var} = {owner}.{constructor}[{args_type}, {result}]({name_arg}func(a *{args_type}, s *golem.ToolCommandSpec) {{"
            ));
            writer.indent();
            for field in &fields {
                writer.line(format!("s.{}", field.binding("a")));
            }
            if !raises.is_empty() {
                writer.line(format!("s.Raises({})", raises.join(", ")));
            }
            writer.dedent();
            writer.line("})");
        }
        writer.blank();
        Ok(())
    }

    /// The globals structs a command's arguments embed, root first.
    fn ancestor_globals(&self, index: usize) -> anyhow::Result<Vec<String>> {
        let mut chain = vec![index];
        let mut at = index;
        while at != 0 {
            at = self.parent_of(at)?;
            chain.push(at);
        }
        chain.reverse();
        Ok(chain
            .into_iter()
            .filter_map(|node| self.names.globals.get(&node).cloned())
            .collect())
    }

    fn label(&self, index: usize) -> anyhow::Result<String> {
        Ok(format!("`{}`", command_path(&self.tool, index)?.join(" ")))
    }

    /// The struct fields and spec bindings of canonical input surfaces.
    fn fields(
        &self,
        command_index: usize,
        surfaces: &[CanonicalSurfaceRef],
        reserved: &[&str],
        writer: &mut GoWriter,
    ) -> anyhow::Result<Vec<Field>> {
        let mut canonical = Vec::with_capacity(surfaces.len());
        for surface in surfaces {
            canonical.push(
                self.tool
                    .canonical_field_for_surface(command_index, *surface)
                    .with_context(|| format!("unresolved input surface {surface:?}"))?,
            );
        }
        let idents = unique_idents_with_reserved(
            canonical.iter().map(|f| to_field_ident(&f.name)).collect(),
            reserved,
        );

        let mut out = Vec::with_capacity(surfaces.len());
        for ((surface, field), ident) in surfaces.iter().zip(&canonical).zip(idents) {
            let typ = self.inner.render(&field.type_, writer)?;
            let (method, default) = self.binding_of(command_index, *surface)?;
            let mut binding = format!("{method}(&{{}}.{ident})");
            if go_kebab(&ident) != field.name {
                binding.push_str(&format!(".Name({})", go_string(&field.name)));
            }
            if let Some(default) = default {
                binding.push_str(&format!(".Default({default})"));
            }
            out.push(Field {
                ident,
                typ,
                binding,
            });
        }
        Ok(out)
    }

    /// The spec method a surface binds with, and the Go literal of its default
    /// when it has one a literal can spell.
    fn binding_of(
        &self,
        command_index: usize,
        surface: CanonicalSurfaceRef,
    ) -> anyhow::Result<(&'static str, Option<String>)> {
        let option = |o: &golem_common::schema::tool::OptionSpec| {
            let method = match o.shape {
                OptionShape::RepeatableList(_) => "List",
                OptionShape::RepeatableMap(_) => "Map",
                _ => "Option",
            };
            let default = match (&o.shape, &o.default) {
                (OptionShape::Scalar(t) | OptionShape::OptionalScalar(t), Some(v)) => {
                    self.literal(t, v)
                }
                _ => None,
            };
            (method, default)
        };
        let flag = |f: &golem_common::schema::tool::FlagSpec| match f.shape {
            FlagShape::BoolFlag(shape) => ("Flag", shape.default.then(|| "true".to_string())),
            FlagShape::CountFlag(_) => ("CountFlag", None),
        };
        let body = || {
            self.node(command_index)
                .ok()
                .and_then(|node| node.body.as_ref())
                .context("surface of a command without a body")
        };
        Ok(match surface {
            CanonicalSurfaceRef::GlobalOption { node, index } => {
                option(&self.node(node)?.globals.options[index])
            }
            CanonicalSurfaceRef::GlobalFlag { node, index } => {
                flag(&self.node(node)?.globals.flags[index])
            }
            CanonicalSurfaceRef::BodyPositional { index } => {
                let p = &body()?.positionals.fixed[index];
                (
                    "Positional",
                    p.default.as_ref().and_then(|v| self.literal(&p.type_, v)),
                )
            }
            CanonicalSurfaceRef::BodyTail => ("Tail", None),
            CanonicalSurfaceRef::BodyOption { index } => option(&body()?.options[index]),
            CanonicalSurfaceRef::BodyFlag { index } => flag(&body()?.flags[index]),
        })
    }

    /// A Go literal for a default value of a primitive or enum type. A default
    /// no literal spells is left to the caller, who sets the field.
    fn literal(&self, typ: &SchemaType, value: &SchemaValue) -> Option<String> {
        Some(match value {
            SchemaValue::Bool(b) => b.to_string(),
            SchemaValue::S8(n) => n.to_string(),
            SchemaValue::S16(n) => n.to_string(),
            SchemaValue::S32(n) => n.to_string(),
            SchemaValue::S64(n) => n.to_string(),
            SchemaValue::U8(n) => n.to_string(),
            SchemaValue::U16(n) => n.to_string(),
            SchemaValue::U32(n) => n.to_string(),
            SchemaValue::U64(n) => n.to_string(),
            SchemaValue::F32(n) if n.is_finite() => format!("{n:?}"),
            SchemaValue::F64(n) if n.is_finite() => format!("{n:?}"),
            SchemaValue::Char(c) => format!("{}", *c as u32),
            SchemaValue::String(s) => go_string(s),
            SchemaValue::Enum { case } => {
                let name = self.inner.named(typ)?;
                let SchemaType::Enum { cases, .. } = self.inner.resolve(typ) else {
                    return None;
                };
                case_idents(&name, cases.iter().map(String::as_str))
                    .into_iter()
                    .nth(*case as usize)?
            }
            _ => return None,
        })
    }

    fn write_error(&self, error: &ErrorVar, writer: &mut GoWriter) -> anyhow::Result<()> {
        let payload = match &error.payload {
            Some(payload) => self.inner.render(payload, writer)?,
            None => "golem.Unit".to_string(),
        };
        writer.doc(&if error.summary.is_empty() {
            format!(
                "{} is the {} error of {}.",
                error.ident, error.name, self.tool_name
            )
        } else {
            format!(
                "{} is the {} error of {}: {}",
                error.ident,
                error.name,
                self.tool_name,
                lower_first(&error.summary)
            )
        });
        let kind = match error.kind {
            ErrorKind::UsageError => "golem.UsageError",
            ErrorKind::RuntimeError => "golem.RuntimeError",
        };
        writer.line(format!(
            "var {} = golem.DefineToolError[{payload}]({TOOL_VAR}, {}, golem.ToolErrorSpec{{Kind: {kind}, ExitCode: {}}})",
            error.ident,
            go_string(&error.name),
            error.exit_code
        ));
        writer.blank();
        Ok(())
    }
}

impl ToolNames {
    fn new(tool: &Tool) -> anyhow::Result<Self> {
        let mut commands = Vec::new();
        let mut globals = Vec::new();
        let mut groups = BTreeMap::new();
        for (index, node) in tool.commands.nodes.iter().enumerate() {
            let path = command_path(tool, index)?;
            let stem = if index == 0 {
                "Root".to_string()
            } else {
                to_exported_ident(&path[1..].join("-"))
            };
            if node.body.is_some() {
                commands.push((index, stem.clone()));
            }
            if !node.globals.options.is_empty() || !node.globals.flags.is_empty() {
                globals.push((index, format!("{stem}Globals")));
            }
            if index != 0 && !node.subcommands.is_empty() {
                groups.insert(
                    index,
                    format!("{}Group", to_unexported_ident(&path[1..].join("-"))),
                );
            }
        }

        // One variable per case name, unless two commands give the same name
        // different payloads; those get a variable per command.
        let mut by_name: BTreeMap<String, Vec<(usize, &golem_common::schema::tool::ErrorCase)>> =
            BTreeMap::new();
        for (index, node) in tool.commands.nodes.iter().enumerate() {
            if let Some(body) = &node.body {
                for case in &body.errors {
                    by_name
                        .entry(case.name.clone())
                        .or_default()
                        .push((index, case));
                }
            }
        }
        let command_stem: BTreeMap<usize, String> = commands.iter().cloned().collect();
        let mut error_candidates = Vec::new();
        for (name, uses) in &by_name {
            let shared = uses.iter().all(|(_, c)| c.payload == uses[0].1.payload);
            if shared {
                error_candidates.push((format!("Err{}", to_exported_ident(name)), uses.clone()));
            } else {
                for (index, case) in uses {
                    error_candidates.push((
                        format!("Err{}{}", command_stem[index], to_exported_ident(name)),
                        vec![(*index, *case)],
                    ));
                }
            }
        }

        // Every exported name shares one namespace, so they are made unique
        // together, in a stable order.
        let tool_name = tool
            .commands
            .nodes
            .first()
            .map(|n| n.name.as_str())
            .unwrap_or_default();
        let mut all = vec![
            TOOL_VAR.to_string(),
            format!("{}Tool", to_exported_ident(tool_name)),
        ];
        all.extend(commands.iter().map(|(_, stem)| stem.clone()));
        all.extend(commands.iter().map(|(_, stem)| format!("{stem}Args")));
        all.extend(globals.iter().map(|(_, name)| name.clone()));
        all.extend(error_candidates.iter().map(|(ident, _)| ident.clone()));
        let unique = unique_idents(all);
        let mut it = unique.into_iter().skip(1);
        let marker = it.next().expect("a name");

        let command_vars = commands
            .iter()
            .map(|(index, _)| (*index, it.next().expect("a name")))
            .collect::<BTreeMap<_, _>>();
        let args = commands
            .iter()
            .map(|(index, _)| (*index, it.next().expect("a name")))
            .collect::<BTreeMap<_, _>>();
        let globals = globals
            .iter()
            .map(|(index, _)| (*index, it.next().expect("a name")))
            .collect::<BTreeMap<_, _>>();
        let mut errors = Vec::new();
        let mut error_of = BTreeMap::new();
        for (_, uses) in error_candidates {
            let ident = it.next().expect("a name");
            let case = uses[0].1;
            for (index, case) in &uses {
                error_of.insert((*index, case.name.clone()), ident.clone());
            }
            errors.push(ErrorVar {
                ident,
                name: case.name.clone(),
                kind: case.kind,
                exit_code: case.exit_code,
                payload: case.payload.clone(),
                summary: case.doc.summary.trim().to_string(),
            });
        }

        Ok(Self {
            marker,
            commands: command_vars,
            args,
            globals,
            groups,
            errors,
            error_of,
        })
    }

    fn reserved(&self) -> Vec<String> {
        let mut out = vec![TOOL_VAR.to_string(), self.marker.clone()];
        out.extend(self.commands.values().cloned());
        out.extend(self.args.values().cloned());
        out.extend(self.globals.values().cloned());
        out.extend(self.errors.iter().map(|e| e.ident.clone()));
        out
    }
}

/// One generated struct field and the spec call that binds it; `{}` in the
/// binding stands for the struct variable.
struct Field {
    ident: String,
    typ: String,
    binding: String,
}

impl Field {
    fn binding(&self, var: &str) -> String {
        self.binding.replace("{}", var)
    }
}

fn write_struct(
    name: &str,
    embedded: &[String],
    fields: &[Field],
    allow_empty: bool,
    writer: &mut GoWriter,
) {
    if embedded.is_empty() && fields.is_empty() && allow_empty {
        writer.line(format!("type {name} struct{{}}"));
        writer.blank();
        return;
    }
    writer.line(format!("type {name} struct {{"));
    writer.indent();
    for e in embedded {
        writer.line(e);
    }
    let width = fields.iter().map(|f| f.ident.len()).max().unwrap_or(0);
    for field in fields {
        writer.line(format!("{:<width$} {}", field.ident, field.typ));
    }
    writer.dedent();
    writer.line("}");
    writer.blank();
}

/// The wire name the guest SDK derives from a Go field name, which decides
/// whether a binding has to name its field explicitly. It must stay the same
/// as the SDK's own `kebab`.
fn go_kebab(name: &str) -> String {
    let chars = name.chars().collect::<Vec<_>>();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev_lower =
                i > 0 && (chars[i - 1].is_lowercase() || chars[i - 1].is_ascii_digit());
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            let prev_upper = i > 0 && chars[i - 1].is_uppercase();
            if i > 0 && (prev_lower || (prev_upper && next_lower)) {
                out.push('-');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::go_kebab;
    use test_r::test;

    #[test]
    fn go_kebab_matches_the_sdk() {
        for (input, expected) in [
            ("Name", "name"),
            ("GitDir", "git-dir"),
            ("URLPath", "url-path"),
            ("MaxCount", "max-count"),
            ("HTTP", "http"),
            ("Retry2Times", "retry2-times"),
            ("ID", "id"),
        ] {
            assert_eq!(go_kebab(input), expected, "{input}");
        }
    }
}
