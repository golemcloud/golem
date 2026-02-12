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
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::{AgentType, DataSchema, ElementSchema, NamedElementSchemas};
use golem_wasm::analysis::AnalysedType;
use heck::{ToKebabCase, ToLowerCamelCase, ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use moonbit_component_generator::{
    to_moonbit_ident, MoonBitComponent, MoonBitPackage, PackageName, Warning, WarningControl,
};
use std::fmt::Write;
use std::path::Path;

pub fn generate_moonbit_wrapper(
    ctx: AgentWrapperGeneratorContext,
    target: &Path,
) -> anyhow::Result<()> {
    let wit = &ctx.single_file_wrapper_wit_source;
    let mut component = MoonBitComponent::empty_from_wit(wit, Some("agent-wrapper"))?;
    component.disable_cleanup(); // TODO: remove!!!

    component
        .define_bindgen_packages()
        .context("Defining bindgen packages")?;

    let moonbit_root_package = component.moonbit_root_package()?;
    let pkg_namespace = component.root_pkg_namespace()?.to_snake_case();
    let escaped_pkg_namespace = to_moonbit_ident(&pkg_namespace);
    let pkg_name = component.root_pkg_name()?.to_snake_case();
    let escaped_pkg_name = to_moonbit_ident(&pkg_name);

    // Adding the builder and extractor packages
    add_builder_package(&mut component, &moonbit_root_package)?;
    add_extractor_package(&mut component, &moonbit_root_package)?;

    setup_dependencies(
        &mut component,
        ctx.has_types,
        &moonbit_root_package,
        &escaped_pkg_namespace,
        &escaped_pkg_name,
        &ctx.agent_types,
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

    const AGENT_LOAD_SNAPSHOT_MBT: &str = include_str!("../../../agent_wrapper/load_snapshot.mbt");
    const AGENT_SAVE_SNAPSHOT_MBT: &str = include_str!("../../../agent_wrapper/save_snapshot.mbt");

    component.write_interface_stub(
        &PackageName {
            namespace: "golem".to_string(),
            name: "api".to_string(),
            version: None,
        },
        "loadSnapshot",
        AGENT_LOAD_SNAPSHOT_MBT,
    )?;
    component.write_interface_stub(
        &PackageName {
            namespace: "golem".to_string(),
            name: "api".to_string(),
            version: None,
        },
        "saveSnapshot",
        AGENT_SAVE_SNAPSHOT_MBT,
    )?;

    if ctx.has_types {
        component.write_interface_stub(
            &PackageName {
                namespace: pkg_namespace.clone(),
                name: pkg_name.clone(),
                version: None,
            },
            "types",
            "",
        )?;
    }

    component.set_warning_control(
        &format!("{moonbit_root_package}/interface/golem/agent/guest"),
        vec![
            WarningControl::Disable(Warning::Specific(35)), //  Warning: The word `constructor` is reserved for possible future use. Please consider using another name.
        ],
    )?;

    for agent in &ctx.agent_types {
        let agent_stub = generate_agent_stub(&ctx, agent)?;
        let agent_name = agent.type_name.0.to_lower_camel_case();

        component.set_warning_control(
            &format!(
                "{moonbit_root_package}/gen/interface/{escaped_pkg_namespace}/{escaped_pkg_name}/{agent_name}"
            ),
            vec![
                // These are disabled so we don't have to check if the generated inside try .. catch is always
                // capable of throwing an exception
                WarningControl::Disable(Warning::Specific(23)),
                WarningControl::Disable(Warning::Specific(11)),
                // Unused package warning (can be that @agentTypes is not used)
                WarningControl::Disable(Warning::Specific(29)),
                //  Warning: The word `constructor` is reserved for possible future use. Please consider using another name.
                WarningControl::Disable(Warning::Specific(35)),
            ],
        )?;

        component.write_interface_stub(
            &PackageName {
                namespace: pkg_namespace.clone(),
                name: pkg_name.clone(),
                version: None,
            },
            &agent_name,
            &agent_stub,
        )?;
    }

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

    let dependencies = vec![(
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
    )];

    let builder_package = MoonBitPackage {
        name: format!("{moonbit_root_package}/builder"),
        mbt_files: vec![
            Utf8Path::new("builder").join("child_items_builder.mbt"),
            Utf8Path::new("builder").join("item_builder.mbt"),
            Utf8Path::new("builder").join("tests.mbt"),
            Utf8Path::new("builder").join("top.mbt"),
        ],
        warning_control: vec![],
        alert_control: vec![],
        output: Utf8Path::new("target")
            .join("wasm")
            .join("release")
            .join("build")
            .join("builder")
            .join("builder.core"),
        dependencies,
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

    let dependencies = vec![
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
    ];

    let extractor_package = MoonBitPackage {
        name: format!("{moonbit_root_package}/extractor"),
        mbt_files: vec![
            Utf8Path::new("extractor").join("tests.mbt"),
            Utf8Path::new("extractor").join("top.mbt"),
        ],
        warning_control: vec![],
        alert_control: vec![],
        output: Utf8Path::new("target")
            .join("wasm")
            .join("release")
            .join("build")
            .join("extractor")
            .join("extractor.core"),
        dependencies,
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
    has_types: bool,
    moonbit_root_package: &str,
    escaped_pkg_namespace: &str,
    escaped_pkg_name: &str,
    agent_types: &[AgentType],
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

    let depends_on_types = |component: &mut MoonBitComponent,
                            has_types: bool,
                            escaped_pkg_namespace: &str,
                            escaped_pkg_name: &str,
                            name: &str| {
        if has_types {
            component.add_dependency(
                &format!("{moonbit_root_package}/{name}"),
                &Utf8Path::new("target")
                    .join("wasm")
                    .join("release")
                    .join("build")
                    .join("gen")
                    .join("interface")
                    .join(escaped_pkg_namespace)
                    .join(escaped_pkg_name)
                    .join("types")
                    .join("types.mi"),
                "types",
            )
        } else {
            Ok(())
        }
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

    component.add_dependency(
        &format!("{moonbit_root_package}/gen/interface/golem/api/loadSnapshot"),
        &Utf8Path::new("target")
            .join("wasm")
            .join("release")
            .join("build")
            .join("interface")
            .join("golem")
            .join("api")
            .join("loadSnapshot")
            .join("loadSnapshot.mi"),
        "loadSnapshot",
    )?;
    component.add_dependency(
        &format!("{moonbit_root_package}/gen/interface/golem/api/loadSnapshot"),
        &Utf8Path::new("target")
            .join("wasm")
            .join("release")
            .join("build")
            .join("interface")
            .join("golem")
            .join("api")
            .join("host")
            .join("host.mi"),
        "host",
    )?;
    component.add_dependency(
        &format!("{moonbit_root_package}/gen/interface/golem/api/saveSnapshot"),
        &Utf8Path::new("target")
            .join("wasm")
            .join("release")
            .join("build")
            .join("interface")
            .join("golem")
            .join("api")
            .join("saveSnapshot")
            .join("saveSnapshot.mi"),
        "saveSnapshot",
    )?;
    component.add_dependency(
        &format!("{moonbit_root_package}/gen/interface/golem/api/saveSnapshot"),
        &Utf8Path::new("target")
            .join("wasm")
            .join("release")
            .join("build")
            .join("interface")
            .join("golem")
            .join("api")
            .join("host")
            .join("host.mi"),
        "host",
    )?;

    if has_types {
        depends_on_golem_agent_common(
            component,
            &format!("gen/interface/{escaped_pkg_namespace}/{escaped_pkg_name}/types"),
        )?;
    }

    for agent in agent_types {
        let agent_name = agent.type_name.0.to_lower_camel_case();

        depends_on_golem_agent_common(
            component,
            &format!("gen/interface/{escaped_pkg_namespace}/{escaped_pkg_name}/{agent_name}"),
        )?;
        depends_on_golem_agent_guest(
            component,
            &format!("gen/interface/{escaped_pkg_namespace}/{escaped_pkg_name}/{agent_name}"),
        )?;
        depends_on_types(
            component,
            has_types,
            escaped_pkg_namespace,
            escaped_pkg_name,
            &format!("gen/interface/{escaped_pkg_namespace}/{escaped_pkg_name}/{agent_name}"),
        )?;

        component.add_dependency(
            &format!(
                "{moonbit_root_package}/gen/interface/{escaped_pkg_namespace}/{escaped_pkg_name}/{agent_name}"
            ),
            &Utf8Path::new("target")
                .join("wasm")
                .join("release")
                .join("build")
                .join("builder")
                .join("builder.mi"),
            "builder",
        )?;
        component.add_dependency(
            &format!(
                "{moonbit_root_package}/gen/interface/{escaped_pkg_namespace}/{escaped_pkg_name}/{agent_name}"
            ),
            &Utf8Path::new("target")
                .join("wasm")
                .join("release")
                .join("build")
                .join("extractor")
                .join("extractor.mi"),
            "extractor",
        )?;
        component.add_dependency(
            &format!(
                "{moonbit_root_package}/gen/interface/{escaped_pkg_namespace}/{escaped_pkg_name}/{agent_name}"
            ),
            &Utf8Path::new("target")
                .join("wasm")
                .join("release")
                .join("build")
                .join("interface")
                .join("wasi")
                .join("logging")
                .join("logging")
                .join("logging.mi"),
            "logging",
        )?;
        component.add_dependency(
            &format!("{moonbit_root_package}/gen/interface/{escaped_pkg_namespace}/{escaped_pkg_name}/{agent_name}"),
            &Utf8Path::new("target")
                .join("wasm")
                .join("release")
                .join("build")
                .join("interface")
                .join("golem")
                .join("rpc")
                .join("types")
                .join("types.mi"),
            "rpcTypes",
        )?; // NOTE: cannot use depends_on_wasm_rpc_types because needs a different alias
    }

    depends_on_golem_agent_common(component, "gen/interface/golem/agent/guest")?;
    depends_on_golem_agent_guest(component, "gen/interface/golem/agent/guest")?;

    depends_on_wasm_rpc_types(component, "interface/golem/agent/common")?;
    depends_on_wasm_rpc_types(component, "interface/golem/agent/guest")?;

    let depends_on_wasi_io_poll = |component: &mut MoonBitComponent, name: &str| {
        component.add_dependency(
            &format!("{moonbit_root_package}/{name}"),
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
        )
    };

    let depends_on_golem_api_host = |component: &mut MoonBitComponent, name: &str| {
        component.add_dependency(
            &format!("{moonbit_root_package}/{name}"),
            &Utf8Path::new("target")
                .join("wasm")
                .join("release")
                .join("build")
                .join("interface")
                .join("golem")
                .join("api")
                .join("host")
                .join("host.mi"),
            "host",
        )
    };

    depends_on_wasm_rpc_types(component, "interface/golem/api/host")?;
    depends_on_wasi_io_poll(component, "interface/golem/api/host")?;

    depends_on_golem_api_host(component, "interface/golem/api/loadSnapshot")?;
    depends_on_golem_api_host(component, "interface/golem/api/saveSnapshot")?;

    depends_on_wasi_io_poll(component, "interface/wasi/clocks/monotonicClock")?;

    depends_on_golem_agent_common(component, "gen")?;
    depends_on_wasm_rpc_types(component, "gen")?;
    depends_on_golem_api_host(component, "gen")?;

    depends_on_wasi_io_poll(component, "interface/golem/rpc/types")?;
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

fn generate_agent_stub(
    ctx: &AgentWrapperGeneratorContext,
    agent: &AgentType,
) -> anyhow::Result<String> {
    let mut result = String::new();

    let original_agent_name = &agent.type_name;

    let constructor_params =
        to_moonbit_parameter_list(ctx, &agent.constructor.input_schema, "constructor", false)?;
    writeln!(result, "pub fn initialize({constructor_params}) -> Unit {{")?;
    writeln!(result, "  let encoded_params = try ")?;
    build_data_value(
        &mut result,
        ctx,
        &agent.constructor.input_schema,
        "constructor",
    )?;
    writeln!(result, "    catch {{")?;
    writeln!(
        result,
        "      BuilderError(msg) => {{ @logging.log(@logging.Level::ERROR, \"{original_agent_name} initialize\", msg); panic(); }}"
    )?;
    writeln!(result, "    }}")?;
    // Fixme: thread the correct value through once we have new invocation api
    writeln!(
        result,
        "  match @guest.initialize(\"{original_agent_name}\", encoded_params, @common.Principal::Anonymous) {{"
    )?;
    writeln!(result, "    Ok(result) => result")?;
    writeln!(result, "    Err(error) => {{ @logging.log(@logging.Level::ERROR, \"{original_agent_name} initialize\", error.to_string()); panic(); }}")?;
    writeln!(result, "  }}")?;
    writeln!(result, "}}")?;
    writeln!(result)?;

    writeln!(result, "pub fn get_definition() -> @common.AgentType {{")?;
    writeln!(result, "  @guest.get_definition()")?;
    writeln!(result, "}}")?;
    writeln!(result)?;

    for method in &agent.methods {
        let original_method_name = &method.name;
        let method_name = to_moonbit_ident(&method.name);

        let moonbit_param_defs =
            to_moonbit_parameter_list(ctx, &method.input_schema, original_method_name, false)?;
        let moonbit_return_type =
            to_moonbit_return_type(ctx, &method.output_schema, original_method_name)?;

        writeln!(
            result,
            "pub fn {method_name}({moonbit_param_defs}) -> {moonbit_return_type} {{"
        )?;
        writeln!(result, "  let input = try ")?;
        build_data_value(&mut result, ctx, &method.input_schema, original_method_name)?;
        writeln!(result, "    catch {{")?;
        writeln!(
            result,
            "      BuilderError(msg) => {{ @logging.log(@logging.Level::ERROR, \"{original_agent_name} {method_name}\", msg); panic(); }}"
        )?;
        writeln!(result, "    }}")?;

        // Fixme: thread the correct value through once we have new invocation api
        writeln!(
            result,
            "  let result = match @guest.invoke(\"{original_method_name}\", input, @common.Principal::Anonymous) {{"
        )?;
        writeln!(result, "    Ok(result) => result")?;
        writeln!(result, "    Err(error) => {{ @logging.log(@logging.Level::ERROR, \"{original_agent_name} {method_name}\", error.to_string()); panic(); }}")?;
        writeln!(result, "  }}")?;

        extract_data_value(
            &mut result,
            ctx,
            &method.output_schema,
            "  ",
            original_method_name,
        )?;

        writeln!(result, "}}")?;
        writeln!(result)?;
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

                let param_name = to_moonbit_ident(&parameter.name);
                match &parameter.schema {
                    ElementSchema::ComponentModel(typ) => {
                        write!(
                            result,
                            "{param_name}: {}",
                            moonbit_type_ref(ctx, &typ.element_type)?
                        )?;
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
            let context_kebab = context.to_kebab_case();
            let name = format!("{context_kebab}-input");

            let type_name = ctx
                .multimodal_variants
                .get(&name)
                .ok_or_else(|| anyhow!(format!("Missing multimodal variant {name}")))?
                .to_upper_camel_case();

            if add_initial_comma {
                write!(result, ", ")?;
            }
            write!(result, "input: Array[@types.{type_name}]")?;
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
                        write!(result, "{}", moonbit_type_ref(ctx, &typ.element_type)?)?;
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
                            write!(result, "{}", moonbit_type_ref(ctx, &typ.element_type)?)?;
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
            let context_kebab = context.to_kebab_case();
            let name = format!("{context_kebab}-output");
            let type_name = ctx
                .multimodal_variants
                .get(&name)
                .ok_or_else(|| anyhow!(format!("Missing multimodal variant {name}")))?
                .to_upper_camel_case();

            write!(result, "Array[@types.{type_name}]")?;
        }
    }

    Ok(result)
}

fn moonbit_type_ref(
    ctx: &AgentWrapperGeneratorContext,
    typ: &AnalysedType,
) -> anyhow::Result<String> {
    if let Some(name) = ctx.type_names.get(typ) {
        Ok(format!("@types.{}", name.to_upper_camel_case()))
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
        ElementSchema::ComponentModel(schema) => {
            writeln!(result, "{indent}@common.ElementValue::ComponentModel(")?;
            write_builder(
                result,
                ctx,
                &schema.element_type,
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
                let param_name = to_moonbit_ident(&element.name);
                build_element_value(result, ctx, &element.schema, &param_name, "      ")?;
                writeln!(result, ",")?;
            }

            writeln!(result, "     ])")?;
        }
        DataSchema::Multimodal(NamedElementSchemas { elements }) => {
            let context_kebab = context.to_kebab_case();

            let name = format!("{context_kebab}-input");
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
                    .to_wit_naming()
                    .to_upper_camel_case();
                writeln!(
                    result,
                    "          @types.{type_name}::{case_name}(value) => {{"
                )?;
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
                let case_name = case.name.to_wit_naming().to_upper_camel_case();
                match &case.typ {
                    Some(_) => {
                        writeln!(
                            result,
                            "{indent}    @types.{variant_name}::{case_name}(_) => {case_idx}"
                        )?;
                    }
                    None => {
                        writeln!(
                            result,
                            "{indent}    @types.{variant_name}::{case_name} => {case_idx}"
                        )?;
                    }
                }
            }

            writeln!(result, "{indent}  }},")?;
            writeln!(result, "{indent}  match {access} {{")?;

            for case in &variant.cases {
                let case_name = case.name.to_wit_naming().to_upper_camel_case();
                match &case.typ {
                    Some(_) => {
                        writeln!(
                            result,
                            "{indent}    @types.{variant_name}::{case_name}(value) => {{"
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
                        writeln!(
                            result,
                            "{indent}    @types.{variant_name}::{case_name} => None"
                        )?;
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
                    writeln!(result, "{indent}  Some(fn (builder: @builder.ItemBuilder) -> Unit raise @builder.BuilderError {{")?;
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
                    writeln!(result, "{indent}  Some(fn (builder: @builder.ItemBuilder) -> Unit raise @builder.BuilderError {{")?;
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
                let field_name = to_moonbit_ident(&field.name);
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
            } else if elements.len() == 1 {
                writeln!(
                    result,
                    "{indent}let values = @extractor.extract_tuple(result)"
                )?;
                let element = &elements[0];
                match &element.schema {
                    ElementSchema::ComponentModel(schema) => {
                        writeln!(
                            result,
                            "{indent}let wit_value = @extractor.extract_component_model_value(values[0])"
                        )?;
                        extract_wit_value(result, ctx, &schema.element_type, "wit_value", indent)?;
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
                for (idx, element) in elements.iter().enumerate() {
                    match &element.schema {
                        ElementSchema::ComponentModel(schema) => {
                            writeln!(
                                result,
                                "{indent}let wit_value = @extractor.extract_component_model_value(values[{idx}])"
                            )?;
                            writeln!(result, "{{")?;
                            extract_wit_value(
                                result,
                                ctx,
                                &schema.element_type,
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
            }
        }
        DataSchema::Multimodal(NamedElementSchemas { elements }) => {
            let context_kebab = context.to_kebab_case();
            let name = format!("{context_kebab}-output");
            let variant_name = ctx
                .multimodal_variants
                .get(&name)
                .ok_or_else(|| anyhow!(format!("Missing multimodal variant {name}")))?
                .to_upper_camel_case();

            writeln!(
                result,
                "{indent}let values = @extractor.extract_multimodal(result)"
            )?;
            writeln!(result, "{indent}values.map(pair => {{")?;
            writeln!(result, "{indent}  match pair.0 {{")?;
            for element in elements {
                let case_name = element.name.to_wit_naming().to_upper_camel_case();
                writeln!(result, "{indent}    \"{}\" => {{", element.name)?;
                match &element.schema {
                    ElementSchema::ComponentModel(schema) => {
                        writeln!(result, "{indent}      match pair.1 {{")?;
                        writeln!(
                            result,
                            "{indent}        @common.ElementValue::ComponentModel(wit_value) => {{"
                        )?;
                        writeln!(
                            result,
                            "{indent}        @types.{variant_name}::{case_name}("
                        )?;
                        extract_wit_value(
                            result,
                            ctx,
                            &schema.element_type,
                            "@extractor.extract(wit_value)",
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
                            "{indent}        @common.ElementValue::UnstructuredText(text) => @types.{variant_name}::{case_name}(text)"
                        )?;
                        writeln!(result, "{indent}        _ => panic()")?;
                        writeln!(result, "{indent}      }}")?;
                    }
                    ElementSchema::UnstructuredBinary(_) => {
                        writeln!(result, "{indent}      match pair.1 {{")?;
                        writeln!(
                            result,
                            "{indent}        @common.ElementValue::UnstructuredBinary(binary) => @types.{variant_name}::{case_name}(binary)"
                        )?;
                        writeln!(result, "{indent}        _ => panic()")?;
                        writeln!(result, "{indent}      }}")?;
                    }
                }
                writeln!(result, "{indent}    }}")?;
            }
            writeln!(result, "{indent}  }}")?;
            writeln!(result, "{indent}}})")?;
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
                let case_name = case.name.to_wit_naming().to_upper_camel_case();

                writeln!(result, "{indent}    {idx} => {{")?;
                if let Some(inner_type) = &case.typ {
                    writeln!(result, "{indent}      @types.{variant_name}::{case_name}(")?;
                    extract_wit_value(
                        result,
                        ctx,
                        inner_type,
                        "inner.unwrap()",
                        &format!("{indent}        "),
                    )?;
                    writeln!(result, "{indent}      )")?;
                } else {
                    writeln!(result, "{indent}      @types.{variant_name}::{case_name}")?;
                }
                writeln!(result, "{indent}    }}")?;
            }

            writeln!(result, "{indent}  }}")?;
            writeln!(result, "{indent}}}")?;
        }
        AnalysedType::Result(result_type) => {
            writeln!(result, "{indent}{from}.result().unwrap().map(inner => {{")?;
            if let Some(ok) = &result_type.ok {
                extract_wit_value(result, ctx, ok, "inner.unwrap()", &format!("{indent}    "))?;
            } else {
                writeln!(result, "{indent}  ()")?;
            }
            writeln!(result, "{indent}}}).map_err(inner => {{")?;
            if let Some(err) = &result_type.err {
                extract_wit_value(result, ctx, err, "inner.unwrap()", &format!("{indent}    "))?;
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
                "{indent}@types.{enum_type_name}::from({from}.enum_value().unwrap().reinterpret_as_int())"
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

            writeln!(result, "{indent}@types.{record_name}::{{")?;

            for (idx, field) in record.fields.iter().enumerate() {
                let field_name = to_moonbit_ident(&field.name);

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
                writeln!(
                    result,
                    "{indent}FixedArray::from_array({from}.list_elements().unwrap().map(item => {{"
                )?;
                extract_wit_value(result, ctx, &list.inner, "item", &format!("{indent}  "))?;
                writeln!(result, "{indent}}}))")?;
            } else {
                writeln!(
                    result,
                    "{indent}{from}.list_elements().unwrap().map(item => {{"
                )?;
                extract_wit_value(result, ctx, &list.inner, "item", &format!("{indent}  "))?;
                writeln!(result, "{indent}}})")?;
            }
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
#[test_r::sequential]
mod tests {
    use crate::model::agent::moonbit::generate_moonbit_wrapper;
    use crate::model::agent::test;
    use crate::model::agent::test::{
        reproducer_for_issue_with_enums, reproducer_for_issue_with_result_types,
        reproducer_for_multiple_types_called_element,
    };
    use crate::model::agent::wit::generate_agent_wrapper_wit;
    use golem_common::model::component::ComponentName;
    use golem_templates::model::GuestLanguage;
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
        let component_name = ComponentName("example:multi1".to_string());
        let agent_types = test::multi_agent_wrapper_2_types();
        let ctx = generate_agent_wrapper_wit(&component_name, &agent_types).unwrap();

        let target = NamedTempFile::new().unwrap();
        generate_moonbit_wrapper(ctx, target.path()).unwrap();
    }

    #[test]
    fn single_agent_with_wit_keywords(_trace: &Trace) {
        let component_name = ComponentName("example:single1".to_string());
        let agent_types = test::agent_type_with_wit_keywords();
        let ctx = generate_agent_wrapper_wit(&component_name, &agent_types).unwrap();

        let target = NamedTempFile::new().unwrap();
        generate_moonbit_wrapper(ctx, target.path()).unwrap();
    }

    #[test]
    fn bug_multiple_types_called_element() {
        let component_name = ComponentName("example:bug".to_string());
        let agent_types = reproducer_for_multiple_types_called_element();
        let ctx = generate_agent_wrapper_wit(&component_name, &agent_types).unwrap();

        let target = NamedTempFile::new().unwrap();
        generate_moonbit_wrapper(ctx, target.path()).unwrap();
    }

    #[test]
    fn single_agent_with_test_in_package_name(_trace: &Trace) {
        let component_name = ComponentName("test:agent".to_string());
        let agent_types = test::agent_type_with_wit_keywords();
        let ctx = generate_agent_wrapper_wit(&component_name, &agent_types).unwrap();

        let target = NamedTempFile::new().unwrap();
        generate_moonbit_wrapper(ctx, target.path()).unwrap();
    }

    #[test]
    fn enum_type() {
        let component_name = ComponentName("test:agent".to_string());
        let agent_types = reproducer_for_issue_with_enums();
        let ctx = generate_agent_wrapper_wit(&component_name, &agent_types).unwrap();

        let target = NamedTempFile::new().unwrap();
        generate_moonbit_wrapper(ctx, target.path()).unwrap();
    }

    #[test]
    fn bug_result_types() {
        let component_name = ComponentName("example:bug".to_string());
        let agent_types = reproducer_for_issue_with_result_types();
        let ctx = generate_agent_wrapper_wit(&component_name, &agent_types).unwrap();

        let target = NamedTempFile::new().unwrap();
        generate_moonbit_wrapper(ctx, target.path()).unwrap();
    }

    #[test]
    pub fn multimodal_untagged_variant_in_out() {
        let component_name = ComponentName("example:bug".to_string());
        let agent_types = test::multimodal_untagged_variant_in_out();
        let ctx = generate_agent_wrapper_wit(&component_name, &agent_types).unwrap();

        let target = NamedTempFile::new().unwrap();
        generate_moonbit_wrapper(ctx, target.path()).unwrap();
    }

    #[test]
    pub fn char_type() {
        let component_name = "example:bug".try_into().unwrap();
        let agent_types = test::char_type();
        let ctx = generate_agent_wrapper_wit(&component_name, &agent_types).unwrap();

        let target = NamedTempFile::new().unwrap();
        generate_moonbit_wrapper(ctx, target.path()).unwrap();
    }

    #[test]
    pub fn unit_result_type() {
        let component_name = "example:bug".try_into().unwrap();
        let agent_types = test::unit_result_type();
        let ctx = generate_agent_wrapper_wit(&component_name, &agent_types).unwrap();

        let target = NamedTempFile::new().unwrap();
        generate_moonbit_wrapper(ctx, target.path()).unwrap();
    }

    #[test]
    pub fn ts_code_first_snippets() {
        let component_name = "example:code-first-snippets".try_into().unwrap();
        let agent_types = test::code_first_snippets_agent_types(GuestLanguage::TypeScript);
        let ctx = generate_agent_wrapper_wit(&component_name, &agent_types).unwrap();

        let target = NamedTempFile::new().unwrap();
        generate_moonbit_wrapper(ctx, target.path()).unwrap();
    }
}
