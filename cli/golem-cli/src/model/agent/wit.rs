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

use anyhow::{anyhow, bail, Context};
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::{
    AgentType, DataSchema, ElementSchema, NamedElementSchema, NamedElementSchemas,
};
use golem_common::model::component::ComponentName;
use golem_wasm::analysis::analysed_type::{case, variant};
use golem_wasm::analysis::AnalysedType;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::path::Path;
use wit_component::WitPrinter;
use wit_parser::{PackageId, Resolve, SourceMap};

pub fn generate_agent_wrapper_wit(
    component_name: &ComponentName,
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
    pub has_types: bool,
}

pub struct AgentWrapperGeneratorContextState {
    agent_types: Vec<AgentType>,
    type_names: HashMap<AnalysedType, String>,
    used_names: HashSet<String>,
    multimodal_variants: HashMap<String, String>,
    wrapper_package_wit_source: Option<String>,
    single_file_wrapper_wit_source: Option<String>,
    has_types: bool,
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
            has_types: false,
        }
    }

    /// Generate the wrapper WIT that also imports and exports golem:agent/guest
    fn generate_wit_source(&mut self, component_name: &ComponentName) -> anyhow::Result<()> {
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
        self.has_types = has_types;
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
        writeln!(result, "  import golem:api/save-snapshot@1.3.0;")?;
        writeln!(result, "  import golem:api/load-snapshot@1.3.0;")?;
        writeln!(result, "  import wasi:logging/logging;")?;
        writeln!(result, "  export golem:agent/guest;")?;
        writeln!(result, "  export golem:api/save-snapshot@1.3.0;")?;
        writeln!(result, "  export golem:api/load-snapshot@1.3.0;")?;
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
            has_types: self.has_types,
        })
    }

    fn generate_agent_wrapper_interface(
        &mut self,
        result: &mut String,
        agent: &AgentType,
    ) -> anyhow::Result<String> {
        let interface_name = escape_wit_keyword(&agent.type_name.to_wit_naming().to_string());

        writeln!(result, "/// {}", agent.description)?;
        writeln!(result, "interface {interface_name} {{")?;
        writeln!(
            result,
            "  use golem:agent/common.{{agent-type, binary-reference, text-reference}};"
        )?;
        writeln!(result)?;

        writeln!(result, "  /// {}", agent.constructor.description)?;
        write!(result, "  initialize: func(")?;
        self.write_parameter_list(result, &agent.constructor.input_schema, "constructor")?;
        writeln!(result, ");")?;

        writeln!(result)?;
        writeln!(result, "  get-definition: func() -> agent-type;")?;
        writeln!(result)?;

        for method in &agent.methods {
            let name = escape_wit_keyword(&method.name.to_wit_naming());
            writeln!(result, "  /// {}", method.description)?;
            write!(result, "  {name}: func(")?;
            self.write_parameter_list(result, &method.input_schema, &name)?;
            writeln!(result, ")")?;
            if !method.output_schema.is_unit() {
                write!(result, " -> ")?;
                self.write_return_type(result, &method.output_schema, &name)?;
            }
            writeln!(result, ";")?;
        }

        let mut used_names = self.used_names.iter().cloned().collect::<Vec<_>>();
        used_names.sort();
        if !used_names.is_empty() {
            let used_names_list = used_names.join(", ");
            writeln!(result)?;
            writeln!(result, "  use types.{{{used_names_list}}};")?;
        }

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
                    write!(
                        result,
                        "      {}",
                        escape_wit_keyword(&case.name.to_wit_naming())
                    )?;
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
                    let case = escape_wit_keyword(&case.to_wit_naming());
                    writeln!(result, "      {case},")?;
                }
                writeln!(result, "    }}")?;
            }
            AnalysedType::Flags(flags) => {
                writeln!(result, "    flags {name} {{")?;
                for case in &flags.names {
                    let case = escape_wit_keyword(&case.to_wit_naming());
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
                        escape_wit_keyword(&field.name.to_wit_naming()),
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

                    let param_name = &escape_wit_keyword(&element.name.to_wit_naming());
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
            ElementSchema::ComponentModel(schema) => {
                write!(result, "{}", self.wit_type_reference(&schema.element_type)?)?;
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
                .to_wit_naming();
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
            let case_name = escape_wit_keyword(&named_element_schema.name.to_wit_naming());
            let case_type = match &named_element_schema.schema {
                ElementSchema::ComponentModel(schema) => schema.element_type.clone(),
                ElementSchema::UnstructuredText(_) => variant(vec![]).named("text-reference"),
                ElementSchema::UnstructuredBinary(_) => variant(vec![]).named("binary-reference"),
            };
            variant_cases.push(case(&case_name, case_type));
        }
        variant(variant_cases)
    }
}

pub(crate) const WIT_KEYWORDS: &[&str] = &[
    "bool",
    "char",
    "enum",
    "export",
    "f32",
    "f64",
    "flags",
    "func",
    "import",
    "interface",
    "list",
    "option",
    "package",
    "record",
    "resource",
    "result",
    "s16",
    "s32",
    "s64",
    "s8",
    "static",
    "string",
    "tuple",
    "type",
    "u16",
    "u32",
    "u64",
    "u8",
    "use",
    "variant",
    "world",
];

fn escape_wit_keyword(name: &str) -> String {
    if WIT_KEYWORDS.contains(&name) {
        format!("%{}", name)
    } else {
        name.to_string()
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
    wasi_logging_package_id: PackageId,
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
        let wasi_logging_package_id = add_wasi_logging(&mut resolve)?;

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
            wasi_logging_package_id,
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
                self.wasi_logging_package_id,
            ],
        )?;
        Ok(wit_printer.output.to_string())
    }
}

fn add_wasi_logging(resolve: &mut Resolve) -> anyhow::Result<PackageId> {
    const WASI_LOGGING_WIT: &str = include_str!("../../../wit/deps/logging/logging.wit");
    let mut source_map = SourceMap::default();

    source_map.push(Path::new("wit/deps/logging/logging.wit"), WASI_LOGGING_WIT);
    let package_group = source_map.parse()?;
    resolve.push_group(package_group)
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
    use crate::model::agent::test::{
        agent_type_with_wit_keywords, reproducer_for_issue_with_enums,
        reproducer_for_issue_with_result_types, reproducer_for_multiple_types_called_element,
        single_agent_wrapper_types,
    };

    use golem_common::model::agent::{
        AgentConstructor, AgentMethod, AgentMode, AgentType, BinaryDescriptor,
        ComponentModelElementSchema, DataSchema, ElementSchema, NamedElementSchema,
        NamedElementSchemas, TextDescriptor,
    };
    use golem_common::model::component::ComponentName;
    use golem_templates::model::GuestLanguage;
    use golem_wasm::analysis::analysed_type::{
        case, field, option, r#enum, record, str, u32, unit_case, variant,
    };
    use indoc::indoc;
    use test_r::test;

    #[test]
    fn empty_agent_wrapper() {
        let component_name = ComponentName("example:empty".to_string());
        let agent_types = vec![];
        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        //println!("{wit}");
        assert_wit(
            &wit,
            indoc!(
                r#"package example:empty;

            world agent-wrapper {
              import wasi:clocks/wall-clock@0.2.3;
              import wasi:io/poll@0.2.3;
              import golem:rpc/types@0.2.2;
              import golem:agent/common;
              import golem:agent/guest;
              import golem:api/save-snapshot@1.3.0;
              import golem:api/load-snapshot@1.3.0;
              import wasi:logging/logging;

              export golem:agent/guest;
              export golem:api/save-snapshot@1.3.0;
              export golem:api/load-snapshot@1.3.0;
            }
            "#
            ),
        );
    }

    #[test]
    fn single_agent_wrapper_1() {
        let component_name = ComponentName("example:single1".to_string());
        let agent_types = single_agent_wrapper_types();
        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        //println!("{wit}");
        assert_wit(
            &wit,
            indoc!(
                r#"package example:single1;

            /// An example agent
            interface agent1 {
              use golem:agent/common.{agent-type, binary-reference, text-reference};

              /// Creates an example agent instance
              initialize: func(a: u32, b: option<string>);

              get-definition: func() -> agent-type;

              /// returns a random string
              f1: func() -> string;

              /// adds two numbers
              f2: func(x: u32, y: u32) -> u32;
            }

            world agent-wrapper {
              import wasi:clocks/wall-clock@0.2.3;
              import wasi:io/poll@0.2.3;
              import golem:rpc/types@0.2.2;
              import golem:agent/common;
              import golem:agent/guest;
              import golem:api/save-snapshot@1.3.0;
              import golem:api/load-snapshot@1.3.0;
              import wasi:logging/logging;

              export golem:agent/guest;
              export golem:api/save-snapshot@1.3.0;
              export golem:api/load-snapshot@1.3.0;
              export agent1;
            }
            "#
            ),
        );
    }

    #[test]
    fn single_agent_wrapper_2() {
        let component_name = ComponentName("example:single2".to_string());

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
            type_name: golem_common::model::agent::AgentTypeName("agent1".to_string()),
            description: "An example agent".to_string(),
            constructor: AgentConstructor {
                name: None,
                description: "Creates an example agent instance".into(),
                prompt_hint: None,
                input_schema: DataSchema::Tuple(NamedElementSchemas {
                    elements: vec![
                        NamedElementSchema {
                            name: "person".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: person,
                            }),
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
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: location.clone(),
                            }),
                        }],
                    }),
                    http_endpoint: Vec::new(),
                },
                AgentMethod {
                    name: "f2".to_string(),
                    description: "takes a location and returns a color".to_string(),
                    prompt_hint: None,
                    input_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "location".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: location,
                            }),
                        }],
                    }),
                    output_schema: DataSchema::Tuple(NamedElementSchemas {
                        elements: vec![NamedElementSchema {
                            name: "return".to_string(),
                            schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                                element_type: color,
                            }),
                        }],
                    }),
                    http_endpoint: Vec::new(),
                },
            ],
            dependencies: vec![],
            mode: AgentMode::Durable,
            http_mount: None,
        }];
        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        //println!("{wit}");
        assert_wit(
            &wit,
            indoc!(
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
              use golem:agent/common.{agent-type, binary-reference, text-reference};
              use types.{color, location, person};

              /// Creates an example agent instance
              initialize: func(person: person, description: text-reference, photo: binary-reference);

              get-definition: func() -> agent-type;

              /// returns a location
              f1: func() -> location;

              /// takes a location and returns a color
              f2: func(location: location) -> color;
            }

            world agent-wrapper {
              import wasi:clocks/wall-clock@0.2.3;
              import wasi:io/poll@0.2.3;
              import golem:rpc/types@0.2.2;
              import golem:agent/common;
              import golem:agent/guest;
              import golem:api/save-snapshot@1.3.0;
              import golem:api/load-snapshot@1.3.0;
              import wasi:logging/logging;

              export golem:agent/guest;
              export golem:api/save-snapshot@1.3.0;
              export golem:api/load-snapshot@1.3.0;
              export types;
              export agent1;
            }
            "#
            ),
        );
    }

    #[test]
    fn multi_agent_wrapper_2() {
        let component_name = ComponentName("example:multi1".to_string());
        let agent_types = test::multi_agent_wrapper_2_types();

        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        //println!("{wit}");
        assert_wit(
            &wit,
            indoc!(
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
              use golem:agent/common.{agent-type, binary-reference, text-reference};
              use types.{location, person};

              /// Creates an example agent instance
              initialize: func(person: person, description: text-reference, photo: binary-reference);

              get-definition: func() -> agent-type;

              /// returns a location
              f1: func() -> location;
            }

            /// Another example agent
            interface agent2 {
              use golem:agent/common.{agent-type, binary-reference, text-reference};
              use types.{f2-input, f2-output, location, person};

              /// Creates another example agent instance
              initialize: func(person-group: list<person>);

              get-definition: func() -> agent-type;

              /// takes a location or a color and returns a text or an image
              f2: func(input: list<f2-input>) -> list<f2-output>;
            }

            world agent-wrapper {
              import wasi:clocks/wall-clock@0.2.3;
              import wasi:io/poll@0.2.3;
              import golem:rpc/types@0.2.2;
              import golem:agent/common;
              import golem:agent/guest;
              import golem:api/save-snapshot@1.3.0;
              import golem:api/load-snapshot@1.3.0;
              import wasi:logging/logging;

              export golem:agent/guest;
              export golem:api/save-snapshot@1.3.0;
              export golem:api/load-snapshot@1.3.0;
              export types;
              export agent1;
              export agent2;
            }
            "#
            ),
        );
    }

    #[test]
    fn single_agent_wrapper_1_using_wit_keywords() {
        let component_name = ComponentName("example:single1".to_string());
        let agent_types = agent_type_with_wit_keywords();
        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        //println!("{wit}");
        assert_wit(
            &wit,
            indoc!(
                r#"package example:single1;

            /// An example agent using WIT keywords as names
            interface agent1 {
              use golem:agent/common.{agent-type, binary-reference, text-reference};

              /// Creates an example agent instance
              initialize: func(%export: u32, %func: option<string>);

              get-definition: func() -> agent-type;

              /// returns a random string
              %import: func() -> string;

              /// adds two numbers
              %package: func(%bool: u32, %char: u32, %enum: u32, %export: u32, %f32: u32, %f64: u32, %flags: u32, %func: u32, %import: u32, %interface: u32, %list: u32, %option: u32, %package: u32, %record: u32, %resource: u32, %result: u32, %s16: u32, %s32: u32, %s64: u32, %s8: u32, %static: u32, %string: u32, %tuple: u32, %type: u32, %u16: u32, %u32: u32, %u64: u32, %u8: u32, %use: u32, %variant: u32, %world: u32);
            }

            world agent-wrapper {
              import wasi:clocks/wall-clock@0.2.3;
              import wasi:io/poll@0.2.3;
              import golem:rpc/types@0.2.2;
              import golem:agent/common;
              import golem:agent/guest;
              import golem:api/save-snapshot@1.3.0;
              import golem:api/load-snapshot@1.3.0;
              import wasi:logging/logging;

              export golem:agent/guest;
              export golem:api/save-snapshot@1.3.0;
              export golem:api/load-snapshot@1.3.0;
              export agent1;
            }
            "#
            ),
        );
    }

    #[test]
    fn bug_multiple_types_called_element() {
        let component_name = ComponentName("example:bug".to_string());
        let agent_types = reproducer_for_multiple_types_called_element();
        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        //println!("{wit}");
        assert_wit(
            &wit,
            indoc!(
                r#"package example:bug;

            interface types {
              use golem:agent/common.{text-reference, binary-reference};

              record element {
                x: string,
              }

              record element1 {
                data: string,
                value: s32,
              }
            }

            /// AssistantAgent
            interface assistant-agent {
              use golem:agent/common.{agent-type, binary-reference, text-reference};
              use types.{element};

              /// Constructs [object Object]
              initialize: func();

              get-definition: func() -> agent-type;

              ask-more: func(name: string) -> element;
            }

            /// WeatherAgent
            interface weather-agent {
              use golem:agent/common.{agent-type, binary-reference, text-reference};
              use types.{element, element1};

              /// Constructs [object Object]
              initialize: func(username: string);

              get-definition: func() -> agent-type;

              /// Weather forecast weather for you
              get-weather: func(name: string, param2: element1) -> string;
            }

            world agent-wrapper {
              import wasi:clocks/wall-clock@0.2.3;
              import wasi:io/poll@0.2.3;
              import golem:rpc/types@0.2.2;
              import golem:agent/common;
              import golem:agent/guest;
              import golem:api/save-snapshot@1.3.0;
              import golem:api/load-snapshot@1.3.0;
              import wasi:logging/logging;

              export golem:agent/guest;
              export golem:api/save-snapshot@1.3.0;
              export golem:api/load-snapshot@1.3.0;
              export types;
              export assistant-agent;
              export weather-agent;
            }
            "#
            ),
        );
    }

    #[test]
    fn single_agent_wrapper_1_test_in_package_name() {
        let component_name = ComponentName("test:agent".to_string());
        let agent_types = single_agent_wrapper_types();
        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        //println!("{wit}");
        assert_wit(
            &wit,
            indoc!(
                r#"package test:agent;

            /// An example agent
            interface agent1 {
              use golem:agent/common.{agent-type, binary-reference, text-reference};

              /// Creates an example agent instance
              initialize: func(a: u32, b: option<string>);

              get-definition: func() -> agent-type;

              /// returns a random string
              f1: func() -> string;

              /// adds two numbers
              f2: func(x: u32, y: u32) -> u32;
            }

            world agent-wrapper {
              import wasi:clocks/wall-clock@0.2.3;
              import wasi:io/poll@0.2.3;
              import golem:rpc/types@0.2.2;
              import golem:agent/common;
              import golem:agent/guest;
              import golem:api/save-snapshot@1.3.0;
              import golem:api/load-snapshot@1.3.0;
              import wasi:logging/logging;

              export golem:agent/guest;
              export golem:api/save-snapshot@1.3.0;
              export golem:api/load-snapshot@1.3.0;
              export agent1;
            }
            "#
            ),
        );
    }

    #[test]
    pub fn enum_type() {
        let component_name = ComponentName("test:agent".to_string());
        let agent_types = reproducer_for_issue_with_enums();

        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        // println!("{wit}");
        assert_wit(
            &wit,
            indoc! {
                r#"package test:agent;

                   interface types {
                     use golem:agent/common.{text-reference, binary-reference};

                     enum union-with-only-literals {
                       foo,
                       bar,
                       baz,
                     }
                   }

                   /// FooAgent
                   interface foo-agent {
                     use golem:agent/common.{agent-type, binary-reference, text-reference};
                     use types.{union-with-only-literals};

                     initialize: func(input: string);

                     get-definition: func() -> agent-type;

                     my-fun: func(param: union-with-only-literals) -> union-with-only-literals;
                   }
                "#
            },
        )
    }

    #[test]
    pub fn result_type() {
        let component_name = ComponentName("test:agent".to_string());
        let agent_types = reproducer_for_issue_with_result_types();

        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        println!("{wit}");
        assert_wit(
            &wit,
            indoc! { r#"
                package test:agent;

                /// Constructs the agent bar-agent
                interface bar-agent {
                  use golem:agent/common.{agent-type, binary-reference, text-reference};

                  /// Constructs the agent bar-agent
                  initialize: func();

                  get-definition: func() -> agent-type;

                  fun-either: func(either: result<string, string>) -> result<string, string>;
                }
            "#},
        )
    }

    #[test]
    pub fn multimodal_untagged_variant_in_out() {
        let component_name = ComponentName("test:agent".to_string());
        let agent_types = test::multimodal_untagged_variant_in_out();

        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        println!("{wit}");
        assert_wit(
            &wit,
            indoc! { r#"
                package test:agent;

                interface types {
                  use golem:agent/common.{text-reference, binary-reference};

                  record image {
                    val: list<u8>,
                  }

                  record text {
                    val: string,
                  }

                  variant foo-input {
                    text(text),
                    image(image),
                  }

                  variant foo-output {
                    text(text),
                    image(image),
                  }
                }

                /// Test
                interface test-agent {
                  use golem:agent/common.{agent-type, binary-reference, text-reference};
                  use types.{foo-input, foo-output};

                  /// Constructor
                  initialize: func();

                  get-definition: func() -> agent-type;

                  foo: func(input: list<foo-input>) -> list<foo-output>;
                }
            "#},
        )
    }

    #[test]
    pub fn char_type() {
        let component_name = "test:agent".try_into().unwrap();
        let agent_types = test::char_type();

        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        println!("{wit}");
        assert_wit(
            &wit,
            indoc! { r#"
                package test:agent;

                /// An example agent
                interface agent-using-char {
                  use golem:agent/common.{agent-type, binary-reference, text-reference};

                  /// Creates an example agent instance
                  initialize: func(a: char);

                  get-definition: func() -> agent-type;

                  /// returns a random string
                  f1: func() -> char;
                }
            "#},
        )
    }

    #[test]
    pub fn unit_result_type() {
        let component_name = "test:agent".try_into().unwrap();
        let agent_types = test::unit_result_type();

        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        println!("{wit}");
        assert_wit(
            &wit,
            indoc! { r#"
                package test:agent;

                /// An example agent
                interface agent-unit-result {
                  use golem:agent/common.{agent-type, binary-reference, text-reference};

                  /// Creates an example agent instance
                  initialize: func(a: result);

                  get-definition: func() -> agent-type;

                  /// returns a random string
                  f1: func() -> result;
                }
            "#},
        )
    }

    #[test]
    pub fn ts_code_first_snippets() {
        let component_name = "test:agent".try_into().unwrap();
        let agent_types = test::code_first_snippets_agent_types(GuestLanguage::TypeScript);

        let wit = super::generate_agent_wrapper_wit(&component_name, &agent_types)
            .unwrap()
            .single_file_wrapper_wit_source;
        // println!("{wit}");
        assert_wit(
            &wit,
            indoc! { r#"
                package test:agent;

                interface types {
                  use golem:agent/common.{text-reference, binary-reference};

                  variant element1 {
                    case1(string),
                    case2(f64),
                  }

                  record element3 {
                    param1: string,
                    param2: option<f64>,
                    param3: option<string>,
                  }

                  variant fun-multimodal-advanced-input {
                    text(string),
                    image(list<u8>),
                  }

                  variant fun-multimodal-advanced-output {
                    text(string),
                    image(list<u8>),
                  }

                  variant fun-multimodal-input {
                    text(text-reference),
                    binary(binary-reference),
                  }

                  variant fun-multimodal-output {
                    text(text-reference),
                    binary(binary-reference),
                  }

                  record object-type {
                    a: string,
                    b: f64,
                    c: bool,
                  }

                  variant element {
                    case3(string),
                    case4(f64),
                    case5(object-type),
                    case6(bool),
                  }

                  record element2 {
                    param1: option<element1>,
                    param2: option<string>,
                    param3: option<element1>,
                    param4: option<element1>,
                    param5: option<string>,
                    param6: option<string>,
                    param7: option<element>,
                  }

                  record object-with-union-with-undefined1 {
                    a: option<string>,
                  }

                  record object-with-union-with-undefined2 {
                    a: option<element1>,
                  }

                  record object-with-union-with-undefined3 {
                    a: option<element1>,
                  }

                  record object-with-union-with-undefined4 {
                    a: option<string>,
                  }

                  variant result-like {
                    okay(string),
                    error(option<string>),
                  }

                  record result-like-with-no-tag {
                    ok: option<string>,
                    err: option<string>,
                  }

                  record simple-interface-type {
                    n: f64,
                  }

                  variant union-type {
                    union-type1(string),
                    union-type2(f64),
                    union-type3(object-type),
                    union-type4(bool),
                  }

                  record object-complex-type {
                    a: string,
                    b: f64,
                    c: bool,
                    d: object-type,
                    e: union-type,
                    f: list<string>,
                    g: list<object-type>,
                    h: tuple<string, f64, bool>,
                    i: tuple<string, f64, object-type>,
                    j: list<tuple<string, f64>>,
                    k: simple-interface-type,
                  }

                  variant tagged-union {
                    a(string),
                    b(f64),
                    c(bool),
                    d(union-type),
                    e(object-type),
                    f(list<string>),
                    g(tuple<string, f64, bool>),
                    h(simple-interface-type),
                    i,
                    j,
                  }

                  variant union-complex-type {
                    union-complex-type1(string),
                    union-complex-type2(f64),
                    union-complex-type3(object-complex-type),
                    union-complex-type4(object-type),
                    union-complex-type5(list<string>),
                    union-complex-type6(list<object-type>),
                    union-complex-type7(tuple<string, f64, bool>),
                    union-complex-type8(tuple<string, f64, object-type>),
                    union-complex-type9(list<tuple<string, f64>>),
                    union-complex-type10(simple-interface-type),
                    union-complex-type11(bool),
                  }

                  variant union-with-literals {
                    lit1,
                    lit2,
                    lit3,
                    union-with-literals1(bool),
                  }

                  enum union-with-only-literals {
                    foo,
                    bar,
                    baz,
                  }
                }

                /// TS Code First BarAgent
                interface bar-agent {
                  use golem:agent/common.{agent-type, binary-reference, text-reference};
                  use types.{element, element1, element2, element3, fun-multimodal-advanced-input, fun-multimodal-advanced-output, fun-multimodal-input, fun-multimodal-output, object-complex-type, object-type, object-with-union-with-undefined1, object-with-union-with-undefined2, object-with-union-with-undefined3, object-with-union-with-undefined4, result-like, result-like-with-no-tag, tagged-union, union-complex-type, union-type, union-with-literals, union-with-only-literals};

                  /// TS Code First BarAgent
                  initialize: func(optional-string-type: option<string>, optional-union-type: option<element>);

                  get-definition: func() -> agent-type;

                  fun-all: func(complex-type: object-complex-type, union-type: union-type, union-complex-type: union-complex-type, number-type: f64, string-type: string, boolean-type: bool, map-type: list<tuple<string, f64>>, tuple-complex-type: tuple<string, f64, object-type>, tuple-type: tuple<string, f64, bool>, list-complex-type: list<object-type>, object-type: object-type, result-like: result-like, result-like-with-no-tag: result-like-with-no-tag, union-with-null: option<element1>, object-with-union-with-undefined1: object-with-union-with-undefined1, object-with-union-with-undefined2: object-with-union-with-undefined2, object-with-union-with-undefined3: object-with-union-with-undefined3, object-with-union-with-undefined4: object-with-union-with-undefined4, optional-string-type: option<string>, optional-union-type: option<element>, tagged-union-type: tagged-union) -> string;

                  fun-arrow-sync: func(text: string);

                  fun-boolean: func(boolean-type: bool) -> bool;

                  fun-builtin-result-sn: func(%result: result<string, f64>) -> result<string, f64>;

                  fun-builtin-result-sv: func(%result: result<string>) -> result<string>;

                  fun-builtin-result-vs: func(%result: result<_, string>) -> result<_, string>;

                  fun-list-complex-type: func(list-complex-type: list<object-type>) -> list<object-type>;

                  fun-multimodal: func(input: list<fun-multimodal-input>) -> list<fun-multimodal-output>;

                  fun-multimodal-advanced: func(input: list<fun-multimodal-advanced-input>) -> list<fun-multimodal-advanced-output>;

                  fun-no-return: func(text: string);

                  fun-null-return: func(text: string);

                  fun-number: func(number-type: f64) -> f64;

                  fun-object-complex-type: func(text: object-complex-type) -> object-complex-type;

                  fun-object-type: func(object-type: object-type) -> object-type;

                  fun-optional: func(param1: option<element1>, param2: object-with-union-with-undefined1, param3: object-with-union-with-undefined2, param4: object-with-union-with-undefined3, param5: object-with-union-with-undefined4, param6: option<string>, param7: option<element>) -> element2;

                  fun-optional-q-mark: func(param1: string, param2: option<f64>, param3: option<string>) -> element3;

                  fun-result-exact: func(either: result<string, string>) -> result<string, string>;

                  fun-result-like: func(either-one-optional: result-like) -> result-like;

                  fun-result-like-with-void: func(result-like-with-void: result) -> result;

                  fun-result-no-tag: func(either-both-optional: result-like-with-no-tag) -> result-like-with-no-tag;

                  fun-string: func(string-type: string) -> string;

                  fun-tagged-union: func(tagged-union-type: tagged-union) -> tagged-union;

                  fun-text: func(map-type: list<tuple<string, f64>>) -> list<tuple<string, f64>>;

                  fun-tuple-complex-type: func(complex-type: tuple<string, f64, object-type>) -> tuple<string, f64, object-type>;

                  fun-tuple-type: func(tuple-type: tuple<string, f64, bool>) -> tuple<string, f64, bool>;

                  fun-undefined-return: func(text: string);

                  fun-union-complex-type: func(union-complex-type: union-complex-type) -> union-complex-type;

                  fun-union-type: func(union-type: union-type) -> union-type;

                  fun-union-with-literals: func(union-with-literals: union-with-literals) -> union-with-literals;

                  fun-union-with-only-literals: func(union-with-literals: union-with-only-literals) -> union-with-only-literals;

                  fun-unstructured-binary: func(unstructured-text: binary-reference) -> string;

                  fun-unstructured-text: func(unstructured-text: text-reference) -> string;

                  fun-void-return: func(text: string);
                }

                /// TS Code First FooAgent
                interface foo-agent {
                  use golem:agent/common.{agent-type, binary-reference, text-reference};
                  use types.{element, element1, element2, element3, fun-multimodal-advanced-input, fun-multimodal-advanced-output, fun-multimodal-input, fun-multimodal-output, object-complex-type, object-type, object-with-union-with-undefined1, object-with-union-with-undefined2, object-with-union-with-undefined3, object-with-union-with-undefined4, result-like, result-like-with-no-tag, tagged-union, union-complex-type, union-type, union-with-literals, union-with-only-literals};

                  /// TS Code First FooAgent
                  initialize: func(input: string);

                  get-definition: func() -> agent-type;

                  fun-all: func(complex-type: object-complex-type, union-type: union-type, union-complex-type: union-complex-type, number-type: f64, string-type: string, boolean-type: bool, map-type: list<tuple<string, f64>>, tuple-complex-type: tuple<string, f64, object-type>, tuple-type: tuple<string, f64, bool>, list-complex-type: list<object-type>, object-type: object-type, result-like: result-like, result-like-with-no-tag: result-like-with-no-tag, union-with-null: option<element1>, object-with-union-with-undefined1: object-with-union-with-undefined1, object-with-union-with-undefined2: object-with-union-with-undefined2, object-with-union-with-undefined3: object-with-union-with-undefined3, object-with-union-with-undefined4: object-with-union-with-undefined4, optional-string-type: option<string>, optional-union-type: option<element>, tagged-union-type: tagged-union) -> string;

                  fun-arrow-sync: func(text: string);

                  fun-boolean: func(boolean-type: bool) -> bool;

                  fun-builtin-result-sn: func(%result: result<string, f64>) -> result<string, f64>;

                  fun-builtin-result-sv: func(%result: result<string>) -> result<string>;

                  fun-builtin-result-vs: func(%result: result<_, string>) -> result<_, string>;

                  fun-either-optional: func(either-both-optional: result-like-with-no-tag) -> result-like-with-no-tag;

                  fun-list-complex-type: func(list-complex-type: list<object-type>) -> list<object-type>;

                  fun-map: func(map-type: list<tuple<string, f64>>) -> list<tuple<string, f64>>;

                  fun-multimodal: func(input: list<fun-multimodal-input>) -> list<fun-multimodal-output>;

                  fun-multimodal-advanced: func(input: list<fun-multimodal-advanced-input>) -> list<fun-multimodal-advanced-output>;

                  fun-no-return: func(text: string);

                  fun-null-return: func(text: string);

                  fun-number: func(number-type: f64) -> f64;

                  fun-object-complex-type: func(text: object-complex-type) -> object-complex-type;

                  fun-object-type: func(object-type: object-type) -> object-type;

                  fun-optional: func(param1: option<element1>, param2: object-with-union-with-undefined1, param3: object-with-union-with-undefined2, param4: object-with-union-with-undefined3, param5: object-with-union-with-undefined4, param6: option<string>, param7: option<element>) -> element2;

                  fun-optional-q-mark: func(param1: string, param2: option<f64>, param3: option<string>) -> element3;

                  fun-result-exact: func(either: result<string, string>) -> result<string, string>;

                  fun-result-like: func(either-one-optional: result-like) -> result-like;

                  fun-result-like-with-void: func(result-like-with-void: result) -> result;

                  fun-string: func(string-type: string) -> string;

                  fun-tagged-union: func(tagged-union-type: tagged-union) -> tagged-union;

                  fun-tuple-complex-type: func(complex-type: tuple<string, f64, object-type>) -> tuple<string, f64, object-type>;

                  fun-tuple-type: func(tuple-type: tuple<string, f64, bool>) -> tuple<string, f64, bool>;

                  fun-undefined-return: func(text: string);

                  fun-union-complex-type: func(union-complex-type: union-complex-type) -> union-complex-type;

                  fun-union-type: func(union-type: union-type) -> union-type;

                  fun-union-with-literals: func(union-with-literals: union-with-literals) -> union-with-literals;

                  fun-union-with-only-literals: func(union-with-literals: union-with-only-literals) -> union-with-only-literals;

                  fun-unstructured-binary: func(unstructured-text: binary-reference) -> string;

                  fun-unstructured-text: func(unstructured-text: text-reference) -> string;

                  fun-void-return: func(text: string);
                }
                "#},
        )
    }

    fn assert_wit(actual: &str, expected: &str) {
        let line_count = expected.lines().count();
        let actual_prefix = actual
            .lines()
            .take(line_count)
            .collect::<Vec<_>>()
            .join("\n");
        pretty_assertions::assert_eq!(actual_prefix, expected.trim_end())
    }
}
