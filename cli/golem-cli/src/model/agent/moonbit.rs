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

use crate::model::agent::wit::AgentWrapperGeneratorContext;
use anyhow::{anyhow, Context};
use camino::Utf8Path;
use golem_client::model::AnalysedType;
use golem_common::model::agent::{DataSchema, ElementSchema, NamedElementSchemas};
use heck::{ToKebabCase, ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use moonbit_component_generator::{MoonBitComponent, MoonBitPackage, Warning, WarningControl};
use std::fmt::Write;
use std::path::Path;
use wit_parser::PackageName;

pub fn generate_moonbit_wrapper(
    ctx: AgentWrapperGeneratorContext,
    target: &Path,
) -> anyhow::Result<()> {
    let wit = &ctx.single_file_wrapper_wit_source;
    let mut component = MoonBitComponent::empty_from_wit(wit, Some("agent-wrapper"))?;

    component
        .define_bindgen_packages()
        .context("Defining bindgen packages")?;

    let moonbit_root_package = component.moonbit_root_package()?;
    let pkg_namespace = component.root_pkg_namespace()?.to_snake_case();
    let pkg_name = component.root_pkg_name()?.to_snake_case();

    // Adding the builder and extractor packages
    add_builder_package(&mut component, &moonbit_root_package)?;
    add_extractor_package(&mut component, &moonbit_root_package)?;

    setup_dependencies(
        &mut component,
        &moonbit_root_package,
        &pkg_namespace,
        &pkg_name,
    )?;

    let world_stub_mbt = String::new();
    component
        .write_world_stub(&world_stub_mbt)
        .context("Writing world stub")?;

    const AGENT_GUEST_MBT: &str = include_str!("../../../agent_wrapper/guest.mbt");

    component.write_interface_stub(
        &PackageName {
            namespace: "golem".to_string(),
            name: "agent".to_string(),
            version: None,
        },
        "guest",
        AGENT_GUEST_MBT,
    )?;

    component.set_warning_control(
        &format!("{moonbit_root_package}/gen/interface/{pkg_namespace}/{pkg_name}/agent"),
        vec![
            // These are disabled so we don't have to check if the generated inside of try .. catch is always
            // capable of throwing an exception
            WarningControl::Disable(Warning::Specific(23)),
            WarningControl::Disable(Warning::Specific(11)),
        ],
    )?;
    let agent_stub = generate_agent_stub(ctx)?;
    component.write_interface_stub(
        &PackageName {
            namespace: pkg_namespace,
            name: pkg_name,
            version: None,
        },
        "agent",
        &agent_stub,
    )?;

    component
        .build(
            None,
            Utf8Path::from_path(target).ok_or_else(|| {
                anyhow!("Invalid target path for the agent wrapper component: {target:?}")
            })?,
        )
        .context("Building component")?;

    Ok(())
}

fn add_builder_package(
    component: &mut MoonBitComponent,
    moonbit_root_package: &str,
) -> anyhow::Result<()> {
    const BUILDER_CHILD_ITEMS_BUILDER_MBT: &str =
        include_str!("../../../agent_wrapper/builder/child_items_builder.mbt");
    const BUILDER_ITEM_BUILDER_MBT: &str =
        include_str!("../../../agent_wrapper/builder/item_builder.mbt");
    const BUILDER_TESTS_MBT: &str = include_str!("../../../agent_wrapper/builder/tests.mbt");
    const BUILDER_TOP_MBT: &str = include_str!("../../../agent_wrapper/builder/top.mbt");

    component.write_file(
        Utf8Path::new("builder/child_items_builder.mbt"),
        BUILDER_CHILD_ITEMS_BUILDER_MBT,
    )?;
    component.write_file(
        Utf8Path::new("builder/item_builder.mbt"),
        BUILDER_ITEM_BUILDER_MBT,
    )?;
    component.write_file(Utf8Path::new("builder/tests.mbt"), BUILDER_TESTS_MBT)?;
    component.write_file(Utf8Path::new("builder/top.mbt"), BUILDER_TOP_MBT)?;

    let builder_package = MoonBitPackage {
        name: format!("{moonbit_root_package}/builder"),
        mbt_files: vec![
            Utf8Path::new("builder").join("child_items_builder.mbt"),
            Utf8Path::new("builder").join("item_builder.mbt"),
            Utf8Path::new("builder").join("tests.mbt"),
            Utf8Path::new("builder").join("top.mbt"),
        ],
        warning_control: vec![],
        output: Utf8Path::new("target")
            .join("wasm")
            .join("release")
            .join("build")
            .join("builder")
            .join("builder.core"),
        dependencies: vec![(
            Utf8Path::new("target")
                .join("wasm")
                .join("release")
                .join("build")
                .join("interface")
                .join("golem")
                .join("rpc")
                .join("types")
                .join("types.mi"),
            "types".to_string(),
        )],
        package_sources: vec![(
            format!("{moonbit_root_package}/builder"),
            Utf8Path::new("builder").to_path_buf(),
        )],
    };
    component.define_package(builder_package);
    Ok(())
}

fn add_extractor_package(
    component: &mut MoonBitComponent,
    moonbit_root_package: &str,
) -> anyhow::Result<()> {
    const EXTRACTOR_TESTS_MBT: &str = include_str!("../../../agent_wrapper/extractor/tests.mbt");
    const EXTRACTOR_TOP_MBT: &str = include_str!("../../../agent_wrapper/extractor/top.mbt");

    component.write_file(Utf8Path::new("extractor/tests.mbt"), EXTRACTOR_TESTS_MBT)?;
    component.write_file(Utf8Path::new("extractor/top.mbt"), EXTRACTOR_TOP_MBT)?;

    let extractor_package = MoonBitPackage {
        name: format!("{moonbit_root_package}/extractor"),
        mbt_files: vec![
            Utf8Path::new("extractor").join("tests.mbt"),
            Utf8Path::new("extractor").join("top.mbt"),
        ],
        warning_control: vec![],
        output: Utf8Path::new("target")
            .join("wasm")
            .join("release")
            .join("build")
            .join("extractor")
            .join("extractor.core"),
        dependencies: vec![
            (
                Utf8Path::new("target")
                    .join("wasm")
                    .join("release")
                    .join("build")
                    .join("interface")
                    .join("golem")
                    .join("rpc")
                    .join("types")
                    .join("types.mi"),
                "types".to_string(),
            ),
            (
                Utf8Path::new("target")
                    .join("wasm")
                    .join("release")
                    .join("build")
                    .join("interface")
                    .join("golem")
                    .join("agent")
                    .join("common")
                    .join("common.mi"),
                "common".to_string(),
            ),
            (
                Utf8Path::new("target")
                    .join("wasm")
                    .join("release")
                    .join("build")
                    .join("builder")
                    .join("builder.mi"),
                "builder".to_string(),
            ),
        ],
        package_sources: vec![(
            format!("{moonbit_root_package}/extractor"),
            Utf8Path::new("extractor").to_path_buf(),
        )],
    };
    component.define_package(extractor_package);
    Ok(())
}

fn setup_dependencies(
    component: &mut MoonBitComponent,
    moonbit_root_package: &str,
    pkg_namespace: &str,
    pkg_name: &str,
) -> anyhow::Result<()> {
    // NOTE: setting up additional dependencies. this could be automatically done by a better implementation of define_bindgen_packages
    let depends_on_golem_agent_common = |component: &mut MoonBitComponent, name: &str| {
        component.add_dependency(
            &format!("{moonbit_root_package}/{name}"),
            &Utf8Path::new("target")
                .join("wasm")
                .join("release")
                .join("build")
                .join("interface")
                .join("golem")
                .join("agent")
                .join("common")
                .join("common.mi"),
            "common",
        )
    };

    let depends_on_golem_agent_guest = |component: &mut MoonBitComponent, name: &str| {
        component.add_dependency(
            &format!("{moonbit_root_package}/{name}"),
            &Utf8Path::new("target")
                .join("wasm")
                .join("release")
                .join("build")
                .join("interface")
                .join("golem")
                .join("agent")
                .join("guest")
                .join("guest.mi"),
            "guest",
        )
    };

    let depends_on_wasm_rpc_types = |component: &mut MoonBitComponent, name: &str| {
        component.add_dependency(
            &format!("{moonbit_root_package}/{name}"),
            &Utf8Path::new("target")
                .join("wasm")
                .join("release")
                .join("build")
                .join("interface")
                .join("golem")
                .join("rpc")
                .join("types")
                .join("types.mi"),
            "types",
        )
    };

    depends_on_golem_agent_common(component, "interface/golem/agent/guest")?;

    depends_on_golem_agent_common(
        component,
        &format!("gen/interface/{pkg_namespace}/{pkg_name}/agent"),
    )?;
    depends_on_golem_agent_guest(
        component,
        &format!("gen/interface/{pkg_namespace}/{pkg_name}/agent"),
    )?;
    component.add_dependency(
        &format!("{moonbit_root_package}/gen/interface/{pkg_namespace}/{pkg_name}/agent"),
        &Utf8Path::new("target")
            .join("wasm")
            .join("release")
            .join("build")
            .join("builder")
            .join("builder.mi"),
        "builder",
    )?;
    component.add_dependency(
        &format!("{moonbit_root_package}/gen/interface/{pkg_namespace}/{pkg_name}/agent"),
        &Utf8Path::new("target")
            .join("wasm")
            .join("release")
            .join("build")
            .join("extractor")
            .join("extractor.mi"),
        "extractor",
    )?;

    depends_on_golem_agent_common(component, "gen/interface/golem/agent/guest")?;
    depends_on_golem_agent_guest(component, "gen/interface/golem/agent/guest")?;

    depends_on_wasm_rpc_types(component, "interface/golem/agent/common")?;
    depends_on_wasm_rpc_types(component, "interface/golem/agent/guest")?;

    depends_on_golem_agent_common(component, "gen")?;
    depends_on_wasm_rpc_types(component, "gen")?;

    component.add_dependency(
        &format!("{moonbit_root_package}/interface/golem/rpc/types"),
        &Utf8Path::new("target")
            .join("wasm")
            .join("release")
            .join("build")
            .join("interface")
            .join("wasi")
            .join("io")
            .join("poll")
            .join("poll.mi"),
        "poll",
    )?;
    component.add_dependency(
        &format!("{moonbit_root_package}/interface/golem/rpc/types"),
        &Utf8Path::new("target")
            .join("wasm")
            .join("release")
            .join("build")
            .join("interface")
            .join("wasi")
            .join("clocks")
            .join("wallClock")
            .join("wallClock.mi"),
        "wallClock",
    )?;
    Ok(())
}

fn generate_agent_stub(ctx: AgentWrapperGeneratorContext) -> anyhow::Result<String> {
    let mut result = String::new();

    for agent in &ctx.agent_types {
        let original_agent_name = &agent.type_name;
        let agent_name = agent.type_name.to_upper_camel_case();
        let constructor_name = agent_name.to_snake_case();
        let wrapped_agents = format!("Wrapped{agent_name}s");
        let wrapped_agents_var = wrapped_agents.to_snake_case();

        writeln!(
            result,
            "impl Hash for {agent_name} with hash_combine(self, hasher) {{"
        )?;
        writeln!(result, "  hasher.combine(self.rep())")?;
        writeln!(result, "}}")?;
        writeln!(result)?;

        writeln!(result, "priv struct {wrapped_agents} {{")?;
        writeln!(result, "  agents: Map[{agent_name}, @guest.Agent]")?;
        writeln!(result, "  reverse_agents: Map[String, {agent_name}]")?;
        writeln!(result, "  mut last_agent_id: Int")?;
        writeln!(result, "}}")?;
        writeln!(result)?;

        writeln!(result, "let {wrapped_agents_var}: {wrapped_agents} = {{")?;
        writeln!(result, "  agents: {{}},")?;
        writeln!(result, "  reverse_agents: {{}},")?;
        writeln!(result, "  last_agent_id: 0,")?;
        writeln!(result, "}}")?;

        writeln!(
            result,
            "fn {agent_name}::unwrap(self: {agent_name}) -> @guest.Agent {{"
        )?;
        writeln!(result, "  match {wrapped_agents_var}.agents.get(self) {{")?;
        writeln!(result, "    Some(agent) => agent")?;
        writeln!(result, "    None => panic()")?;
        writeln!(result, "  }}")?;
        writeln!(result, "}}")?;
        writeln!(result)?;

        writeln!(
            result,
            "pub fn {agent_name}::dtor(self: {agent_name}) -> Unit {{"
        )?;
        writeln!(
            result,
            "  {wrapped_agents_var}.reverse_agents.remove(self.get_id())"
        )?;
        writeln!(result, "  {wrapped_agents_var}.agents.remove(self)")?;
        writeln!(result, "}}")?;
        writeln!(result)?;

        writeln!(
            result,
            "pub fn {agent_name}::{constructor_name}(agent_id: String) -> {agent_name} {{"
        )?;
        writeln!(
            result,
            "  {wrapped_agents_var}.reverse_agents.get(agent_id).unwrap()"
        )?;
        writeln!(result, "}}")?;
        writeln!(result)?;

        let constructor_params =
            to_moonbit_parameter_list(&ctx, &agent.constructor.input_schema, "constructor", false)?;
        writeln!(result, "pub fn {agent_name}::create({constructor_params}) -> Result[{agent_name}, @common.AgentError] {{")?;
        writeln!(result, "  let encoded_params = try ")?;
        build_data_value(
            &mut result,
            &ctx,
            &agent.constructor.input_schema,
            "constructor",
        )?;
        writeln!(result, "    catch {{")?;
        writeln!(
            result,
            "      BuilderError(msg) => return Err(@common.AgentError::InvalidInput(msg))"
        )?;
        writeln!(result, "    }}")?;
        writeln!(result, "  @guest.Agent::create(\"{original_agent_name}\", encoded_params).bind(inner_agent => {{")?;
        writeln!(
            result,
            "    let agent = {agent_name}::new({wrapped_agents_var}.last_agent_id)"
        )?;
        writeln!(result, "    {wrapped_agents_var}.last_agent_id += 1")?;
        writeln!(
            result,
            "    {wrapped_agents_var}.agents[agent] = inner_agent"
        )?;
        writeln!(
            result,
            "    {wrapped_agents_var}.reverse_agents[inner_agent.get_id()] = agent"
        )?;
        writeln!(result, "    Ok(agent)")?;
        writeln!(result, "  }})")?;
        writeln!(result, "}}")?;
        writeln!(result)?;

        writeln!(
            result,
            "pub fn {agent_name}::get_id(self: {agent_name}) -> String {{"
        )?;
        writeln!(result, "  self.unwrap().get_id()")?;
        writeln!(result, "}}")?;
        writeln!(result)?;

        writeln!(
            result,
            "pub fn {agent_name}::get_definition(self: {agent_name}) -> @common.AgentType {{"
        )?;
        writeln!(result, "  self.unwrap().get_definition()")?;
        writeln!(result, "}}")?;
        writeln!(result)?;

        for method in &agent.methods {
            let original_method_name = &method.name;
            let method_name = method.name.to_snake_case();

            let moonbit_param_defs =
                to_moonbit_parameter_list(&ctx, &method.input_schema, original_method_name, true)?;
            let moonbit_return_type =
                to_moonbit_return_type(&ctx, &method.output_schema, original_method_name)?;

            writeln!(result, "pub fn {agent_name}::{method_name}(self: {agent_name}{moonbit_param_defs}) -> Result[{moonbit_return_type}, @common.AgentError] {{")?;
            writeln!(result, "  let input = try ")?;
            build_data_value(
                &mut result,
                &ctx,
                &method.input_schema,
                original_method_name,
            )?;
            writeln!(result, "    catch {{")?;
            writeln!(
                result,
                "      BuilderError(msg) => return Err(@common.AgentError::InvalidInput(msg))"
            )?;
            writeln!(result, "    }}")?;

            writeln!(
                result,
                "  self.unwrap().invoke(\"{original_method_name}\", input).bind(result => {{"
            )?;

            extract_data_value(
                &mut result,
                &ctx,
                &method.output_schema,
                "    ",
                original_method_name,
            )?;

            writeln!(result, "  }})")?;
            writeln!(result, "}}")?;
            writeln!(result)?;
        }
    }

    Ok(result)
}

fn to_moonbit_parameter_list(
    ctx: &AgentWrapperGeneratorContext,
    schema: &DataSchema,
    context: &str,
    add_initial_comma: bool,
) -> anyhow::Result<String> {
    let mut result = String::new();

    match schema {
        DataSchema::Tuple(parameters) => {
            // tuple input is represented by a parameter list
            for (idx, parameter) in parameters.elements.iter().enumerate() {
                if idx > 0 || add_initial_comma {
                    write!(result, ", ")?;
                }

                let param_name = parameter.name.to_snake_case();
                match &parameter.schema {
                    ElementSchema::ComponentModel(typ) => {
                        write!(result, "{param_name}: {}", moonbit_type_ref(ctx, typ)?)?;
                    }
                    ElementSchema::UnstructuredText(_) => {
                        write!(result, "{param_name}: @common.TextReference")?;
                    }
                    ElementSchema::UnstructuredBinary(_) => {
                        write!(result, "{param_name}: @common.BinaryReference")?;
                    }
                }
            }
        }
        DataSchema::Multimodal(_) => {
            // multi-modal input is represented as a list of a generated variant type
            let name = format!("{context}-input");
            let type_name = ctx
                .multimodal_variants
                .get(&name)
                .ok_or_else(|| anyhow!(format!("Missing multimodal variant {name}")))?
                .to_upper_camel_case();

            if add_initial_comma {
                write!(result, ", ")?;
            }
            write!(result, "input: Array[{type_name}]")?;
        }
    }

    Ok(result)
}

fn to_moonbit_return_type(
    ctx: &AgentWrapperGeneratorContext,
    schema: &DataSchema,
    context: &str,
) -> anyhow::Result<String> {
    let mut result = String::new();

    match schema {
        DataSchema::Tuple(NamedElementSchemas { elements }) => {
            // tuple output is represented by either Unit, a single type, or a tuple of types
            if elements.is_empty() {
                write!(result, "Unit")?;
            } else if elements.len() == 1 {
                let element = &elements[0];
                match &element.schema {
                    ElementSchema::ComponentModel(typ) => {
                        write!(result, "{}", moonbit_type_ref(ctx, typ)?)?;
                    }
                    ElementSchema::UnstructuredText(_) => {
                        write!(result, "@common.TextReference")?;
                    }
                    ElementSchema::UnstructuredBinary(_) => {
                        write!(result, "@common.BinaryReference")?;
                    }
                }
            } else {
                write!(result, "(")?;
                for (idx, element) in elements.iter().enumerate() {
                    if idx > 0 {
                        write!(result, ", ")?;
                    }
                    match &element.schema {
                        ElementSchema::ComponentModel(typ) => {
                            write!(result, "{}", moonbit_type_ref(ctx, typ)?)?;
                        }
                        ElementSchema::UnstructuredText(_) => {
                            write!(result, "@common.TextReference")?;
                        }
                        ElementSchema::UnstructuredBinary(_) => {
                            write!(result, "@common.BinaryReference")?;
                        }
                    }
                }
                write!(result, ")")?;
            }
        }
        DataSchema::Multimodal(_) => {
            // multi-modal output is represented as a list of a generated variant type
            let name = format!("{context}-output");
            let type_name = ctx
                .multimodal_variants
                .get(&name)
                .ok_or_else(|| anyhow!(format!("Missing multimodal variant {name}")))?
                .to_upper_camel_case();

            write!(result, "Array[{type_name}]")?;
        }
    }

    Ok(result)
}

fn moonbit_type_ref(
    ctx: &AgentWrapperGeneratorContext,
    typ: &AnalysedType,
) -> anyhow::Result<String> {
    if let Some(name) = ctx.type_names.get(typ) {
        Ok(name.to_upper_camel_case())
    } else {
        match typ {
            AnalysedType::Variant(_)
            | AnalysedType::Record(_)
            | AnalysedType::Flags(_)
            | AnalysedType::Enum(_)
            | AnalysedType::Handle(_) => Err(anyhow!(
                "Variant, record, flags, enum, and handle types cannot be inlined."
            )),
            AnalysedType::Result(result) => {
                let ok = if let Some(ok) = &result.ok {
                    moonbit_type_ref(ctx, ok)?
                } else {
                    "Unit".to_string()
                };
                let err = if let Some(err) = &result.err {
                    moonbit_type_ref(ctx, err)?
                } else {
                    "Unit".to_string()
                };
                Ok(format!("Result[{ok}, {err}]"))
            }
            AnalysedType::Option(option) => {
                let inner_type = moonbit_type_ref(ctx, &option.inner)?;
                Ok(format!("Option[{inner_type}]"))
            }
            AnalysedType::Tuple(tuple) => {
                let inner_types: Vec<String> = tuple
                    .items
                    .iter()
                    .map(|t| moonbit_type_ref(ctx, t))
                    .collect::<Result<_, _>>()?;
                Ok(format!("({})", inner_types.join(", ")))
            }
            AnalysedType::List(list) => {
                let inner_type = moonbit_type_ref(ctx, &list.inner)?;
                if matches!(
                    &*list.inner,
                    AnalysedType::U8(_)
                        | AnalysedType::U32(_)
                        | AnalysedType::U64(_)
                        | AnalysedType::S32(_)
                        | AnalysedType::S64(_)
                        | AnalysedType::F32(_)
                        | AnalysedType::F64(_)
                ) {
                    // based on https://github.com/bytecodealliance/wit-bindgen/blob/main/crates/moonbit/src/lib.rs#L998-L1009
                    Ok(format!("FixedArray[{inner_type}]"))
                } else {
                    Ok(format!("Array[{inner_type}]"))
                }
            }
            AnalysedType::Str(_) => Ok("String".to_string()),
            AnalysedType::Chr(_) => Ok("Char".to_string()),
            AnalysedType::F64(_) => Ok("Double".to_string()),
            AnalysedType::F32(_) => Ok("Float".to_string()),
            AnalysedType::U64(_) => Ok("UInt64".to_string()),
            AnalysedType::S64(_) => Ok("Int64".to_string()),
            AnalysedType::U32(_) => Ok("UInt".to_string()),
            AnalysedType::S32(_) => Ok("Int".to_string()),
            AnalysedType::U16(_) => Ok("UInt".to_string()),
            AnalysedType::S16(_) => Ok("Int".to_string()),
            AnalysedType::U8(_) => Ok("Byte".to_string()),
            AnalysedType::S8(_) => Ok("Int".to_string()),
            AnalysedType::Bool(_) => Ok("Bool".to_string()),
        }
    }
}

fn build_element_value(
    result: &mut String,
    ctx: &AgentWrapperGeneratorContext,
    schema: &ElementSchema,
    access: &str,
    indent: &str,
) -> anyhow::Result<()> {
    match schema {
        ElementSchema::ComponentModel(typ) => {
            writeln!(result, "{indent}@common.ElementValue::ComponentModel(")?;
            write_builder(
                result,
                ctx,
                typ,
                access,
                "@builder.Builder::new()",
                &format!("{indent}    "),
            )?;
            write!(result, "{indent})")?;
        }
        ElementSchema::UnstructuredText(_) => {
            write!(
                result,
                "{indent}@common.ElementValue::UnstructuredText({access})"
            )?;
        }
        ElementSchema::UnstructuredBinary(_) => {
            write!(
                result,
                "{indent}@common.ElementValue::UnstructuredBinary({access})"
            )?;
        }
    }
    Ok(())
}

fn build_data_value(
    result: &mut String,
    ctx: &AgentWrapperGeneratorContext,
    schema: &DataSchema,
    context: &str,
) -> anyhow::Result<()> {
    match schema {
        DataSchema::Tuple(NamedElementSchemas { elements }) => {
            writeln!(result, "    @common.DataValue::Tuple([")?;

            for element in elements {
                let param_name = element.name.to_snake_case();
                build_element_value(result, ctx, &element.schema, &param_name, "      ")?;
                writeln!(result, ",")?;
            }

            writeln!(result, "     ])")?;
        }
        DataSchema::Multimodal(NamedElementSchemas { elements }) => {
            let name = format!("{context}-input");
            let type_name = ctx
                .multimodal_variants
                .get(&name)
                .ok_or_else(|| anyhow!(format!("Missing multimodal variant {name}")))?
                .to_upper_camel_case();
            writeln!(result, "    @common.DataValue::Multimodal(")?;
            writeln!(result, "      input.map(item => {{")?;
            writeln!(result, "        match item {{")?;

            for named_element_schema in elements {
                let case_name = named_element_schema
                    .name
                    .to_kebab_case()
                    .to_upper_camel_case();
                writeln!(result, "          {type_name}::{case_name}(value) => {{")?;
                writeln!(result, "            (\"{}\",", named_element_schema.name)?;
                build_element_value(
                    result,
                    ctx,
                    &named_element_schema.schema,
                    "value",
                    "              ",
                )?;
                writeln!(result)?;
                writeln!(result, "            )")?;
                writeln!(result, "          }}")?;
            }

            writeln!(result, "        }}")?;
            writeln!(result, "      }})")?;
            writeln!(result, "    )")?;
        }
    }

    Ok(())
}

fn write_builder(
    result: &mut String,
    ctx: &AgentWrapperGeneratorContext,
    typ: &AnalysedType,
    access: &str,
    builder: &str,
    indent: &str,
) -> anyhow::Result<()> {
    match typ {
        AnalysedType::Variant(variant) => {
            let variant_name = ctx
                .type_names
                .get(typ)
                .ok_or_else(|| anyhow!("Missing type name for variant: {typ:?}"))?
                .to_upper_camel_case();

            writeln!(result, "{indent}{builder}.variant_option(")?;
            writeln!(result, "{indent}  match {access} {{")?;

            for (case_idx, case) in variant.cases.iter().enumerate() {
                let case_name = case.name.to_kebab_case().to_upper_camel_case();
                match &case.typ {
                    Some(_) => {
                        writeln!(
                            result,
                            "{indent}    {variant_name}::{case_name}(_) => {case_idx}"
                        )?;
                    }
                    None => {
                        writeln!(
                            result,
                            "{indent}    {variant_name}::{case_name} => {case_idx}"
                        )?;
                    }
                }
            }

            writeln!(result, "{indent}  }},")?;
            writeln!(result, "{indent}  match {access} {{")?;

            for case in &variant.cases {
                let case_name = case.name.to_kebab_case().to_upper_camel_case();
                match &case.typ {
                    Some(_) => {
                        writeln!(
                            result,
                            "{indent}    {variant_name}::{case_name}(value) => {{"
                        )?;
                        writeln!(result, "{indent}       Some(builder => {{")?;
                        write_builder(
                            result,
                            ctx,
                            case.typ.as_ref().unwrap(),
                            "value",
                            "builder",
                            &format!("{indent}         "),
                        )?;
                        writeln!(result, "{indent}       }})")?;
                        writeln!(result, "{indent}    }}")?;
                    }
                    None => {
                        writeln!(result, "{indent}    {variant_name}::{case_name} => None")?;
                    }
                }
            }

            writeln!(result, "{indent}  }}")?;
            writeln!(result, "{indent})")?;
        }
        AnalysedType::Result(result_type) => {
            writeln!(result, "{indent}{builder}.result({access}.map(inner => {{")?;
            match &result_type.ok {
                Some(ok) => {
                    writeln!(result, "{indent}  Some(builder => {{")?;
                    write_builder(
                        result,
                        ctx,
                        ok,
                        "inner",
                        "builder",
                        &format!("{indent}    "),
                    )?;
                    writeln!(result, "{indent}  }})")?;
                }
                None => {
                    writeln!(result, "{indent}  None")?;
                }
            }
            writeln!(result, "{indent}}}).map_err(inner => {{")?;
            match &result_type.err {
                Some(err) => {
                    writeln!(result, "{indent}  Some(builder => {{")?;
                    write_builder(
                        result,
                        ctx,
                        err,
                        "inner",
                        "builder",
                        &format!("{indent}    "),
                    )?;
                    writeln!(result, "{indent}  }})")?;
                }
                None => {
                    writeln!(result, "{indent}  None")?;
                }
            }
            writeln!(result, "{indent}}}))")?;
        }
        AnalysedType::Option(option) => {
            writeln!(result, "{indent}{builder}.option({access}.map(inner => {{")?;
            writeln!(result, "{indent}  builder => {{")?;
            write_builder(
                result,
                ctx,
                &option.inner,
                "inner",
                "builder",
                &format!("{indent}    "),
            )?;
            writeln!(result, "{indent}  }}")?;
            writeln!(result, "{indent}}}))")?;
        }
        AnalysedType::Enum(_) => {
            writeln!(
                result,
                "{indent}{builder}.enum_value({access}.ordinal().reinterpret_as_uint())"
            )?;
        }
        AnalysedType::Flags(flags) => {
            let flags_type = ctx
                .type_names
                .get(typ)
                .ok_or_else(|| anyhow!("Missing type name for flags: {typ:?}"))?
                .to_upper_camel_case();

            writeln!(result, "{indent}{builder}.flags([")?;
            for flag in &flags.names {
                let flag_name = flag.to_shouty_snake_case();
                writeln!(
                    result,
                    "{indent}  {access}.is_set({flags_type}Flag::{flag_name}),"
                )?;
            }
            writeln!(result, "{indent}{builder}])")?;
        }
        AnalysedType::Record(record) => {
            writeln!(result, "{indent}{builder}.record(builder => {{")?;
            for field in &record.fields {
                let field_name = field.name.to_snake_case();
                write_builder(
                    result,
                    ctx,
                    &field.typ,
                    &format!("{access}.{field_name}"),
                    "builder",
                    &format!("{indent}  "),
                )?;
            }
            writeln!(result, "{indent}}})")?;
        }
        AnalysedType::Tuple(tuple) => {
            writeln!(result, "{indent}{builder}.tuple(builder => {{")?;
            for (idx, typ) in tuple.items.iter().enumerate() {
                write_builder(
                    result,
                    ctx,
                    typ,
                    &format!("{access}.{idx}"),
                    "builder",
                    &format!("{indent}  "),
                )?;
            }
            writeln!(result, "{indent}}})")?;
        }
        AnalysedType::List(list) => {
            writeln!(result, "{indent}{builder}.list(builder => {{")?;
            writeln!(result, "{indent}  for item in {access} {{")?;
            write_builder(
                result,
                ctx,
                &list.inner,
                "item",
                "builder",
                &format!("{indent}    "),
            )?;
            writeln!(result, "{indent}  }}")?;
            writeln!(result, "{indent}}})")?;
        }
        AnalysedType::Str(_) => {
            writeln!(result, "{indent}{builder}.string({access})")?;
        }
        AnalysedType::Chr(_) => {
            writeln!(result, "{indent}{builder}.char({access})")?;
        }
        AnalysedType::F64(_) => {
            writeln!(result, "{indent}{builder}.f64({access})")?;
        }
        AnalysedType::F32(_) => {
            writeln!(result, "{indent}{builder}.f32({access})")?;
        }
        AnalysedType::U64(_) => {
            writeln!(result, "{indent}{builder}.u64({access})")?;
        }
        AnalysedType::S64(_) => {
            writeln!(result, "{indent}{builder}.s64({access})")?;
        }
        AnalysedType::U32(_) => {
            writeln!(result, "{indent}{builder}.u32({access})")?;
        }
        AnalysedType::S32(_) => {
            writeln!(result, "{indent}{builder}.s32({access})")?;
        }
        AnalysedType::U16(_) => {
            writeln!(result, "{indent}{builder}.u16({access})")?;
        }
        AnalysedType::S16(_) => {
            writeln!(result, "{indent}{builder}.s16({access})")?;
        }
        AnalysedType::U8(_) => {
            writeln!(result, "{indent}{builder}.u8({access})")?;
        }
        AnalysedType::S8(_) => {
            writeln!(result, "{indent}{builder}.s8({access})")?;
        }
        AnalysedType::Bool(_) => {
            writeln!(result, "{indent}{builder}.bool({access})")?;
        }
        AnalysedType::Handle(_) => Err(anyhow!(
            "Handle types are not supported in the static agent wrappers."
        ))?,
    }
    Ok(())
}

// TODO: the extraction logic panics on type mismatches currently. we may want it to be an AgentError instead.
fn extract_data_value(
    result: &mut String,
    ctx: &AgentWrapperGeneratorContext,
    schema: &DataSchema,
    indent: &str,
    context: &str,
) -> anyhow::Result<()> {
    match schema {
        DataSchema::Tuple(NamedElementSchemas { elements }) => {
            if elements.is_empty() {
                writeln!(result, "{indent}Ok(())")?;
            } else if elements.len() == 1 {
                writeln!(
                    result,
                    "{indent}let values = @extractor.extract_tuple(result)"
                )?;
                let element = &elements[0];
                match &element.schema {
                    ElementSchema::ComponentModel(typ) => {
                        writeln!(
                            result,
                            "{indent}let wit_value = @extractor.extract_component_model_value(values[0])"
                        )?;
                        writeln!(result, "{indent}Ok(")?;
                        extract_wit_value(result, ctx, typ, "wit_value", indent)?;
                        writeln!(result, "{indent})")?;
                    }
                    ElementSchema::UnstructuredText(_) => {
                        writeln!(
                            result,
                            "{indent}@extractor.extract_unstructured_text(values[0])"
                        )?;
                    }
                    ElementSchema::UnstructuredBinary(_) => {
                        writeln!(
                            result,
                            "{indent}@extractor.extract_unstructured_binary(values[0])"
                        )?;
                    }
                }
            } else {
                writeln!(
                    result,
                    "{indent}let values = @extractor.extract_tuple(result)"
                )?;
                writeln!(result, "{indent}Ok((")?;
                for (idx, element) in elements.iter().enumerate() {
                    match &element.schema {
                        ElementSchema::ComponentModel(typ) => {
                            writeln!(
                                result,
                                "{indent}let wit_value = @extractor.extract_component_model_value(values[{idx}])"
                            )?;
                            writeln!(result, "{{")?;
                            extract_wit_value(
                                result,
                                ctx,
                                typ,
                                "@extractor.extract(wit_value)",
                                &format!("{indent}  "),
                            )?;
                            writeln!(result, "}}")?;
                        }
                        ElementSchema::UnstructuredText(_) => {
                            writeln!(
                                result,
                                "{indent}@extractor.extract_unstructured_text(values[{idx}])"
                            )?;
                        }
                        ElementSchema::UnstructuredBinary(_) => {
                            writeln!(
                                result,
                                "{indent}@extractor.extract_unstructured_binary(values[{idx}])"
                            )?;
                        }
                    }
                }
                writeln!(result, "{indent}))")?;
            }
        }
        DataSchema::Multimodal(NamedElementSchemas { elements }) => {
            let name = format!("{context}-output");
            let variant_name = ctx
                .multimodal_variants
                .get(&name)
                .ok_or_else(|| anyhow!(format!("Missing multimodal variant {name}")))?
                .to_upper_camel_case();

            writeln!(
                result,
                "{indent}let values = @extractor.extract_multimodal(result)"
            )?;
            writeln!(result, "{indent}Ok(values.map(pair => {{")?;
            writeln!(result, "{indent}  match pair.0 {{")?;
            for element in elements {
                let case_name = element.name.to_kebab_case().to_upper_camel_case();
                writeln!(result, "{indent}    \"{}\" => {{", element.name)?;
                match &element.schema {
                    ElementSchema::ComponentModel(typ) => {
                        writeln!(result, "{indent}      match pair.1 {{")?;
                        writeln!(
                            result,
                            "{indent}        @common.ElementValue::ComponentModel(wit_value) => {{"
                        )?;
                        writeln!(result, "{indent}        {variant_name}::{case_name}(")?;
                        extract_wit_value(
                            result,
                            ctx,
                            typ,
                            "wit_value",
                            &format!("{indent}          "),
                        )?;
                        writeln!(result, "{indent}          )")?;
                        writeln!(result, "{indent}        }}")?;
                        writeln!(result, "{indent}        _ => panic()")?;
                        writeln!(result, "{indent}      }}")?;
                    }
                    ElementSchema::UnstructuredText(_) => {
                        writeln!(result, "{indent}      match pair.1 {{")?;
                        writeln!(
                            result,
                            "{indent}        @common.ElementValue::UnstructuredText(text) => {variant_name}::{case_name}(text)"
                        )?;
                        writeln!(result, "{indent}        _ => panic()")?;
                        writeln!(result, "{indent}      }}")?;
                    }
                    ElementSchema::UnstructuredBinary(_) => {
                        writeln!(result, "{indent}      match pair.1 {{")?;
                        writeln!(
                            result,
                            "{indent}        @common.ElementValue::UnstructuredBinary(binary) => {variant_name}::{case_name}(binary)"
                        )?;
                        writeln!(result, "{indent}        _ => panic()")?;
                        writeln!(result, "{indent}      }}")?;
                    }
                }
                writeln!(result, "{indent}    }}")?;
            }
            writeln!(result, "{indent}  }}")?;
            writeln!(result, "{indent}}}))")?;
        }
    }

    Ok(())
}

fn extract_wit_value(
    result: &mut String,
    ctx: &AgentWrapperGeneratorContext,
    typ: &AnalysedType,
    from: &str,
    indent: &str,
) -> anyhow::Result<()> {
    match typ {
        AnalysedType::Variant(variant) => {
            let variant_name = ctx
                .type_names
                .get(typ)
                .ok_or_else(|| anyhow!("Missing type name for variant: {typ:?}"))?
                .to_upper_camel_case();

            writeln!(result, "{indent}{{")?;
            writeln!(
                result,
                "{indent}  let (idx, inner) = {from}.variant().unwrap()"
            )?;
            writeln!(result, "{indent}  match idx {{")?;

            for (idx, case) in variant.cases.iter().enumerate() {
                let case_name = case.name.to_kebab_case().to_upper_camel_case();

                writeln!(result, "{indent}    {idx} => {{")?;
                if let Some(inner_type) = &case.typ {
                    writeln!(result, "{indent}      {variant_name}::{case_name}(")?;
                    extract_wit_value(
                        result,
                        ctx,
                        inner_type,
                        "inner.unwrap()",
                        &format!("{indent}        "),
                    )?;
                    writeln!(result, "{indent}      )")?;
                } else {
                    writeln!(result, "{indent}      {variant_name}::{case_name}")?;
                }
                writeln!(result, "{indent}    }}")?;
            }

            writeln!(result, "{indent}  }}")?;
            writeln!(result, "{indent}}}")?;
        }
        AnalysedType::Result(result_type) => {
            writeln!(result, "{indent}{from}.result().unwrap().map(inner => {{")?;
            if let Some(ok) = &result_type.ok {
                extract_wit_value(result, ctx, ok, "inner", &format!("{indent}  "))?;
            } else {
                writeln!(result, "{indent}  ()")?;
            }
            writeln!(result, "{indent}}}).map_err(inner => {{")?;
            if let Some(err) = &result_type.err {
                writeln!(result, "{indent}  inner.map(inner => {{")?;
                extract_wit_value(result, ctx, err, "inner", &format!("{indent}    "))?;
                writeln!(result, "{indent}  }})")?;
            } else {
                writeln!(result, "{indent}  ()")?;
            }
            writeln!(result, "{indent}}})")?;
        }
        AnalysedType::Option(option) => {
            writeln!(result, "{indent}{from}.option().unwrap().map(inner => {{")?;
            extract_wit_value(result, ctx, &option.inner, "inner", &format!("{indent}  "))?;
            writeln!(result, "{indent}}})")?;
        }
        AnalysedType::Enum(_) => {
            let enum_type_name = ctx
                .type_names
                .get(typ)
                .ok_or_else(|| anyhow!("Missing type name for enum: {typ:?}"))?
                .to_upper_camel_case();
            writeln!(
                result,
                "{indent}{enum_type_name}::from({from}.enum_value().unwrap().reinterpret_as_int())"
            )?;
        }
        AnalysedType::Flags(flags) => {
            let flags_type = ctx
                .type_names
                .get(typ)
                .ok_or_else(|| anyhow!("Missing type name for flags: {typ:?}"))?
                .to_upper_camel_case();

            writeln!(result, "{indent}{{")?;
            writeln!(result, "{indent}  let bitmap = {from}.flags().unwrap()")?;
            writeln!(result, "{indent}  let mut result = {flags_type}::default()")?;
            for (idx, flag) in flags.names.iter().enumerate() {
                let flag_name = flag.to_shouty_snake_case();
                writeln!(
                    result,
                    "{indent}  if bitmap[{idx}] {{ result.set({flags_type}Flag::{flag_name}) }}"
                )?;
            }
            writeln!(result, "{indent}  result")?;
            writeln!(result, "{indent}}}")?;
        }
        AnalysedType::Record(record) => {
            let record_name = ctx
                .type_names
                .get(typ)
                .ok_or_else(|| anyhow!("Missing type name for record: {typ:?}"))?
                .to_upper_camel_case();

            writeln!(result, "{indent}{record_name}::{{")?;

            for (idx, field) in record.fields.iter().enumerate() {
                let field_name = field.name.to_snake_case();

                writeln!(result, "{indent}  {field_name}: {{")?;
                extract_wit_value(
                    result,
                    ctx,
                    &field.typ,
                    &format!("{from}.field({idx}).unwrap()"),
                    &format!("{indent}    "),
                )?;
                writeln!(result, "{indent}  }},")?;
            }

            writeln!(result, "{indent}}}")?;
        }
        AnalysedType::Tuple(tuple) => {
            writeln!(result, "{indent}(")?;

            for (idx, typ) in tuple.items.iter().enumerate() {
                writeln!(result, "{indent}  {{")?;
                extract_wit_value(
                    result,
                    ctx,
                    typ,
                    &format!("{from}.tuple_element({idx}).unwrap()"),
                    &format!("{indent}    "),
                )?;
                writeln!(result, "{indent}  }},")?;
            }

            writeln!(result, "{indent})")?;
        }
        AnalysedType::List(list) => {
            writeln!(
                result,
                "{indent}{from}.list_elements().unwrap().map(item => {{"
            )?;
            extract_wit_value(result, ctx, &list.inner, "item", &format!("{indent}  "))?;
            writeln!(result, "{indent}}})")?;
        }
        AnalysedType::Str(_) => {
            writeln!(result, "{indent}{from}.string().unwrap()")?;
        }
        AnalysedType::Chr(_) => {
            writeln!(result, "{indent}{from}.char().unwrap()")?;
        }
        AnalysedType::F64(_) => {
            writeln!(result, "{indent}{from}.f64().unwrap()")?;
        }
        AnalysedType::F32(_) => {
            writeln!(result, "{indent}{from}.f32().unwrap()")?;
        }
        AnalysedType::U64(_) => {
            writeln!(result, "{indent}{from}.u64().unwrap()")?;
        }
        AnalysedType::S64(_) => {
            writeln!(result, "{indent}{from}.s64().unwrap()")?;
        }
        AnalysedType::U32(_) => {
            writeln!(result, "{indent}{from}.u32().unwrap()")?;
        }
        AnalysedType::S32(_) => {
            writeln!(result, "{indent}{from}.s32().unwrap()")?;
        }
        AnalysedType::U16(_) => {
            writeln!(result, "{indent}{from}.u16().unwrap()")?;
        }
        AnalysedType::S16(_) => {
            writeln!(result, "{indent}{from}.s16().unwrap()")?;
        }
        AnalysedType::U8(_) => {
            writeln!(result, "{indent}{from}.u8().unwrap()")?;
        }
        AnalysedType::S8(_) => {
            writeln!(result, "{indent}{from}.s8().unwrap()")?;
        }
        AnalysedType::Bool(_) => {
            writeln!(result, "{indent}{from}.bool().unwrap()")?;
        }
        AnalysedType::Handle(_) => {
            return Err(anyhow!(
                "Handle types are not supported in the static agent wrappers."
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::model::agent::moonbit::generate_moonbit_wrapper;
    use crate::model::agent::test;
    use crate::model::agent::wit::generate_agent_wrapper_wit;
    use crate::model::app::AppComponentName;
    use tempfile::NamedTempFile;
    use test_r::test;

    #[cfg(test)]
    struct Trace;

    #[cfg(test)]
    #[test_r::test_dep]
    fn initialize_trace() -> Trace {
        pretty_env_logger::formatted_builder()
            .filter_level(log::LevelFilter::Debug)
            .write_style(pretty_env_logger::env_logger::WriteStyle::Always)
            .init();
        Trace
    }

    #[test]
    fn multi_agent_example(_trace: &Trace) {
        let component_name: AppComponentName = "example:multi1".into();
        let agent_types = test::multi_agent_wrapper_2_types();
        let ctx = generate_agent_wrapper_wit(&component_name, &agent_types).unwrap();

        let target = NamedTempFile::new().unwrap();
        generate_moonbit_wrapper(ctx, target.path()).unwrap();
    }
}
