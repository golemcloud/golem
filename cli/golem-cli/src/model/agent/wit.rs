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

use crate::model::app::AppComponentName;
use anyhow::{anyhow, bail, Context};
use golem_common::model::agent::{
    AgentType, DataSchema, ElementSchema, NamedElementSchema, NamedElementSchemas,
};
use golem_wasm_ast::analysis::analysed_type::{case, variant};
use golem_wasm_ast::analysis::AnalysedType;
use heck::ToKebabCase;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::path::Path;
use wit_component::WitPrinter;
use wit_parser::{PackageId, Resolve, SourceMap};

pub fn generate_agent_wrapper_wit(
    component_name: &AppComponentName,
    agent_types: &[AgentType],
) -> anyhow::Result<AgentWrapperGeneratorContext> {
    let mut ctx = AgentWrapperGeneratorContextState::new(agent_types.to_vec());
    ctx.generate_wit_source(component_name)
        .context("Generating WIT source")?;
    ctx.generate_single_file_wrapper_wit()
        .context("Generating single file wrapper")?;
    ctx.finalize()
}

pub struct AgentWrapperGeneratorContext {
    pub agent_types: Vec<AgentType>,
    pub type_names: HashMap<AnalysedType, String>,
    pub used_names: HashSet<String>,
    pub multimodal_variants: HashMap<String, String>,
    pub wrapper_package_wit_source: String,
    pub single_file_wrapper_wit_source: String,
}

pub struct AgentWrapperGeneratorContextState {
    agent_types: Vec<AgentType>,
    type_names: HashMap<AnalysedType, String>,
    used_names: HashSet<String>,
    multimodal_variants: HashMap<String, String>,
    wrapper_package_wit_source: Option<String>,
    single_file_wrapper_wit_source: Option<String>,
}

impl AgentWrapperGeneratorContextState {
    fn new(agent_types: Vec<AgentType>) -> Self {
        Self {
            type_names: HashMap::new(),
            used_names: HashSet::new(),
            multimodal_variants: HashMap::new(),
            wrapper_package_wit_source: None,
            single_file_wrapper_wit_source: None,
            agent_types,
        }
    }

    /// Generate the wrapper WIT that also imports and exports golem:agent/guest
    fn generate_wit_source(&mut self, component_name: &AppComponentName) -> anyhow::Result<()> {
        if self.wrapper_package_wit_source.is_some() {
            return Err(anyhow!("generate_wit_source has been called already"));
        }

        let mut result = String::new();

        let package_name = component_name.to_string();
        let parts = package_name.split(':').collect::<Vec<_>>();
        if parts.len() != 2 {
            bail!(
                "Component name `{package_name}` is not a valid WIT package name. It should be in the format `namespace:name`.",
            )
        }

        writeln!(result, "package {package_name};")?;
        writeln!(result)?;

        let mut interface_names = Vec::new();
        let agent_types = self.agent_types.clone();
        for agent in &agent_types {
            interface_names.push(self.generate_agent_wrapper_interface(&mut result, agent)?);
        }

        self.traverse_gathered_types()?;
        let mut types = self
            .type_names
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .filter(|(k, _)| {
                k.name() != Some("text-reference") && k.name() != Some("binary-reference")
            })
            .collect::<Vec<_>>();
        types.sort_by_key(|(_, name)| name.clone());

        let has_types = !types.is_empty();
        if has_types {
            writeln!(result, "  interface types {{")?;
            writeln!(
                result,
                "    use golem:agent/common.{{text-reference, binary-reference}};"
            )?;
            for (typ, name) in types {
                self.generate_type_definition(&mut result, &typ, &name)?;
            }
            writeln!(result, "  }}")?;
        }

        writeln!(result)?;
        writeln!(result, "world agent-wrapper {{")?;
        writeln!(result, "  import golem:agent/guest;")?;
        writeln!(result, "  export golem:agent/guest;")?;
        for interface_name in &interface_names {
            writeln!(result, "  export {interface_name};")?;
        }
        if has_types {
            writeln!(result, "  export types;")?;
        }
        writeln!(result, "}}")?;

        self.wrapper_package_wit_source = Some(result);
        Ok(())
    }

    fn generate_single_file_wrapper_wit(&mut self) -> anyhow::Result<()> {
        if self.single_file_wrapper_wit_source.is_some() {
            return Err(anyhow!(
                "generate_single_file_wrapper_wit has been called already"
            ));
        }

        let resolved = ResolvedWrapper::new(
            self.wrapper_package_wit_source
                .as_ref()
                .ok_or_else(|| anyhow!("Must call generate_wit_source first"))?
                .clone(),
        )?;

        self.single_file_wrapper_wit_source = Some(resolved.into_single_file_wrapper_wit()?);
        Ok(())
    }

    fn finalize(self) -> anyhow::Result<AgentWrapperGeneratorContext> {
        Ok(AgentWrapperGeneratorContext {
            type_names: self.type_names,
            used_names: self.used_names,
            multimodal_variants: self.multimodal_variants,
            wrapper_package_wit_source: self
                .wrapper_package_wit_source
                .ok_or_else(|| anyhow!("Must call generate_single_file_wrapper_wit first"))?,
            single_file_wrapper_wit_source: self
                .single_file_wrapper_wit_source
                .ok_or_else(|| anyhow!("Must call generate_wit_source first"))?,
            agent_types: self.agent_types,
        })
    }

    fn generate_agent_wrapper_interface(
        &mut self,
        result: &mut String,
        agent: &AgentType,
    ) -> anyhow::Result<String> {
        let interface_name = agent.type_name.to_kebab_case();
        let previous_used_names = self.used_names.clone();
        self.used_names.clear();

        writeln!(result, "/// {}", agent.description)?;
        writeln!(result, "interface {interface_name} {{")?;
        writeln!(
            result,
            "  use golem:agent/common.{{agent-error, agent-type, binary-reference, text-reference}};"
        )?;
        writeln!(result)?;

        writeln!(result, "  /// {}", agent.constructor.description)?;
        write!(result, "  initialize: func(")?;
        self.write_parameter_list(result, &agent.constructor.input_schema, "constructor")?;
        writeln!(result, ") -> result<_, agent-error>;")?;

        writeln!(result)?;
        writeln!(result, "  get-definition: func() -> agent-type;")?;
        writeln!(result)?;

        for method in &agent.methods {
            let name = method.name.to_kebab_case();
            writeln!(result, "  /// {}", method.description)?;
            write!(result, "  {name}: func(")?;
            self.write_parameter_list(result, &method.input_schema, &name)?;
            write!(result, ") -> result<")?;
            self.write_return_type(result, &method.output_schema, &name)?;
            writeln!(result, ", agent-error>;")?;
        }

        let mut used_names = self.used_names.iter().cloned().collect::<Vec<_>>();
        used_names.sort();
        if !used_names.is_empty() {
            let used_names_list = used_names.join(", ");
            writeln!(result)?;
            writeln!(result, "  use types.{{{used_names_list}}};")?;
        }

        self.used_names = previous_used_names
            .union(&self.used_names)
            .cloned()
            .collect();
        writeln!(result, "}}")?;

        Ok(interface_name)
    }

    fn traverse_gathered_types(&mut self) -> anyhow::Result<()> {
        let mut visited = HashSet::new();
        let mut stack = Vec::new();
        for typ in self.type_names.keys() {
            stack.push(typ.clone());
        }

        while let Some(typ) = stack.pop() {
            if !visited.contains(&typ) {
                visited.insert(typ.clone());
                match typ {
                    AnalysedType::Variant(variant) => {
                        for case in &variant.cases {
                            if let Some(ty) = &case.typ {
                                stack.push(ty.clone());
                            }
                        }
                    }
                    AnalysedType::Result(result) => {
                        if let Some(ty) = &result.ok {
                            stack.push((**ty).clone());
                        }
                        if let Some(ty) = &result.err {
                            stack.push((**ty).clone());
                        }
                    }
                    AnalysedType::Option(option) => {
                        stack.push((*option.inner).clone());
                    }
                    AnalysedType::Record(record) => {
                        for field in &record.fields {
                            stack.push(field.typ.clone());
                        }
                    }
                    AnalysedType::Tuple(items) => {
                        for item in &items.items {
                            stack.push(item.clone());
                        }
                    }
                    AnalysedType::List(list) => {
                        stack.push((*list.inner).clone());
                    }
                    _ => {}
                }
            }
        }

        for typ in visited {
            if matches!(
                typ,
                AnalysedType::Variant(_)
                    | AnalysedType::Enum(_)
                    | AnalysedType::Flags(_)
                    | AnalysedType::Record(_)
            ) {
                self.register_type(&typ);
            }
        }

        Ok(())
    }

    fn generate_type_definition(
        &mut self,
        result: &mut String,
        typ: &AnalysedType,
        name: &str,
    ) -> anyhow::Result<()> {
        match typ {
            AnalysedType::Variant(variant) => {
                writeln!(result, "    variant {name} {{")?;
                for case in &variant.cases {
                    write!(result, "      {}", case.name.to_kebab_case())?;
                    if let Some(typ) = &case.typ {
                        write!(result, "({})", self.wit_type_reference(typ)?)?;
                    }
                    writeln!(result, ",")?;
                }
                writeln!(result, "    }}")?;
            }
            AnalysedType::Enum(enum_) => {
                writeln!(result, "    enum {name} {{")?;
                for case in &enum_.cases {
                    let case = case.to_kebab_case();
                    writeln!(result, "      {case},")?;
                }
                writeln!(result, "    }}")?;
            }
            AnalysedType::Flags(flags) => {
                writeln!(result, "    flags {name} {{")?;
                for case in &flags.names {
                    let case = case.to_kebab_case();
                    writeln!(result, "      {case},")?;
                }
                writeln!(result, "    }}")?;
            }
            AnalysedType::Record(fields) => {
                writeln!(result, "    record {name} {{")?;
                for field in &fields.fields {
                    writeln!(
                        result,
                        "      {}: {},",
                        field.name.to_kebab_case(),
                        self.wit_type_reference(&field.typ)?
                    )?;
                }
                writeln!(result, "    }}")?;
            }
            _ => {
                writeln!(
                    result,
                    "    type {name} = {};",
                    self.wit_type_reference(typ)?
                )?;
            }
        }
        Ok(())
    }

    fn write_parameter_list(
        &mut self,
        result: &mut String,
        input: &DataSchema,
        context: &str,
    ) -> anyhow::Result<()> {
        match input {
            DataSchema::Tuple(NamedElementSchemas { elements }) => {
                for (n, element) in elements.iter().enumerate() {
                    if n > 0 {
                        write!(result, ", ")?;
                    }

                    let param_name = &element.name.to_kebab_case();
                    write!(result, "{param_name}: ")?;
                    self.write_element_schema_type_ref(result, &element.schema)?;
                }
            }
            DataSchema::Multimodal(NamedElementSchemas { elements: cases }) => {
                let variant = Self::multimodal_variant(cases).named(format!("{context}-input"));
                let name = self.register_multimodal_variant(&variant);
                write!(result, "input: list<{name}>")?;
            }
        }
        Ok(())
    }

    fn write_return_type(
        &mut self,
        result: &mut String,
        schema: &DataSchema,
        context: &str,
    ) -> anyhow::Result<()> {
        match schema {
            DataSchema::Tuple(NamedElementSchemas { elements }) if elements.is_empty() => {
                write!(result, "_")?;
            }
            DataSchema::Tuple(NamedElementSchemas { elements }) if elements.len() == 1 => {
                self.write_element_schema_type_ref(
                    result,
                    &elements.iter().next().as_ref().unwrap().schema,
                )?;
            }
            DataSchema::Tuple(NamedElementSchemas { elements }) => {
                write!(result, "tuple<")?;
                for (n, element) in elements.iter().enumerate() {
                    if n > 0 {
                        write!(result, ", ")?;
                    }

                    self.write_element_schema_type_ref(result, &element.schema)?;
                }
                write!(result, ">")?;
            }
            DataSchema::Multimodal(NamedElementSchemas { elements: cases }) => {
                let variant = Self::multimodal_variant(cases).named(format!("{context}-output"));
                let name = self.register_multimodal_variant(&variant);
                write!(result, "list<{name}>")?;
            }
        }
        Ok(())
    }

    fn write_element_schema_type_ref(
        &mut self,
        result: &mut String,
        element: &ElementSchema,
    ) -> anyhow::Result<()> {
        match element {
            ElementSchema::ComponentModel(typ) => {
                write!(result, "{}", self.wit_type_reference(typ)?)?;
            }
            ElementSchema::UnstructuredText(_text_descriptor) => {
                write!(result, "text-reference")?;
            }
            ElementSchema::UnstructuredBinary(_bin_descriptor) => {
                write!(result, "binary-reference")?;
            }
        }
        Ok(())
    }

    fn wit_type_reference(&mut self, typ: &AnalysedType) -> anyhow::Result<String> {
        match typ {
            AnalysedType::Variant(_)
            | AnalysedType::Enum(_)
            | AnalysedType::Flags(_)
            | AnalysedType::Record(_) => {
                let name = self.register_type(typ);
                Ok(name)
            }
            AnalysedType::Result(result) => {
                let ok_type_ref = if let Some(ok) = &result.ok {
                    self.wit_type_reference(ok)?
                } else {
                    "_".to_string()
                };
                let err_type_ref = if let Some(err) = &result.err {
                    self.wit_type_reference(err)?
                } else {
                    "".to_string()
                };

                if err_type_ref.is_empty() {
                    if ok_type_ref == "_" {
                        Ok("result".to_string())
                    } else {
                        Ok(format!("result<{ok_type_ref}>"))
                    }
                } else {
                    Ok(format!("result<{ok_type_ref}, {err_type_ref}>"))
                }
            }
            AnalysedType::Option(opt) => {
                let inner_type_ref = self.wit_type_reference(&opt.inner)?;
                Ok(format!("option<{inner_type_ref}>"))
            }

            AnalysedType::Tuple(tuple) => {
                let inner_type_refs = tuple
                    .items
                    .iter()
                    .map(|t| self.wit_type_reference(t))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!("tuple<{}>", inner_type_refs.join(", ")))
            }
            AnalysedType::List(list) => {
                let inner_type_ref = self.wit_type_reference(&list.inner)?;
                Ok(format!("list<{inner_type_ref}>"))
            }
            AnalysedType::Str(_) => Ok("string".to_string()),
            AnalysedType::Chr(_) => Ok("char".to_string()),
            AnalysedType::F64(_) => Ok("f64".to_string()),
            AnalysedType::F32(_) => Ok("f32".to_string()),
            AnalysedType::U64(_) => Ok("u64".to_string()),
            AnalysedType::S64(_) => Ok("s64".to_string()),
            AnalysedType::U32(_) => Ok("u32".to_string()),
            AnalysedType::S32(_) => Ok("s32".to_string()),
            AnalysedType::U16(_) => Ok("u16".to_string()),
            AnalysedType::S16(_) => Ok("s16".to_string()),
            AnalysedType::U8(_) => Ok("u8".to_string()),
            AnalysedType::S8(_) => Ok("s8".to_string()),
            AnalysedType::Bool(_) => Ok("bool".to_string()),
            AnalysedType::Handle(_) => Err(anyhow!(
                "Handles are not supported on agent interfaces currently."
            )),
        }
    }

    fn register_multimodal_variant(&mut self, typ: &AnalysedType) -> String {
        let name = self.register_type(typ);
        self.multimodal_variants
            .insert(typ.name().unwrap().to_string(), name.clone());
        name
    }

    fn register_type(&mut self, typ: &AnalysedType) -> String {
        if let Some(name) = self.type_names.get(typ) {
            self.used_names.insert(name.clone());
            name.clone()
        } else {
            let proposed_name = typ
                .name()
                .map(|n| n.to_string())
                .unwrap_or("element".to_string())
                .to_kebab_case();
            let name = self.find_unused_name(proposed_name);
            self.type_names.insert(typ.clone(), name.clone());
            self.used_names.insert(name.clone());
            name
        }
    }

    fn find_unused_name(&self, name: String) -> String {
        let mut current = name.clone();
        let mut counter = 1;
        while self.used_names.contains(&current) {
            current = format!("{name}{counter}");
            counter += 1;
        }
        current
    }

    fn multimodal_variant(cases: &[NamedElementSchema]) -> AnalysedType {
        let mut variant_cases = Vec::new();
        for named_element_schema in cases {
            let case_name = named_element_schema.name.to_kebab_case();
            let case_type = match &named_element_schema.schema {
                ElementSchema::ComponentModel(typ) => typ.clone(),
                ElementSchema::UnstructuredText(_) => variant(vec![]).named("text-reference"),
                ElementSchema::UnstructuredBinary(_) => variant(vec![]).named("binary-reference"),
            };
            variant_cases.push(case(&case_name, case_type));
        }
        variant(variant_cases)
    }
}

struct ResolvedWrapper {
    resolve: Resolve,
    package_id: PackageId,
    golem_agent_package_id: PackageId,
    golem_rpc_package_id: PackageId,
    golem_host_package_id: PackageId,
    wasi_clocks_package_id: PackageId,
    wasi_io_package_id: PackageId,
}

impl ResolvedWrapper {
    /// Create a resolve of the generated WIT and push the golem:agent package in it
    pub fn new(package_source: String) -> anyhow::Result<Self> {
        let mut resolve = Resolve::new();

        let wasi_io_package_id = add_wasi_io(&mut resolve)?;
        let wasi_clocks_package_id = add_wasi_clocks(&mut resolve)?;
        let golem_rpc_package_id = add_golem_rpc(&mut resolve)?;
        let golem_host_package_id = add_golem_host(&mut resolve)?;
        let golem_agent_package_id = add_golem_agent(&mut resolve)?;

        let package_id = resolve
            .push_str("wrapper.wit", &package_source)
            .context(format!("Resolving generated WIT: {package_source}"))?;

        Ok(Self {
            resolve,
            package_id,
            golem_agent_package_id,
            golem_host_package_id,
            golem_rpc_package_id,
            wasi_clocks_package_id,
            wasi_io_package_id,
        })
    }

    /// Render a multi-package WIT and return it
    pub fn into_single_file_wrapper_wit(self) -> anyhow::Result<String> {
        let mut wit_printer = WitPrinter::default();
        wit_printer.print(
            &self.resolve,
            self.package_id,
            &[
                self.golem_agent_package_id,
                self.golem_rpc_package_id,
                self.wasi_clocks_package_id,
                self.wasi_io_package_id,
                self.golem_host_package_id,
            ],
        )?;
        Ok(wit_printer.output.to_string())
    }
}

fn add_wasi_io(resolve: &mut Resolve) -> anyhow::Result<PackageId> {
    const WASI_IO_ERROR_WIT: &str = include_str!("../../../wit/deps/io/error.wit");
    const WASI_IO_POLL_WIT: &str = include_str!("../../../wit/deps/io/poll.wit");
    const WASI_IO_STREAMS_WIT: &str = include_str!("../../../wit/deps/io/streams.wit");
    const WASI_IO_WORLD_WIT: &str = include_str!("../../../wit/deps/io/world.wit");

    let mut source_map = SourceMap::default();
    source_map.push(Path::new("wit/deps/io/error.wit"), WASI_IO_ERROR_WIT);
    source_map.push(Path::new("wit/deps/io/poll.wit"), WASI_IO_POLL_WIT);
    source_map.push(Path::new("wit/deps/io/streams.wit"), WASI_IO_STREAMS_WIT);
    source_map.push(Path::new("wit/deps/io/world.wit"), WASI_IO_WORLD_WIT);
    let package_group = source_map.parse()?;
    resolve.push_group(package_group)
}

fn add_wasi_clocks(resolve: &mut Resolve) -> anyhow::Result<PackageId> {
    const WASI_CLOCKS_MONOTONIC_CLOCK_WIT: &str =
        include_str!("../../../wit/deps/clocks/monotonic-clock.wit");
    const WASI_CLOCKS_TIMEZONE_WIT: &str = include_str!("../../../wit/deps/clocks/timezone.wit");
    const WASI_CLOCKS_WALL_CLOCK_WIT: &str =
        include_str!("../../../wit/deps/clocks/wall-clock.wit");
    const WASI_CLOCKS_WORLD: &str = include_str!("../../../wit/deps/clocks/world.wit");

    let mut source_map = SourceMap::default();
    source_map.push(
        Path::new("wit/deps/clocks/monotonic-clock.wit"),
        WASI_CLOCKS_MONOTONIC_CLOCK_WIT,
    );
    source_map.push(
        Path::new("wit/deps/clocks/timezone.wit"),
        WASI_CLOCKS_TIMEZONE_WIT,
    );
    source_map.push(
        Path::new("wit/deps/clocks/wall-clock.wit"),
        WASI_CLOCKS_WALL_CLOCK_WIT,
    );
    source_map.push(Path::new("wit/deps/clocks/world.wit"), WASI_CLOCKS_WORLD);

    let package_group = source_map.parse()?;
    resolve.push_group(package_group)
}

fn add_golem_rpc(resolve: &mut Resolve) -> anyhow::Result<PackageId> {
    const GOLEM_RPC_WIT: &str = include_str!("../../../wit/deps/golem-rpc/wasm-rpc.wit");
    resolve.push_str("wit/deps/golem-rpc/wasm-rpc.wit", GOLEM_RPC_WIT)
}

fn add_golem_host(resolve: &mut Resolve) -> anyhow::Result<PackageId> {
    const GOLEM_CONTEXT_WIT: &str = include_str!("../../../wit/deps/golem-1.x/golem-context.wit");
    const GOLEM_HOST_WIT: &str = include_str!("../../../wit/deps/golem-1.x/golem-host.wit");
    const GOLEM_OPLOG_WIT: &str = include_str!("../../../wit/deps/golem-1.x/golem-oplog.wit");
    const GOLEM_OPLOG_PROCESSOR_WIT: &str =
        include_str!("../../../wit/deps/golem-1.x/golem-oplog-processor.wit");

    let mut source_map = SourceMap::default();
    source_map.push(
        Path::new("wit/deps/golem-1.x/golem-context.wit"),
        GOLEM_CONTEXT_WIT,
    );
    source_map.push(
        Path::new("wit/deps/golem-1.x/golem-host.wit"),
        GOLEM_HOST_WIT,
    );
    source_map.push(
        Path::new("wit/deps/golem-1.x/golem-oplog.wit"),
        GOLEM_OPLOG_WIT,
    );
    source_map.push(
        Path::new("wit/deps/golem-1.x/golem-oplog-processor.wit"),
        GOLEM_OPLOG_PROCESSOR_WIT,
    );
    let package_group = source_map.parse()?;
    resolve.push_group(package_group)
}

fn add_golem_agent(resolve: &mut Resolve) -> anyhow::Result<PackageId> {
    const GOLEM_AGENT_GUEST_WIT: &str = include_str!("../../../wit/deps/golem-agent/guest.wit");
    const GOLEM_AGENT_HOST_WIT: &str = include_str!("../../../wit/deps/golem-agent/host.wit");
    const GOLEM_AGENT_COMMON_WIT: &str = include_str!("../../../wit/deps/golem-agent/common.wit");

    let mut golem_agent_source = SourceMap::default();
    golem_agent_source.push(
        Path::new("wit/deps/golem-agent/common.wit"),
        GOLEM_AGENT_COMMON_WIT,
    );
    golem_agent_source.push(
        Path::new("wit/deps/golem-agent/guest.wit"),
        GOLEM_AGENT_GUEST_WIT,
    );
    golem_agent_source.push(
        Path::new("wit/deps/golem-agent/host.wit"),
        GOLEM_AGENT_HOST_WIT,
    );
    let golem_agent_pkg_group = golem_agent_source.parse()?;
    resolve.push_group(golem_agent_pkg_group)
}

#[cfg(test)]
mod tests {
    use crate::model::agent::test;
    use golem_common::model::agent::{
        AgentConstructor, AgentMethod, AgentType, BinaryDescriptor, DataSchema, ElementSchema,
        NamedElementSchema, NamedElementSchemas, TextDescriptor,
    };
    use golem_wasm_ast::analysis::analysed_type::{
        case, field, option, r#enum, record, str, u32, unit_case, variant,
    };
    use indoc::indoc;
    use test_r::test;

    #[test]
    fn empty_agent_wrapper() {
        let component_name = "example:empty".into();
        let agent_types = vec![];
        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        println!("{wit}");
        assert!(wit.contains(indoc!(
            r#"package example:empty;

            world agent-wrapper {
              import wasi:clocks/wall-clock@0.2.3;
              import wasi:io/poll@0.2.3;
              import golem:rpc/types@0.2.2;
              import golem:agent/common;
              import golem:agent/guest;

              export golem:agent/guest;
            }
            "#
        )));
    }

    #[test]
    fn single_agent_wrapper_1() {
        let component_name = "example:single1".into();
        let agent_types = vec![AgentType {
            type_name: "agent1".to_string(),
            description: "An example agent".to_string(),
            constructor: AgentConstructor {
                name: None,
                description: "Creates an example agent instance".into(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![
                        NamedElementSchema {
                            name: "a".to_string(),
                            schema: ElementSchema::ComponentModel(u32()),
                        },
                        NamedElementSchema {
                            name: "b".to_string(),
                            schema: ElementSchema::ComponentModel(option(str())),
                        },
                    ],
                }),
            },
            methods: vec![
                AgentMethod {
                    name: "f1".to_string(),
                    description: "returns a random string".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "a".to_string(),
                            schema: ElementSchema::ComponentModel(str()),
                        }],
                    }),
                },
                AgentMethod {
                    name: "f2".to_string(),
                    description: "adds two numbers".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![
                            NamedElementSchema {
                                name: "x".to_string(),
                                schema: ElementSchema::ComponentModel(u32()),
                            },
                            NamedElementSchema {
                                name: "y".to_string(),
                                schema: ElementSchema::ComponentModel(u32()),
                            },
                        ],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(u32()),
                        }],
                    }),
                },
            ],
            dependencies: vec![],
        }];
        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        println!("{wit}");
        assert!(wit.contains(indoc!(
            r#"package example:single1;

            /// An example agent
            interface agent1 {
              use golem:agent/common.{agent-error, agent-type, binary-reference, text-reference};

              /// Creates an example agent instance
              initialize: func(a: u32, b: option<string>) -> result<_, agent-error>;

              get-definition: func() -> agent-type;

              /// returns a random string
              f1: func() -> result<string, agent-error>;

              /// adds two numbers
              f2: func(x: u32, y: u32) -> result<u32, agent-error>;
            }

            world agent-wrapper {
              import wasi:clocks/wall-clock@0.2.3;
              import wasi:io/poll@0.2.3;
              import golem:rpc/types@0.2.2;
              import golem:agent/common;
              import golem:agent/guest;

              export golem:agent/guest;
              export agent1;
            }
            "#
        )));
    }

    #[test]
    fn single_agent_wrapper_2() {
        let component_name = "example:single2".into();

        let color = r#enum(&["red", "green", "blue"]).named("color");

        let person = record(vec![
            field("first-name", str()),
            field("last-name", str()),
            field("age", option(u32())),
            field("eye-color", color.clone()),
        ])
        .named("person");

        let location = variant(vec![
            case("home", str()),
            case("work", str()),
            unit_case("unknown"),
        ])
        .named("location");

        let agent_types = vec![AgentType {
            type_name: "agent1".to_string(),
            description: "An example agent".to_string(),
            constructor: AgentConstructor {
                name: None,
                description: "Creates an example agent instance".into(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![
                        NamedElementSchema {
                            name: "person".to_string(),
                            schema: ElementSchema::ComponentModel(person),
                        },
                        NamedElementSchema {
                            name: "description".to_string(),
                            schema: ElementSchema::UnstructuredText(TextDescriptor {
                                restrictions: None,
                            }),
                        },
                        NamedElementSchema {
                            name: "photo".to_string(),
                            schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                                restrictions: None,
                            }),
                        },
                    ],
                }),
            },
            methods: vec![
                AgentMethod {
                    name: "f1".to_string(),
                    description: "returns a location".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas { elements: vec![] }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(location.clone()),
                        }],
                    }),
                },
                AgentMethod {
                    name: "f2".to_string(),
                    description: "takes a location and returns a color".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "location".to_string(),
                            schema: ElementSchema::ComponentModel(location),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(color),
                        }],
                    }),
                },
            ],
            dependencies: vec![],
        }];
        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        println!("{wit}");
        assert!(wit.contains(indoc!(
            r#"package example:single2;

            interface types {
              use golem:agent/common.{text-reference, binary-reference};

              enum color {
                red,
                green,
                blue,
              }

              variant location {
                home(string),
                work(string),
                unknown,
              }

              record person {
                first-name: string,
                last-name: string,
                age: option<u32>,
                eye-color: color,
              }
            }

            /// An example agent
            interface agent1 {
              use golem:agent/common.{agent-error, agent-type, binary-reference, text-reference};
              use types.{color, location, person};

              /// Creates an example agent instance
              initialize: func(person: person, description: text-reference, photo: binary-reference) -> result<_, agent-error>;

              get-definition: func() -> agent-type;

              /// returns a location
              f1: func() -> result<location, agent-error>;

              /// takes a location and returns a color
              f2: func(location: location) -> result<color, agent-error>;
            }

            world agent-wrapper {
              import wasi:clocks/wall-clock@0.2.3;
              import wasi:io/poll@0.2.3;
              import golem:rpc/types@0.2.2;
              import golem:agent/common;
              import golem:agent/guest;

              export golem:agent/guest;
              export types;
              export agent1;
            }
            "#
        )));
    }

    #[test]
    fn multi_agent_wrapper_2() {
        let component_name = "example:multi1".into();
        let agent_types = test::multi_agent_wrapper_2_types();

        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        println!("{wit}");
        assert!(wit.contains(indoc!(
            r#"package example:multi1;

            interface types {
              use golem:agent/common.{text-reference, binary-reference};

              enum color {
                red,
                green,
                blue,
              }

              variant f2-output {
                text(text-reference),
                image(binary-reference),
              }

              variant location {
                home(string),
                work(string),
                unknown,
              }

              variant f2-input {
                place(location),
                color(color),
              }

              record person {
                first-name: string,
                last-name: string,
                age: option<u32>,
                eye-color: color,
              }
            }

            /// An example agent
            interface agent1 {
              use golem:agent/common.{agent-error, agent-type, binary-reference, text-reference};
              use types.{location, person};

              /// Creates an example agent instance
              initialize: func(person: person, description: text-reference, photo: binary-reference) -> result<_, agent-error>;

              get-definition: func() -> agent-type;

              /// returns a location
              f1: func() -> result<location, agent-error>;
            }

            /// Another example agent
            interface agent2 {
              use golem:agent/common.{agent-error, agent-type, binary-reference, text-reference};
              use types.{f2-input, f2-output, person};

              /// Creates another example agent instance
              initialize: func(person-group: list<person>) -> result<_, agent-error>;

              get-definition: func() -> agent-type;

              /// takes a location or a color and returns a text or an image
              f2: func(input: list<f2-input>) -> result<list<f2-output>, agent-error>;
            }

            world agent-wrapper {
              import wasi:clocks/wall-clock@0.2.3;
              import wasi:io/poll@0.2.3;
              import golem:rpc/types@0.2.2;
              import golem:agent/common;
              import golem:agent/guest;

              export golem:agent/guest;
              export types;
              export agent1;
              export agent2;
            }
            "#
        )));
    }
}
