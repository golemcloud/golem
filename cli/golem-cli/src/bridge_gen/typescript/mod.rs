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

mod javascript;
#[allow(dead_code)]
mod ts_writer;
mod type_name;

pub use type_name::TypeScriptTypeName;

use crate::bridge_gen::parameter_naming::ParameterNaming;
use crate::bridge_gen::type_naming::TypeNaming;
use crate::bridge_gen::typescript::javascript::escape_js_ident;
use crate::bridge_gen::typescript::ts_writer::{
    FunctionWriter, TsAnonymousFunctionWriter, TsFunctionWriter, TsWriter, indent,
};
use crate::bridge_gen::{BridgeGenerator, bridge_client_directory_name};
use crate::fs;
use crate::sdk_overrides::{sdk_overrides, workspace_root};
use anyhow::anyhow;
use camino::{Utf8Path, Utf8PathBuf};
use golem_common::model::agent::{
    AgentConfigDeclaration, AgentConfigSource, AgentMethod, AgentMode, AgentType, BinaryDescriptor,
    DataSchema, ElementSchema, NamedElementSchema, NamedElementSchemas, TextDescriptor,
};
use golem_common::schema::graph::SchemaTypeDef;
use golem_common::schema::schema_type::SchemaType;
use golem_wasm::analysis::AnalysedType;
use heck::{ToLowerCamelCase, ToUpperCamelCase};
use indoc::formatdoc;
use serde_json::json;

const TS_BRIDGE_PACKAGE_NAME: &str = "@golemcloud/golem-ts-bridge";
const MULTIMODAL_INPUT_NAME: &str = "multimodalInput";

pub struct TypeScriptBridgeGenerator {
    target_path: Utf8PathBuf,
    type_naming: TypeNaming<TypeScriptTypeName>,
    agent_type: AgentType,
    testing: bool,
    same_language: bool,
}

impl BridgeGenerator for TypeScriptBridgeGenerator {
    fn new(agent_type: AgentType, target_path: &Utf8Path, testing: bool) -> anyhow::Result<Self> {
        TypeScriptBridgeGenerator::new(agent_type, target_path, testing)
    }

    fn generate(&mut self) -> anyhow::Result<()> {
        let library_name = self.library_name();

        let ts_path = self.target_path.join(format!("{library_name}.ts"));
        let package_json_path = self.target_path.join("package.json");
        let tsconfig_json_path = self.target_path.join("tsconfig.json");
        let test_path = self.target_path.join("test.ts");

        if !self.target_path.exists() {
            std::fs::create_dir_all(&self.target_path)?;
        }
        self.generate_ts(&ts_path)?;
        self.generate_package_json(&package_json_path)?;
        self.generate_tsconfig_json(&tsconfig_json_path)?;
        if self.testing {
            self.generate_test(&test_path)?;
        }

        Ok(())
    }
}

impl TypeScriptBridgeGenerator {
    pub fn new(
        agent_type: AgentType,
        target_path: &Utf8Path,
        testing: bool,
    ) -> anyhow::Result<Self> {
        let same_language = agent_type
            .source_language
            .eq_ignore_ascii_case("typescript")
            || agent_type.source_language.eq_ignore_ascii_case("ts");
        Ok(Self {
            target_path: target_path.to_path_buf(),
            type_naming: TypeNaming::new(&agent_type, same_language)?,
            agent_type,
            testing,
            same_language,
        })
    }

    /// Resolve a legacy [`AnalysedType`] from the agent declaration to the
    /// [`SchemaType`] that was lowered for it during `TypeNaming::new`.
    /// Returning the memoised result is critical: a fresh
    /// `analysed_type_to_schema_graph(typ)` here would only see the type
    /// in isolation and emit the base `TypeId`, which would silently
    /// collide with the disambiguated id in `type_naming.graph()` and
    /// cause two distinct same-name composites to be rendered as the
    /// first one's body.
    fn import_analysed_type(&self, typ: &AnalysedType) -> anyhow::Result<SchemaType> {
        self.type_naming
            .imported_schema_type(typ)
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "Legacy AnalysedType was not collected during TypeNaming::new — \
                     bridge_gen lost track of an emit-time type. typ = {typ:?}"
                )
            })
    }

    /// Resolve a [`SchemaType::Ref`] against [`TypeNaming::graph`] and return
    /// the def body. For inline schema types this returns the input
    /// unchanged.
    fn resolve_ref<'a>(&'a self, typ: &'a SchemaType) -> &'a SchemaType {
        match typ {
            SchemaType::Ref { id, .. } => {
                let def: &SchemaTypeDef = self
                    .type_naming
                    .graph()
                    .lookup(id)
                    .expect("Ref points to a def in the shared graph");
                &def.body
            }
            other => other,
        }
    }

    fn bridge_package_dep(testing: bool) -> anyhow::Result<String> {
        if testing {
            return Ok(fs::path_to_str(
                &workspace_root()?.join("sdks/ts/packages/golem-ts-bridge"),
            )?
            .to_string());
        }

        sdk_overrides()?.ts_package_dep("golem-ts-bridge")
    }

    /// Generates the client library's package.json
    fn generate_package_json(&self, path: &Utf8Path) -> anyhow::Result<()> {
        let scripts = if self.testing {
            json!(
              {
                "build": "tsc",
                "test": "npx tsx test.ts"
            })
        } else {
            json!({
                "build": "tsc",
            })
        };
        let package_json = json! {
            {
                "name": self.library_name(),
                "version": "0.0.1", // TODO: use user-defined agent version if available
                "description": "Generated by golem-cli",
                "type": "module",
                "main": format!("{}.js", self.library_name()),
                "types": format!("{}.d.ts", self.library_name()),
                "scripts": scripts,
                "dependencies": {
                    "uuid": "^13",
                    (TS_BRIDGE_PACKAGE_NAME): Self::bridge_package_dep(self.testing)?,
                },
                "devDependencies": {
                    "typescript": "^5.9",
                    "tsx": "^4.7",
                    "@types/node": "^25",
                }
            }
        };
        std::fs::write(path, serde_json::to_string_pretty(&package_json)?)
            .map_err(|e| anyhow!("Failed to write package.json file: {e}"))?;
        Ok(())
    }

    /// Generates the client library's tsconfig.json
    fn generate_tsconfig_json(&self, path: &Utf8Path) -> anyhow::Result<()> {
        let mut include = vec![format!("{}.ts", self.library_name())];
        if self.testing {
            include.push("test.ts".to_string());
        }

        let tsconfig_json = json! {
            {
                "compilerOptions": {
                    "target": "es2020",
                    "module": "esnext",
                    "moduleResolution": "node",
                    "strict": true,
                    "esModuleInterop": true,
                    "skipLibCheck": true,
                    "forceConsistentCasingInFileNames": true,
                    "declaration": true,
                },
                "include": include
            }
        };
        std::fs::write(path, serde_json::to_string_pretty(&tsconfig_json)?)
            .map_err(|e| anyhow!("Failed to write tsconfig.json file: {e}"))?;
        Ok(())
    }

    /// Generates the test.ts module. This module exposes encoding/decoding functions via
    /// stdin/out to be used from tests only. The test module is not usable by itself and
    /// should never be part of the generated NPM package outside of Golem's internal tests.
    fn generate_test(&self, path: &Utf8Path) -> anyhow::Result<()> {
        let mut writer = TsWriter::new();

        self.generate_test_imports(&mut writer);
        self.generate_test_type_definitions(&mut writer)?;
        self.generate_test_read_stdin_helper(&mut writer);
        self.generate_test_method_functions(&mut writer)?;
        self.generate_test_functions_map(&mut writer);
        self.generate_test_main_handler(&mut writer)?;
        self.generate_test_entry_point(&mut writer);

        writer.finish(path)
    }

    /// Writes the imports section of the test module.
    fn generate_test_imports(&self, writer: &mut TsWriter) {
        writer.import_module("base", TS_BRIDGE_PACKAGE_NAME);
    }

    /// Defines the test types and their corresponding encode/decode functions. These types and functions are
    /// also generated into the main module, but there they are private. For testing, we duplicate
    /// them in the test module.
    fn generate_test_type_definitions(&self, writer: &mut TsWriter) -> anyhow::Result<()> {
        self.generate_ts_type_definitions(writer)
    }

    /// Write a helper function to the test module to read a JSON from stdin
    fn generate_test_read_stdin_helper(&self, writer: &mut TsWriter) {
        let mut read_stdin = writer.begin_export_async_function("readStdin");
        read_stdin.result("any");
        read_stdin.write_line("let input = '';");
        read_stdin.write_line("for await (const chunk of process.stdin) {");
        read_stdin.indent();
        read_stdin.write_line("input += chunk;");
        read_stdin.unindent();
        read_stdin.write_line("}");
        read_stdin.write_line("return JSON.parse(input);");
        drop(read_stdin);

        writer.write_line("");
    }

    /// Generate encode/decode test functions for each agent method's input and output schema
    fn generate_test_method_functions(&self, writer: &mut TsWriter) -> anyhow::Result<()> {
        // Generate test functions for each method using the same code generators as the main library
        for method_def in &self.agent_type.methods {
            self.generate_test_method_encode_input(writer, method_def)?;
            writer.write_line("");
            self.generate_test_method_decode_input(writer, method_def)?;
            writer.write_line("");
            self.generate_test_method_encode_output(writer, method_def)?;
            writer.write_line("");
            self.generate_test_method_decode_output(writer, method_def)?;
            writer.write_line("");
        }
        Ok(())
    }

    /// Generates a test function that simulates the encoding of an agent method's parameters. The
    /// input coming from stdin is supposed to match the generated method's parameter signature, and
    /// it encodes the values into a SchemaValue to be passed to the invocation API.
    fn generate_test_method_encode_input(
        &self,
        writer: &mut TsWriter,
        method_def: &AgentMethod,
    ) -> anyhow::Result<()> {
        let method_name_pascal = self.to_method_pascal(&method_def.name);
        let mut encode_input =
            writer.begin_export_async_function(&format!("encode{}Input", method_name_pascal));
        encode_input.result("void");
        encode_input.write_line("const __json = await readStdin();");
        if !method_def.input_schema.is_unit() {
            encode_input.write("const [");
            self.write_parameter_name_list(&mut encode_input, &method_def.input_schema);
            encode_input.write_line("] = __json;");
        }
        encode_input.write_line("const __result: base.SchemaValue = ");
        self.write_encode_input_record(
            &mut encode_input,
            &method_def.input_schema,
            MULTIMODAL_INPUT_NAME,
        )?;
        encode_input.write_line("console.log(JSON.stringify(__result));");
        Ok(())
    }

    /// Generates a test function that simulates the decoding of an agent method's parameters. The input
    /// coming from the stdin is a SchemaValue, and it decodes it into the method's parameter
    /// signature.
    fn generate_test_method_decode_input(
        &self,
        writer: &mut TsWriter,
        method_def: &AgentMethod,
    ) -> anyhow::Result<()> {
        let method_name_pascal = self.to_method_pascal(&method_def.name);
        let mut decode_input =
            writer.begin_export_async_function(&format!("decode{}Input", method_name_pascal));
        decode_input.result("void");
        decode_input.write_line("const __jsonResult: base.SchemaValue = await readStdin();");
        decode_input.write_line("const __decoded = (() => {");
        decode_input.indent();
        self.write_decode_input(&mut decode_input, &method_def.input_schema, "__jsonResult")?;
        decode_input.unindent();
        decode_input.write_line("})();");
        decode_input.write_line("console.log(JSON.stringify(__decoded));");
        Ok(())
    }

    /// Generates a test function that simulates the encoding of an agent method's return value. The
    /// input coming from stdin is supposed to match the generated method's return signature, and it
    /// encodes the values into a SchemaValue to be passed to the invocation API.
    fn generate_test_method_encode_output(
        &self,
        writer: &mut TsWriter,
        method_def: &AgentMethod,
    ) -> anyhow::Result<()> {
        let method_name_pascal = self.to_method_pascal(&method_def.name);
        let mut encode_output =
            writer.begin_export_async_function(&format!("encode{}Output", method_name_pascal));
        encode_output.result("void");
        if method_def.output_schema.is_unit() {
            encode_output.write_line("console.log('void');");
        } else {
            encode_output.write_line("const __json = await readStdin();");
            if !method_def.output_schema.is_unit() {
                encode_output.write("const [");
                self.write_parameter_name_list(&mut encode_output, &method_def.output_schema);
                encode_output.write_line("] = __json;");
            }
            encode_output.write_line("const __result: base.SchemaValue =");
            self.write_encode_output_value(
                &mut encode_output,
                &method_def.output_schema,
                MULTIMODAL_INPUT_NAME,
            )?;
            encode_output.write_line("console.log(JSON.stringify(__result));");
        }
        Ok(())
    }

    /// Generates a test function that simulates the decoding of an agent method's return value. The
    /// input coming from the stdin is a SchemaValue, and it decodes it into the method's return signature.
    fn generate_test_method_decode_output(
        &self,
        writer: &mut TsWriter,
        method_def: &AgentMethod,
    ) -> anyhow::Result<()> {
        let method_name_pascal = self.to_method_pascal(&method_def.name);
        let mut decode_output =
            writer.begin_export_async_function(&format!("decode{}Output", method_name_pascal));
        decode_output.result("void");
        if method_def.output_schema.is_unit() {
            decode_output.write_line("console.log('void');");
        } else {
            decode_output.write_line("const __jsonResult: base.SchemaValue = await readStdin();");
            decode_output.write_line("const __typed = { value: __jsonResult };");
            decode_output.write_line("const __decoded = (() => {");
            decode_output.indent();
            self.write_decode_output(&mut decode_output, &method_def.output_schema, "__typed")?;
            decode_output.unindent();
            decode_output.write_line("})();");
            decode_output.write_line("console.log(JSON.stringify(__decoded));");
        }
        Ok(())
    }

    /// Generates a map of encode/decode pairs keyed by the method name
    fn generate_test_functions_map(&self, writer: &mut TsWriter) {
        // Create a map of available functions
        writer.write_line("const testFunctions: { [key: string]: () => Promise<void> | void } = {");
        writer.indent();
        for method_def in &self.agent_type.methods {
            let method_name_pascal = self.to_method_pascal(&method_def.name);
            writer.write_line(format!("encode{}Input,", method_name_pascal));
            writer.write_line(format!("decode{}Input,", method_name_pascal));
            writer.write_line(format!("encode{}Output,", method_name_pascal));
            writer.write_line(format!("decode{}Output,", method_name_pascal));
        }
        writer.unindent();
        writer.write_line("};");
        writer.write_line("");
    }

    /// Generates the main function for the test module
    fn generate_test_main_handler(&self, writer: &mut TsWriter) -> anyhow::Result<()> {
        let mut main = writer.begin_export_async_function("main");
        main.result("void");

        self.generate_test_main_arg_validation(&mut main)?;
        self.generate_test_main_function_lookup(&mut main);
        self.generate_test_main_function_call(&mut main);

        Ok(())
    }

    /// Generates the command line argument validation part of the test module's main function
    fn generate_test_main_arg_validation(
        &self,
        main: &mut TsFunctionWriter<'_>,
    ) -> anyhow::Result<()> {
        main.write_line("const args = process.argv.slice(2);");
        main.write_line("if (args.length === 0) {");
        main.indent();
        main.write_line("console.error('Usage: npx tsx test.ts <function-name>');");
        main.write_line("console.error('Available functions:');");

        for method_def in &self.agent_type.methods {
            let method_name_pascal = self.to_method_pascal(&method_def.name);
            main.write_line(format!(
                "console.error('  encode{}Input, decode{}Input, encode{}Output, decode{}Output');",
                method_name_pascal, method_name_pascal, method_name_pascal, method_name_pascal
            ));
        }

        main.write_line("process.exit(1);");
        main.unindent();
        main.write_line("}");

        Ok(())
    }

    /// Lookup an encode/decode function based on the provided function name
    fn generate_test_main_function_lookup(&self, main: &mut TsFunctionWriter<'_>) {
        main.write_line("const functionName = args[0];");
        main.write_line("const fn = testFunctions[functionName];");
        main.write_line("if (!fn) {");
        main.indent();
        main.write_line("console.error(`Unknown function: ${functionName}`);");
        main.write_line(
            "console.error('Available functions:', Object.keys(testFunctions).join(', '));",
        );
        main.write_line("process.exit(1);");
        main.unindent();
        main.write_line("}");
    }

    /// Call the encode/decode function based on the provided function name
    fn generate_test_main_function_call(&self, main: &mut TsFunctionWriter<'_>) {
        main.write_line("try {");
        main.indent();
        main.write_line("await fn();");
        main.unindent();
        main.write_line("} catch (error) {");
        main.indent();
        main.write_line("console.error('Error:', error);");
        main.write_line("process.exit(1);");
        main.unindent();
        main.write_line("}");
    }

    /// Entry point of the test module
    fn generate_test_entry_point(&self, writer: &mut TsWriter) {
        writer.write_line("");
        writer.write_line("main();");
    }

    /// Generates the client module
    fn generate_ts(&self, path: &Utf8Path) -> anyhow::Result<()> {
        let mut writer = TsWriter::new();

        let config_var = self.global_config_var_name();

        self.generate_ts_imports(&mut writer);
        self.generate_ts_config_global(&mut writer, &config_var);
        self.generate_ts_type_definitions(&mut writer)?;
        self.generate_ts_class(&mut writer, &config_var)?;
        self.generate_ts_configure_function(&mut writer, &config_var);

        writer.finish(path)
    }

    /// Generates the import section of the client library
    fn generate_ts_imports(&self, writer: &mut TsWriter) {
        writer.import_item("v4", "uuidv4", "uuid");
        writer.import_module("base", TS_BRIDGE_PACKAGE_NAME);
    }

    /// Generates the global variables of the client library.
    ///
    /// Configuration is stored in a global variable, set by the exported `configure` function,
    /// instead of being passed to the agent constructors. The primary reason for this is to
    /// make the agent constructors look exactly like they do in agent-to-agent communication,
    /// and to help the REPL use case by allowing pre-configuration of the client classes.
    fn generate_ts_config_global(&self, writer: &mut TsWriter, config_var: &str) {
        writer.declare_global(
            config_var,
            "base.Configuration | undefined",
            Some("undefined"),
        );
    }

    /// Generates a type definition and an encode/decode function pair for custom types used
    /// by the agent.
    fn generate_ts_type_definitions(&self, writer: &mut TsWriter) -> anyhow::Result<()> {
        for (typ, name) in self.type_naming.types() {
            self.generate_ts_schema_type_def(writer, name, typ)?;
            self.generate_ts_schema_type_encode(writer, name, typ)?;
            self.generate_ts_schema_type_decode(writer, name, typ)?;
        }
        Ok(())
    }

    /// Generates the agent client class
    fn generate_ts_class(&self, writer: &mut TsWriter, config_var: &str) -> anyhow::Result<()> {
        let class_name = &self.agent_type.type_name.0;

        writer.write_doc(&self.agent_type.description);
        writer.begin_export_class(class_name);

        self.generate_ts_class_fields(writer);
        self.generate_ts_class_constructor(writer);
        self.generate_ts_constructor_methods(writer, class_name, config_var)?;
        self.generate_ts_config_getter(writer, config_var);
        self.generate_ts_agent_id_getter(writer);
        self.generate_ts_get_configuration(writer, config_var);
        self.generate_ts_remote_methods(writer, class_name)?;

        writer.end_export_class();

        Ok(())
    }

    /// Generates fields of the agent client class.
    ///
    /// We store the encoded parameters, phantom ID, and agent ID of the targeted agent.
    fn generate_ts_class_fields(&self, writer: &mut TsWriter) {
        writer.declare_field("parameters", "base.SchemaValue", None);
        writer.declare_field("phantomId", "base.PhantomId | undefined", None);
        writer.declare_field("_agentId", "base.AgentId", None);
    }

    /// Generates the private constructor of the agent class. The user-facing constructors
    /// are static methods matching the agent-to-agent API (get, getPhantom, newPhantom)
    fn generate_ts_class_constructor(&self, writer: &mut TsWriter) {
        let mut constructor = writer.begin_private_constructor();
        constructor.param("parameters", "base.SchemaValue");
        constructor.param("phantomId", "base.PhantomId | undefined");
        constructor.param("agentId", "base.AgentId");
        constructor.write_line("this.parameters = parameters;");
        constructor.write_line("this.phantomId = phantomId;");
        constructor.write_line("this._agentId = agentId;");
    }

    /// Generates the static methods for constructing agent clients. For durable agents we
    /// generate `get`, and for any agent we also generate `getPhantom` and `newPhantom`.
    fn generate_ts_constructor_methods(
        &self,
        writer: &mut TsWriter,
        class_name: &str,
        config_var: &str,
    ) -> anyhow::Result<()> {
        if self.agent_type.mode == AgentMode::Durable {
            self.generate_ts_constructor_get_method(writer, class_name, config_var)?;
        }

        self.generate_ts_constructor_get_phantom_method(writer, class_name, config_var)?;
        self.generate_ts_constructor_new_phantom_method(writer, class_name, config_var)?;

        // Generate WithConfig variants if there are local config declarations
        let local_configs: Vec<_> = self
            .agent_type
            .config
            .iter()
            .filter(|c| c.source == AgentConfigSource::Local)
            .collect();

        if !local_configs.is_empty() {
            if self.agent_type.mode == AgentMode::Durable {
                self.generate_ts_constructor_get_with_config_method(
                    writer,
                    class_name,
                    config_var,
                    &local_configs,
                )?;
            }
            self.generate_ts_constructor_get_phantom_with_config_method(
                writer,
                class_name,
                config_var,
                &local_configs,
            )?;
            self.generate_ts_constructor_new_phantom_with_config_method(
                writer,
                class_name,
                config_var,
                &local_configs,
            )?;
        }

        Ok(())
    }

    /// Generates the `get` constructor method
    fn generate_ts_constructor_get_method(
        &self,
        writer: &mut TsWriter,
        class_name: &str,
        config_var: &str,
    ) -> anyhow::Result<()> {
        writer.write_doc(&format!(
            "Gets or creates an instance of this agent\n{}",
            self.agent_type.constructor.description
        ));
        let mut get = writer.begin_static_async_method("get");
        self.write_parameter_list(&mut get, &self.agent_type.constructor.input_schema)?;
        get.result(class_name);

        get.write_line("const parameters: base.SchemaValue = ");
        self.write_encode_input_record(
            &mut get,
            &self.agent_type.constructor.input_schema,
            MULTIMODAL_INPUT_NAME,
        )?;
        get.write_line("const phantomId = undefined;");
        self.write_create_agent_call(&mut get, config_var, "[]");
        get.write_line(format!(
            "return new {class_name}(parameters, phantomId, __createResponse.agentId);"
        ));

        Ok(())
    }

    /// Generates the `getPhantom` constructor method
    fn generate_ts_constructor_get_phantom_method(
        &self,
        writer: &mut TsWriter,
        class_name: &str,
        config_var: &str,
    ) -> anyhow::Result<()> {
        writer.write_doc(&format!(
            "Gets or creates a phantom instance of this agent with a specific phantom ID\n{}",
            self.agent_type.constructor.description
        ));
        let mut get_phantom = writer.begin_static_async_method("getPhantom");
        get_phantom.param("phantomId", "base.PhantomId");
        self.write_parameter_list(&mut get_phantom, &self.agent_type.constructor.input_schema)?;
        get_phantom.result(class_name);

        get_phantom.write_line("const parameters: base.SchemaValue = ");
        self.write_encode_input_record(
            &mut get_phantom,
            &self.agent_type.constructor.input_schema,
            MULTIMODAL_INPUT_NAME,
        )?;
        self.write_create_agent_call(&mut get_phantom, config_var, "[]");
        get_phantom.write_line(format!(
            "return new {class_name}(parameters, phantomId, __createResponse.agentId);"
        ));

        Ok(())
    }

    /// Generates the `newPhantom` constructor method
    fn generate_ts_constructor_new_phantom_method(
        &self,
        writer: &mut TsWriter,
        class_name: &str,
        config_var: &str,
    ) -> anyhow::Result<()> {
        writer.write_doc(&format!(
            "Creates a new phantom instance of this agent\n{}",
            self.agent_type.constructor.description
        ));
        let mut new_phantom = writer.begin_static_async_method("newPhantom");
        self.write_parameter_list(&mut new_phantom, &self.agent_type.constructor.input_schema)?;
        new_phantom.result(class_name);

        new_phantom.write_line("const parameters: base.SchemaValue = ");
        self.write_encode_input_record(
            &mut new_phantom,
            &self.agent_type.constructor.input_schema,
            MULTIMODAL_INPUT_NAME,
        )?;
        new_phantom.write_line("const phantomId = uuidv4();");
        self.write_create_agent_call(&mut new_phantom, config_var, "[]");
        new_phantom.write_line(format!(
            "return new {class_name}(parameters, phantomId, __createResponse.agentId);"
        ));

        Ok(())
    }

    /// Writes the `await base.createAgent(...)` call into the constructor
    fn write_create_agent_call(
        &self,
        writer: &mut TsFunctionWriter<'_>,
        config_var: &str,
        agent_config_expr: &str,
    ) {
        let agent_type_name = &self.agent_type.type_name.0;
        writer.write_line(format!("const __config = {config_var};"));
        writer.write_line(format!(
            "if (!__config) {{ throw new Error(\"{agent_type_name} configuration is not set\"); }}"
        ));
        writer.write_line("const __createResponse = await base.createAgent(__config.server, {");
        writer.indent();
        writer.write_line("appName: __config.application,");
        writer.write_line("envName: __config.environment,");
        writer.write_line(format!("agentTypeName: \"{agent_type_name}\","));
        writer.write_line("parameters,");
        writer.write_line("phantomId,");
        writer.write_line(format!("config: {agent_config_expr},"));
        writer.unindent();
        writer.write_line("});");
    }

    /// Generates the `getWithConfig` constructor method
    fn generate_ts_constructor_get_with_config_method(
        &self,
        writer: &mut TsWriter,
        class_name: &str,
        config_var: &str,
        local_configs: &[&AgentConfigDeclaration],
    ) -> anyhow::Result<()> {
        writer.write_doc(&format!(
            "Gets or creates an instance of this agent with configuration\n{}",
            self.agent_type.constructor.description
        ));
        let mut method = writer.begin_static_async_method("getWithConfig");
        self.write_parameter_list(&mut method, &self.agent_type.constructor.input_schema)?;
        self.write_config_parameter_list(&mut method, local_configs)?;
        method.result(class_name);

        method.write_line("const parameters: base.SchemaValue = ");
        self.write_encode_input_record(
            &mut method,
            &self.agent_type.constructor.input_schema,
            MULTIMODAL_INPUT_NAME,
        )?;
        method.write_line("const phantomId = undefined;");
        self.write_config_encoding(&mut method, local_configs)?;
        self.write_create_agent_call(&mut method, config_var, "agentConfig");
        method.write_line(format!(
            "return new {class_name}(parameters, phantomId, __createResponse.agentId);"
        ));

        Ok(())
    }

    /// Generates the `getPhantomWithConfig` constructor method
    fn generate_ts_constructor_get_phantom_with_config_method(
        &self,
        writer: &mut TsWriter,
        class_name: &str,
        config_var: &str,
        local_configs: &[&AgentConfigDeclaration],
    ) -> anyhow::Result<()> {
        writer.write_doc(&format!(
            "Gets or creates a phantom instance of this agent with configuration and a specific phantom ID\n{}",
            self.agent_type.constructor.description
        ));
        let mut method = writer.begin_static_async_method("getPhantomWithConfig");
        method.param("phantomId", "base.PhantomId");
        self.write_parameter_list(&mut method, &self.agent_type.constructor.input_schema)?;
        self.write_config_parameter_list(&mut method, local_configs)?;
        method.result(class_name);

        method.write_line("const parameters: base.SchemaValue = ");
        self.write_encode_input_record(
            &mut method,
            &self.agent_type.constructor.input_schema,
            MULTIMODAL_INPUT_NAME,
        )?;
        self.write_config_encoding(&mut method, local_configs)?;
        self.write_create_agent_call(&mut method, config_var, "agentConfig");
        method.write_line(format!(
            "return new {class_name}(parameters, phantomId, __createResponse.agentId);"
        ));

        Ok(())
    }

    /// Generates the `newPhantomWithConfig` constructor method
    fn generate_ts_constructor_new_phantom_with_config_method(
        &self,
        writer: &mut TsWriter,
        class_name: &str,
        config_var: &str,
        local_configs: &[&AgentConfigDeclaration],
    ) -> anyhow::Result<()> {
        writer.write_doc(&format!(
            "Creates a new phantom instance of this agent with configuration\n{}",
            self.agent_type.constructor.description
        ));
        let mut method = writer.begin_static_async_method("newPhantomWithConfig");
        self.write_parameter_list(&mut method, &self.agent_type.constructor.input_schema)?;
        self.write_config_parameter_list(&mut method, local_configs)?;
        method.result(class_name);

        method.write_line("const parameters: base.SchemaValue = ");
        self.write_encode_input_record(
            &mut method,
            &self.agent_type.constructor.input_schema,
            MULTIMODAL_INPUT_NAME,
        )?;
        method.write_line("const phantomId = uuidv4();");
        self.write_config_encoding(&mut method, local_configs)?;
        self.write_create_agent_call(&mut method, config_var, "agentConfig");
        method.write_line(format!(
            "return new {class_name}(parameters, phantomId, __createResponse.agentId);"
        ));

        Ok(())
    }

    /// Writes optional config parameters to the method signature
    fn write_config_parameter_list(
        &self,
        writer: &mut TsFunctionWriter<'_>,
        local_configs: &[&AgentConfigDeclaration],
    ) -> anyhow::Result<()> {
        for config in local_configs {
            let param_name = format!(
                "config{}?",
                config
                    .path
                    .iter()
                    .map(|s| s.to_upper_camel_case())
                    .collect::<String>()
            );
            let config_schema_type = self.import_analysed_type(&config.value_type)?;
            let param_type = self.type_reference(&config_schema_type)?;
            writer.param(&param_name, &param_type);
        }
        Ok(())
    }

    /// Writes code that builds the agentConfig array from optional config params
    fn write_config_encoding(
        &self,
        writer: &mut TsFunctionWriter<'_>,
        local_configs: &[&AgentConfigDeclaration],
    ) -> anyhow::Result<()> {
        writer.write_line("const agentConfig: base.AgentConfigEntry[] = [];");
        for config in local_configs {
            let param_name = format!(
                "config{}",
                config
                    .path
                    .iter()
                    .map(|s| s.to_upper_camel_case())
                    .collect::<String>()
            );
            let path_array = config
                .path
                .iter()
                .map(|s| format!("\"{}\"", s))
                .collect::<Vec<_>>()
                .join(", ");
            let config_schema_type = self.import_analysed_type(&config.value_type)?;
            let encoded_value = self.encode_schema_value(&param_name, &config_schema_type)?;
            writer.write_line(format!("if ({param_name} !== undefined) {{"));
            writer.indent();
            writer.write_line(format!(
                "agentConfig.push({{ path: [{path_array}], value: {encoded_value} }});"
            ));
            writer.unindent();
            writer.write_line("}");
        }
        Ok(())
    }

    /// Generates a private helper method for getting the global configuration and failing if it is missing
    fn generate_ts_config_getter(&self, writer: &mut TsWriter, config_var: &str) {
        let mut get_config = writer.begin_private_method("__getConfig");
        get_config.result("base.Configuration");
        get_config.write_line(format!("if (!{config_var}) {{"));
        get_config.indent();
        get_config.write_line(format!(
            "  throw new Error(\"{} configuration is not set\");",
            self.agent_type.type_name.0
        ));
        get_config.unindent();
        get_config.write_line("}");
        get_config.write_line(format!("return {};", config_var));
    }

    /// Generates a public getter for the agent's identity
    fn generate_ts_agent_id_getter(&self, writer: &mut TsWriter) {
        writer
            .write_doc("Returns the agent's identity, containing the component ID and agent name.");
        let mut get_agent_id = writer.begin_method("get agentId");
        get_agent_id.result("base.AgentId");
        get_agent_id.write_line("return this._agentId;");
    }

    /// Generates a public static method to access the current configuration
    fn generate_ts_get_configuration(&self, writer: &mut TsWriter, config_var: &str) {
        writer.write_doc("Returns the current configuration, or throws if not configured.");
        let mut get_configuration = writer.begin_static_method("getConfiguration");
        get_configuration.result("base.Configuration");
        get_configuration.write_line(format!("if (!{config_var}) {{"));
        get_configuration.indent();
        get_configuration.write_line(format!(
            "  throw new Error(\"{} configuration is not set\");",
            self.agent_type.type_name.0
        ));
        get_configuration.unindent();
        get_configuration.write_line("}");
        get_configuration.write_line(format!("return {};", config_var));
    }

    /// Generates the remote agent methods
    fn generate_ts_remote_methods(
        &self,
        writer: &mut TsWriter,
        _class_name: &str,
    ) -> anyhow::Result<()> {
        for method_def in &self.agent_type.methods {
            self.generate_ts_remote_method(writer, method_def)?;
        }
        Ok(())
    }

    /// Generates a specific remote agent method. Agent methods are exposed the same was as agent-to-agent communication,
    /// instead of a simple method, it is an object which is callable (in that case acting as an async 'invoke and await' call),
    /// but also expose a `trigger` and a `schedule` method.
    fn generate_ts_remote_method(
        &self,
        writer: &mut TsWriter,
        method_def: &AgentMethod,
    ) -> anyhow::Result<()> {
        let get_server_config_fn = self.build_get_server_config_fn();
        let get_around_invoke_hook_fn = self.build_get_around_invoke_hook_fn();
        let get_method_request_fn = self.build_get_method_request_fn(method_def);
        let encode_args_fn = self.build_encode_args_fn(method_def)?;
        let decode_result_fn = self.build_decode_result_fn(method_def)?;

        writer.write_doc(&method_def.description);
        writer.declare_field(
            &self.to_js_ident(&method_def.name),
            &format!(
                "base.RemoteMethod<[{}], {}>",
                self.data_schema_as_type_list(&method_def.input_schema)?,
                self.data_schema_as_result_type(&method_def.output_schema)?
            ),
            Some(&formatdoc! {"
                base.createRemoteMethod(
                    {},
                    {},
                    {},
                    {},
                    {},
                )
            ",
                get_server_config_fn.trim(),
                get_around_invoke_hook_fn.trim(),
                get_method_request_fn.trim(),
                encode_args_fn.trim(),
                decode_result_fn.trim(),
            }),
        );

        Ok(())
    }

    /// Builds the function that extracts the configured server for the remote method implementation
    fn build_get_server_config_fn(&self) -> String {
        let mut get_server_config = TsAnonymousFunctionWriter::new();
        get_server_config.write_line("return this.__getConfig().server;");
        get_server_config.build()
    }

    /// Builds the function that extracts the configured around invoke hook for the remote method implementation
    fn build_get_around_invoke_hook_fn(&self) -> String {
        let mut get_server_config = TsAnonymousFunctionWriter::new();
        get_server_config.write_line("return this.__getConfig().aroundInvokeHook;");
        get_server_config.build()
    }

    /// Builds the function that constructs the base invocation request, with no method parameters set yet
    fn build_get_method_request_fn(&self, method_def: &AgentMethod) -> String {
        let mut get_method_request = TsAnonymousFunctionWriter::new();
        get_method_request.write_line("return {");
        get_method_request.indent();
        get_method_request.write_line("appName: this.__getConfig().application,");
        get_method_request.write_line("envName: this.__getConfig().environment,");
        get_method_request.write_line(format!(
            "agentTypeName: \"{}\",",
            self.agent_type.type_name.0
        ));
        get_method_request.write_line("parameters: this.parameters,");
        get_method_request.write_line("phantomId: this.phantomId,");
        get_method_request.write_line(format!("methodName: \"{}\",", method_def.name));
        get_method_request.write_line("mode: \"await\",");
        get_method_request
            .write_line("methodParameters: { kind: 'record', value: { fields: [] } }");
        get_method_request.unindent();
        get_method_request.write_line("};");
        get_method_request.build()
    }

    /// Builds the function that takes the method's parameters and encodes them into a SchemaValue,
    /// to be injected into the invocation request
    fn build_encode_args_fn(&self, method_def: &AgentMethod) -> anyhow::Result<String> {
        let mut parameter_naming = ParameterNaming::new();
        match &method_def.input_schema {
            DataSchema::Tuple(params) => {
                parameter_naming.reserve_many(
                    params
                        .elements
                        .iter()
                        .map(|param| self.to_js_ident(&param.name)),
                );
            }
            DataSchema::Multimodal(_) => parameter_naming.reserve(MULTIMODAL_INPUT_NAME),
        }

        let args_tuple_name = parameter_naming.fresh("__args");
        let multimodal_input_name = parameter_naming.fresh("__multimodalInput");
        let method_parameters_name = parameter_naming.fresh("__methodParameters");

        let mut encode_args = TsAnonymousFunctionWriter::new();
        encode_args.param(
            &args_tuple_name,
            &format!(
                "[{}]",
                self.data_schema_as_type_list(&method_def.input_schema)?
            ),
        );
        self.destructure_args_tuple(
            &mut encode_args,
            &args_tuple_name,
            &method_def.input_schema,
            &multimodal_input_name,
        )?;
        encode_args.write_line(format!(
            "const {method_parameters_name}: base.SchemaValue = "
        ));
        self.write_encode_input_record(
            &mut encode_args,
            &method_def.input_schema,
            &multimodal_input_name,
        )?;
        encode_args.write_line(format!("return {method_parameters_name};"));
        Ok(encode_args.build())
    }

    /// Builds the function that takes the invocation API's result `TypedSchemaValue` and decodes it
    /// to the function's expected return type
    fn build_decode_result_fn(&self, method_def: &AgentMethod) -> anyhow::Result<String> {
        let mut decode_result = TsAnonymousFunctionWriter::new();
        decode_result.param("result", "base.AgentInvocationResult");
        self.write_decode_output(
            &mut decode_result,
            &method_def.output_schema,
            "result.result",
        )?;
        Ok(decode_result.build())
    }

    /// Generates the global function to set the client's configuration
    fn generate_ts_configure_function(&self, writer: &mut TsWriter, config_var: &str) {
        writer.write_doc("Sets the global configuration for this agent client");
        let mut configure = writer.begin_export_function("configure");
        configure.param("config", "base.Configuration");
        configure.write_line(format!("{} = config;", config_var));
    }

    /// Generates an encode function mapping a TypeScript value of the named type
    /// to its schema-native `SchemaValue` wire form.
    fn generate_ts_schema_type_encode(
        &self,
        writer: &mut TsWriter,
        ts_name: &TypeScriptTypeName,
        typ: &SchemaType,
    ) -> anyhow::Result<()> {
        let encode_fn_name = format!("encode{ts_name}");

        let mut func = writer.begin_function(&encode_fn_name);
        func.param("value", ts_name.as_str());
        func.result("base.SchemaValue");

        // Encode the actual structure, not delegate to itself: resolve through
        // `Ref` and emit the body shape directly via the `_body` builder, which
        // skips the named-type lookup that would otherwise map back here.
        let inner_typ = self.resolve_ref(typ);
        let body = self.encode_schema_value_body("value", inner_typ)?;
        func.write_line(format!("return {body};"));

        Ok(())
    }

    /// Generates a decode function mapping a schema-native `SchemaValue` wire
    /// value back to a TypeScript value of the named type.
    fn generate_ts_schema_type_decode(
        &self,
        writer: &mut TsWriter,
        ts_name: &TypeScriptTypeName,
        typ: &SchemaType,
    ) -> anyhow::Result<()> {
        let decode_fn_name = format!("decode{ts_name}");

        let mut func = writer.begin_function(&decode_fn_name);
        func.param("value", "base.SchemaValue");
        func.result(ts_name.as_str());

        // Decode the actual structure, not delegate to itself: resolve through
        // `Ref` and emit the body shape directly via the `_body` builder, which
        // skips the named-type lookup that would otherwise map back here.
        let inner_typ = self.resolve_ref(typ);
        let body = self.decode_schema_value_body("value", inner_typ)?;
        func.write_line(format!("return {body};"));

        Ok(())
    }

    /// Writes an exported type definition
    fn generate_ts_schema_type_def(
        &self,
        writer: &mut TsWriter,
        ts_name: &TypeScriptTypeName,
        typ: &SchemaType,
    ) -> anyhow::Result<()> {
        let def = self.type_definition(typ)?;
        writer.export_type(ts_name, &def);
        Ok(())
    }

    /// Decodes one element value (component-model / text / binary) from the
    /// `SchemaValue` expression `value_expr` into a TS value expression.
    /// `parameter_name` is used for text/binary restriction error messages.
    fn decode_element_value(
        &self,
        schema: &ElementSchema,
        value_expr: &str,
        parameter_name: &str,
    ) -> anyhow::Result<String> {
        Ok(match schema {
            ElementSchema::ComponentModel(component_model) => {
                let element_schema_type =
                    self.import_analysed_type(&component_model.element_type)?;
                self.decode_schema_value(value_expr, &element_schema_type)?
            }
            ElementSchema::UnstructuredText(descriptor) => {
                format!(
                    "base.UnstructuredText.fromSchemaValue('{parameter_name}', {value_expr}, [{}])",
                    Self::text_restriction_codes(descriptor)
                )
            }
            ElementSchema::UnstructuredBinary(descriptor) => {
                format!(
                    "base.UnstructuredBinary.fromSchemaValue('{parameter_name}', {value_expr}, [{}])",
                    Self::binary_restriction_mimes(descriptor)
                )
            }
        })
    }

    /// Writes a `return <list>.value.elements.map(...)` statement reconstructing
    /// a multimodal TS array from a `list<variant<…>>` `SchemaValue` referenced
    /// by `list_expr` (variant case index → element name).
    fn write_decode_multimodal_list<W: FunctionWriter>(
        &self,
        writer: &mut W,
        multimodal: &NamedElementSchemas,
        list_expr: &str,
    ) -> anyhow::Result<()> {
        writer.write_line(format!("if ({list_expr}.kind !== 'list') {{"));
        writer.indent();
        writer.write_line(format!(
            "throw new Error(`Invalid value. Expected a multimodal list value, got ${{{list_expr}.kind}}`);"
        ));
        writer.unindent();
        writer.write_line("}");
        writer.write_line(format!(
            "return {list_expr}.value.elements.map((item: any) => {{"
        ));
        writer.indent();
        for (idx, element) in multimodal.elements.iter().enumerate() {
            let if_or_else = if idx == 0 { "if" } else { "else if" };
            writer.write_line(format!("{if_or_else} (item.value.case === {idx}) {{"));
            writer.indent();
            let decoded =
                self.decode_element_value(&element.schema, "item.value.payload", &element.name)?;
            writer.write_line(format!(
                "return {{ type: '{}', value: {decoded} }};",
                element.name
            ));
            writer.unindent();
            writer.write_line("}");
        }
        writer.write_line("throw new Error(`Unknown multimodal case index: ${item.value.case}`);");
        writer.unindent();
        writer.write_line("});");
        Ok(())
    }

    /// Writes a `return` statement that decodes the method's output
    /// `TypedSchemaValue` (`{ value: SchemaValue }`, referenced by `typed_expr`)
    /// into the TS return value. The output wire is the bare value the server
    /// pairs with the method output schema: a single element inline, a
    /// multimodal `list<variant<…>>`, or the empty tuple (unit).
    fn write_decode_output<W: FunctionWriter>(
        &self,
        writer: &mut W,
        schema: &DataSchema,
        typed_expr: &str,
    ) -> anyhow::Result<()> {
        match schema {
            DataSchema::Tuple(params) if params.elements.is_empty() => {
                writer.write_line("return;");
                Ok(())
            }
            _ => {
                writer.write_line(format!("const __out = {typed_expr};"));
                writer.write_line("if (!__out) {");
                writer.indent();
                writer.write_line("throw new Error('Invalid result value: missing result value');");
                writer.unindent();
                writer.write_line("}");
                writer.write_line("const __outValue: base.SchemaValue = __out.value;");
                match schema {
                    DataSchema::Tuple(params) if params.elements.len() == 1 => {
                        let param = &params.elements[0];
                        let decoded =
                            self.decode_element_value(&param.schema, "__outValue", "result")?;
                        writer.write_line(format!("return {decoded};"));
                    }
                    DataSchema::Tuple(params) => {
                        writer.write_line("if (__outValue.kind !== 'tuple') {");
                        writer.indent();
                        writer.write_line(
                            "throw new Error(`Invalid result value. Expected a tuple value, got ${__outValue.kind}`);",
                        );
                        writer.unindent();
                        writer.write_line("}");
                        writer.write_line("return [");
                        writer.indent();
                        for (idx, param) in params.elements.iter().enumerate() {
                            let elem = format!("__outValue.value.elements[{idx}]");
                            let decoded =
                                self.decode_element_value(&param.schema, &elem, &param.name)?;
                            writer.write_line(format!("{decoded},"));
                        }
                        writer.unindent();
                        writer.write_line("];");
                    }
                    DataSchema::Multimodal(multimodal) => {
                        self.write_decode_multimodal_list(writer, multimodal, "__outValue")?;
                    }
                }
                Ok(())
            }
        }
    }

    /// Writes a `return` statement that decodes the input `record` `SchemaValue`
    /// (referenced by `value_expr`) back into the TS argument list. Only used by
    /// the generated test harness (the inverse of `write_encode_input_record`).
    fn write_decode_input<W: FunctionWriter>(
        &self,
        writer: &mut W,
        schema: &DataSchema,
        value_expr: &str,
    ) -> anyhow::Result<()> {
        writer.write_line(format!("const __rec: base.SchemaValue = {value_expr};"));
        writer.write_line("if (__rec.kind !== 'record') {");
        writer.indent();
        writer.write_line(
            "throw new Error(`Invalid input value. Expected a record value, got ${__rec.kind}`);",
        );
        writer.unindent();
        writer.write_line("}");
        match schema {
            DataSchema::Tuple(params) => {
                writer.write_line("return [");
                writer.indent();
                for (idx, param) in params.elements.iter().enumerate() {
                    let elem = format!("__rec.value.fields[{idx}]");
                    let decoded = self.decode_element_value(&param.schema, &elem, &param.name)?;
                    writer.write_line(format!("{decoded},"));
                }
                writer.unindent();
                writer.write_line("];");
            }
            DataSchema::Multimodal(multimodal) => {
                writer.write_line("const __parts: base.SchemaValue = __rec.value.fields[0];");
                self.write_decode_multimodal_list(writer, multimodal, "__parts")?;
            }
        }
        Ok(())
    }

    /// Destructures the function arguments coming in `tuple` as a TypeScript tuple
    fn destructure_args_tuple<Target: FunctionWriter>(
        &self,
        writer: &mut Target,
        tuple: &str,
        schema: &DataSchema,
        multimodal_input_name: &str,
    ) -> anyhow::Result<()> {
        match schema {
            DataSchema::Tuple(params) => {
                let param_names: Vec<String> = params
                    .elements
                    .iter()
                    .map(|param| self.to_js_ident(&param.name))
                    .collect();
                writer.write_line(format!("const [{}] = {};", param_names.join(", "), tuple));
                Ok(())
            }
            DataSchema::Multimodal(_) => {
                // For multimodal input, extract the array from the args tuple
                writer.write_line(format!("const {multimodal_input_name} = {tuple}[0];"));
                Ok(())
            }
        }
    }

    /// Comma-separated, single-quoted list of allowed language codes for a
    /// text descriptor (empty when unrestricted).
    fn text_restriction_codes(descriptor: &TextDescriptor) -> String {
        descriptor
            .restrictions
            .as_ref()
            .map_or(String::new(), |restrictions| {
                restrictions
                    .iter()
                    .map(|tt| format!("'{}'", tt.language_code))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
    }

    /// Comma-separated, single-quoted list of allowed mime types for a binary
    /// descriptor (empty when unrestricted).
    fn binary_restriction_mimes(descriptor: &BinaryDescriptor) -> String {
        descriptor
            .restrictions
            .as_ref()
            .map_or(String::new(), |restrictions| {
                restrictions
                    .iter()
                    .map(|bt| format!("'{}'", bt.mime_type))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
    }

    /// Encodes one element value (component-model / text / binary) into a
    /// schema-native `SchemaValue` expression, given the TS variable holding
    /// the value.
    fn encode_element_value(
        &self,
        schema: &ElementSchema,
        value_var: &str,
    ) -> anyhow::Result<String> {
        Ok(match schema {
            ElementSchema::ComponentModel(component_model) => {
                let element_schema_type =
                    self.import_analysed_type(&component_model.element_type)?;
                self.encode_schema_value(value_var, &element_schema_type)?
            }
            ElementSchema::UnstructuredText(_) => {
                format!("base.UnstructuredText.toSchemaValue({value_var})")
            }
            ElementSchema::UnstructuredBinary(_) => {
                format!("base.UnstructuredBinary.toSchemaValue({value_var})")
            }
        })
    }

    /// Writes the `{ kind: 'list', value: { elements: <input>.map(...) } }`
    /// expression for a multimodal value, mapping each TS `{ type, value }`
    /// element to a `variant` `SchemaValue` whose case index is the element's
    /// position in the schema (matching the server's `parts` `list<variant<…>>`).
    /// `terminator` is appended after the closing braces (`,` inside a field
    /// list, `;` as a statement value).
    fn write_multimodal_list_expr<Target: FunctionWriter>(
        &self,
        writer: &mut Target,
        multimodal: &NamedElementSchemas,
        multimodal_input_name: &str,
        terminator: &str,
    ) -> anyhow::Result<()> {
        writer.write_line(format!(
            "{{ kind: 'list', value: {{ elements: {multimodal_input_name}.map((item: any) => {{"
        ));
        writer.indent();
        for (idx, element) in multimodal.elements.iter().enumerate() {
            let if_or_else = if idx == 0 { "if" } else { "else if" };
            writer.write_line(format!(
                "{if_or_else} (item.type === '{}') {{",
                element.name
            ));
            writer.indent();
            let payload_expr = self.encode_element_value(&element.schema, "item.value")?;
            writer.write_line(format!(
                "return {{ kind: 'variant', value: {{ case: {idx}, payload: {payload_expr} }} }};"
            ));
            writer.unindent();
            writer.write_line("}");
        }
        writer.write_line("throw new Error(`Unknown multimodal type: ${item.type}`);");
        writer.unindent();
        writer.write_line(format!("}}) }} }}{terminator}"));
        Ok(())
    }

    /// Encodes the declared input parameters into a schema-native `record`
    /// `SchemaValue` whose fields are the parameters in declaration order, as
    /// expected by the server's `json_input_schema_value_to_typed_schema_value`.
    /// Multimodal input is a single `parts` field of type `list<variant<…>>`.
    fn write_encode_input_record<Target: FunctionWriter>(
        &self,
        writer: &mut Target,
        schema: &DataSchema,
        multimodal_input_name: &str,
    ) -> anyhow::Result<()> {
        writer.indent();
        writer.write_line("{ kind: 'record', value: { fields: [");
        writer.indent();
        match schema {
            DataSchema::Tuple(params) => {
                for param in &params.elements {
                    let param_name = self.to_js_ident(&param.name);
                    let field_expr = self.encode_element_value(&param.schema, &param_name)?;
                    writer.write_line(format!("{field_expr},"));
                }
            }
            DataSchema::Multimodal(multimodal) => {
                self.write_multimodal_list_expr(writer, multimodal, multimodal_input_name, ",")?;
            }
        }
        writer.unindent();
        writer.write_line("] } };");
        writer.unindent();
        Ok(())
    }

    /// Encodes a method's return value into the bare output `SchemaValue` that
    /// the server pairs with the method's output schema (single element inline,
    /// multimodal `list<variant<…>>`, or the canonical empty tuple). Only used
    /// by the generated test harness; the unit case is handled by the caller.
    fn write_encode_output_value<Target: FunctionWriter>(
        &self,
        writer: &mut Target,
        schema: &DataSchema,
        multimodal_input_name: &str,
    ) -> anyhow::Result<()> {
        writer.indent();
        match schema {
            DataSchema::Tuple(params) => {
                if params.elements.is_empty() {
                    writer.write_line("{ kind: 'tuple', value: { elements: [] } };");
                } else if params.elements.len() == 1 {
                    let param = &params.elements[0];
                    let param_name = self.to_js_ident(&param.name);
                    let value_expr = self.encode_element_value(&param.schema, &param_name)?;
                    writer.write_line(format!("{value_expr};"));
                } else {
                    writer.write_line("{ kind: 'tuple', value: { elements: [");
                    writer.indent();
                    for param in &params.elements {
                        let param_name = self.to_js_ident(&param.name);
                        let value_expr = self.encode_element_value(&param.schema, &param_name)?;
                        writer.write_line(format!("{value_expr},"));
                    }
                    writer.unindent();
                    writer.write_line("] } };");
                }
            }
            DataSchema::Multimodal(multimodal) => {
                self.write_multimodal_list_expr(writer, multimodal, multimodal_input_name, ";")?;
            }
        }
        writer.unindent();
        Ok(())
    }

    /// Decodes a schema-native `SchemaValue` wire value (`value`) into a TS
    /// value of the given [`SchemaType`]. Named types delegate to their
    /// generated `decode<Name>` function; everything else is decoded inline.
    fn decode_schema_value(&self, value: &str, typ: &SchemaType) -> anyhow::Result<String> {
        if let Some(name) = self.type_naming.type_name_for_type(typ) {
            return Ok(format!("decode{}({})", name, value));
        }
        self.decode_schema_value_body(value, typ)
    }

    /// Inline schema-native decode for a single [`SchemaType`], without the
    /// named-type lookup. `value` is a `SchemaValue` wire-node expression
    /// (`{ kind, value }`); the result is a TS value expression.
    fn decode_schema_value_body(&self, value: &str, typ: &SchemaType) -> anyhow::Result<String> {
        let rendered = match typ {
            SchemaType::String { .. } | SchemaType::Char { .. } => {
                format!("((n: any) => n.value as string)({value})")
            }
            SchemaType::F64 { .. }
            | SchemaType::F32 { .. }
            | SchemaType::U64 { .. }
            | SchemaType::S64 { .. }
            | SchemaType::U32 { .. }
            | SchemaType::S32 { .. }
            | SchemaType::U16 { .. }
            | SchemaType::S16 { .. }
            | SchemaType::U8 { .. }
            | SchemaType::S8 { .. } => {
                format!("((n: any) => n.value as number)({value})")
            }
            SchemaType::Bool { .. } => {
                format!("((n: any) => n.value as boolean)({value})")
            }
            SchemaType::Option { inner, .. } => {
                let inner_decode = self.decode_schema_value("item", inner)?;
                format!("base.decodeOption({value}, (item) => ({inner_decode}))")
            }
            SchemaType::List { element, .. } => {
                // Special handling for lists of u8 which are Uint8Array
                if matches!(**element, SchemaType::U8 { .. }) {
                    format!(
                        "((n: any) => new Uint8Array(n.value.elements.map((e: any) => e.value as number)))({value})"
                    )
                } else {
                    let inner_decode = self.decode_schema_value("item", element)?;
                    format!(
                        "((n: any) => n.value.elements.map((item: any) => ({inner_decode})))({value})"
                    )
                }
            }
            SchemaType::Enum { cases, .. } => {
                let cases_array = cases
                    .iter()
                    .map(|case| format!("\"{}\"", case))
                    .collect::<Vec<_>>()
                    .join(", ");
                let cases_union = cases
                    .iter()
                    .map(|case| format!("\"{}\"", case))
                    .collect::<Vec<_>>()
                    .join(" | ");
                format!(
                    "((n: any) => {{ const __cases = [{cases_array}]; const __i = n.value.case; if (__i < 0 || __i >= __cases.length) {{ throw new Error(`Invalid enum case index ${{__i}}`); }} return __cases[__i] as ({cases_union}); }})({value})"
                )
            }
            SchemaType::Flags { flags, .. } => {
                // Wire form is a positional `bits` boolean array; `base.decodeFlags`
                // maps it onto the JS-cased fields of the `initial` shape (every
                // field starts `false`) using the declaration-ordered pairs.
                let flag_initializers = flags
                    .iter()
                    .map(|name| format!("{}: false", self.to_js_ident(name)))
                    .collect::<Vec<_>>()
                    .join(", ");
                let flag_pairs = flags
                    .iter()
                    .map(|name| format!("['{}', '{}']", name, self.to_js_ident(name)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("base.decodeFlags({value}, {{ {flag_initializers} }}, [{flag_pairs}])")
            }
            SchemaType::Tuple { elements, .. } => {
                let items: Vec<String> = elements
                    .iter()
                    .enumerate()
                    .map(|(idx, item_type)| {
                        self.decode_schema_value(&format!("n.value.elements[{idx}]"), item_type)
                    })
                    .collect::<anyhow::Result<_>>()?;
                format!("((n: any) => [{}])({value})", items.join(", "))
            }
            SchemaType::Record { fields, .. } => {
                let field_decoders: Vec<String> = fields
                    .iter()
                    .enumerate()
                    .map(|(idx, field)| {
                        let js_field_name = self.to_js_ident(&field.name);
                        let field_decode = self
                            .decode_schema_value(&format!("n.value.fields[{idx}]"), &field.body)?;
                        Ok::<_, anyhow::Error>(format!("{js_field_name}: {field_decode}"))
                    })
                    .collect::<anyhow::Result<_>>()?;
                format!(
                    "((n: any) => ({{ {} }}))({value})",
                    field_decoders.join(", ")
                )
            }
            SchemaType::Variant { cases, .. } => {
                let arms = cases
                    .iter()
                    .enumerate()
                    .map(|(idx, case)| match &case.payload {
                        Some(case_type) => {
                            let value_decode =
                                self.decode_schema_value("n.value.payload", case_type)?;
                            Ok::<_, anyhow::Error>(format!(
                                "if (n.value.case === {idx}) {{ return {{ tag: '{}', val: {value_decode} }}; }}",
                                case.name
                            ))
                        }
                        None => Ok(format!(
                            "if (n.value.case === {idx}) {{ return {{ tag: '{}' }}; }}",
                            case.name
                        )),
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?
                    .join(" ");
                format!(
                    "((n: any) => {{ {arms} throw new Error(`Unknown variant case index ${{n.value.case}}`); }})({value})"
                )
            }
            SchemaType::Result { spec, .. } => {
                let ok_expr = match spec.ok.as_deref() {
                    Some(ok_type) => {
                        let decoded = self.decode_schema_value("n.value.value", ok_type)?;
                        format!("{{ ok: {decoded} }}")
                    }
                    None => "{ ok: undefined }".to_string(),
                };
                let err_expr = match spec.err.as_deref() {
                    Some(err_type) => {
                        let decoded = self.decode_schema_value("n.value.value", err_type)?;
                        format!("{{ err: {decoded} }}")
                    }
                    None => "{ err: undefined }".to_string(),
                };
                format!("((n: any) => n.value.tag === 'ok' ? {ok_expr} : {err_expr})({value})")
            }
            SchemaType::Ref { .. } => {
                // The named-type ref should have already been resolved via
                // `type_name_for_type` in `decode_schema_value`; reaching here
                // means a Ref slipped through without a registered name.
                anyhow::bail!(
                    "Unresolved SchemaType::Ref reached decode_schema_value_body; \
                         missing name in type_naming. value expr = {value}"
                );
            }
            // Rich schema variants without a legacy AnalysedType counterpart
            // cannot round-trip through the current `IntoValue` / `FromValue`
            // SDK contract.
            SchemaType::FixedList { .. }
            | SchemaType::Map { .. }
            | SchemaType::Text { .. }
            | SchemaType::Binary { .. }
            | SchemaType::Path { .. }
            | SchemaType::Url { .. }
            | SchemaType::Datetime { .. }
            | SchemaType::Duration { .. }
            | SchemaType::Quantity { .. }
            | SchemaType::Union { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => {
                anyhow::bail!(
                    "Rich SchemaType variant has no legacy AnalysedType \
                         decoding for the TypeScript bridge; type = {typ:?}"
                );
            }
        };
        Ok(rendered)
    }

    /// Encodes a TS value of the given [`SchemaType`] into its schema-native
    /// `SchemaValue` wire form. Named types delegate to their generated
    /// `encode<Name>` function; everything else is encoded inline.
    fn encode_schema_value(&self, value: &str, typ: &SchemaType) -> anyhow::Result<String> {
        if let Some(name) = self.type_naming.type_name_for_type(typ) {
            return Ok(format!("encode{}({})", name, value));
        }
        self.encode_schema_value_body(value, typ)
    }

    /// Inline schema-native encode for a single [`SchemaType`], without the
    /// named-type lookup. `value` is a TS value expression; the result is a
    /// `SchemaValue` wire-node expression (`{ kind, value }`).
    fn encode_schema_value_body(&self, value: &str, typ: &SchemaType) -> anyhow::Result<String> {
        let rendered = match typ {
            SchemaType::Bool { .. } => format!("{{ kind: 'bool', value: {value} }}"),
            SchemaType::S8 { .. } => format!("{{ kind: 's8', value: {value} }}"),
            SchemaType::S16 { .. } => format!("{{ kind: 's16', value: {value} }}"),
            SchemaType::S32 { .. } => format!("{{ kind: 's32', value: {value} }}"),
            SchemaType::S64 { .. } => format!("{{ kind: 's64', value: {value} }}"),
            SchemaType::U8 { .. } => format!("{{ kind: 'u8', value: {value} }}"),
            SchemaType::U16 { .. } => format!("{{ kind: 'u16', value: {value} }}"),
            SchemaType::U32 { .. } => format!("{{ kind: 'u32', value: {value} }}"),
            SchemaType::U64 { .. } => format!("{{ kind: 'u64', value: {value} }}"),
            SchemaType::F32 { .. } => format!("{{ kind: 'f32', value: {value} }}"),
            SchemaType::F64 { .. } => format!("{{ kind: 'f64', value: {value} }}"),
            SchemaType::Char { .. } => format!("{{ kind: 'char', value: {value} }}"),
            SchemaType::String { .. } => format!("{{ kind: 'string', value: {value} }}"),
            SchemaType::Option { inner, .. } => {
                let inner_encode = self.encode_schema_value("item", inner)?;
                format!("base.encodeOption({value}, (item) => ({inner_encode}))")
            }
            SchemaType::List { element, .. } => {
                // `Array.from` handles both plain arrays and the `Uint8Array`
                // surface used for `list<u8>`.
                let inner_encode = self.encode_schema_value("item", element)?;
                format!(
                    "{{ kind: 'list', value: {{ elements: Array.from({value} as Iterable<any>).map((item: any) => ({inner_encode})) }} }}"
                )
            }
            SchemaType::Enum { cases, .. } => {
                let cases_array = cases
                    .iter()
                    .map(|case| format!("\"{}\"", case))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "((v: any) => {{ const __i = [{cases_array}].indexOf(v); if (__i < 0) {{ throw new Error(`Invalid enum value ${{v}}`); }} return {{ kind: 'enum', value: {{ case: __i }} }}; }})({value})"
                )
            }
            SchemaType::Flags { flags, .. } => {
                // Wire form is a positional `bits` boolean array aligned with the
                // declared flag names; `base.encodeFlags` reads the JS-cased
                // fields in declaration order.
                let flag_pairs = flags
                    .iter()
                    .map(|name| format!("['{}', '{}']", name, self.to_js_ident(name)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("base.encodeFlags({value} as Record<string, boolean>, [{flag_pairs}])")
            }
            SchemaType::Tuple { elements, .. } => {
                let items: Vec<String> = elements
                    .iter()
                    .enumerate()
                    .map(|(idx, item_type)| {
                        self.encode_schema_value(&format!("{value}[{idx}]"), item_type)
                    })
                    .collect::<anyhow::Result<_>>()?;
                format!(
                    "{{ kind: 'tuple', value: {{ elements: [{}] }} }}",
                    items.join(", ")
                )
            }
            SchemaType::Record { fields, .. } => {
                let items: Vec<String> = fields
                    .iter()
                    .map(|field| {
                        let js_field_name = self.to_js_ident(&field.name);
                        self.encode_schema_value(&format!("{value}.{js_field_name}"), &field.body)
                    })
                    .collect::<anyhow::Result<_>>()?;
                format!(
                    "{{ kind: 'record', value: {{ fields: [{}] }} }}",
                    items.join(", ")
                )
            }
            SchemaType::Variant { cases, .. } => {
                let arms = cases
                    .iter()
                    .enumerate()
                    .map(|(idx, case)| match &case.payload {
                        Some(case_type) => {
                            let encoded = self.encode_schema_value("v.val", case_type)?;
                            Ok::<_, anyhow::Error>(format!(
                                "if (v.tag === '{}') {{ return {{ kind: 'variant', value: {{ case: {idx}, payload: {encoded} }} }}; }}",
                                case.name
                            ))
                        }
                        None => Ok(format!(
                            "if (v.tag === '{}') {{ return {{ kind: 'variant', value: {{ case: {idx} }} }}; }}",
                            case.name
                        )),
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?
                    .join(" ");
                format!(
                    "((v: any) => {{ {arms} throw new Error(`Unknown variant case ${{v.tag}}`); }})({value})"
                )
            }
            SchemaType::Result { spec, .. } => {
                // Discriminator MUST be `'ok' in v`. `v.ok !== undefined` would
                // route `{ ok: undefined }` (a valid `Ok(())` / `Ok(None)`
                // payload) to the err branch.
                let ok_expr = match spec.ok.as_deref() {
                    Some(ok_type) => {
                        let encoded = self.encode_schema_value("(v as any).ok", ok_type)?;
                        format!("{{ tag: 'ok', value: {encoded} }}")
                    }
                    None => "{ tag: 'ok' }".to_string(),
                };
                let err_expr = match spec.err.as_deref() {
                    Some(err_type) => {
                        let encoded = self.encode_schema_value("(v as any).err", err_type)?;
                        format!("{{ tag: 'err', value: {encoded} }}")
                    }
                    None => "{ tag: 'err' }".to_string(),
                };
                format!(
                    "((v: any) => ({{ kind: 'result', value: ('ok' in v) ? {ok_expr} : {err_expr} }}))({value})"
                )
            }
            SchemaType::Ref { .. } => {
                // Refs are resolved via the type_naming lookup in
                // `encode_schema_value`; reaching here means a Ref slipped
                // through without a registered name.
                anyhow::bail!(
                    "Unresolved SchemaType::Ref reached encode_schema_value_body; \
                         missing name in type_naming. value expr = {value}"
                );
            }
            // Rich schema variants without a legacy AnalysedType counterpart
            // cannot round-trip through the current `IntoValue` / `FromValue`
            // SDK contract.
            SchemaType::FixedList { .. }
            | SchemaType::Map { .. }
            | SchemaType::Text { .. }
            | SchemaType::Binary { .. }
            | SchemaType::Path { .. }
            | SchemaType::Url { .. }
            | SchemaType::Datetime { .. }
            | SchemaType::Duration { .. }
            | SchemaType::Quantity { .. }
            | SchemaType::Union { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => {
                anyhow::bail!(
                    "Rich SchemaType variant has no legacy AnalysedType \
                         encoding for the TypeScript bridge; type = {typ:?}"
                );
            }
        };
        Ok(rendered)
    }

    fn unstructured_text_type(descriptor: &TextDescriptor) -> String {
        if let Some(restrictions) = &descriptor.restrictions {
            format!(
                "base.UnstructuredText<[{}]>",
                restrictions
                    .iter()
                    .map(|tt| { format!("'{}'", tt.language_code) })
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else {
            "base.UnstructuredText".to_string()
        }
    }

    fn unstructured_binary_type(descriptor: &BinaryDescriptor) -> String {
        if let Some(restrictions) = &descriptor.restrictions {
            format!(
                "base.UnstructuredBinary<[{}]>",
                restrictions
                    .iter()
                    .map(|bt| { format!("'{}'", bt.mime_type) })
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else {
            "base.UnstructuredBinary".to_string()
        }
    }

    fn data_schema_as_type_list(&self, schema: &DataSchema) -> anyhow::Result<String> {
        Ok(match schema {
            DataSchema::Tuple(params) => params
                .elements
                .iter()
                .map(|elem| self.named_element_schema_as_type(elem))
                .collect::<Result<Vec<_>, _>>()?
                .join(", "),
            DataSchema::Multimodal(multimodal) => {
                let union_types = multimodal
                    .elements
                    .iter()
                    .map(|elem| self.named_element_schema_as_type(elem))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(" | ");
                format!("({})[]", union_types)
            }
        })
    }

    fn named_element_schema_as_type(&self, schema: &NamedElementSchema) -> anyhow::Result<String> {
        self.element_schema_as_type(&schema.schema)
    }

    fn named_element_schemas_as_type(
        &self,
        schemas: &NamedElementSchemas,
    ) -> anyhow::Result<String> {
        Ok(schemas
            .elements
            .iter()
            .map(|element| {
                Ok::<String, anyhow::Error>(format!(
                    "{{ type: '{}', value: {} }}",
                    element.name,
                    self.named_element_schema_as_type(element)?
                ))
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(" | "))
    }

    fn element_schema_as_type(&self, schema: &ElementSchema) -> anyhow::Result<String> {
        Ok(match schema {
            ElementSchema::ComponentModel(component_model) => {
                let element_schema_type =
                    self.import_analysed_type(&component_model.element_type)?;
                self.type_reference(&element_schema_type)?
            }
            ElementSchema::UnstructuredText(descriptor) => Self::unstructured_text_type(descriptor),
            ElementSchema::UnstructuredBinary(descriptor) => {
                Self::unstructured_binary_type(descriptor)
            }
        })
    }

    fn write_parameter_name_list(&self, writer: &mut TsFunctionWriter<'_>, schema: &DataSchema) {
        match schema {
            DataSchema::Tuple(params) => {
                let param_names: Vec<String> = params
                    .elements
                    .iter()
                    .map(|param| self.to_js_ident(&param.name))
                    .collect();
                writer.write(param_names.join(", "));
            }
            DataSchema::Multimodal(_) => {
                writer.write(MULTIMODAL_INPUT_NAME);
            }
        }
    }

    fn write_parameter_list(
        &self,
        writer: &mut TsFunctionWriter<'_>,
        schema: &DataSchema,
    ) -> anyhow::Result<()> {
        match schema {
            DataSchema::Tuple(params) => {
                for param in &params.elements {
                    let param_name = self.to_js_ident(&param.name);
                    match &param.schema {
                        ElementSchema::ComponentModel(component_model) => {
                            let element_schema_type =
                                self.import_analysed_type(&component_model.element_type)?;
                            writer.param(&param_name, &self.type_reference(&element_schema_type)?)
                        }
                        ElementSchema::UnstructuredText(descriptor) => {
                            writer.param(&param_name, &Self::unstructured_text_type(descriptor));
                        }
                        ElementSchema::UnstructuredBinary(descriptor) => {
                            writer.param(&param_name, &Self::unstructured_binary_type(descriptor));
                        }
                    }
                }
                Ok(())
            }
            DataSchema::Multimodal(multimodal) => {
                writer.param(
                    MULTIMODAL_INPUT_NAME,
                    &format!("({})[]", self.named_element_schemas_as_type(multimodal)?),
                );
                Ok(())
            }
        }
    }

    fn data_schema_as_result_type(&self, schema: &DataSchema) -> anyhow::Result<String> {
        Ok(match schema {
            DataSchema::Tuple(params) => {
                if params.elements.is_empty() {
                    "void".to_string()
                } else if params.elements.len() == 1 {
                    self.named_element_schema_as_type(&params.elements[0])?
                } else {
                    let types = params
                        .elements
                        .iter()
                        .map(|elem| self.named_element_schema_as_type(elem))
                        .collect::<Result<Vec<_>, _>>()?
                        .join(", ");
                    format!("[{}]", types)
                }
            }
            DataSchema::Multimodal(multimodal) => {
                format!("({})[]", self.named_element_schemas_as_type(multimodal)?)
            }
        })
    }

    fn type_reference(&self, typ: &SchemaType) -> anyhow::Result<String> {
        match self.type_naming.type_name_for_type(typ) {
            Some(name) => Ok(name.to_string()),
            None => {
                match typ {
                    SchemaType::String { .. } => Ok("string".to_string()),
                    SchemaType::Char { .. } => Ok("string".to_string()),
                    SchemaType::F64 { .. } => Ok("number".to_string()),
                    SchemaType::F32 { .. } => Ok("number".to_string()),
                    SchemaType::U64 { .. } => Ok("number".to_string()),
                    SchemaType::S64 { .. } => Ok("number".to_string()),
                    SchemaType::U32 { .. } => Ok("number".to_string()),
                    SchemaType::S32 { .. } => Ok("number".to_string()),
                    SchemaType::U16 { .. } => Ok("number".to_string()),
                    SchemaType::S16 { .. } => Ok("number".to_string()),
                    SchemaType::U8 { .. } => Ok("number".to_string()),
                    SchemaType::S8 { .. } => Ok("number".to_string()),
                    SchemaType::Bool { .. } => Ok("boolean".to_string()),
                    SchemaType::Option { inner, .. } => {
                        let inner_ts_type = self.type_reference(inner)?;
                        Ok(format!("{} | undefined", inner_ts_type)) // TODO: use ? in parameter and field positions
                    }
                    SchemaType::List { element, .. } => {
                        if matches!(**element, SchemaType::U8 { .. }) {
                            Ok("Uint8Array".to_string())
                        } else {
                            let inner_ts_type = self.type_reference(element)?;
                            Ok(format!("{}[]", inner_ts_type))
                        }
                    }
                    SchemaType::Tuple { elements, .. } => {
                        let types: Vec<String> = elements
                            .iter()
                            .map(|item| self.type_reference(item))
                            .collect::<Result<_, _>>()?;
                        Ok(format!("[{}]", types.join(", ")))
                    }
                    SchemaType::Result { spec, .. } => {
                        let ok_type = spec
                            .ok
                            .as_deref()
                            .map(|t| self.type_reference(t))
                            .transpose()?
                            .unwrap_or("void".to_string());
                        let err_type = spec
                            .err
                            .as_deref()
                            .map(|t| self.type_reference(t))
                            .transpose()?
                            .unwrap_or("void".to_string());
                        Ok(format!("base.JsonResult<{ok_type}, {err_type}>"))
                    }
                    // Named-composite refs resolve via [`type_name_for_type`]
                    // above; reaching this arm means the type_naming pass
                    // did not register a name for an anonymous composite.
                    // Fall through to an inline `type_definition`.
                    SchemaType::Variant { .. }
                    | SchemaType::Enum { .. }
                    | SchemaType::Flags { .. }
                    | SchemaType::Record { .. } => self.type_definition(typ),
                    SchemaType::Ref { .. } => self.type_definition(typ),
                    // Rich schema variants without a legacy AnalysedType
                    // counterpart have no TS surface in the current SDK
                    // template.
                    SchemaType::FixedList { .. }
                    | SchemaType::Map { .. }
                    | SchemaType::Text { .. }
                    | SchemaType::Binary { .. }
                    | SchemaType::Path { .. }
                    | SchemaType::Url { .. }
                    | SchemaType::Datetime { .. }
                    | SchemaType::Duration { .. }
                    | SchemaType::Quantity { .. }
                    | SchemaType::Union { .. }
                    | SchemaType::Secret { .. }
                    | SchemaType::QuotaToken { .. }
                    | SchemaType::Future { .. }
                    | SchemaType::Stream { .. } => Err(anyhow!(
                        "Cannot emit TypeScript type reference for unsupported schema variant: {typ:?}"
                    )),
                }
            }
        }
    }

    fn type_definition(&self, typ: &SchemaType) -> anyhow::Result<String> {
        // Resolve through `Ref` so the body shape drives the type definition.
        let resolved = self.resolve_ref(typ);
        match resolved {
            SchemaType::Variant { cases, .. } => {
                let mut case_defs = Vec::new();
                for case in cases {
                    let case_name = &case.name;
                    match &case.payload {
                        Some(ty) => {
                            let case_type = self.type_reference(ty)?;
                            case_defs
                                .push(format!("{{\n  tag: '{case_name}'\n  val: {case_type}\n}}"));
                        }
                        None => {
                            // No type means it's a unit variant
                            case_defs.push(format!("{{\n  tag: '{case_name}'\n}}"));
                        }
                    }
                }
                let cases = format!("\n{}", case_defs.join(" |\n"));
                Ok(cases)
            }
            SchemaType::Result { spec, .. } => {
                let ok_type = spec
                    .ok
                    .as_deref()
                    .map(|t| self.type_reference(t))
                    .transpose()?
                    .unwrap_or("void".to_string());
                let err_type = spec
                    .err
                    .as_deref()
                    .map(|t| self.type_reference(t))
                    .transpose()?
                    .unwrap_or("void".to_string());
                Ok(format!("base.JsonResult<{ok_type}, {err_type}>")) // TODO: convert to a more convenient result type
            }
            SchemaType::Option { inner, .. } => {
                let inner_ts_type = self.type_reference(inner)?;
                Ok(format!("{} | undefined", inner_ts_type))
            }
            SchemaType::Enum { cases, .. } => {
                let cases = cases
                    .iter()
                    .map(|case| format!("\"{}\"", case))
                    .collect::<Vec<_>>();
                Ok(cases.join(" | "))
            }
            SchemaType::Flags { flags, .. } => {
                let mut flags_def = String::new();
                flags_def.push_str("{\n");
                for flag in flags {
                    let flag_name = self.to_js_ident(flag);
                    flags_def.push_str(&format!("  {flag_name}: boolean;\n"));
                }
                flags_def.push('}');
                Ok(flags_def)
            }
            SchemaType::Record { fields, .. } => {
                let mut record_def = String::new();
                record_def.push_str("{\n");
                for field in fields {
                    let js_name = self.to_js_ident(&field.name);
                    let field_str = if let SchemaType::Option { inner, .. } = &field.body {
                        let field_type = self.type_reference(inner)?;
                        format!("{js_name}?: {field_type};\n")
                    } else {
                        let field_type = self.type_reference(&field.body)?;
                        format!("{js_name}: {field_type};\n")
                    };
                    let indented = indent(&field_str, 2);
                    record_def.push_str(&indented);
                }
                record_def.push('}');
                Ok(record_def)
            }
            SchemaType::Tuple { elements, .. } => {
                let types: Vec<String> = elements
                    .iter()
                    .map(|item| self.type_reference(item))
                    .collect::<Result<_, _>>()?;
                Ok(format!("[{}]", types.join(", ")))
            }
            SchemaType::List { element, .. } => {
                if matches!(**element, SchemaType::U8 { .. }) {
                    Ok("Uint8Array".to_string())
                } else {
                    let inner_type = self.type_reference(element)?;
                    Ok(format!("{}[]", inner_type))
                }
            }
            SchemaType::String { .. } => Ok("string".to_string()),
            SchemaType::Char { .. } => Ok("string".to_string()),
            SchemaType::F64 { .. } => Ok("number".to_string()),
            SchemaType::F32 { .. } => Ok("number".to_string()),
            SchemaType::U64 { .. } => Ok("number".to_string()),
            SchemaType::S64 { .. } => Ok("number".to_string()),
            SchemaType::U32 { .. } => Ok("number".to_string()),
            SchemaType::S32 { .. } => Ok("number".to_string()),
            SchemaType::U16 { .. } => Ok("number".to_string()),
            SchemaType::S16 { .. } => Ok("number".to_string()),
            SchemaType::U8 { .. } => Ok("number".to_string()),
            SchemaType::S8 { .. } => Ok("number".to_string()),
            SchemaType::Bool { .. } => Ok("boolean".to_string()),
            // Refs are resolved by [`resolve_ref`] above.
            SchemaType::Ref { .. } => {
                unreachable!("Ref was resolved to its body via resolve_ref")
            }
            // Rich schema variants without a legacy AnalysedType
            // counterpart have no TS surface in the current SDK template.
            SchemaType::FixedList { .. }
            | SchemaType::Map { .. }
            | SchemaType::Text { .. }
            | SchemaType::Binary { .. }
            | SchemaType::Path { .. }
            | SchemaType::Url { .. }
            | SchemaType::Datetime { .. }
            | SchemaType::Duration { .. }
            | SchemaType::Quantity { .. }
            | SchemaType::Union { .. }
            | SchemaType::Secret { .. }
            | SchemaType::QuotaToken { .. }
            | SchemaType::Future { .. }
            | SchemaType::Stream { .. } => Err(anyhow!(
                "Cannot emit TypeScript type definition for unsupported schema variant: {typ:?}"
            )),
        }
    }

    fn library_name(&self) -> String {
        bridge_client_directory_name(&self.agent_type.type_name)
    }

    fn global_config_var_name(&self) -> String {
        format!(
            "{}Configuration",
            self.agent_type.type_name.0.to_lower_camel_case()
        )
    }

    /// Converts a name to a JS/TS identifier (camelCase for cross-language, as-is for same language).
    fn to_js_ident(&self, name: &str) -> String {
        if self.same_language {
            escape_js_ident(name)
        } else {
            escape_js_ident(name.to_lower_camel_case())
        }
    }

    /// Converts a method name to PascalCase for use in generated function names like `encodeXxxInput`.
    /// These are internal generated names, not user-facing API names, so always use PascalCase.
    fn to_method_pascal(&self, name: &str) -> String {
        if self.same_language {
            // Already camelCase; capitalize the first letter to get PascalCase
            let mut chars = name.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        } else {
            name.to_upper_camel_case()
        }
    }
}
