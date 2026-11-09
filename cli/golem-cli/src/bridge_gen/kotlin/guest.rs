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

//! Guest-mode Kotlin client: a typed surface over the Kotlin SDK's `WasmRpc`.

use crate::bridge_gen::kotlin::kotlin::{escape_kotlin_ident, unique_idents_with_reserved};
use crate::bridge_gen::kotlin::kotlin_writer::KotlinWriter;
use crate::bridge_gen::kotlin::sdk_wit::{sdk_wit_lowered_type, sdk_wit_type};
use crate::bridge_gen::kotlin::{
    KotlinBridgeGenerator, KotlinBridgeMode, PACKAGE_ROOT_NAMES, client_object_name,
    kotlin_string_literal, remote_class_name,
};
use crate::bridge_gen::type_naming::user_supplied_fields;
use crate::bridge_gen::{BridgeGenerator, bridge_client_directory_name};
use crate::fs;
use crate::sdk_overrides::sdk_overrides;
use crate::versions::kotlin_dep;
use camino::{Utf8Path, Utf8PathBuf};
use golem_common::model::agent::{AgentConfigSource, AgentMode};
use golem_common::schema::agent::AgentConfigDeclarationSchema;
use golem_common::schema::{
    AgentMethodSchema, AgentTypeSchema, InputSchema, NamedField, OutputSchema,
};
use heck::ToLowerCamelCase;

const RPC: &str = "cloud.golem.runtime.WasmRpc";
const RPC_RESULT: &str = "cloud.golem.runtime.RpcResult";
const RPC_EXCEPTION: &str = "cloud.golem.runtime.RpcException";
const FUTURE_INVOKE_RESULT: &str = "cloud.golem.runtime.FutureInvokeResult";
const CANCELLATION_TOKEN: &str = "cloud.golem.runtime.CancellationToken";
const INVOCATION_METADATA: &str = "cloud.golem.runtime.InvocationMetadata";
const SCOPE_CARD: &str = "cloud.golem.permissions.Card";
const CONFIG_OVERRIDE: &str = "cloud.golem.config.ConfigOverride";
const TYPED_SCHEMA_VALUE: &str = "cloud.golem.runtime.TypedSchemaValue";
const GENERATE_IDEMPOTENCY_KEY: &str = "cloud.golem.runtime.HostApi.generateIdempotencyKey()";

/// Locals and members of the generated remote class and client object. A
/// parameter must not take them, because the parameter would shadow them.
pub(super) const GUEST_RESERVED_PARAM_NAMES: &[&str] = &[
    "golemRpc",
    "golemResolved",
    "golemInput",
    "golemResult",
    "golemValue",
    "golemError",
    "golemAt",
    "golemFuture",
    "golemPhantom",
    "golemConfig",
    "golemOverrides",
    "agentTypeName",
    "close",
    "withDefaultScopeCard",
    "withoutDefaultScopeCard",
    "withScopeCard",
];

/// Prefix of every name the generated code declares in a class or function scope; a user name with it gets a trailing underscore.
pub(super) const GENERATED_PREFIX: &str = "golem";

/// Keeps a user-chosen name clear of the names the generated code declares.
pub(super) fn avoid_generated_prefix(name: String) -> String {
    if name.starts_with(GENERATED_PREFIX) {
        format!("{name}_")
    } else {
        name
    }
}

/// Names of the temporaries the structural codecs introduce at depth zero.
const CODEC_TEMP_NAMES: &[&str] = &["e0", "elems0", "k0", "v0", "r0", "p", "t0"];

impl BridgeGenerator for KotlinBridgeGenerator {
    fn new(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        testing: bool,
    ) -> anyhow::Result<Self> {
        Self::new_with_mode(
            agent_type,
            target_path,
            testing,
            KotlinBridgeMode::ExternalRest,
        )
    }

    fn generate(&mut self) -> anyhow::Result<()> {
        if !self.target_path.exists() {
            fs::create_dir_all(&self.target_path)?;
        }
        self.write_build_files()?;
        self.write_runtime()?;
        self.write_client()
    }
}

impl KotlinBridgeGenerator {
    pub fn new_with_mode(
        agent_type: AgentTypeSchema,
        target_path: &Utf8Path,
        _testing: bool,
        mode: KotlinBridgeMode,
    ) -> anyhow::Result<Self> {
        Self::new_core(agent_type, target_path, mode, std::iter::empty::<String>())
    }

    pub(crate) fn library_name(&self) -> String {
        bridge_client_directory_name(&self.agent_type.type_name, self.mode.bridge_mode())
    }

    fn source_set(&self) -> &'static str {
        match self.mode {
            KotlinBridgeMode::GuestWasmRpc => "wasmWasiMain",
            KotlinBridgeMode::ExternalRest => "main",
        }
    }

    fn write_build_files(&self) -> anyhow::Result<()> {
        let name = self.library_name();
        match self.mode {
            KotlinBridgeMode::GuestWasmRpc => {
                let sdk_version = sdk_overrides()?.kotlin_sdk_dep();
                fs::write_str(
                    self.target_path.join("settings.gradle.kts"),
                    settings_gradle(&name, kotlin_dep::KOTLIN_PLUGIN_VERSION),
                )?;
                fs::write_str(
                    self.target_path.join("build.gradle.kts"),
                    build_gradle(&sdk_version),
                )?;
            }
            KotlinBridgeMode::ExternalRest => {
                fs::write_str(
                    self.target_path.join("settings.gradle.kts"),
                    super::external::settings_gradle(&name, kotlin_dep::KOTLIN_PLUGIN_VERSION),
                )?;
                fs::write_str(
                    self.target_path.join("build.gradle.kts"),
                    super::external::build_gradle(),
                )?;
            }
        }
        fs::write_str(
            self.target_path.join("gradle.properties"),
            GRADLE_PROPERTIES,
        )
    }

    fn write_runtime(&self) -> anyhow::Result<()> {
        match self.mode {
            KotlinBridgeMode::GuestWasmRpc => Ok(()),
            KotlinBridgeMode::ExternalRest => {
                super::external::write_runtime(&self.target_path.join("src/main/kotlin"))
            }
        }
    }

    fn write_client(&self) -> anyhow::Result<()> {
        let content = match self.mode {
            KotlinBridgeMode::GuestWasmRpc => self.generate_guest_client_source()?,
            KotlinBridgeMode::ExternalRest => self.generate_external_client_source()?,
        };
        let mut path: Utf8PathBuf = self
            .target_path
            .join("src")
            .join(self.source_set())
            .join("kotlin");
        for segment in self.client_package().split('.') {
            path.push(segment);
        }
        path.push(format!("{}.kt", client_object_name(&self.agent_type)));
        fs::write_str(path, content)
    }

    /// The full source file of the guest client: package, imports, generated
    /// types, codecs, the client object and the remote class.
    pub(crate) fn generate_guest_client_source(&self) -> anyhow::Result<String> {
        let mut body = KotlinWriter::new();
        self.write_guest_client_object(&mut body)?;
        // The type definitions are rendered after the client so that every
        // tuple arity the client mentions is known when the tuple classes are
        // written.
        let mut types = KotlinWriter::new();
        self.write_type_definitions(&mut types)?;
        self.write_codecs(&mut types)?;
        let mut tuples = KotlinWriter::new();
        self.write_tuple_types(&mut tuples);

        let mut out = KotlinWriter::new();
        out.line("// Generated by golem-cli. Do not edit.");
        out.line("@file:Suppress(\"unused\", \"RedundantVisibilityModifier\", \"UNCHECKED_CAST\", \"NAME_SHADOWING\")");
        out.blank();
        out.line(format!("package {}", self.client_package()));
        out.blank();
        out.line("import kotlin.time.toDuration");
        out.blank();
        out.line(types.finish());
        out.line(tuples.finish());
        out.line(body.finish());
        out.blank();
        out.line(self.codec_helper_source());
        Ok(out.finish())
    }

    // --- Names --------------------------------------------------------------

    pub(super) fn camel(&self, name: &str) -> String {
        if self.same_language {
            name.to_string()
        } else {
            name.to_lower_camel_case()
        }
    }

    /// The identifiers of the parameters of a constructor or method, in
    /// declaration order, clear of everything the generated code declares.
    pub(super) fn input_param_idents(&self, fields: &[&NamedField]) -> Vec<String> {
        let mut reserved: Vec<String> = PACKAGE_ROOT_NAMES
            .iter()
            .chain(GUEST_RESERVED_PARAM_NAMES)
            .chain(CODEC_TEMP_NAMES)
            .map(|s| s.to_string())
            .collect();
        reserved.push("phantom".to_string());
        for i in 0..fields.len().max(1) {
            reserved.push(format!("f{i}"));
        }
        for i in 0..32 {
            reserved.push(format!("te{i}"));
        }
        let reserved_refs: Vec<&str> = reserved.iter().map(|s| s.as_str()).collect();
        unique_idents_with_reserved(
            fields
                .iter()
                .map(|f| escape_kotlin_ident(&avoid_generated_prefix(self.camel(&f.name))))
                .collect(),
            &reserved_refs,
        )
    }

    /// `(name, type)` for each user-supplied input field.
    pub(super) fn input_params(
        &self,
        input: &InputSchema,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let fields = user_supplied_fields(input);
        let names = self.input_param_idents(&fields);
        fields
            .iter()
            .zip(names)
            .map(|(field, name)| Ok((name, self.type_reference(&field.schema)?)))
            .collect()
    }

    /// The `SchemaValue.Record` expression of an input's parameters.
    pub(super) fn input_record_expr(&self, input: &InputSchema) -> anyhow::Result<String> {
        let fields = user_supplied_fields(input);
        let names = self.input_param_idents(&fields);
        let mut lines = Vec::new();
        let mut elems = Vec::new();
        for (idx, field) in fields.iter().enumerate() {
            let enc = self.encode_expr(&names[idx], &field.schema, 0)?;
            lines.push(format!("val f{idx} = {enc}"));
            elems.push(format!("f{idx}"));
        }
        let record = if elems.is_empty() {
            format!("{}.Record(kotlin.collections.emptyList())", self.sv())
        } else {
            format!(
                "{}.Record(kotlin.collections.listOf({}))",
                self.sv(),
                elems.join(", ")
            )
        };
        if lines.is_empty() {
            Ok(record)
        } else {
            Ok(format!("run {{\n{}\n{record}\n}}", lines.join("\n")))
        }
    }

    pub(super) fn local_configs(&self) -> Vec<&AgentConfigDeclarationSchema> {
        self.agent_type
            .config
            .iter()
            .filter(|c| c.source == AgentConfigSource::Local)
            .collect()
    }

    // --- Client object ------------------------------------------------------

    fn write_guest_client_object(&self, writer: &mut KotlinWriter) -> anyhow::Result<()> {
        let object_name = client_object_name(&self.agent_type);
        let remote_name = remote_class_name(&self.agent_type);
        let type_name = self.agent_type.type_name.as_str();

        writer.line(format!(
            "/** Typed client of the agent type `{}`. */",
            crate::bridge_gen::kotlin::kdoc_safe(type_name)
        ));
        writer.line(format!("object {object_name} {{"));
        writer.indent();
        writer.line(format!(
            "const val agentTypeName: kotlin.String = {}",
            kotlin_string_literal(type_name)
        ));
        writer.blank();
        self.write_guest_constructors(writer, &remote_name)?;
        writer.dedent();
        writer.line("}");
        writer.blank();

        self.write_remote_future(writer);
        writer.blank();
        self.write_guest_remote_class(writer, &remote_name)?;
        Ok(())
    }

    fn write_remote_future(&self, writer: &mut KotlinWriter) {
        writer.line("/** A call in flight; await it for its result, or cancel it. */");
        writer.line(format!(
            "class RemoteFuture<R> internal constructor(private val future: {FUTURE_INVOKE_RESULT}, private val decode: ({RPC_RESULT}) -> R) : kotlin.AutoCloseable {{"
        ));
        writer.indent();
        writer.line(format!("private var outcome: {RPC_RESULT}? = null"));
        // `cancel` is a plain WIT method beside a separate `[resource-drop]`, and the
        // SDK guards only `close`. Every path therefore shares one flag: the handle is
        // dropped exactly once, and nothing reaches the host after that.
        writer.line("private var released = false");
        writer.blank();
        writer.line("/** Blocks for the result and releases the future; later calls return the same result. Throws the RPC error, if the call failed. */");
        writer.line("fun await(): R {");
        writer.line("    val result = outcome ?: run {");
        writer.line("        check(!released) { \"this call was cancelled or closed before its result was awaited\" }");
        writer.line("        future.get().also {");
        writer.line("            outcome = it");
        writer.line("            released = true");
        writer.line("            future.close()");
        writer.line("        }");
        writer.line("    }");
        writer.line("    return decode(result)");
        writer.line("}");
        writer.blank();
        writer.line("/** Cancels the call and releases it. Has no effect once the result was awaited, or once cancelled or closed. */");
        writer.line("fun cancel() {");
        writer.line("    if (released) return");
        writer.line("    released = true");
        writer.line("    future.cancel()");
        writer.line("    future.close()");
        writer.line("}");
        writer.blank();
        writer.line("override fun close() {");
        writer.line("    if (released) return");
        writer.line("    released = true");
        writer.line("    future.close()");
        writer.line("}");
        writer.dedent();
        writer.line("}");
    }

    fn write_guest_constructors(
        &self,
        writer: &mut KotlinWriter,
        remote_name: &str,
    ) -> anyhow::Result<()> {
        let input = &self.agent_type.constructor.input_schema;
        let params = self.input_params(input)?;
        let decls = params
            .iter()
            .map(|(n, t)| format!("{n}: {t}"))
            .collect::<Vec<_>>()
            .join(", ");
        let input_expr = self.input_record_expr(input)?;
        let local_configs = self.local_configs();
        let param_names: Vec<String> = params.iter().map(|(n, _)| n.clone()).collect();
        let config_defs = self.config_param_defs(&param_names, &local_configs)?;
        let config_decls = config_defs
            .iter()
            .map(|(n, t)| format!("{n}: {t}? = null"))
            .collect::<Vec<_>>()
            .join(", ");
        let uuid = self.runtime.uuid;
        let durable = self.agent_type.mode == AgentMode::Durable;

        writer.line(format!(
            "private fun golemInput({decls}): {} = {input_expr}",
            self.sv()
        ));
        writer.blank();
        writer.line(format!(
            "private fun golemResolve(golemInput: {sv}, golemPhantom: {uuid}?, golemOverrides: kotlin.collections.List<{CONFIG_OVERRIDE}>): {remote_name} =\n    {remote_name}({RPC}(agentTypeName, golemInput, golemOverrides, golemPhantom))",
            sv = self.sv()
        ));
        writer.blank();

        let args = param_names.join(", ");
        let join = |parts: &[&str]| {
            parts
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(", ")
        };
        if durable {
            writer.doc("Connects to the durable agent with these constructor parameters, creating it if it does not exist.\n\nThrows an RPC exception when the host refuses.");
            writer.line(format!("fun get({decls}): {remote_name} ="));
            writer.line(format!(
                "    golemResolve(golemInput({args}), null, kotlin.collections.emptyList())"
            ));
            writer.blank();
            writer.doc("Connects to the phantom instance `phantom` of the agent.");
            writer.line(format!(
                "fun getPhantom({}): {remote_name} =",
                join(&[&decls, &format!("phantom: {uuid}")])
            ));
            writer.line(format!(
                "    golemResolve(golemInput({args}), phantom, kotlin.collections.emptyList())"
            ));
            writer.blank();
            writer.doc("Connects to a new phantom instance of the agent.");
            writer.line(format!("fun newPhantom({decls}): {remote_name} ="));
            writer.line(format!(
                "    golemResolve(golemInput({args}), {GENERATE_IDEMPOTENCY_KEY}, kotlin.collections.emptyList())"
            ));
        } else {
            writer.doc("Creates a new instance of the ephemeral agent.");
            writer.line(format!("fun newPhantom({decls}): {remote_name} ="));
            writer.line(format!(
                "    golemResolve(golemInput({args}), null, kotlin.collections.emptyList())"
            ));
        }

        if !local_configs.is_empty() {
            let config_names: Vec<String> = config_defs.iter().map(|(n, _)| n.clone()).collect();
            writer.blank();
            writer.line(format!(
                "private fun golemConfig({}): kotlin.collections.List<{CONFIG_OVERRIDE}> {{",
                config_decls
            ));
            writer.indent();
            writer.line(format!(
                "val golemOverrides = kotlin.collections.ArrayList<{CONFIG_OVERRIDE}>()"
            ));
            for (idx, config) in local_configs.iter().enumerate() {
                let name = &config_names[idx];
                let path = config
                    .path
                    .iter()
                    .map(|segment| kotlin_string_literal(segment))
                    .collect::<Vec<_>>()
                    .join(", ");
                let enc = self.encode_expr("golemValue", &config.value_type, 0)?;
                let wit = sdk_wit_lowered_type(self.type_naming.graph(), &config.value_type)?;
                writer.line(format!("{name}?.let {{ golemValue ->"));
                writer.indent();
                writer.line(format!(
                    "golemOverrides.add({CONFIG_OVERRIDE}(kotlin.collections.listOf({path}), {TYPED_SCHEMA_VALUE}({}, {enc})))",
                    kotlin_string_literal(&wit)
                ));
                writer.dedent();
                writer.line("}");
            }
            writer.line("return golemOverrides");
            writer.dedent();
            writer.line("}");
            let config_args = config_names.join(", ");
            let with_config = |extra: &str| join(&[&decls, extra, &config_decls]);
            if durable {
                writer.blank();
                writer.line(format!(
                    "fun getWithConfig({}): {remote_name} =",
                    with_config("")
                ));
                writer.line(format!(
                    "    golemResolve(golemInput({args}), null, golemConfig({config_args}))"
                ));
                writer.blank();
                writer.line(format!(
                    "fun getPhantomWithConfig({}): {remote_name} =",
                    with_config(&format!("phantom: {uuid}"))
                ));
                writer.line(format!(
                    "    golemResolve(golemInput({args}), phantom, golemConfig({config_args}))"
                ));
            }
            writer.blank();
            writer.line(format!(
                "fun newPhantomWithConfig({}): {remote_name} =",
                with_config("")
            ));
            let phantom = if durable {
                GENERATE_IDEMPOTENCY_KEY
            } else {
                "null"
            };
            writer.line(format!(
                "    golemResolve(golemInput({args}), {phantom}, golemConfig({config_args}))"
            ));
        }
        Ok(())
    }

    /// Parameter declarations for the local config overrides, each nullable
    /// with a `null` default. Names come from the config path.
    fn config_param_defs(
        &self,
        constructor_param_names: &[String],
        local_configs: &[&AgentConfigDeclarationSchema],
    ) -> anyhow::Result<Vec<(String, String)>> {
        if local_configs.is_empty() {
            return Ok(Vec::new());
        }
        let base_names: Vec<String> = local_configs
            .iter()
            .map(|config| {
                let joined = config.path.join("-").to_lower_camel_case();
                escape_kotlin_ident(&avoid_generated_prefix(if joined.is_empty() {
                    "config".to_string()
                } else {
                    joined
                }))
            })
            .collect();
        let mut reserved: Vec<&str> = PACKAGE_ROOT_NAMES
            .iter()
            .chain(GUEST_RESERVED_PARAM_NAMES)
            .chain(CODEC_TEMP_NAMES)
            .copied()
            .collect();
        reserved.extend(constructor_param_names.iter().map(|s| s.as_str()));
        reserved.push("phantom");
        let names = unique_idents_with_reserved(base_names, &reserved);
        local_configs
            .iter()
            .zip(names)
            .map(|(config, name)| Ok((name, self.type_reference(&config.value_type)?)))
            .collect()
    }

    // --- Remote class -------------------------------------------------------

    fn write_guest_remote_class(
        &self,
        writer: &mut KotlinWriter,
        remote_name: &str,
    ) -> anyhow::Result<()> {
        let methods = &self.agent_type.methods;
        let names = self.method_names(methods);

        writer.line(format!(
            "/** A connection to one instance of the agent `{}`. Close it when done. */",
            crate::bridge_gen::kotlin::kdoc_safe(self.agent_type.type_name.as_str())
        ));
        writer.line(format!(
            "class {remote_name} internal constructor(private val golemRpc: {RPC}) : kotlin.AutoCloseable {{"
        ));
        writer.indent();
        writer.line(
            "val agentTypeName: kotlin.String get() = ".to_string()
                + &format!("{}.agentTypeName", client_object_name(&self.agent_type)),
        );
        writer.blank();
        for (idx, method) in methods.iter().enumerate() {
            self.write_guest_method(writer, idx, method, &names[idx])?;
            writer.blank();
        }
        writer.line("/** Lends `card` to the remote agent for every call made through this connection (`null` stops lending). */");
        writer.line(format!(
            "fun withDefaultScopeCard(card: {SCOPE_CARD}?): {remote_name} {{"
        ));
        writer.line("    golemRpc.defaultScopeCard = card");
        writer.line("    return this");
        writer.line("}");
        writer.blank();
        writer.line("/** Runs `block` with `card` lent to the remote agent, then restores the previous card. */");
        writer.line(format!(
            "fun <T> withScopeCard(card: {SCOPE_CARD}, block: {remote_name}.() -> T): T {{"
        ));
        writer.line("    val previous = golemRpc.defaultScopeCard");
        writer.line("    golemRpc.defaultScopeCard = card");
        writer.line("    try {");
        writer.line("        return block()");
        writer.line("    } finally {");
        writer.line("        golemRpc.defaultScopeCard = previous");
        writer.line("    }");
        writer.line("}");
        writer.blank();
        writer.line("/** Releases the underlying wasm-rpc handle. Idempotent. */");
        writer.line("override fun close() = golemRpc.close()");
        writer.dedent();
        writer.line("}");
        Ok(())
    }

    /// Identifiers of the awaited, async, trigger and schedule functions of
    /// every method. A method's own name always wins over a derived name.
    pub(super) fn method_names(&self, methods: &[AgentMethodSchema]) -> Vec<MethodNames> {
        let reserved = [
            "agentTypeName",
            "getAgentTypeName",
            "getAgentId",
            "close",
            "withDefaultScopeCard",
            "withScopeCard",
            "equals",
            "hashCode",
            "toString",
            "golemRpc",
        ];
        let raw: Vec<String> = methods
            .iter()
            .map(|m| avoid_generated_prefix(self.camel(&m.name)))
            .collect();
        let mut taken: Vec<String> = reserved.iter().map(|s| s.to_string()).collect();
        let pass = |suffix: &str, taken: &mut Vec<String>| -> Vec<String> {
            let refs: Vec<&str> = taken.iter().map(|s| s.as_str()).collect();
            let idents = unique_idents_with_reserved(
                raw.iter()
                    .map(|r| escape_kotlin_ident(&format!("{r}{suffix}")))
                    .collect(),
                &refs,
            );
            taken.extend(idents.iter().cloned());
            idents
        };
        let call = pass("", &mut taken);
        let asynchronous = pass("Async", &mut taken);
        let trigger = pass("Trigger", &mut taken);
        let schedule = pass("ScheduleAt", &mut taken);
        let schedule_cancelable = pass("ScheduleCancelableAt", &mut taken);
        (0..methods.len())
            .map(|i| MethodNames {
                call: call[i].clone(),
                asynchronous: asynchronous[i].clone(),
                trigger: trigger[i].clone(),
                schedule: schedule[i].clone(),
                schedule_cancelable: schedule_cancelable[i].clone(),
            })
            .collect()
    }

    fn write_guest_method(
        &self,
        writer: &mut KotlinWriter,
        index: usize,
        method: &AgentMethodSchema,
        names: &MethodNames,
    ) -> anyhow::Result<()> {
        let params = self.input_params(&method.input_schema)?;
        let decls = params
            .iter()
            .map(|(n, t)| format!("{n}: {t}"))
            .collect::<Vec<_>>()
            .join(", ");
        let input_expr = self.input_record_expr(&method.input_schema)?;
        let wire = kotlin_string_literal(&method.name);
        let (ret_ty, wit, decode) = self.guest_output(&method.output_schema)?;
        let wit_lit = kotlin_string_literal(&wit);
        let decoder = format!("golemDecode{index}");
        let with_decls = |extra: &str| {
            [decls.as_str(), extra]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let exc = self.exception_name();

        writer.line(format!(
            "private fun {decoder}(golemResult: {RPC_RESULT}): {ret_ty} {{"
        ));
        writer.indent();
        writer.line("when (golemResult) {");
        writer.indent();
        writer.line(format!(
            "is {RPC_RESULT}.Err -> throw {RPC_EXCEPTION}(golemResult.error)"
        ));
        writer.line(format!("is {RPC_RESULT}.Ok -> {{"));
        writer.indent();
        match decode {
            Some(dec) => {
                writer.line(format!(
                    "val golemValue = golemResult.value ?: throw {exc}(\"Missing result value for an awaited invocation\")"
                ));
                writer.line(format!("return {dec}"));
            }
            None => writer.line("return kotlin.Unit"),
        }
        writer.dedent();
        writer.line("}");
        writer.dedent();
        writer.line("}");
        writer.dedent();
        writer.line("}");
        writer.blank();

        let doc = method.description.trim();
        let kdoc = |writer: &mut KotlinWriter, extra: &str| {
            let text = if doc.is_empty() {
                extra.to_string()
            } else {
                format!("{doc}\n\n{extra}")
            };
            writer.doc(&text);
        };
        kdoc(
            writer,
            "Calls the method and blocks for its result. Throws an RPC exception on failure.",
        );
        writer.line(format!("fun {}({decls}): {ret_ty} {{", names.call));
        writer.line(format!("    val golemInput = {}", indent_tail(&input_expr)));
        writer.line(format!(
            "    return {decoder}(golemRpc.invokeAndAwait({wire}, golemInput, {wit_lit}))"
        ));
        writer.line("}");
        writer.blank();

        kdoc(
            writer,
            "Starts the call and returns at once; await the result on the returned future.",
        );
        writer.line(format!(
            "fun {}({decls}): RemoteFuture<{ret_ty}> {{",
            names.asynchronous
        ));
        writer.line(format!("    val golemInput = {}", indent_tail(&input_expr)));
        writer.line(format!(
            "    return RemoteFuture(golemRpc.asyncInvokeAndAwait({wire}, golemInput, {wit_lit})) {{ golemResult -> {decoder}(golemResult) }}"
        ));
        writer.line("}");
        writer.blank();

        kdoc(writer, "Sends the call without waiting for the result.");
        writer.line(format!("fun {}({decls}) {{", names.trigger));
        writer.line(format!("    val golemInput = {}", indent_tail(&input_expr)));
        writer.line(format!(
            "    val golemError = golemRpc.invoke({wire}, golemInput)"
        ));
        writer.line(format!(
            "    if (golemError != null) throw {RPC_EXCEPTION}(golemError)"
        ));
        writer.line("}");
        writer.blank();

        let datetime = self.runtime.datetime;
        kdoc(writer, "Schedules the call for the given time.");
        writer.line(format!(
            "fun {}({}): {INVOCATION_METADATA} {{",
            names.schedule,
            with_decls(&format!("golemAt: {datetime}"))
        ));
        writer.line(format!("    val golemInput = {}", indent_tail(&input_expr)));
        writer.line(format!(
            "    return golemRpc.scheduleInvocation(golemAt.seconds, golemAt.nanoseconds, {wire}, golemInput)"
        ));
        writer.line("}");
        writer.blank();

        kdoc(
            writer,
            "Schedules the call for the given time; the returned token cancels it.",
        );
        writer.line(format!(
            "fun {}({}): {CANCELLATION_TOKEN} {{",
            names.schedule_cancelable,
            with_decls(&format!("golemAt: {datetime}"))
        ));
        writer.line(format!("    val golemInput = {}", indent_tail(&input_expr)));
        writer.line(format!(
            "    return golemRpc.scheduleCancelableInvocation(golemAt.seconds, golemAt.nanoseconds, {wire}, golemInput)"
        ));
        writer.line("}");
        Ok(())
    }

    /// `(Kotlin return type, SDK value grammar, decode expression over
    /// `golemValue`)`. The decode expression is `None` for a unit result.
    fn guest_output(
        &self,
        output: &OutputSchema,
    ) -> anyhow::Result<(String, String, Option<String>)> {
        match output {
            OutputSchema::Unit => Ok(("kotlin.Unit".to_string(), "()".to_string(), None)),
            OutputSchema::Single(ty) => {
                let ret = self.type_reference(ty)?;
                let wit = sdk_wit_type(self.type_naming.graph(), ty)?;
                let dec = self.decode_expr("golemValue", ty, 0)?;
                Ok((ret, wit, Some(dec)))
            }
        }
    }

    pub(super) fn exception_name(&self) -> &'static str {
        self.runtime.exception
    }
}

pub(super) const GRADLE_PROPERTIES: &str = "org.gradle.jvmargs=-Xmx2g\nkotlin.suppressGradlePluginWarnings=DeprecatedGradleVersionWarning\n";

/// `settings.gradle.kts` of a generated guest project.
pub(super) fn settings_gradle(name: &str, kotlin_version: &str) -> String {
    format!(
        "pluginManagement {{\n    repositories {{\n        mavenLocal()\n        gradlePluginPortal()\n    }}\n    plugins {{\n        kotlin(\"multiplatform\") version \"{kotlin_version}\"\n    }}\n}}\n\nrootProject.name = \"{name}\"\n"
    )
}

/// `build.gradle.kts` of a generated guest project: a Kotlin/Wasm library on the Golem Kotlin SDK.
pub(super) fn build_gradle(sdk_version: &str) -> String {
    format!(
        "plugins {{\n    kotlin(\"multiplatform\")\n}}\n\ngroup = \"golem.bridge\"\nversion = \"0.0.1\"\n\nrepositories {{\n    mavenLocal()\n    mavenCentral()\n}}\n\nkotlin {{\n    @OptIn(org.jetbrains.kotlin.gradle.ExperimentalWasmDsl::class)\n    wasmWasi {{\n        nodejs()\n    }}\n\n    sourceSets {{\n        val wasmWasiMain by getting {{\n            dependencies {{\n                implementation(\"cloud.golem:golem-kotlin-sdk:{sdk_version}\")\n            }}\n        }}\n    }}\n}}\n"
    )
}

pub(super) struct MethodNames {
    pub(super) call: String,
    pub(super) asynchronous: String,
    pub(super) trigger: String,
    pub(super) schedule: String,
    pub(super) schedule_cancelable: String,
}

/// Indents every line after the first of a multi-line expression by one
/// level, so it reads correctly after `val x = `.
pub(super) fn indent_tail(expr: &str) -> String {
    let mut lines = expr.split('\n');
    let first = lines.next().unwrap_or_default().to_string();
    lines.fold(first, |acc, line| {
        if line.is_empty() {
            format!("{acc}\n")
        } else {
            format!("{acc}\n    {line}")
        }
    })
}
