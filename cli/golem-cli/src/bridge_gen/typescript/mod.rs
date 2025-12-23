// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

mod dts_writer;
mod javascript;

use crate::bridge_gen::typescript::dts_writer::{indent, DtsFunctionWriter, DtsWriter};
use crate::bridge_gen::typescript::javascript::escape_js_ident;
use crate::bridge_gen::BridgeGenerator;
use anyhow::anyhow;
use camino::{Utf8Path, Utf8PathBuf};
use golem_common::model::agent::{AgentMode, AgentType, DataSchema, ElementSchema};
use golem_wasm::analysis::AnalysedType;
use heck::{ToLowerCamelCase, ToSnakeCase, ToUpperCamelCase};

struct TypeScriptBridgeGenerator {
    target_path: Utf8PathBuf,
    agent_type: AgentType,
}

impl TypeScriptBridgeGenerator {
    fn generate_ts(&self, path: &Utf8Path) -> anyhow::Result<()> {
        // TODO: Generate configure function
        // TODO: Generate the client class
        // TODO: Generate exports
        Ok(())
    }

    fn generate_dts(&self, path: &Utf8Path) -> anyhow::Result<()> {
        let mut writer = DtsWriter::new();
        writer.begin_declare_module(&self.agent_type.type_name.to_snake_case());

        let types = super::collect_all_wit_types(&self.agent_type);
        for typ in types {
            self.generate_dts_wit_type_def(&mut writer, &typ)?;
        }

        let class_name = self.agent_type.type_name.to_upper_camel_case();
        writer.begin_export_class(&class_name);

        if self.agent_type.mode == AgentMode::Durable {
            let mut get = writer.begin_export_function("get");
            Self::write_parameter_list(&mut get, &self.agent_type.constructor.input_schema)?;
            get.result(&class_name);
        }
        let mut get_phantom = writer.begin_export_function("getPhantom");
        get_phantom.param("phantomId", "Uuid"); // TODO: we need an Uuid type in the common lib
        Self::write_parameter_list(&mut get_phantom, &self.agent_type.constructor.input_schema)?;
        get_phantom.result(&class_name);

        let mut new_phantom = writer.begin_export_function("newPhantom");
        Self::write_parameter_list(&mut new_phantom, &self.agent_type.constructor.input_schema)?;
        new_phantom.result(&class_name);

        for method_def in &self.agent_type.methods {
            let mut method = writer.begin_export_async_function(&method_def.name);
            Self::write_parameter_list(&mut method, &method_def.input_schema)?;
            Self::write_result(&mut method, &method_def.output_schema)?;

            // TODO: trigger, schedule
        }

        writer.end_export_class();

        let mut configure = writer.begin_export_function("configure");
        configure.param("server", "GolemServer"); // TODO: local | cloud | { url: string } into the base lib

        writer.end_declare_module();
        writer.finish(path)
    }

    fn generate_dts_wit_type_def(
        &self,
        writer: &mut DtsWriter,
        typ: &AnalysedType,
    ) -> anyhow::Result<()> {
        let name = typ
            .name()
            .ok_or_else(|| anyhow!("Trying to generate a type definition for a type without name"))?
            .to_upper_camel_case(); // TODO: use owner too?

        let def = Self::type_definition(typ)?;
        writer.export_type(&name, &def);

        Ok(())
    }

    fn write_parameter_list(
        writer: &mut DtsFunctionWriter<'_>,
        schema: &DataSchema,
    ) -> anyhow::Result<()> {
        match schema {
            DataSchema::Tuple(params) => {
                for param in &params.elements {
                    let param_name = param.name.to_lower_camel_case();
                    match param.schema {
                        ElementSchema::ComponentModel(component_model) => writer.param(
                            &param_name,
                            &Self::type_reference(&component_model.element_type)?,
                        ),
                        ElementSchema::UnstructuredText(_) => {
                            todo!()
                        }
                        ElementSchema::UnstructuredBinary(_) => {
                            todo!()
                        }
                    }
                }
                Ok(())
            }
            DataSchema::Multimodal(_) => {
                todo!()
            }
        }
    }

    fn write_result(writer: &mut DtsFunctionWriter<'_>, schema: &DataSchema) -> anyhow::Result<()> {
        match schema {
            DataSchema::Tuple(params) => {
                for param in &params.elements {
                    match param.schema {
                        ElementSchema::ComponentModel(component_model) => {
                            writer.result(&Self::type_reference(&component_model.element_type)?)
                        }
                        ElementSchema::UnstructuredText(_) => {
                            todo!()
                        }
                        ElementSchema::UnstructuredBinary(_) => {
                            todo!()
                        }
                    }
                }
                Ok(())
            }
            DataSchema::Multimodal(_) => {
                todo!()
            }
        }
    }

    fn type_reference(typ: &AnalysedType) -> anyhow::Result<String> {
        match typ {
            AnalysedType::Str(_) => Ok("string".to_string()),
            AnalysedType::Chr(_) => Ok("string".to_string()),
            AnalysedType::F64(_) => Ok("number".to_string()),
            AnalysedType::F32(_) => Ok("number".to_string()),
            AnalysedType::U64(_) => Ok("number".to_string()),
            AnalysedType::S64(_) => Ok("number".to_string()),
            AnalysedType::U32(_) => Ok("number".to_string()),
            AnalysedType::S32(_) => Ok("number".to_string()),
            AnalysedType::U16(_) => Ok("number".to_string()),
            AnalysedType::S16(_) => Ok("number".to_string()),
            AnalysedType::U8(_) => Ok("number".to_string()),
            AnalysedType::S8(_) => Ok("number".to_string()),
            AnalysedType::Bool(_) => Ok("boolean".to_string()),
            AnalysedType::Handle(_) => Err(anyhow!("Handle types are not supported")),
            _ => match typ.name() {
                Some(name) => Ok(name.to_upper_camel_case()), // TODO: use owner too?
                None => Err(anyhow!("Complex type reference with no type name")),
            },
        }
    }

    fn type_definition(typ: &AnalysedType) -> anyhow::Result<String> {
        match typ {
            AnalysedType::Variant(variant) => {
                let mut case_defs = Vec::new();
                for case in &variant.cases {
                    let case_name = &case.name;
                    match &case.typ {
                        Some(ty) => {
                            let case_type = Self::type_reference(ty)?;
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
            AnalysedType::Result(result) => {
                let ok_type = result
                    .ok
                    .map(|t| Self::type_reference(&t))
                    .transpose()?
                    .unwrap_or("void".to_string());
                let err_type = result
                    .err
                    .map(|t| Self::type_reference(&t))
                    .transpose()?
                    .unwrap_or("Error".to_string());
                Ok(format!("Result<{ok_type}, {err_type}>")) // TODO: we need a Result type in the common lib
            }
            AnalysedType::Option(option) => {
                let inner_ts_type = Self::type_reference(&*option.inner)?;
                Ok(format!("{} | undefined", inner_ts_type))
            }
            AnalysedType::Enum(r#enum) => {
                let cases = r#enum
                    .cases
                    .iter()
                    .map(|case| format!("\"{}\"", case))
                    .collect::<Vec<_>>();
                Ok(cases.join(" | "))
            }
            AnalysedType::Flags(flags) => {
                let mut flags_def = String::new();
                flags_def.push_str("{\n");
                for flag in &flags.names {
                    let flag_name = escape_js_ident(flag.to_lower_camel_case());
                    flags_def.push_str(&format!("  {flag_name}: boolean;\n"));
                }
                flags_def.push('}');
                Ok(flags_def)
            }
            AnalysedType::Record(record) => {
                let mut record_def = String::new();
                record_def.push_str("{\n");
                for field in &record.fields {
                    let js_name = escape_js_ident(field.name.to_lower_camel_case());
                    let field_str = if let AnalysedType::Option(option) = &field.typ {
                        let field_type = Self::type_reference(&*option.inner)?;
                        format!("{js_name}?: {field_type};\n")
                    } else {
                        let field_type = Self::type_reference(&field.typ)?;
                        format!("{js_name}: {field_type};\n")
                    };
                    let indented = indent(&field_str, 2);
                    record_def.push_str(&indented);
                }
                record_def.push('}');
                Ok(record_def)
            }
            AnalysedType::Tuple(tuple) => {
                let types: Vec<String> = tuple
                    .items
                    .iter()
                    .map(|t| Self::type_reference(t))
                    .collect::<Result<_, _>>()?;
                Ok(format!("[{}]", types.join(", ")))
            }
            AnalysedType::List(list) => {
                if matches!(*list.inner, AnalysedType::U8(_)) {
                    Ok("Uint8Array".to_string())
                } else {
                    let inner_type = Self::type_reference(&*list.inner)?;
                    Ok(format!("{}[]", inner_type))
                }
            }
            AnalysedType::Str(_) => Ok("string".to_string()),
            AnalysedType::Chr(_) => Ok("string".to_string()),
            AnalysedType::F64(_) => Ok("number".to_string()),
            AnalysedType::F32(_) => Ok("number".to_string()),
            AnalysedType::U64(_) => Ok("number".to_string()),
            AnalysedType::S64(_) => Ok("number".to_string()),
            AnalysedType::U32(_) => Ok("number".to_string()),
            AnalysedType::S32(_) => Ok("number".to_string()),
            AnalysedType::U16(_) => Ok("number".to_string()),
            AnalysedType::S16(_) => Ok("number".to_string()),
            AnalysedType::U8(_) => Ok("number".to_string()),
            AnalysedType::S8(_) => Ok("number".to_string()),
            AnalysedType::Bool(_) => Ok("boolean".to_string()),
            AnalysedType::Handle(_) => Err(anyhow!("Handle types are not supported")),
        }
    }
}

impl BridgeGenerator for TypeScriptBridgeGenerator {
    fn new(agent_type: AgentType, target_path: &Utf8Path) -> Self {
        Self {
            target_path: target_path.to_path_buf(),
            agent_type,
        }
    }

    fn generate(&self) -> anyhow::Result<()> {
        let library_name = self.agent_type.type_name.to_snake_case();

        // TODO: do not generate d.ts, just generate the TS and tsc will emit d.ts

        let dts_path = self.target_path.join(format!("{library_name}.d.ts"));
        let ts_path = self.target_path.join(format!("{library_name}.ts"));
        let package_json_path = self.target_path.join("package.json".to_string());
        let tsconfig_json_path = self.target_path.join("tsconfig.json".to_string());

        self.generate_dts(&dts_path)?;
        self.generate_ts(&ts_path)?;

        Ok(())
    }
}
