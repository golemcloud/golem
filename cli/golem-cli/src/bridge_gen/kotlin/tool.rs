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

//! Guest-mode Kotlin tool client: a typed surface over the Kotlin SDK's
//! `ToolRpc`.
//!
//! A tool's command tree becomes one client class per command that has
//! subcommands. A command group with global options is reached through an
//! accessor that takes those options and passes them to every command below.
//! The input record of a call lists the inherited globals first and the
//! command's own fields after them, in the host's canonical order.

use crate::bridge_gen::kotlin::guest;
use crate::bridge_gen::kotlin::kotlin::{
    escape_kotlin_ident, to_kotlin_term_ident, to_kotlin_type_ident, unique_idents_with_reserved,
};
use crate::bridge_gen::kotlin::kotlin_writer::KotlinWriter;
use crate::bridge_gen::kotlin::sdk_wit::sdk_wit_input_type;
use crate::bridge_gen::kotlin::{
    KotlinBridgeGenerator, KotlinBridgeMode, PACKAGE_ROOT_NAMES, kotlin_string_literal,
};
use crate::bridge_gen::tool_bridge_client_directory_name;
use crate::bridge_gen::tool_common::{
    command_path, field_names, global_surfaces, idx_to_usize, synthetic_agent_type,
};
use crate::fs;
use crate::sdk_overrides::sdk_overrides;
use crate::versions::kotlin_dep;
use anyhow::{Context, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use golem_common::schema::tool::canonical::{CanonicalInputField, CanonicalSurfaceRef};
use golem_common::schema::tool::{CommandBody, CommandNode, Tool};
use heck::ToUpperCamelCase;
use std::collections::{BTreeMap, BTreeSet};

const R: &str = "cloud.golem.runtime";
const NOTHING: &str = "kotlin.Nothing";

/// Parameter names the generated client classes use for their own members and
/// locals, which a user parameter must not shadow.
const RESERVED_PARAM_NAMES: &[&str] = &[
    "golemRpc",
    "golemInherited",
    "golemSv",
    "golemValue",
    "stdin",
    "close",
];

/// Names of the temporaries the structural codecs introduce at depth zero.
const CODEC_TEMP_NAMES: &[&str] = &["e0", "elems0", "k0", "v0", "r0", "p", "t0"];

#[derive(Clone)]
struct ClientNode {
    index: usize,
    class_name: String,
    provided: Vec<CanonicalSurfaceRef>,
}

type CollectedNames = (
    BTreeMap<usize, ClientNode>,
    BTreeMap<usize, String>,
    Vec<String>,
);

/// Generates a Kotlin/Wasm guest tool client project.
pub struct KotlinToolBridgeGenerator {
    tool: Tool,
    tool_name: String,
    rpc_tool_name: String,
    target_path: Utf8PathBuf,
    inner: KotlinBridgeGenerator,
    clients: BTreeMap<usize, ClientNode>,
    error_names: BTreeMap<usize, String>,
}

impl KotlinToolBridgeGenerator {
    pub fn new(tool: Tool, target_path: &Utf8Path, testing: bool) -> anyhow::Result<Self> {
        let tool_name = tool
            .name()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("tool command tree must contain a root command"))?;
        Self::new_with_rpc_tool_name(tool, tool_name, target_path, testing)
    }

    pub fn new_with_rpc_tool_name(
        tool: Tool,
        rpc_tool_name: String,
        target_path: &Utf8Path,
        _testing: bool,
    ) -> anyhow::Result<Self> {
        let tool_name = tool
            .name()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("tool command tree must contain a root command"))?;
        let (clients, error_names, reserved) = collect_names(&tool, &tool_name)?;
        let mut synthetic = synthetic_agent_type(&tool, &tool_name)?;
        synthetic.type_name = golem_common::model::agent::AgentTypeName(rpc_tool_name.clone());
        let mut inner = KotlinBridgeGenerator::new_core(
            synthetic,
            target_path,
            KotlinBridgeMode::GuestWasmRpc,
            reserved,
        )?;
        inner.package_suffix = "_tool";
        Ok(Self {
            tool,
            tool_name,
            rpc_tool_name,
            target_path: target_path.to_path_buf(),
            inner,
            clients,
            error_names,
        })
    }

    pub fn generate(&mut self) -> anyhow::Result<()> {
        if !self.target_path.exists() {
            fs::create_dir_all(&self.target_path)?;
        }
        self.write_build_files()?;
        self.write_client()
    }

    fn write_build_files(&self) -> anyhow::Result<()> {
        let name = tool_bridge_client_directory_name(&self.rpc_tool_name);
        let sdk_version = sdk_overrides()?.kotlin_sdk_dep();
        fs::write_str(
            self.target_path.join("settings.gradle.kts"),
            guest::settings_gradle(&name, kotlin_dep::KOTLIN_PLUGIN_VERSION),
        )?;
        fs::write_str(
            self.target_path.join("build.gradle.kts"),
            guest::build_gradle(&sdk_version),
        )?;
        fs::write_str(
            self.target_path.join("gradle.properties"),
            guest::GRADLE_PROPERTIES,
        )
    }

    fn write_client(&mut self) -> anyhow::Result<()> {
        let content = self.generate_client_source()?;
        let mut path = self
            .target_path
            .join("src")
            .join("wasmWasiMain")
            .join("kotlin");
        for segment in self.inner.client_package().split('.') {
            path.push(segment);
        }
        path.push(format!("{}.kt", self.root_client_name()?));
        fs::write_str(path, content)
    }

    fn generate_client_source(&mut self) -> anyhow::Result<String> {
        let mut body = KotlinWriter::new();
        self.client_items(&mut body)?;
        self.error_items(&mut body)?;
        let mut types = KotlinWriter::new();
        self.inner.write_type_definitions(&mut types)?;
        self.inner.write_codecs(&mut types)?;
        let mut tuples = KotlinWriter::new();
        self.inner.write_tuple_types(&mut tuples);

        let mut out = KotlinWriter::new();
        out.line("// Generated by golem-cli. Do not edit.");
        out.line("@file:Suppress(\"unused\", \"RedundantVisibilityModifier\", \"UNCHECKED_CAST\", \"NAME_SHADOWING\")");
        out.blank();
        out.line(format!("package {}", self.inner.client_package()));
        out.blank();
        out.line("import kotlin.time.toDuration");
        out.line("import cloud.golem.runtime.callTyped as golemCallTyped");
        out.line("import cloud.golem.runtime.callTypedWithOutputs as golemCallTypedWithOutputs");
        out.line("import cloud.golem.runtime.toolInput as golemToolInput");
        out.blank();
        out.line(body.finish());
        out.line(types.finish());
        out.line(tuples.finish());
        out.blank();
        out.line(self.inner.codec_helper_source());
        Ok(out.finish())
    }

    // --- Clients ------------------------------------------------------------

    fn client_items(&mut self, writer: &mut KotlinWriter) -> anyhow::Result<()> {
        let clients = self.clients.values().cloned().collect::<Vec<_>>();
        for client in clients {
            let node = self.node(client.index)?.clone();
            write_doc(writer, &node.doc.summary, &node.doc.description);
            let root = client.index == 0;
            let implements = if root { " : kotlin.AutoCloseable" } else { "" };
            writer.line(format!(
                "class {} internal constructor(private val golemRpc: {R}.ToolRpc, private val golemInherited: kotlin.collections.List<{R}.ToolInputField>){implements} {{",
                client.class_name
            ));
            writer.indent();
            if root {
                writer.line(format!(
                    "companion object {{\n    /** Connects to the tool `{}`. */\n    fun newClient(): {} = {}({R}.ToolRpc({}), kotlin.collections.emptyList())\n}}",
                    crate::bridge_gen::kotlin::kdoc_safe(&self.rpc_tool_name),
                    client.class_name,
                    client.class_name,
                    kotlin_string_literal(&self.rpc_tool_name)
                ));
                writer.blank();
            }
            self.client_methods(writer, &client)?;
            if root {
                writer.line("/** Releases the underlying tool-rpc handle. */");
                writer.line("override fun close() = golemRpc.close()");
            }
            writer.dedent();
            writer.line("}");
            writer.blank();
        }
        Ok(())
    }

    fn client_methods(
        &mut self,
        writer: &mut KotlinWriter,
        client: &ClientNode,
    ) -> anyhow::Result<()> {
        let reserved = [
            "golemRpc",
            "golemInherited",
            "close",
            "newClient",
            "equals",
            "hashCode",
            "toString",
        ];
        let mut planned_names = Vec::new();
        let mut planned = Vec::new();
        if self.node(client.index)?.body.is_some() {
            let name = if client.index == 0 {
                self.tool_name.clone()
            } else {
                self.node(client.index)?.name.clone()
            };
            planned_names.push(to_kotlin_term_ident(&name, false));
            planned.push((client.index, false));
        }
        for child_index in self.child_indices(client.index)? {
            let child = self.node(child_index)?;
            let has_subcommands = !child.subcommands.is_empty();
            if has_subcommands || child.body.is_some() {
                planned_names.push(to_kotlin_term_ident(&child.name, false));
                planned.push((child_index, has_subcommands));
            }
        }
        let method_names = unique_idents_with_reserved(planned_names, &reserved);
        for ((index, is_accessor), method_name) in planned.into_iter().zip(method_names) {
            if is_accessor {
                self.accessor_method(writer, index, client, &method_name)?;
            } else {
                self.leaf_method(writer, index, client, &method_name)?;
            }
            writer.blank();
        }
        Ok(())
    }

    fn accessor_method(
        &mut self,
        writer: &mut KotlinWriter,
        child_index: usize,
        parent: &ClientNode,
        method_name: &str,
    ) -> anyhow::Result<()> {
        let child_client = self
            .clients
            .get(&child_index)
            .cloned()
            .context("missing child client")?;
        let child = self.node(child_index)?.clone();
        write_doc(writer, &child.doc.summary, &child.doc.description);
        let fields = global_surfaces(&self.tool, child_index)
            .into_iter()
            .filter(|surface| !parent.provided.contains(surface))
            .map(|surface| self.field(child_index, surface))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let names = self.param_names(&fields);
        let params = self.param_decls(&fields, &names)?;
        let inherited = self.input_fields(&fields, &names)?;
        let list = if fields.is_empty() {
            "golemInherited".to_string()
        } else {
            format!(
                "golemInherited + kotlin.collections.listOf({})",
                inherited.join(", ")
            )
        };
        writer.line(format!(
            "fun {method_name}({}): {} =",
            params.join(", "),
            child_client.class_name
        ));
        writer.line(format!("    {}(golemRpc, {list})", child_client.class_name));
        Ok(())
    }

    fn leaf_method(
        &mut self,
        writer: &mut KotlinWriter,
        command_index: usize,
        client: &ClientNode,
        method_name: &str,
    ) -> anyhow::Result<()> {
        let node = self.node(command_index)?.clone();
        let body = node
            .body
            .as_ref()
            .context("leaf method needs command body")?;
        write_doc(writer, &node.doc.summary, &node.doc.description);
        let (_, remaining_fields) = self.leaf_fields(command_index, &client.provided)?;
        let names = self.param_names(&remaining_fields);
        let mut params = self.param_decls(&remaining_fields, &names)?;
        let stdin_arg = match &body.stdin {
            Some(stdin) if stdin.required => {
                params.push("stdin: kotlin.ByteArray".to_string());
                Some("stdin")
            }
            Some(_) => {
                params.push("stdin: kotlin.ByteArray? = null".to_string());
                Some("stdin")
            }
            None => None,
        };
        let result_ty = self.result_type(body)?;
        let path_expr = self.command_path_expr(command_index)?;
        let error_type = self.error_type_ref(command_index, body);
        let has_outputs = body.stdout.is_some() || body.stderr.is_some();
        let value_type = if has_outputs {
            format!("{R}.WithOutputs<{result_ty}>")
        } else {
            result_ty.clone()
        };

        let own_pairs = remaining_fields
            .iter()
            .zip(&names)
            .map(|(field, _)| {
                Ok(format!(
                    "{} to {}",
                    kotlin_string_literal(&field.name),
                    kotlin_string_literal(&sdk_wit_input_type(
                        self.inner.type_naming.graph(),
                        &field.type_
                    )?)
                ))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let values = remaining_fields
            .iter()
            .zip(&names)
            .map(|(field, name)| self.inner.encode_expr(name, &field.type_, 0))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let input = format!(
            "golemToolInput(golemInherited, kotlin.collections.listOf({}), kotlin.collections.listOf({}))",
            own_pairs.join(", "),
            values.join(", ")
        );
        let error_decoder = if body.errors.is_empty() {
            "{ _, _ -> null }".to_string()
        } else {
            format!("{}::decode", self.error_type_ref(command_index, body))
        };
        let decode_result = self.result_decode(body)?;
        let (call_cap, stdin_part, capture_part) = if has_outputs {
            (
                "CallTypedWithOutputs",
                format!("{}, ", stdin_arg.unwrap_or("null")),
                format!("{}, {}, ", body.stdout.is_some(), body.stderr.is_some()),
            )
        } else {
            (
                "CallTyped",
                stdin_arg.map(|s| format!("{s}, ")).unwrap_or_default(),
                String::new(),
            )
        };
        writer.line(format!(
            "fun {method_name}({}): {R}.Either<{R}.ToolInvokeError<{error_type}>, {value_type}> =",
            params.join(", ")
        ));
        writer.line(format!(
            "    golemRpc.golem{call_cap}({path_expr}, {input}, {stdin_part}{capture_part}{error_decoder}, {decode_result})"
        ));
        Ok(())
    }

    fn result_decode(&mut self, body: &CommandBody) -> anyhow::Result<String> {
        match &body.result {
            Some(result) => {
                let dec = self.inner.decode_expr("golemValue", &result.type_, 0)?;
                Ok(format!(
                    "{{ golemSv ->\n        val golemValue = golemSv ?: throw kotlin.IllegalStateException(\"tool result did not contain a value\")\n        {dec}\n    }}"
                ))
            }
            None => Ok(
                "{ golemSv ->\n        if (golemSv != null) throw kotlin.IllegalStateException(\"tool result unexpectedly contained a value\")\n        kotlin.Unit\n    }"
                    .to_string(),
            ),
        }
    }

    // --- Errors -------------------------------------------------------------

    fn error_items(&mut self, writer: &mut KotlinWriter) -> anyhow::Result<()> {
        for (command_index, error_name) in self.error_names.clone() {
            let node = self.node(command_index)?.clone();
            let body = node
                .body
                .as_ref()
                .context("error class command has no body")?;
            let qualified = self.client_type_ref(&error_name);
            let variants = error_variant_names(body);
            writer.line(format!("sealed class {error_name} {{"));
            writer.indent();
            for (case, variant) in body.errors.iter().zip(&variants) {
                if let Some(payload) = &case.payload {
                    let typ = self.inner.type_reference(payload)?;
                    writer.line(format!(
                        "data class {variant}(val value: {typ}) : {qualified}()"
                    ));
                } else {
                    writer.line(format!("data object {variant} : {qualified}()"));
                }
            }
            writer.blank();
            writer.line("companion object {");
            writer.indent();
            writer.line(format!(
                "/** The declared error `name` with `payload`, or `null` when the pair matches no declared case. */\nfun decode(name: kotlin.String, payload: {R}.TypedSchemaValue): {qualified}? = when (name) {{"
            ));
            writer.indent();
            for (case, variant) in body.errors.iter().zip(&variants) {
                let lit = kotlin_string_literal(&case.name);
                if let Some(p) = &case.payload {
                    let dec = self.inner.decode_expr("payload.value", p, 0)?;
                    writer.line(format!(
                        "{lit} -> try {{ {qualified}.{variant}({dec}) }} catch (e: kotlin.RuntimeException) {{ null }}"
                    ));
                } else {
                    writer.line(format!("{lit} -> {qualified}.{variant}"));
                }
            }
            writer.line("else -> null");
            writer.dedent();
            writer.line("}");
            writer.dedent();
            writer.line("}");
            writer.dedent();
            writer.line("}");
            writer.blank();
        }
        Ok(())
    }

    // --- Helpers ------------------------------------------------------------

    fn result_type(&mut self, body: &CommandBody) -> anyhow::Result<String> {
        match &body.result {
            Some(result) => self.inner.type_reference(&result.type_),
            None => Ok("kotlin.Unit".to_string()),
        }
    }

    fn client_type_ref(&self, name: &str) -> String {
        format!("{}.{name}", self.inner.client_package())
    }

    fn error_type_ref(&self, command_index: usize, body: &CommandBody) -> String {
        if body.errors.is_empty() {
            NOTHING.to_string()
        } else {
            let error_name = self
                .error_names
                .get(&command_index)
                .expect("error class name");
            self.client_type_ref(error_name)
        }
    }

    fn leaf_fields(
        &self,
        command_index: usize,
        provided: &[CanonicalSurfaceRef],
    ) -> anyhow::Result<(Vec<CanonicalInputField>, Vec<CanonicalInputField>)> {
        let provided_fields = provided
            .iter()
            .map(|surface| self.field(command_index, *surface))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let provided_names = provided_fields
            .iter()
            .flat_map(field_names)
            .collect::<BTreeSet<_>>();
        let mut remaining = Vec::new();
        for surface in self.tool.canonical_input_surfaces(command_index) {
            let field = self.field(command_index, surface)?;
            if provided.contains(&surface)
                || field_names(&field)
                    .into_iter()
                    .any(|name| provided_names.contains(&name))
            {
                continue;
            }
            remaining.push(field);
        }
        Ok((provided_fields, remaining))
    }

    fn param_names(&self, fields: &[CanonicalInputField]) -> Vec<String> {
        let mut reserved: Vec<String> = PACKAGE_ROOT_NAMES
            .iter()
            .chain(RESERVED_PARAM_NAMES)
            .chain(CODEC_TEMP_NAMES)
            .map(|s| s.to_string())
            .collect();
        reserved.extend((0..32).map(|i| format!("te{i}")));
        let refs: Vec<&str> = reserved.iter().map(String::as_str).collect();
        unique_idents_with_reserved(
            fields
                .iter()
                .map(|field| {
                    guest::avoid_generated_prefix(to_kotlin_term_ident(&field.name, false))
                })
                .collect(),
            &refs,
        )
    }

    fn param_decls(
        &mut self,
        fields: &[CanonicalInputField],
        names: &[String],
    ) -> anyhow::Result<Vec<String>> {
        fields
            .iter()
            .zip(names)
            .map(|(field, name)| {
                Ok(format!(
                    "{name}: {}",
                    self.inner.type_reference(&field.type_)?
                ))
            })
            .collect()
    }

    /// `ToolInputField(name, grammar, value)` expressions for inherited globals.
    fn input_fields(
        &self,
        fields: &[CanonicalInputField],
        names: &[String],
    ) -> anyhow::Result<Vec<String>> {
        fields
            .iter()
            .zip(names)
            .map(|(field, name)| {
                Ok(format!(
                    "{R}.ToolInputField({}, {}, {})",
                    kotlin_string_literal(&field.name),
                    kotlin_string_literal(&sdk_wit_input_type(
                        self.inner.type_naming.graph(),
                        &field.type_
                    )?),
                    self.inner.encode_expr(name, &field.type_, 0)?
                ))
            })
            .collect()
    }

    fn command_path_expr(&self, command_index: usize) -> anyhow::Result<String> {
        let segments = command_path(&self.tool, command_index)?
            .into_iter()
            .skip(1)
            .map(|segment| kotlin_string_literal(&segment))
            .collect::<Vec<_>>();
        Ok(format!(
            "kotlin.collections.listOf<kotlin.String>({})",
            segments.join(", ")
        ))
    }

    fn root_client_name(&self) -> anyhow::Result<String> {
        self.clients
            .get(&0)
            .map(|client| client.class_name.clone())
            .context("missing root client")
    }

    fn node(&self, index: usize) -> anyhow::Result<&CommandNode> {
        self.tool
            .commands
            .nodes
            .get(index)
            .ok_or_else(|| anyhow!("command node index {index} out of bounds"))
    }

    fn child_indices(&self, index: usize) -> anyhow::Result<Vec<usize>> {
        self.node(index)?
            .subcommands
            .iter()
            .map(|idx| idx_to_usize(*idx))
            .collect()
    }

    fn field(
        &self,
        command_index: usize,
        surface: CanonicalSurfaceRef,
    ) -> anyhow::Result<CanonicalInputField> {
        self.tool
            .canonical_field_for_surface(command_index, surface)
            .ok_or_else(|| {
                anyhow!("canonical surface {surface:?} did not resolve for command {command_index}")
            })
    }
}

fn collect_names(tool: &Tool, tool_name: &str) -> anyhow::Result<CollectedNames> {
    let mut clients = BTreeMap::new();
    let mut error_names = BTreeMap::new();
    let mut reserved = BTreeSet::new();
    let mut used = BTreeSet::new();
    collect_node_names(
        tool,
        tool_name,
        0,
        Vec::new(),
        Vec::new(),
        &mut clients,
        &mut error_names,
        &mut used,
        &mut reserved,
    )?;
    Ok((clients, error_names, reserved.into_iter().collect()))
}

#[allow(clippy::too_many_arguments)]
fn collect_node_names(
    tool: &Tool,
    tool_name: &str,
    index: usize,
    path: Vec<String>,
    provided: Vec<CanonicalSurfaceRef>,
    clients: &mut BTreeMap<usize, ClientNode>,
    error_names: &mut BTreeMap<usize, String>,
    used: &mut BTreeSet<String>,
    reserved: &mut BTreeSet<String>,
) -> anyhow::Result<()> {
    let node = tool
        .commands
        .nodes
        .get(index)
        .context("command index out of bounds")?;
    if !node.subcommands.is_empty() || index == 0 {
        let class_name = unique_name(client_class_name(tool_name, &path), used);
        reserved.insert(class_name.clone());
        clients.insert(
            index,
            ClientNode {
                index,
                class_name,
                provided: provided.clone(),
            },
        );
    }
    if node
        .body
        .as_ref()
        .is_some_and(|body| !body.errors.is_empty())
    {
        let error_name = unique_name(error_class_name(tool_name, &path), used);
        reserved.insert(error_name.clone());
        error_names.insert(index, error_name);
    }
    for child_index in &node.subcommands {
        let child_index = idx_to_usize(*child_index)?;
        let mut child_path = path.clone();
        let child = tool
            .commands
            .nodes
            .get(child_index)
            .context("child command index out of bounds")?;
        child_path.push(child.name.clone());
        let mut child_provided = provided.clone();
        child_provided.extend(
            global_surfaces(tool, child_index)
                .into_iter()
                .filter(|surface| !provided.contains(surface)),
        );
        collect_node_names(
            tool,
            tool_name,
            child_index,
            child_path,
            child_provided,
            clients,
            error_names,
            used,
            reserved,
        )?;
    }
    Ok(())
}

fn prefixed_name(tool_name: &str, path: &[String], suffix: &str) -> String {
    let mut name = tool_name.to_upper_camel_case();
    for segment in path {
        name.push_str(&segment.to_upper_camel_case());
    }
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        name = format!("Tool{name}");
    }
    name.push_str(suffix);
    to_kotlin_type_ident(&name, false)
}

fn client_class_name(tool_name: &str, path: &[String]) -> String {
    prefixed_name(tool_name, path, "Client")
}

fn error_class_name(tool_name: &str, path: &[String]) -> String {
    prefixed_name(tool_name, path, "Error")
}

fn error_variant_names(body: &CommandBody) -> Vec<String> {
    unique_idents_with_reserved(
        body.errors
            .iter()
            .map(|case| escape_kotlin_ident(&case.name.to_upper_camel_case()))
            .collect(),
        &["Companion"],
    )
}

fn unique_name(preferred: String, used: &mut BTreeSet<String>) -> String {
    if used.insert(preferred.clone()) {
        return preferred;
    }
    for suffix in 2.. {
        let candidate = format!("{preferred}{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!()
}

fn write_doc(writer: &mut KotlinWriter, summary: &str, description: &str) {
    let doc = if summary.is_empty() {
        description.to_string()
    } else if description.is_empty() {
        summary.to_string()
    } else {
        format!("{summary}\n\n{description}")
    };
    writer.doc(&doc);
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::graph::{SchemaGraph, SchemaTypeDef};
    use golem_common::schema::metadata::TypeId;
    use golem_common::schema::schema_type::SchemaType;
    use golem_common::schema::tool::{
        BoolFlagShape, CommandIndex, CommandTree, Doc, ErrorCase, ErrorKind, FlagShape, FlagSpec,
        Globals, OptionShape, OptionSpec, Positional, Positionals, ResultSpec, StreamSpec,
        TailPositional,
    };
    use test_r::test;

    fn doc(summary: &str) -> Doc {
        Doc {
            summary: summary.to_string(),
            description: String::new(),
            examples: vec![],
        }
    }

    fn body() -> CommandBody {
        CommandBody {
            positionals: Positionals::default(),
            options: vec![],
            flags: vec![],
            constraints: vec![],
            stdin: None,
            stdout: None,
            stderr: None,
            result: None,
            errors: vec![],
            annotations: None,
        }
    }

    fn node(name: &str) -> CommandNode {
        CommandNode {
            name: name.to_string(),
            aliases: vec![],
            doc: doc(name),
            globals: Globals::default(),
            subcommands: vec![],
            body: None,
        }
    }

    fn positional(name: &str, type_: SchemaType) -> Positional {
        Positional {
            name: name.to_string(),
            doc: doc(name),
            value_name: None,
            type_,
            default: None,
            required: true,
            accepts_stdio: false,
        }
    }

    fn option(long: &str, shape: OptionShape) -> OptionSpec {
        OptionSpec {
            long: long.to_string(),
            short: None,
            aliases: vec![],
            doc: doc(long),
            value_name: None,
            shape,
            default: None,
            required: false,
            env_var: None,
        }
    }

    fn flag(long: &str, shape: FlagShape) -> FlagSpec {
        FlagSpec {
            long: long.to_string(),
            short: None,
            aliases: vec![],
            doc: doc(long),
            shape,
            env_var: None,
        }
    }

    /// A `grep` tool: a root command with a global option and flag, an
    /// enum-typed global, declared errors, and a `replace` subcommand that
    /// captures stdout.
    fn grep_tool() -> Tool {
        let color_type = TypeId::from("color-mode");
        let color_ref = SchemaType::ref_to(color_type.clone());
        let mut root = node("grep");
        root.globals = Globals {
            options: vec![option("color", OptionShape::Scalar(color_ref))],
            flags: vec![flag(
                "case-sensitive",
                FlagShape::BoolFlag(BoolFlagShape {
                    default: false,
                    negatable: false,
                }),
            )],
        };
        root.subcommands = vec![CommandIndex(1)];
        root.body = Some(CommandBody {
            positionals: Positionals {
                fixed: vec![positional("pattern", SchemaType::string())],
                tail: Some(TailPositional {
                    name: "files".to_string(),
                    doc: doc("files"),
                    value_name: None,
                    item_type: SchemaType::string(),
                    min: 0,
                    max: None,
                    separator: None,
                    verbatim: false,
                    accepts_stdio: false,
                }),
            },
            options: vec![option(
                "max-count",
                OptionShape::OptionalScalar(SchemaType::u32()),
            )],
            flags: vec![flag("verbosity", FlagShape::CountFlag(None))],
            stdin: Some(StreamSpec {
                doc: doc("stdin"),
                mime: vec![],
                required: false,
            }),
            result: Some(ResultSpec {
                type_: SchemaType::list(SchemaType::string()),
                doc: doc("matches"),
                formatters: vec![],
                default_formatter: String::new(),
            }),
            errors: vec![
                ErrorCase {
                    name: "bad-pattern".to_string(),
                    doc: doc("bad pattern"),
                    kind: ErrorKind::UsageError,
                    exit_code: 2,
                    payload: Some(SchemaType::string()),
                },
                ErrorCase {
                    name: "io".to_string(),
                    doc: doc("io"),
                    kind: ErrorKind::RuntimeError,
                    exit_code: 1,
                    payload: None,
                },
            ],
            ..body()
        });
        let mut replace = node("replace");
        replace.body = Some(CommandBody {
            positionals: Positionals {
                fixed: vec![
                    positional("pattern", SchemaType::string()),
                    positional("replacement", SchemaType::string()),
                ],
                tail: None,
            },
            stdout: Some(StreamSpec {
                doc: doc("stdout"),
                mime: vec![],
                required: true,
            }),
            ..body()
        });
        Tool {
            version: "1".to_string(),
            requires_filesystem: false,
            commands: CommandTree {
                nodes: vec![root, replace],
            },
            schema: SchemaGraph {
                defs: vec![SchemaTypeDef {
                    id: color_type,
                    name: None,
                    body: SchemaType::r#enum(vec![
                        "never".to_string(),
                        "always".to_string(),
                        "auto".to_string(),
                    ]),
                }],
                root: SchemaType::record(vec![]),
            },
        }
    }

    fn generated(rpc_name: &str) -> (Utf8PathBuf, String) {
        let dir = tempfile::TempDir::new().unwrap().keep();
        let target = Utf8PathBuf::from_path_buf(dir.join("out")).unwrap();
        let mut generator = KotlinToolBridgeGenerator::new_with_rpc_tool_name(
            grep_tool(),
            rpc_name.to_string(),
            &target,
            true,
        )
        .unwrap();
        generator.generate().unwrap();
        let segment = format!("{}_tool", rpc_name.replace('-', "_"));
        let source = std::fs::read_to_string(target.join(format!(
            "src/wasmWasiMain/kotlin/golem/bridge/client/{segment}/GrepClient.kt"
        )))
        .unwrap();
        (target, source)
    }

    #[test]
    fn the_presented_root_name_is_separate_from_the_rpc_name() {
        let (target, source) = generated("runtime-search");
        for expected in [
            "package golem.bridge.client.runtime_search_tool",
            "class GrepClient internal constructor(",
            "fun newClient(): GrepClient = GrepClient(cloud.golem.runtime.ToolRpc(\"runtime-search\"), kotlin.collections.emptyList())",
            "fun grep(",
            "fun replace(",
            "sealed class GrepError {",
            "golem.bridge.client.runtime_search_tool.ColorMode",
        ] {
            assert!(source.contains(expected), "missing {expected}:\n{source}");
        }
        let settings = std::fs::read_to_string(target.join("settings.gradle.kts")).unwrap();
        assert!(
            settings.contains("rootProject.name = \"runtime-search-tool-guest-client\""),
            "{settings}"
        );
    }

    #[test]
    fn refined_arguments_are_sent_with_their_bounds_in_the_call_input_schema() {
        use golem_common::schema::schema_type::{NumericBound, NumericRestrictions};
        let mut tool = grep_tool();
        let level = SchemaType::S32 {
            restrictions: Some(NumericRestrictions {
                min: Some(NumericBound::Signed(1)),
                max: Some(NumericBound::Signed(10)),
                unit: None,
            }),
            metadata: golem_common::schema::MetadataEnvelope::default(),
        };
        tool.commands.nodes[1]
            .body
            .as_mut()
            .unwrap()
            .positionals
            .fixed
            .push(positional("level", level));
        let dir = tempfile::TempDir::new().unwrap().keep();
        let target = Utf8PathBuf::from_path_buf(dir.join("out")).unwrap();
        KotlinToolBridgeGenerator::new_with_rpc_tool_name(tool, "grep".to_string(), &target, true)
            .unwrap()
            .generate()
            .unwrap();
        let source = std::fs::read_to_string(
            target.join("src/wasmWasiMain/kotlin/golem/bridge/client/grep_tool/GrepClient.kt"),
        )
        .unwrap();
        assert!(
            source.contains("\"level\" to \"s32~min=1;max=10\""),
            "{source}"
        );
    }

    /// A tool argument's `path` carries its direction and kind through the hex-encoded lowered
    /// grammar `sdk_wit.rs` renders, so generation succeeds and the exact spec appears in the
    /// call's input schema string. (`flags` and `fixed-list` were refused here too until the
    /// grammar learned to carry their names and length.)
    #[test]
    fn a_path_argument_carries_its_direction_and_kind_in_the_call_input_schema() {
        use golem_common::schema::schema_type::{PathDirection, PathKind, PathSpec};
        let mut tool = grep_tool();
        tool.commands.nodes[1]
            .body
            .as_mut()
            .unwrap()
            .positionals
            .fixed
            .push(positional(
                "arg",
                SchemaType::path(PathSpec {
                    direction: PathDirection::Input,
                    kind: PathKind::File,
                    allowed_mime_types: None,
                    allowed_extensions: None,
                }),
            ));
        let dir = tempfile::TempDir::new().unwrap().keep();
        let target = Utf8PathBuf::from_path_buf(dir.join("out")).unwrap();
        KotlinToolBridgeGenerator::new_with_rpc_tool_name(tool, "grep".to_string(), &target, true)
            .unwrap()
            .generate()
            .unwrap();
        let source = std::fs::read_to_string(
            target.join("src/wasmWasiMain/kotlin/golem/bridge/client/grep_tool/GrepClient.kt"),
        )
        .unwrap();
        assert!(
            source.contains("\"arg\" to \"path<input,file,_,_>\""),
            "{source}"
        );
    }

    /// The same for a `url`'s schemes and hosts, which the lowered grammar refused to carry until
    /// it gained `url-restrictions`: a generated client that dropped them would send an input
    /// schema the host compares with the command's and rejects.
    #[test]
    fn a_url_argument_carries_its_schemes_in_the_call_input_schema() {
        use golem_common::schema::schema_type::UrlRestrictions;
        let mut tool = grep_tool();
        tool.commands.nodes[1]
            .body
            .as_mut()
            .unwrap()
            .positionals
            .fixed
            .push(positional(
                "arg",
                SchemaType::url(UrlRestrictions {
                    allowed_schemes: Some(vec!["https".into()]),
                    allowed_hosts: None,
                }),
            ));
        let dir = tempfile::TempDir::new().unwrap().keep();
        let target = Utf8PathBuf::from_path_buf(dir.join("out")).unwrap();
        KotlinToolBridgeGenerator::new_with_rpc_tool_name(tool, "grep".to_string(), &target, true)
            .unwrap()
            .generate()
            .unwrap();
        let source = std::fs::read_to_string(
            target.join("src/wasmWasiMain/kotlin/golem/bridge/client/grep_tool/GrepClient.kt"),
        )
        .unwrap();
        assert!(
            source.contains("\"arg\" to \"url<<6874747073>,_>\""),
            "{source}"
        );
    }

    #[test]
    fn a_command_call_lists_globals_then_positionals_tail_options_and_flags() {
        let (_, source) = generated("grep");
        assert!(
            source.contains("fun grep(color: golem.bridge.client.grep_tool.ColorMode?, caseSensitive: kotlin.Boolean, pattern: kotlin.String, files: kotlin.collections.List<kotlin.String>, maxCount: kotlin.UInt?, verbosity: kotlin.UInt, stdin: kotlin.ByteArray? = null): cloud.golem.runtime.Either<cloud.golem.runtime.ToolInvokeError<golem.bridge.client.grep_tool.GrepError>, kotlin.collections.List<kotlin.String>> ="),
            "{source}"
        );
        assert!(
            source.contains("golemRpc.golemCallTyped(kotlin.collections.listOf<kotlin.String>(), golemToolInput(golemInherited, kotlin.collections.listOf(\"color\" to \"option<enum<never,always,auto>>\", \"case-sensitive\" to \"bool\", \"pattern\" to \"string\", \"files\" to \"list<string>\", \"max-count\" to \"option<u32>\", \"verbosity\" to \"u32\"),"),
            "{source}"
        );
        assert!(
            source.contains("stdin, golem.bridge.client.grep_tool.GrepError::decode"),
            "{source}"
        );
    }

    #[test]
    fn a_command_with_outputs_captures_only_the_streams_it_declares() {
        let (_, source) = generated("grep");
        assert!(
            source.contains("cloud.golem.runtime.WithOutputs<kotlin.Unit>>"),
            "{source}"
        );
        assert!(
            source.contains(
                "golemCallTypedWithOutputs(kotlin.collections.listOf<kotlin.String>(\"replace\")"
            ),
            "{source}"
        );
        assert!(
            source.contains("null, true, false, { _, _ -> null }"),
            "{source}"
        );
    }

    #[test]
    fn declared_errors_decode_by_name_and_malformed_payloads_decode_to_null() {
        let (_, source) = generated("grep");
        assert!(
            source.contains("data class BadPattern(val value: kotlin.String) : golem.bridge.client.grep_tool.GrepError()"),
            "{source}"
        );
        assert!(
            source.contains("data object Io : golem.bridge.client.grep_tool.GrepError()"),
            "{source}"
        );
        assert!(
            source.contains(
                "\"bad-pattern\" -> try { golem.bridge.client.grep_tool.GrepError.BadPattern("
            ),
            "{source}"
        );
        assert!(
            source.contains("\"io\" -> golem.bridge.client.grep_tool.GrepError.Io"),
            "{source}"
        );
        assert!(
            source.contains("catch (e: kotlin.RuntimeException) { null }"),
            "{source}"
        );
    }

    #[test]
    fn the_root_client_closes_the_rpc_handle() {
        let (_, source) = generated("grep");
        assert!(source.contains("class GrepClient internal constructor(private val golemRpc: cloud.golem.runtime.ToolRpc, private val golemInherited: kotlin.collections.List<cloud.golem.runtime.ToolInputField>) : kotlin.AutoCloseable {"), "{source}");
        assert!(
            source.contains("override fun close() = golemRpc.close()"),
            "{source}"
        );
    }

    /// Writes the generated tool project below `GOLEM_KOTLIN_BRIDGE_DUMP_DIR`
    /// for manual builds. It does nothing unless the variable is set.
    #[test]
    fn dump_generated_tool_project_for_manual_builds() {
        let Some(dir) = std::env::var_os("GOLEM_KOTLIN_BRIDGE_DUMP_DIR") else {
            return;
        };
        let target =
            Utf8PathBuf::from_path_buf(std::path::PathBuf::from(dir).join("tool")).unwrap();
        KotlinToolBridgeGenerator::new(grep_tool(), &target, true)
            .unwrap()
            .generate()
            .unwrap();
    }
}
