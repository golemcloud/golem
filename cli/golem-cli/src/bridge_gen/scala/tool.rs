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

use super::scala::{
    is_scala_keyword, to_scala_term_ident, to_scala_type_ident, unique_idents_with_reserved,
};
use super::scala_writer::ScalaWriter;
use super::{
    CLIENT_PKG_BASE, GUEST_CODEC, GUEST_SCHEMA_VALUE_TYPE, LIST, SCALA_SOURCE_ROOT,
    ScalaBridgeGenerator, scala_string_literal, stringify_precision_sensitive_numbers,
};
use crate::bridge_gen::tool_bridge_client_directory_name;
use crate::bridge_gen::tool_common::{
    command_path, field_names, global_surfaces, idx_to_usize, synthetic_agent_type,
};
use crate::fs;
use crate::sdk_overrides::workspace_root;
use anyhow::{Context, anyhow};
use camino::{Utf8Path, Utf8PathBuf};
use golem_common::schema::graph::{SchemaGraph, reachable_defs};
use golem_common::schema::tool::canonical::{
    CanonicalInputField, CanonicalSurfaceRef, record_schema_from_field_graphs,
};
use golem_common::schema::tool::{CommandBody, CommandNode, Tool};
use heck::{ToSnakeCase, ToUpperCamelCase};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
struct ClientNode {
    index: usize,
    struct_name: String,
    provided: Vec<CanonicalSurfaceRef>,
}

type CollectedNames = (
    BTreeMap<usize, ClientNode>,
    BTreeMap<usize, String>,
    Vec<String>,
);

/// Generates a Scala.js guest TOOL client project.
pub struct ScalaToolBridgeGenerator {
    tool: Tool,
    tool_name: String,
    target_path: Utf8PathBuf,
    testing: bool,
    inner: ScalaBridgeGenerator,
    clients: BTreeMap<usize, ClientNode>,
    error_names: BTreeMap<usize, String>,
}

impl ScalaToolBridgeGenerator {
    pub fn new(tool: Tool, target_path: &Utf8Path, testing: bool) -> anyhow::Result<Self> {
        let tool_name = tool
            .commands
            .nodes
            .first()
            .map(|node| node.name.clone())
            .ok_or_else(|| anyhow!("tool command tree must contain a root command"))?;
        let (clients, error_names, reserved) = collect_names(&tool, &tool_name)?;
        let synthetic = synthetic_agent_type(&tool, &tool_name)?;
        let inner = ScalaBridgeGenerator::new_guest_with_extra_reserved_names(
            synthetic,
            target_path,
            testing,
            reserved,
        )?;
        Ok(Self {
            tool,
            tool_name,
            target_path: target_path.to_path_buf(),
            testing,
            inner,
            clients,
            error_names,
        })
    }

    pub fn generate(&mut self) -> anyhow::Result<()> {
        if !self.target_path.exists() {
            fs::create_dir_all(&self.target_path)?;
        }
        self.inner.write_build_files()?;
        self.write_tool_project_name()?;
        self.write_client()?;
        Ok(())
    }

    fn write_tool_project_name(&self) -> anyhow::Result<()> {
        let build_sbt_path = self.target_path.join("build.sbt");
        let build_sbt = fs::read_to_string(&build_sbt_path)?;
        let tool_project_name = tool_bridge_client_directory_name(&self.tool_name);
        let rewritten = build_sbt
            .lines()
            .map(|line| {
                if line.trim_start().starts_with("name") && line.contains(":=") {
                    format!(
                        "                    name                         := {tool_project_name:?},"
                    )
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let rewritten = if self.testing {
            let sdk_root = workspace_root()?.join("sdks/scala");
            rewritten.replace(
                "lazy val root = (project in file(\".\"))\n  .enablePlugins(ScalaJSPlugin)",
                &format!(
                    "lazy val golemScalaCore = ProjectRef(file({}), \"core\")\n\nlazy val root = (project in file(\".\"))\n  .dependsOn(golemScalaCore)\n  .enablePlugins(ScalaJSPlugin)",
                    scala_string_literal(&sdk_root.to_string_lossy())
                ),
            )
        } else {
            rewritten
        };
        fs::write_str(build_sbt_path, format!("{rewritten}\n"))?;
        Ok(())
    }

    fn write_client(&mut self) -> anyhow::Result<()> {
        let content = self.generate_client_source()?;
        let client_path = self
            .target_path
            .join(SCALA_SOURCE_ROOT)
            .join("golem")
            .join("bridge")
            .join("client")
            .join(self.client_package_segment())
            .join(format!("{}.scala", self.root_client_name()?));
        fs::write_str(client_path, content)?;
        Ok(())
    }

    fn generate_client_source(&mut self) -> anyhow::Result<String> {
        let mut writer = ScalaWriter::new();
        writer.line(format!("package {}", self.client_package()));
        writer.blank();
        writer.line("// Generated by golem-cli. Do not edit.");
        writer.blank();

        self.client_items(&mut writer)?;
        self.error_items(&mut writer)?;
        self.inner.write_type_definitions(&mut writer)?;
        self.inner.write_guest_unstructured_definitions(&mut writer);
        self.inner.write_multimodal_definitions(&mut writer)?;
        self.inner.write_codecs(&mut writer)?;

        Ok(self.inner.rewrite_guest_runtime_refs(writer.finish()))
    }

    fn client_items(&mut self, writer: &mut ScalaWriter) -> anyhow::Result<()> {
        let clients = self.clients.values().cloned().collect::<Vec<_>>();
        for client in clients {
            let node = self.node(client.index)?;
            write_doc(writer, &node.doc.summary, &node.doc.description);
            writer.line(format!(
                "final class {}(private val rpc: _root_.golem.tool.ToolRpcTransport, private val inherited: {LIST}[{GUEST_SCHEMA_VALUE_TYPE}]) {{",
                client.struct_name
            ));
            writer.indent();
            self.client_methods(writer, &client)?;
            self.write_decode_result_value_helper(writer);
            writer.dedent();
            writer.line("}");
            writer.blank();

            if client.index == 0 {
                writer.line(format!("object {} {{", client.struct_name));
                writer.indent();
                writer.line(format!(
                    "def apply(): {} = new {}(_root_.golem.runtime.tool.client.ToolRpcClient.transport({}), {LIST}())",
                    client.struct_name,
                    client.struct_name,
                    scala_string_literal(&self.tool_name)
                ));
                writer.line(format!("def newClient(): {} = apply()", client.struct_name));
                writer.dedent();
                writer.line("}");
                writer.blank();
            }
        }
        Ok(())
    }

    fn client_methods(
        &mut self,
        writer: &mut ScalaWriter,
        client: &ClientNode,
    ) -> anyhow::Result<()> {
        let mut reserved = vec![
            "apply".to_string(),
            "rpc".to_string(),
            "inherited".to_string(),
            "decodeResultValue".to_string(),
            "toString".to_string(),
            "hashCode".to_string(),
            "equals".to_string(),
            "getClass".to_string(),
            "isInstanceOf".to_string(),
            "asInstanceOf".to_string(),
            "notify".to_string(),
            "notifyAll".to_string(),
            "wait".to_string(),
            "clone".to_string(),
            "finalize".to_string(),
            "synchronized".to_string(),
            "##".to_string(),
            "==".to_string(),
            "!=".to_string(),
            "eq".to_string(),
            "ne".to_string(),
        ];
        let mut planned_names = Vec::new();
        let mut planned = Vec::new();
        if self.node(client.index)?.body.is_some() {
            let name = if client.index == 0 {
                self.tool_name.clone()
            } else {
                self.node(client.index)?.name.clone()
            };
            planned_names.push(to_scala_term_ident(&name, false));
            planned.push((client.index, false));
        }
        for child_index in self.child_indices(client.index)? {
            let child = self.node(child_index)?;
            let has_subcommands = !child.subcommands.is_empty();
            let has_body = child.body.is_some();
            if has_subcommands || has_body {
                planned_names.push(to_scala_term_ident(&child.name, false));
                planned.push((child_index, has_subcommands));
            }
        }
        let reserved_refs = reserved.iter().map(String::as_str).collect::<Vec<_>>();
        let method_names = unique_idents_with_reserved(planned_names, &reserved_refs);
        reserved.extend(method_names.clone());

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

    fn write_decode_result_value_helper(&self, writer: &mut ScalaWriter) {
        writer.line("private def decodeResultValue[T](__value: => T): _root_.scala.Either[_root_.golem.tool.ToolError[Nothing], T] =");
        writer.indent();
        writer.line("try _root_.scala.Right(__value)");
        writer.line("catch { case __e: _root_.scala.Throwable => _root_.scala.Left(_root_.golem.tool.ToolClientRuntime.protocolError(__e.getMessage)) }");
        writer.dedent();
    }

    fn accessor_method(
        &mut self,
        writer: &mut ScalaWriter,
        child_index: usize,
        parent: &ClientNode,
        method_name: &str,
    ) -> anyhow::Result<()> {
        let child_client = self
            .clients
            .get(&child_index)
            .cloned()
            .context("missing child client")?;
        let child = self.node(child_index)?;
        write_doc(writer, &child.doc.summary, &child.doc.description);
        let fields = global_surfaces(&self.tool, child_index)
            .into_iter()
            .filter(|surface| !parent.provided.contains(surface))
            .map(|surface| self.field(child_index, surface))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let param_names = self.param_names(&fields, &["stdin"]);
        let params = self.param_decls(&fields, &param_names)?;
        writer.line(format!(
            "def {method_name}({}): {} = {{",
            params.join(", "),
            child_client.struct_name
        ));
        writer.indent();
        writer.line(format!("val __inherited = inherited ++ {LIST}("));
        writer.indent();
        for (idx, field) in fields.iter().enumerate() {
            let comma = if idx + 1 < fields.len() { "," } else { "" };
            let enc = self.inner.encode_expr(&param_names[idx], &field.type_, 0)?;
            writer.line(format!("{enc}{comma}"));
        }
        writer.dedent();
        writer.line(")");
        writer.line(format!(
            "new {}(rpc, __inherited)",
            child_client.struct_name
        ));
        writer.dedent();
        writer.line("}");
        Ok(())
    }

    fn leaf_method(
        &mut self,
        writer: &mut ScalaWriter,
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
        let (provided_fields, remaining_fields) =
            self.leaf_fields(command_index, &client.provided)?;
        let mut all_fields = provided_fields.clone();
        all_fields.extend(remaining_fields.clone());
        let schema_json = self.record_schema_json(&all_fields)?;
        let param_names = self.param_names(&remaining_fields, &["stdin"]);
        let mut params = self.param_decls(&remaining_fields, &param_names)?;
        let stdin_expr = match &body.stdin {
            Some(stdin) if stdin.required => {
                params.push("stdin: _root_.golem.tool.ToolInputStream".to_string());
                "_root_.scala.Some(stdin)".to_string()
            }
            Some(_) => {
                params.push(
                    "stdin: _root_.scala.Option[_root_.golem.tool.ToolInputStream]".to_string(),
                );
                "stdin".to_string()
            }
            None => "_root_.scala.None".to_string(),
        };
        let ret_ty = self.return_type(body)?;
        let path_expr = self.command_path_expr(command_index)?;

        writer.line(format!(
            "def {method_name}({}): _root_.scala.concurrent.Future[_root_.scala.Either[_root_.golem.tool.ToolError[{}], {ret_ty}]] = {{",
            params.join(", "),
            self.error_type(command_index, body)
        ));
        writer.indent();
        writer.line("val __input: _root_.scala.Either[_root_.golem.tool.ToolError[Nothing], _root_.golem.schema.TypedSchemaValue] =");
        writer.indent();
        writer.line("try {");
        writer.indent();
        writer.line(format!(
            "val __schema = {GUEST_CODEC}.schemaGraphFromJson({})",
            scala_string_literal(&schema_json)
        ));
        writer.line(format!("val __fields = inherited ++ {LIST}("));
        writer.indent();
        for (idx, field) in remaining_fields.iter().enumerate() {
            let comma = if idx + 1 < remaining_fields.len() {
                ","
            } else {
                ""
            };
            let enc = self.inner.encode_expr(&param_names[idx], &field.type_, 0)?;
            writer.line(format!("{enc}{comma}"));
        }
        writer.dedent();
        writer.line(")");
        writer.line("_root_.scala.Right(_root_.golem.schema.TypedSchemaValue(__schema, _root_.golem.schema.SchemaValue.RecordValue(__fields)))");
        writer.dedent();
        writer.line("} catch {");
        writer.indent();
        writer.line("case __e: _root_.scala.Throwable => _root_.scala.Left(_root_.golem.tool.ToolClientRuntime.protocolError(s\"failed to encode tool input: ${__e.getMessage}\"))");
        writer.dedent();
        writer.line("}");
        writer.dedent();

        let call = self.invoke_call(command_index, body, &path_expr, &stdin_expr)?;
        writer.line(format!("val __call = {call}"));
        writer.line("_root_.golem.tool.ToolClientRuntime.complete(__call) { __result =>");
        writer.indent();
        self.result_decode(writer, body)?;
        writer.dedent();
        writer.line("}");
        writer.dedent();
        writer.line("}");
        Ok(())
    }

    fn invoke_call(
        &mut self,
        command_index: usize,
        body: &CommandBody,
        path_expr: &str,
        stdin_expr: &str,
    ) -> anyhow::Result<String> {
        if body.errors.is_empty() {
            Ok(format!(
                "__input match {{ case _root_.scala.Left(__e) => _root_.scala.concurrent.Future.successful(_root_.scala.Left(__e)); case _root_.scala.Right(__record) => _root_.golem.tool.ToolClientRuntime.invokeAndAwaitInfallible(rpc, {path_expr}, __record, {stdin_expr}) }}"
            ))
        } else {
            let error_type = self.error_type(command_index, body);
            Ok(format!(
                "__input match {{ case _root_.scala.Left(__e) => _root_.scala.concurrent.Future.successful(_root_.scala.Left(__e)); case _root_.scala.Right(__record) => _root_.golem.tool.ToolClientRuntime.invokeAndAwait[{error_type}](rpc, {path_expr}, __record, {stdin_expr}, {}.decodeError) }}",
                self.error_names
                    .get(&command_index)
                    .context("missing error enum")?
            ))
        }
    }

    fn result_decode(
        &mut self,
        writer: &mut ScalaWriter,
        body: &CommandBody,
    ) -> anyhow::Result<()> {
        match (&body.result, &body.stdout) {
            (Some(result), Some(stdout)) if stdout.required => {
                let dec = self.inner.decode_expr("__typed.value", &result.type_, 0)?;
                writer.line("for {");
                writer.indent();
                writer.line("__stdout <- __result.stdout.toRight(_root_.golem.tool.ToolClientRuntime.protocolError(\"tool result did not contain declared stdout stream\"))");
                writer.line("__typed <- __result.result.toRight(_root_.golem.tool.ToolClientRuntime.protocolError(\"tool result did not contain a value\"))");
                writer.line(format!("__decoded <- decodeResultValue({dec})"));
                writer.dedent();
                writer.line("} yield (__decoded, __stdout)");
            }
            (Some(result), Some(_)) => {
                let dec = self.inner.decode_expr("__typed.value", &result.type_, 0)?;
                writer.line("for {");
                writer.indent();
                writer.line("__typed <- __result.result.toRight(_root_.golem.tool.ToolClientRuntime.protocolError(\"tool result did not contain a value\"))");
                writer.line(format!("__decoded <- decodeResultValue({dec})"));
                writer.dedent();
                writer.line("} yield (__decoded, __result.stdout)");
            }
            (Some(result), None) => {
                let dec = self.inner.decode_expr("__typed.value", &result.type_, 0)?;
                writer.line("for {");
                writer.indent();
                writer.line("_ <- if (__result.stdout.isDefined) _root_.scala.Left(_root_.golem.tool.ToolClientRuntime.protocolError(\"tool result unexpectedly contained stdout stream\")) else _root_.scala.Right(())");
                writer.line("__typed <- __result.result.toRight(_root_.golem.tool.ToolClientRuntime.protocolError(\"tool result did not contain a value\"))");
                writer.line(format!("__decoded <- decodeResultValue({dec})"));
                writer.dedent();
                writer.line("} yield __decoded");
            }
            (None, Some(stdout)) if stdout.required => {
                writer.line("for {");
                writer.indent();
                writer.line("__stdout <- __result.stdout.toRight(_root_.golem.tool.ToolClientRuntime.protocolError(\"tool result did not contain declared stdout stream\"))");
                writer.line("_ <- if (__result.result.isDefined) _root_.scala.Left(_root_.golem.tool.ToolClientRuntime.protocolError(\"tool result unexpectedly contained a value\")) else _root_.scala.Right(())");
                writer.dedent();
                writer.line("} yield __stdout");
            }
            (None, Some(_)) => {
                writer.line("for {");
                writer.indent();
                writer.line("_ <- if (__result.result.isDefined) _root_.scala.Left(_root_.golem.tool.ToolClientRuntime.protocolError(\"tool result unexpectedly contained a value\")) else _root_.scala.Right(())");
                writer.dedent();
                writer.line("} yield __result.stdout");
            }
            (None, None) => {
                writer.line("for {");
                writer.indent();
                writer.line("_ <- if (__result.stdout.isDefined) _root_.scala.Left(_root_.golem.tool.ToolClientRuntime.protocolError(\"tool result unexpectedly contained stdout stream\")) else _root_.scala.Right(())");
                writer.line("_ <- if (__result.result.isDefined) _root_.scala.Left(_root_.golem.tool.ToolClientRuntime.protocolError(\"tool result unexpectedly contained a value\")) else _root_.scala.Right(())");
                writer.dedent();
                writer.line("} yield ()");
            }
        }
        Ok(())
    }

    fn error_items(&mut self, writer: &mut ScalaWriter) -> anyhow::Result<()> {
        for (command_index, error_name) in self.error_names.clone() {
            let node = self.node(command_index)?.clone();
            let body = node
                .body
                .as_ref()
                .context("error enum command has no body")?;
            writer.line(format!(
                "sealed trait {error_name} extends Product with Serializable"
            ));
            writer.line(format!("object {error_name} {{"));
            writer.indent();
            let variants = error_variant_names(body);
            for (case, variant) in body.errors.iter().zip(&variants) {
                if let Some(payload) = &case.payload {
                    let typ = self.inner.type_reference(payload)?;
                    writer.line(format!(
                        "final case class {variant}(value: {typ}) extends {error_name}"
                    ));
                } else {
                    writer.line(format!("case object {variant} extends {error_name}"));
                }
            }
            writer.blank();
            self.error_decoder(writer, &error_name, body, &variants)?;
            writer.dedent();
            writer.line("}");
            writer.blank();
        }
        Ok(())
    }

    fn error_decoder(
        &mut self,
        writer: &mut ScalaWriter,
        error_name: &str,
        body: &CommandBody,
        variants: &[String],
    ) -> anyhow::Result<()> {
        writer.line(format!(
            "def decodeError(__value: _root_.golem.schema.TypedSchemaValue): _root_.scala.Either[_root_.scala.Predef.String, {error_name}] = {{"
        ));
        writer.indent();
        for (case, variant) in body.errors.iter().zip(variants) {
            if let Some(payload) = &case.payload {
                let dec = self.inner.decode_expr("__value.value", payload, 0)?;
                writer.line("try {");
                writer.indent();
                writer.line(format!(
                    "return _root_.scala.Right({error_name}.{variant}({dec}))"
                ));
                writer.dedent();
                writer.line(format!(
                    "}} catch {{ case _: _root_.scala.Throwable => () }}"
                ));
            } else {
                writer.line("__value.value match {");
                writer.indent();
                writer.line(format!("case _root_.golem.schema.SchemaValue.TupleValue(values) if values.isEmpty => return _root_.scala.Right({error_name}.{variant})"));
                writer.line("case _ => ()");
                writer.dedent();
                writer.line("}");
            }
        }
        writer.line("_root_.scala.Left(\"remote tool error payload did not match any declared error case\")");
        writer.dedent();
        writer.line("}");
        Ok(())
    }

    fn return_type(&mut self, body: &CommandBody) -> anyhow::Result<String> {
        let stdout = "_root_.golem.tool.ToolOutputStream".to_string();
        let stdout_type = |required: bool| {
            if required {
                stdout.clone()
            } else {
                format!("_root_.scala.Option[{stdout}]")
            }
        };
        match (&body.result, &body.stdout) {
            (Some(result), Some(stdout)) => Ok(format!(
                "({}, {stdout})",
                self.inner.type_reference(&result.type_)?,
                stdout = stdout_type(stdout.required)
            )),
            (Some(result), None) => self.inner.type_reference(&result.type_),
            (None, Some(stdout)) => Ok(stdout_type(stdout.required)),
            (None, None) => Ok("_root_.scala.Unit".to_string()),
        }
    }

    fn error_type(&self, command_index: usize, body: &CommandBody) -> String {
        if body.errors.is_empty() {
            "Nothing".to_string()
        } else {
            self.error_names
                .get(&command_index)
                .expect("error enum name")
                .clone()
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

    fn record_schema_json(&self, fields: &[CanonicalInputField]) -> anyhow::Result<String> {
        let field_graphs = fields
            .iter()
            .map(|field| FieldGraph {
                name: field.name.clone(),
                graph: SchemaGraph {
                    defs: reachable_defs(&self.tool.schema, &field.type_),
                    root: field.type_.clone(),
                },
            })
            .collect::<Vec<_>>();
        let schema = record_schema_from_field_graphs(
            field_graphs.iter().map(|f| (f.name.as_str(), &f.graph)),
        )
        .map_err(|e| anyhow!("failed to build tool input record schema: {e}"))?;
        let mut value = serde_json::to_value(&schema)
            .context("failed to serialize embedded tool input schema")?;
        stringify_precision_sensitive_numbers(&mut value);
        serde_json::to_string(&value).context("failed to serialize embedded tool input schema")
    }

    fn param_names(&self, fields: &[CanonicalInputField], extra_reserved: &[&str]) -> Vec<String> {
        let mut reserved = vec![
            "__input",
            "__schema",
            "__fields",
            "__call",
            "__result",
            "__typed",
            "__decoded",
            "__stdout",
            "rpc",
            "inherited",
            "decodeResultValue",
            "e0",
            "elems0",
            "k0",
            "v0",
            "r0",
            "l0",
            "p",
            "t0",
        ];
        let tuple_elems = (0..22).map(|i| format!("te{i}")).collect::<Vec<_>>();
        reserved.extend(tuple_elems.iter().map(String::as_str));
        reserved.extend(extra_reserved);
        unique_idents_with_reserved(
            fields
                .iter()
                .map(|field| to_scala_term_ident(&field.name, false))
                .collect(),
            &reserved,
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

    fn command_path_expr(&self, command_index: usize) -> anyhow::Result<String> {
        let segments = command_path(&self.tool, command_index)?
            .into_iter()
            .skip(1)
            .map(|segment| scala_string_literal(&segment))
            .collect::<Vec<_>>();
        Ok(format!("{LIST}({})", segments.join(", ")))
    }

    fn client_package_segment(&self) -> String {
        let mut seg: String = self
            .tool_name
            .as_str()
            .to_snake_case()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        if seg.is_empty() {
            seg = "agent".to_string();
        }
        if !seg.starts_with(|c: char| c.is_ascii_alphabetic()) {
            seg = format!("agent_{seg}");
        }
        if is_scala_keyword(&seg) {
            seg = format!("{seg}_");
        }
        seg
    }

    fn client_package(&self) -> String {
        format!("{CLIENT_PKG_BASE}.{}", self.client_package_segment())
    }

    fn root_client_name(&self) -> anyhow::Result<String> {
        self.clients
            .get(&0)
            .map(|client| client.struct_name.clone())
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

struct FieldGraph {
    name: String,
    graph: SchemaGraph,
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
        let struct_name = unique_name(client_struct_name(tool_name, &path), used);
        reserved.insert(struct_name.clone());
        clients.insert(
            index,
            ClientNode {
                index,
                struct_name,
                provided: provided.clone(),
            },
        );
    }
    if node
        .body
        .as_ref()
        .is_some_and(|body| !body.errors.is_empty())
    {
        let error_name = unique_name(error_enum_name(tool_name, &path), used);
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

fn client_struct_name(tool_name: &str, path: &[String]) -> String {
    let mut name = tool_name.to_upper_camel_case();
    for segment in path {
        name.push_str(&segment.to_upper_camel_case());
    }
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        name = format!("Tool{name}");
    }
    name.push_str("Client");
    to_scala_type_ident(&name, false)
}

fn error_enum_name(tool_name: &str, path: &[String]) -> String {
    let mut name = tool_name.to_upper_camel_case();
    for segment in path {
        name.push_str(&segment.to_upper_camel_case());
    }
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
        name = format!("Tool{name}");
    }
    name.push_str("Error");
    to_scala_type_ident(&name, false)
}

fn error_variant_names(body: &CommandBody) -> Vec<String> {
    unique_idents_with_reserved(
        body.errors
            .iter()
            .map(|case| to_scala_type_ident(&case.name.to_upper_camel_case(), false))
            .collect(),
        &["Product", "Serializable"],
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

fn write_doc(writer: &mut ScalaWriter, summary: &str, description: &str) {
    let doc = if summary.is_empty() {
        description.to_string()
    } else if description.is_empty() {
        summary.to_string()
    } else {
        format!("{summary}\n\n{description}")
    };
    writer.doc(&doc);
}

#[allow(dead_code)]
fn _tool_library_name(tool_name: &str) -> String {
    tool_bridge_client_directory_name(tool_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::graph::SchemaTypeDef;
    use golem_common::schema::metadata::TypeId;
    use golem_common::schema::schema_type::SchemaType;
    use golem_common::schema::tool::validation::validate_tool;
    use golem_common::schema::tool::{
        BoolFlagShape, CommandBody, CommandIndex, CommandTree, Doc, ErrorCase, ErrorKind,
        FlagShape, FlagSpec, Formatter, Globals, OptionShape, OptionSpec, Positional, Positionals,
        ResultSpec, StreamSpec, TailPositional,
    };
    use golem_common::schema::{MetadataEnvelope, NamedFieldType};
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

    fn generate(tool: Tool, dir_name: &str) -> Utf8PathBuf {
        let dir = tempfile::TempDir::new().unwrap().keep();
        let target_path = Utf8PathBuf::from_path_buf(dir.join(dir_name)).unwrap();
        let mut generator = ScalaToolBridgeGenerator::new(tool, &target_path, true).unwrap();
        generator.generate().unwrap();
        target_path
    }

    #[test]
    fn tool_generation_escapes_keyword_package_segment() {
        let mut tool = grep_tool();
        tool.commands.nodes[0].name = "type".to_string();

        let target_path = generate(tool, "type-tool-guest-client");

        assert!(
            target_path
                .join("src/main/scala/golem/bridge/client/type_/TypeClient.scala")
                .exists(),
            "Scala tool client package segment must not be a Scala keyword"
        );
    }

    #[test]
    fn tool_generation_prefixes_numeric_client_and_error_names() {
        let mut tool = grep_tool();
        tool.commands.nodes[0].name = "123".to_string();

        let target_path = generate(tool, "123-tool-guest-client");
        let source = std::fs::read_to_string(
            target_path.join("src/main/scala/golem/bridge/client/agent_123/Tool123Client.scala"),
        )
        .unwrap();

        assert!(
            source.contains("final class Tool123Client"),
            "missing prefixed client name:\n{source}"
        );
        assert!(
            source.contains("sealed trait Tool123Error"),
            "missing prefixed error name:\n{source}"
        );
    }

    #[test]
    fn tool_generation_avoids_encoder_temporary_parameter_names() {
        let mut tool = grep_tool();
        let body = tool.commands.nodes[0].body.as_mut().unwrap();
        body.positionals.fixed[0].name = "elems0".to_string();
        body.positionals.fixed[0].type_ = SchemaType::fixed_list(SchemaType::string(), 1);

        let target_path = generate(tool, "grep-tool-guest-client");
        let source = std::fs::read_to_string(
            target_path.join("src/main/scala/golem/bridge/client/grep/GrepClient.scala"),
        )
        .unwrap();

        assert!(
            !source.contains("val elems0 = elems0.map"),
            "Scala tool client parameter must not shadow fixed-list encoder temporary:\n{source}"
        );
    }

    #[test]
    fn tool_generation_emits_guest_client_runtime_boundary() {
        let target_path = generate(grep_tool(), "grep-tool-guest-client");
        let source = std::fs::read_to_string(
            target_path.join("src/main/scala/golem/bridge/client/grep/GrepClient.scala"),
        )
        .unwrap();

        for shape in [
            "final class GrepClient",
            "def grep(",
            "def replace(",
            "sealed trait GrepError",
            "_root_.golem.runtime.tool.client.ToolRpcClient.transport(\"grep\")",
            "_root_.golem.schema.TypedSchemaValue(__schema, _root_.golem.schema.SchemaValue.RecordValue(__fields))",
            "_root_.golem.tool.ToolClientRuntime.invokeAndAwait[GrepError]",
            "_root_.golem.tool.ToolClientRuntime.invokeAndAwaitInfallible",
            "object Codecs",
        ] {
            assert!(source.contains(shape), "missing {shape}:\n{source}");
        }

        assert!(
            !source.contains("decodeValueResult"),
            "tool generator must manually decode named values instead of requiring FromSchema instances:\n{source}"
        );
    }

    #[test]
    fn tool_generation_uses_tool_guest_client_project_name() {
        let target_path = generate(grep_tool(), "grep-tool-guest-client");
        let build_sbt = std::fs::read_to_string(target_path.join("build.sbt")).unwrap();

        assert!(
            build_sbt.contains("\"grep-tool-guest-client\""),
            "Scala tool guest bridge project name must match the tool bridge SDK directory name:\n{build_sbt}"
        );
    }

    #[test]
    fn tool_generation_qualifies_scala_list_when_user_type_is_named_list() {
        let mut tool = grep_tool();
        let list_type = TypeId::from("list");
        tool.schema.defs.push(SchemaTypeDef {
            id: list_type.clone(),
            name: Some("list".to_string()),
            body: SchemaType::record(vec![NamedFieldType {
                name: "value".to_string(),
                body: SchemaType::string(),
                metadata: MetadataEnvelope::default(),
            }]),
        });
        let body = tool.commands.nodes[0].body.as_mut().unwrap();
        body.positionals.fixed[0].type_ = SchemaType::ref_to(list_type);

        let target_path = generate(tool, "grep-tool-guest-client");
        let source = std::fs::read_to_string(
            target_path.join("src/main/scala/golem/bridge/client/grep/GrepClient.scala"),
        )
        .unwrap();

        assert!(
            source.contains("final case class List("),
            "missing generated List type:\n{source}"
        );
        assert!(
            !source.contains("inherited ++ List(") && !source.contains("RecordValue(List("),
            "Scala tool client must qualify Scala collection List constructors when a generated type is named List:\n{source}"
        );
    }

    #[test]
    fn tool_generation_avoids_zero_arg_methods_that_override_anyref_members() {
        let mut tool = grep_tool();
        tool.commands.nodes[0].name = "to-string".to_string();
        tool.commands.nodes[0].globals = Globals::default();
        tool.commands.nodes[0].subcommands = vec![];
        tool.commands.nodes[0].body = Some(body());
        tool.commands.nodes.truncate(1);
        tool.schema = SchemaGraph::empty();
        validate_tool(&tool).unwrap();

        let target_path = generate(tool, "to-string-tool-guest-client");
        let source = std::fs::read_to_string(
            target_path.join("src/main/scala/golem/bridge/client/to_string/ToStringClient.scala"),
        )
        .unwrap();

        assert!(
            !source.contains("def toString():"),
            "zero-argument Scala tool method must not override AnyRef.toString with an incompatible return type:\n{source}"
        );
    }

    #[test]
    fn tool_generation_avoids_decode_result_value_helper_collision() {
        let mut tool = grep_tool();
        tool.commands.nodes[0].name = "decode-result-value".to_string();
        tool.commands.nodes[0].globals = Globals::default();
        tool.commands.nodes[0].subcommands = vec![];
        tool.commands.nodes[0].body = Some(CommandBody {
            positionals: Positionals {
                fixed: vec![positional("value", SchemaType::string())],
                tail: None,
            },
            result: Some(ResultSpec {
                type_: SchemaType::string(),
                doc: doc("result"),
                formatters: vec![Formatter {
                    name: "json".to_string(),
                    doc: Doc::default(),
                }],
                default_formatter: "json".to_string(),
            }),
            ..body()
        });
        tool.commands.nodes.truncate(1);
        tool.schema = SchemaGraph::empty();
        validate_tool(&tool).unwrap();

        let target_path = generate(tool, "decode-result-value-tool-guest-client");
        let source = std::fs::read_to_string(target_path.join(
            "src/main/scala/golem/bridge/client/decode_result_value/DecodeResultValueClient.scala",
        ))
        .unwrap();

        assert!(
            !source.contains("def decodeResultValue(value: _root_.scala.Predef.String)"),
            "Scala tool client method must not overload the generated decodeResultValue helper used to decode results:\n{source}"
        );
    }

    #[test]
    fn tool_generation_avoids_decode_result_value_parameter_shadowing() {
        let mut tool = grep_tool();
        tool.commands.nodes[0].globals = Globals::default();
        tool.commands.nodes[0].subcommands = vec![];
        tool.commands.nodes[0].body = Some(CommandBody {
            positionals: Positionals {
                fixed: vec![positional("decode-result-value", SchemaType::string())],
                tail: None,
            },
            result: Some(ResultSpec {
                type_: SchemaType::string(),
                doc: doc("result"),
                formatters: vec![Formatter {
                    name: "json".to_string(),
                    doc: Doc::default(),
                }],
                default_formatter: "json".to_string(),
            }),
            ..body()
        });
        tool.commands.nodes.truncate(1);
        tool.schema = SchemaGraph::empty();
        validate_tool(&tool).unwrap();

        let target_path = generate(tool, "grep-tool-guest-client");
        let source = std::fs::read_to_string(
            target_path.join("src/main/scala/golem/bridge/client/grep/GrepClient.scala"),
        )
        .unwrap();

        assert!(
            !source.contains("def grep(decodeResultValue: _root_.scala.Predef.String)"),
            "Scala tool client parameter must not shadow the generated decodeResultValue helper used to decode results:\n{source}"
        );
    }

    #[test]
    fn tool_generation_result_decoding_cross_compiles() {
        let mut tool = grep_tool();
        tool.commands.nodes[0].globals = Globals::default();
        tool.commands.nodes[0].subcommands = vec![];
        tool.commands.nodes[0].body = Some(CommandBody {
            positionals: Positionals {
                fixed: vec![positional("decode-result-value", SchemaType::string())],
                tail: None,
            },
            result: Some(ResultSpec {
                type_: SchemaType::string(),
                doc: doc("result"),
                formatters: vec![Formatter {
                    name: "json".to_string(),
                    doc: Doc::default(),
                }],
                default_formatter: "json".to_string(),
            }),
            ..body()
        });
        tool.commands.nodes.truncate(1);
        tool.schema = SchemaGraph::empty();
        validate_tool(&tool).unwrap();

        let target_path = generate(tool, "grep-tool-guest-client");
        let status = std::process::Command::new("sbt")
            .arg("--batch")
            .arg("+compile")
            .current_dir(&target_path)
            .status()
            .expect("failed to run sbt; is it installed?");
        assert!(status.success(), "sbt +compile failed in {target_path}");
    }

    #[test]
    fn tool_generation_optional_stdout_cross_compiles_as_optional_result() {
        let mut tool = grep_tool();
        tool.commands.nodes[0].globals = Globals::default();
        tool.commands.nodes[0].subcommands = vec![];
        tool.commands.nodes[0].body = Some(CommandBody {
            stdout: Some(StreamSpec {
                doc: doc("optional stdout"),
                mime: vec![],
                required: false,
            }),
            ..body()
        });
        tool.commands.nodes.truncate(1);
        tool.schema = SchemaGraph::empty();
        validate_tool(&tool).unwrap();

        let target_path = generate(tool, "grep-tool-guest-client");
        let check_path = target_path
            .join("src/main/scala/golem/bridge/client/grep/OptionalStdoutCompileCheck.scala");
        std::fs::write(
            &check_path,
            r#"package golem.bridge.client.grep

object OptionalStdoutCompileCheck {
  private val rpc = new _root_.golem.tool.ToolRpcTransport {
    def invokeAndAwait(
      commandPath: _root_.scala.List[_root_.scala.Predef.String],
      input: _root_.golem.schema.TypedSchemaValue,
      stdin: _root_.scala.Option[_root_.golem.tool.ToolInputStream]
    ): _root_.scala.concurrent.Future[_root_.scala.Either[_root_.golem.tool.ToolRpcFailure, _root_.golem.tool.ToolInvokeResult]] =
      _root_.scala.concurrent.Future.successful(
        _root_.scala.Right(_root_.golem.tool.ToolInvokeResult(_root_.scala.None, _root_.scala.None))
      )
  }

  private val client = new GrepClient(rpc, _root_.scala.collection.immutable.List())
  val result: _root_.scala.concurrent.Future[
    _root_.scala.Either[
      _root_.golem.tool.ToolError[_root_.scala.Nothing],
      _root_.scala.Option[_root_.golem.tool.ToolOutputStream]
    ]
  ] = client.grep()
}
"#,
        )
        .unwrap();

        let status = std::process::Command::new("sbt")
            .arg("--batch")
            .arg("+compile")
            .current_dir(&target_path)
            .status()
            .expect("failed to run sbt; is it installed?");
        assert!(
            status.success(),
            "generated optional-stdout tool client did not compile in {target_path}"
        );
    }
}
