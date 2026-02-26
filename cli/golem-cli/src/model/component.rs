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

use crate::model::environment::ResolvedEnvironmentIdentity;
use crate::model::wave::function_wave_compatible;
use crate::model::worker::WorkerName;
use chrono::{DateTime, Utc};
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::{
    AgentType, ComponentModelElementSchema, DataSchema, ElementSchema,
};
use golem_common::model::component::{
    ComponentDto, ComponentId, ComponentRevision, InstalledPlugin,
};
use golem_common::model::component::{ComponentName, InitialComponentFile};

use golem_common::model::environment::EnvironmentId;
use golem_common::model::trim_date::TrimDateTime;
use golem_wasm::analysis::wave::DisplayNamedFunc;
use golem_wasm::analysis::{
    AnalysedExport, AnalysedFunction, AnalysedInstance, AnalysedResourceMode, AnalysedType,
    NameOptionTypePair, NameTypePair, TypeEnum, TypeFlags, TypeRecord, TypeTuple, TypeVariant,
};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Display;
use std::path::PathBuf;

pub enum ComponentRevisionSelection<'a> {
    ByWorkerName(&'a WorkerName),
    ByExplicitRevision(ComponentRevision),
}

impl<'a> From<&'a WorkerName> for ComponentRevisionSelection<'a> {
    fn from(value: &'a WorkerName) -> Self {
        Self::ByWorkerName(value)
    }
}

impl From<ComponentRevision> for ComponentRevisionSelection<'_> {
    fn from(value: ComponentRevision) -> Self {
        Self::ByExplicitRevision(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentNameMatchKind {
    AppCurrentDir,
    App,
    Unknown,
}

pub struct SelectedComponents {
    pub environment: ResolvedEnvironmentIdentity,
    pub component_names: Vec<ComponentName>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum AppComponentType {
    /// Durable Golem component
    #[default]
    Agent,
    /// Library component, to be used in composition (not deployable)
    Library,
}

impl AppComponentType {
    pub fn is_deployable(&self) -> bool {
        match self {
            AppComponentType::Agent => true,
            AppComponentType::Library => false,
        }
    }
}
impl Display for AppComponentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppComponentType::Agent => write!(f, "Agent"),
            AppComponentType::Library => write!(f, "Library"),
        }
    }
}

pub enum ComponentUpsertResult {
    Skipped,
    Added(ComponentDto),
    Updated(ComponentDto),
}

impl ComponentUpsertResult {
    pub fn into_component(self) -> Option<ComponentDto> {
        match self {
            ComponentUpsertResult::Skipped => None,
            ComponentUpsertResult::Added(component) => Some(component),
            ComponentUpsertResult::Updated(component) => Some(component),
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentView {
    #[serde(skip)]
    pub show_sensitive: bool,
    #[serde(skip)]
    pub show_exports_for_rib: bool,

    pub component_name: ComponentName,
    pub component_id: ComponentId,
    pub component_version: Option<String>,
    pub component_revision: u64,
    pub component_size: u64,
    pub created_at: DateTime<Utc>,
    pub environment_id: EnvironmentId,
    pub exports: Vec<String>,
    pub agent_types: Vec<AgentType>,
    pub files: Vec<InitialComponentFile>,
    pub plugins: Vec<InstalledPlugin>,
    pub env: BTreeMap<String, String>,
}

impl ComponentView {
    pub fn new_rib_style(show_sensitive: bool, value: ComponentDto) -> Self {
        Self::new(show_sensitive, true, value)
    }

    pub fn new_wit_style(show_sensitive: bool, value: ComponentDto) -> Self {
        Self::new(show_sensitive, false, value)
    }

    pub fn new(show_sensitive: bool, show_exports_for_rib: bool, value: ComponentDto) -> Self {
        let exports = {
            let agent_types = value
                .metadata
                .agent_types()
                .iter()
                .map(|a| a.to_wit_naming())
                .collect::<Vec<_>>();

            show_exported_agents(&agent_types, true, true)
        };

        ComponentView {
            show_sensitive,
            show_exports_for_rib,
            component_name: value.component_name,
            component_id: value.id,
            component_version: value.metadata.root_package_version().clone(),
            component_revision: value.revision.into(),
            component_size: value.component_size,
            created_at: value.created_at,
            environment_id: value.environment_id,
            exports,
            agent_types: value.metadata.agent_types().to_vec(),
            files: value.files,
            plugins: value.installed_plugins,
            env: value.env,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ComponentDeployProperties {
    pub wasm_path: PathBuf,
    pub agent_types: Vec<AgentType>,
    pub files: Vec<crate::model::app::InitialComponentFile>,
    pub plugins: Vec<crate::model::app::PluginInstallation>,
    pub env: BTreeMap<String, String>,
    pub config_vars: BTreeMap<String, String>,
}

impl TrimDateTime for ComponentView {
    fn trim_date_time_ms(self) -> Self {
        Self {
            created_at: self.created_at.trim_date_time_ms(),
            ..self
        }
    }
}

pub fn render_type(typ: &AnalysedType) -> String {
    match typ {
        AnalysedType::Variant(TypeVariant { cases, .. }) => {
            let cases_str = cases
                .iter()
                .map(|NameOptionTypePair { name, typ }| match typ {
                    None => name.to_string(),
                    Some(typ) => format!("{}({})", name, render_type(typ)),
                })
                .collect::<Vec<String>>()
                .join(", ");
            format!("variant {{ {cases_str} }}")
        }
        AnalysedType::Result(boxed) => {
            let ok_str = boxed.ok.as_ref().map(|t| render_type(t));
            let err_str = boxed.err.as_ref().map(|t| render_type(t));

            if let Some(ok) = ok_str {
                if let Some(err) = err_str {
                    format!("result<{ok}, {err}>")
                } else {
                    format!("result<{ok}>")
                }
            } else if let Some(err) = err_str {
                format!("result<_, {err}>")
            } else {
                "result".to_string()
            }
        }
        AnalysedType::Option(boxed) => format!("option<{}>", render_type(&boxed.inner)),
        AnalysedType::Enum(TypeEnum { cases, .. }) => {
            format!("enum {{ {} }}", cases.iter().join(", "))
        }
        AnalysedType::Flags(TypeFlags { names, .. }) => {
            format!("flags {{ {} }}", names.iter().join(", "))
        }
        AnalysedType::Record(TypeRecord { fields, .. }) => {
            let pairs: Vec<String> = fields
                .iter()
                .map(|NameTypePair { name, typ }| format!("{}: {}", name, render_type(typ)))
                .collect();

            format!("record {{ {} }}", pairs.join(", "))
        }
        AnalysedType::Tuple(TypeTuple { items, .. }) => {
            let typs: Vec<String> = items.iter().map(render_type).collect();
            format!("tuple<{}>", typs.join(", "))
        }
        AnalysedType::List(boxed) => format!("list<{}>", render_type(&boxed.inner)),
        AnalysedType::Str { .. } => "string".to_string(),
        AnalysedType::Chr { .. } => "char".to_string(),
        AnalysedType::F64 { .. } => "f64".to_string(),
        AnalysedType::F32 { .. } => "f32".to_string(),
        AnalysedType::U64 { .. } => "u64".to_string(),
        AnalysedType::S64 { .. } => "s64".to_string(),
        AnalysedType::U32 { .. } => "u32".to_string(),
        AnalysedType::S32 { .. } => "s32".to_string(),
        AnalysedType::U16 { .. } => "u16".to_string(),
        AnalysedType::S16 { .. } => "s16".to_string(),
        AnalysedType::U8 { .. } => "u8".to_string(),
        AnalysedType::S8 { .. } => "s8".to_string(),
        AnalysedType::Bool { .. } => "bool".to_string(),
        AnalysedType::Handle(handle) => match handle.mode {
            AnalysedResourceMode::Borrowed => format!("&handle<{}>", handle.resource_id.0),
            AnalysedResourceMode::Owned => format!("handle<{}>", handle.resource_id.0),
        },
    }
}

pub fn show_exported_agents(
    agents: &[AgentType],
    wrapper_naming: bool,
    show_dummy_return_type: bool,
) -> Vec<String> {
    agents
        .iter()
        .flat_map(|agent| render_exported_agent(agent, wrapper_naming, show_dummy_return_type))
        .collect()
}

pub fn show_exported_agent_constructors(agents: &[AgentType], wrapper_naming: bool) -> Vec<String> {
    agents
        .iter()
        .map(|c| render_agent_constructor(c, wrapper_naming, true))
        .collect()
}

fn render_exported_agent(
    agent: &AgentType,
    wrapper_naming: bool,
    show_dummy_return_type: bool,
) -> Vec<String> {
    let mut result = Vec::new();
    result.push(render_agent_constructor(
        agent,
        wrapper_naming,
        show_dummy_return_type,
    ));
    let agent_name = if wrapper_naming {
        format!("{}.", agent.wrapper_type_name())
    } else {
        "  ".to_string()
    };
    for method in &agent.methods {
        let output = render_data_schema(&method.output_schema);
        if output.is_empty() {
            result.push(format!(
                "{}{}({})",
                agent_name,
                method.name,
                render_data_schema(&method.input_schema),
            ));
        } else {
            result.push(format!(
                "{}{}({}) -> {}",
                agent_name,
                method.name,
                render_data_schema(&method.input_schema),
                output
            ));
        }
    }

    result
}

pub fn render_agent_constructor(
    agent: &AgentType,
    wrapper_naming: bool,
    show_dummy_return_type: bool,
) -> String {
    let dummy_return_type = if show_dummy_return_type {
        " agent constructor"
    } else {
        ""
    };
    if wrapper_naming {
        format!(
            "{}({}){}",
            agent.wrapper_type_name(),
            render_data_schema(&agent.constructor.input_schema.to_wit_naming()),
            dummy_return_type
        )
    } else {
        format!(
            "{}({}){}",
            agent.type_name,
            render_data_schema(&agent.constructor.input_schema),
            dummy_return_type
        )
    }
}

fn render_data_schema(schema: &DataSchema) -> String {
    match schema {
        DataSchema::Tuple(elements) => elements
            .elements
            .iter()
            .map(|named_elem| render_element_schema(&named_elem.schema))
            .join(", "),
        DataSchema::Multimodal(elements) => elements
            .elements
            .iter()
            .map(|named_elem| {
                format!(
                    "{}({})",
                    named_elem.name,
                    render_element_schema(&named_elem.schema)
                )
            })
            .join(" | "),
    }
}

fn render_element_schema(schema: &ElementSchema) -> String {
    match schema {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
            render_type(element_type)
        }
        ElementSchema::UnstructuredText(text_descriptor) => {
            let mut result = "text".to_string();
            if let Some(restrictions) = &text_descriptor.restrictions {
                result.push('[');
                result.push_str(&restrictions.iter().map(|r| &r.language_code).join(", "));
                result.push(']');
            }
            result
        }
        ElementSchema::UnstructuredBinary(binary_descriptor) => {
            let mut result = "binary".to_string();
            if let Some(restrictions) = &binary_descriptor.restrictions {
                result.push('[');
                result.push_str(&restrictions.iter().map(|r| &r.mime_type).join(", "));
                result.push(']');
            }
            result
        }
    }
}

pub fn show_exported_functions(
    exports: &[AnalysedExport],
    with_parameters: bool,
    agent_instance_name_filter: Option<&str>,
) -> Vec<String> {
    let is_agent = agent_instance_name_filter.is_some();
    exports
        .iter()
        .flat_map(|exp| match exp {
            AnalysedExport::Instance(AnalysedInstance { name, functions }) => {
                if let Some(instance_name_filter) = agent_instance_name_filter {
                    if name != instance_name_filter {
                        return vec![];
                    }
                }
                let fs: Vec<String> = functions
                    .iter()
                    .filter(|f| !is_agent || f.name != "get-definition")
                    .map(|f| render_exported_function(Some(name), f, with_parameters))
                    .collect();
                fs
            }
            AnalysedExport::Function(f) => {
                vec![render_exported_function(None, f, with_parameters)]
            }
        })
        .collect()
}

pub fn render_exported_function(
    prefix: Option<&str>,
    f: &AnalysedFunction,
    with_parameters: bool,
) -> String {
    // TODO: now that the formatter is implemented, and wave still not supports handles
    //       is there a point in using the customized wave formatter?
    //       Or maybe it should handled in the customized DisplayNamedFunc?
    if with_parameters {
        if function_wave_compatible(f) {
            DisplayNamedFunc {
                name: format_function_name(prefix, &f.name),
                func: f.clone(),
            }
            .to_string()
        } else {
            render_non_wave_compatible_exported_function(prefix, f)
        }
    } else {
        format_function_name(prefix, &f.name)
    }
}

fn render_non_wave_compatible_exported_function(
    prefix: Option<&str>,
    f: &AnalysedFunction,
) -> String {
    let params = f
        .parameters
        .iter()
        .map(|p| format!("{}: {}", p.name, render_type(&p.typ)))
        .collect::<Vec<String>>()
        .join(", ");

    let results = f
        .result
        .iter()
        .map(|res| render_type(&res.typ))
        .collect::<Vec<String>>();

    let res_str = results.join(", ");

    let name = format_function_name(prefix, &f.name);
    if results.is_empty() {
        format!("{name}({params})")
    } else if results.len() == 1 {
        format!("{name}({params}) -> {res_str}")
    } else {
        format!("{name}({params}) -> ({res_str})")
    }
}

pub fn format_function_name(prefix: Option<&str>, name: &str) -> String {
    match prefix {
        Some(prefix) => format!("{prefix}.{{{name}}}"),
        None => name.to_string(),
    }
}

pub fn agent_interface_name(component: &ComponentDto, agent_type_name: &str) -> Option<String> {
    match (
        component.metadata.root_package_name(),
        component.metadata.root_package_version(),
    ) {
        (Some(name), Some(version)) => Some(format!("{}/{}@{}", name, agent_type_name, version)),
        (Some(name), None) => Some(format!("{}/{}", name, agent_type_name)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::model::component::render_exported_function;
    use golem_wasm::analysis::analysed_type::{
        bool, case, chr, f32, f64, field, flags, handle, list, option, r#enum, record, result,
        result_err, result_ok, s16, s32, s64, s8, str, tuple, u16, u32, u64, u8, unit_case,
        variant,
    };
    use golem_wasm::analysis::{
        AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult, AnalysedResourceId,
        AnalysedResourceMode, AnalysedType,
    };

    #[test]
    fn show_exported_function_handles_type_handle() {
        let f = AnalysedFunction {
            name: "n".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult {
                typ: handle(AnalysedResourceId(1), AnalysedResourceMode::Borrowed),
            }),
        };
        let repr = render_exported_function(None, &f, true);

        assert_eq!(repr, "n() -> &handle<1>")
    }

    #[test]
    fn show_no_results_wave() {
        let f = AnalysedFunction {
            name: "abc".to_string(),
            parameters: vec![],
            result: None,
        };

        let repr = render_exported_function(None, &f, true);

        assert_eq!(repr, "abc()")
    }

    #[test]
    fn show_no_results_custom() {
        let f = AnalysedFunction {
            name: "abc".to_string(),
            parameters: vec![AnalysedFunctionParameter {
                name: "n".to_string(),
                typ: handle(AnalysedResourceId(1), AnalysedResourceMode::Owned),
            }],
            result: None,
        };

        let repr = render_exported_function(None, &f, true);

        assert_eq!(repr, "abc(n: handle<1>)")
    }

    #[test]
    fn show_result_wave() {
        let f = AnalysedFunction {
            name: "abc".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult { typ: bool() }),
        };

        let repr = render_exported_function(None, &f, true);

        assert_eq!(repr, "abc() -> bool")
    }

    #[test]
    fn show_result_custom() {
        let f = AnalysedFunction {
            name: "abc".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult {
                typ: handle(AnalysedResourceId(1), AnalysedResourceMode::Owned),
            }),
        };

        let repr = render_exported_function(None, &f, true);

        assert_eq!(repr, "abc() -> handle<1>")
    }

    #[test]
    fn show_params_and_results_wave() {
        let f = AnalysedFunction {
            name: "abc".to_string(),
            parameters: vec![
                AnalysedFunctionParameter {
                    name: "n1".to_string(),
                    typ: bool(),
                },
                AnalysedFunctionParameter {
                    name: "n2".to_string(),
                    typ: bool(),
                },
            ],
            result: Some(AnalysedFunctionResult {
                typ: tuple(vec![bool(), bool()]),
            }),
        };

        let repr = render_exported_function(None, &f, true);

        assert_eq!(repr, "abc(n1: bool, n2: bool) -> tuple<bool, bool>")
    }

    #[test]
    fn show_params_and_results_custom() {
        let f = AnalysedFunction {
            name: "abc".to_string(),
            parameters: vec![
                AnalysedFunctionParameter {
                    name: "n1".to_string(),
                    typ: bool(),
                },
                AnalysedFunctionParameter {
                    name: "n2".to_string(),
                    typ: handle(AnalysedResourceId(1), AnalysedResourceMode::Owned),
                },
            ],
            result: Some(AnalysedFunctionResult {
                typ: tuple(vec![bool(), bool()]),
            }),
        };

        let repr = render_exported_function(None, &f, true);

        assert_eq!(repr, "abc(n1: bool, n2: handle<1>) -> tuple<bool, bool>")
    }

    fn ensure_same_export(typ: AnalysedType, expected: &str) {
        let expected_wave = format!("wn() -> {expected}");
        let expected_custom = format!("cn() -> tuple<handle<1>, {expected}>");

        let wave_f = AnalysedFunction {
            name: "wn".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult { typ: typ.clone() }),
        };
        let wave_res = render_exported_function(None, &wave_f, true);
        assert_eq!(wave_res, expected_wave);

        let custom_f = AnalysedFunction {
            name: "cn".to_string(),
            parameters: vec![],
            result: Some(AnalysedFunctionResult {
                typ: tuple(vec![
                    handle(AnalysedResourceId(1), AnalysedResourceMode::Owned),
                    typ,
                ]),
            }),
        };
        let custom_res = render_exported_function(None, &custom_f, true);
        assert_eq!(custom_res, expected_custom);
    }

    #[test]
    fn same_export_for_variant() {
        ensure_same_export(variant(vec![]), "variant {  }");
        ensure_same_export(variant(vec![case("v1", bool())]), "variant { v1(bool) }");
        ensure_same_export(
            variant(vec![case("v1", bool()), unit_case("v2")]),
            "variant { v1(bool), v2 }",
        );
    }

    #[test]
    fn same_export_for_result() {
        ensure_same_export(result_ok(bool()), "result<bool>");
        ensure_same_export(result_err(bool()), "result<_, bool>");
        ensure_same_export(result(bool(), bool()), "result<bool, bool>");
    }

    #[test]
    fn same_export_for_option() {
        ensure_same_export(option(bool()), "option<bool>")
    }

    #[test]
    fn same_export_for_enum() {
        ensure_same_export(r#enum(&[]), "enum {  }");
        ensure_same_export(r#enum(&["a"]), "enum { a }");
        ensure_same_export(r#enum(&["a", "b"]), "enum { a, b }");
    }

    #[test]
    fn same_export_for_flags() {
        ensure_same_export(flags(&[]), "flags {  }");
        ensure_same_export(flags(&["a"]), "flags { a }");
        ensure_same_export(flags(&["a", "b"]), "flags { a, b }");
    }

    #[test]
    fn same_export_for_record() {
        ensure_same_export(record(vec![]), "record {  }");
        ensure_same_export(record(vec![field("n1", bool())]), "record { n1: bool }");
        ensure_same_export(
            record(vec![field("n1", bool()), field("n2", bool())]),
            "record { n1: bool, n2: bool }",
        );
    }

    #[test]
    fn same_export_for_tuple() {
        ensure_same_export(tuple(vec![]), "tuple<>");
        ensure_same_export(tuple(vec![bool()]), "tuple<bool>");
        ensure_same_export(tuple(vec![bool(), bool()]), "tuple<bool, bool>");
    }

    #[test]
    fn same_export_for_list() {
        ensure_same_export(list(bool()), "list<bool>")
    }

    #[test]
    fn same_export_for_str() {
        ensure_same_export(str(), "string")
    }

    #[test]
    fn same_export_for_chr() {
        ensure_same_export(chr(), "char")
    }

    #[test]
    fn same_export_for_f64() {
        ensure_same_export(f64(), "f64")
    }

    #[test]
    fn same_export_for_f32() {
        ensure_same_export(f32(), "f32")
    }

    #[test]
    fn same_export_for_u64() {
        ensure_same_export(u64(), "u64")
    }

    #[test]
    fn same_export_for_s64() {
        ensure_same_export(s64(), "s64")
    }

    #[test]
    fn same_export_for_u32() {
        ensure_same_export(u32(), "u32")
    }

    #[test]
    fn same_export_for_s32() {
        ensure_same_export(s32(), "s32")
    }

    #[test]
    fn same_export_for_u16() {
        ensure_same_export(u16(), "u16")
    }

    #[test]
    fn same_export_for_s16() {
        ensure_same_export(s16(), "s16")
    }

    #[test]
    fn same_export_for_u8() {
        ensure_same_export(u8(), "u8")
    }

    #[test]
    fn same_export_for_s8() {
        ensure_same_export(s8(), "s8")
    }

    #[test]
    fn same_export_for_bool() {
        ensure_same_export(bool(), "bool")
    }
}
