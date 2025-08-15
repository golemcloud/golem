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

use crate::model::wave::function_wave_compatible;
use crate::model::ComponentName;
use crate::model::ProjectId;
use anyhow::{anyhow, bail};
use chrono::{DateTime, Utc};
use golem_client::model::{
    AnalysedType, ComponentMetadata, ComponentType, InitialComponentFile, VersionedComponentId,
};
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::trim_date::TrimDateTime;
use golem_wasm_ast::analysis::wave::DisplayNamedFunc;
use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunction, AnalysedInstance, AnalysedResourceMode, NameOptionTypePair,
    NameTypePair, TypeEnum, TypeFlags, TypeRecord, TypeTuple, TypeVariant,
};
use rib::{ParsedFunctionName, ParsedFunctionSite};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Display;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub enum ComponentSelection<'a> {
    Name(&'a ComponentName),
    Id(Uuid),
}

impl Display for ComponentSelection<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComponentSelection::Name(name) => write!(f, "{name}"),
            ComponentSelection::Id(id) => write!(f, "{id}"),
        }
    }
}

impl<'a> From<&'a ComponentName> for ComponentSelection<'a> {
    fn from(name: &'a ComponentName) -> Self {
        ComponentSelection::Name(name)
    }
}

impl From<Uuid> for ComponentSelection<'_> {
    fn from(uuid: Uuid) -> Self {
        ComponentSelection::Id(uuid)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    pub versioned_component_id: VersionedComponentId,
    pub component_name: ComponentName,
    pub component_size: u64,
    pub component_type: ComponentType,
    pub metadata: ComponentMetadata,
    pub project_id: Option<ProjectId>,
    pub created_at: Option<DateTime<Utc>>,
    pub files: Vec<InitialComponentFile>,
    pub env: BTreeMap<String, String>,
}

impl From<golem_client::model::Component> for Component {
    fn from(value: golem_client::model::Component) -> Self {
        Component {
            versioned_component_id: value.versioned_component_id,
            component_name: value.component_name.into(),
            component_size: value.component_size,
            metadata: value.metadata,
            project_id: Some(ProjectId(value.project_id)),
            created_at: Some(value.created_at),
            component_type: value.component_type,
            files: value.files,
            env: value.env.into_iter().collect(),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum AppComponentType {
    /// Durable Golem component
    #[default]
    Durable,
    /// Ephemeral Golem component
    Ephemeral,
    /// Library component, to be used in composition (not deployable)
    Library,
}

impl AppComponentType {
    pub fn as_deployable_component_type(&self) -> Option<ComponentType> {
        match self {
            AppComponentType::Durable => Some(ComponentType::Durable),
            AppComponentType::Ephemeral => Some(ComponentType::Ephemeral),
            AppComponentType::Library => None,
        }
    }
}

impl From<ComponentType> for AppComponentType {
    fn from(value: ComponentType) -> Self {
        match value {
            ComponentType::Durable => AppComponentType::Durable,
            ComponentType::Ephemeral => AppComponentType::Ephemeral,
        }
    }
}

impl Display for AppComponentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppComponentType::Durable => write!(f, "Durable"),
            AppComponentType::Ephemeral => write!(f, "Ephemeral"),
            AppComponentType::Library => write!(f, "Library"),
        }
    }
}

pub enum ComponentUpsertResult {
    Skipped,
    Added(Component),
    Updated(Component),
}

impl ComponentUpsertResult {
    pub fn into_component(self) -> Option<Component> {
        match self {
            ComponentUpsertResult::Skipped => None,
            ComponentUpsertResult::Added(component) => Some(component),
            ComponentUpsertResult::Updated(component) => Some(component),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentView {
    #[serde(skip)]
    pub show_sensitive: bool,

    pub component_name: ComponentName,
    pub component_id: Uuid,
    pub component_type: ComponentType,
    pub component_version: u64,
    pub component_size: u64,
    pub created_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub project_id: Option<ProjectId>,
    pub exports: Vec<String>,
    pub dynamic_linking: BTreeMap<String, BTreeMap<String, String>>,
    pub files: Vec<InitialComponentFile>,
    pub env: BTreeMap<String, String>,
}

impl ComponentView {
    pub fn new(show_sensitive: bool, value: Component) -> Self {
        ComponentView {
            show_sensitive,
            component_name: value.component_name,
            component_id: value.versioned_component_id.component_id,
            component_type: value.component_type,
            component_version: value.versioned_component_id.version,
            component_size: value.component_size,
            created_at: value.created_at,
            project_id: value.project_id,
            exports: show_exported_functions(value.metadata.exports(), true),
            dynamic_linking: value
                .metadata
                .dynamic_linking()
                .iter()
                .map(|(name, link)| {
                    (
                        name.clone(),
                        match link {
                            DynamicLinkedInstance::WasmRpc(links) => links
                                .targets
                                .iter()
                                .map(|(resource, target)| {
                                    (resource.clone(), target.interface_name.clone())
                                })
                                .collect::<BTreeMap<String, String>>(),
                        },
                    )
                })
                .collect(),
            files: value.files,
            env: value.env,
        }
    }
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
                    Some(typ) => format!("{name}({})", render_type(typ)),
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
        AnalysedType::Enum(TypeEnum { cases, .. }) => format!("enum {{ {} }}", cases.join(", ")),
        AnalysedType::Flags(TypeFlags { names, .. }) => format!("flags {{ {} }}", names.join(", ")),
        AnalysedType::Record(TypeRecord { fields, .. }) => {
            let pairs: Vec<String> = fields
                .iter()
                .map(|NameTypePair { name, typ }| format!("{name}: {}", render_type(typ)))
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

pub fn show_exported_functions(exports: &[AnalysedExport], with_parameters: bool) -> Vec<String> {
    exports
        .iter()
        .flat_map(|exp| match exp {
            AnalysedExport::Instance(AnalysedInstance { name, functions }) => {
                let fs: Vec<String> = functions
                    .iter()
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

fn resolve_function<'t>(
    component: &'t Component,
    function: &str,
) -> anyhow::Result<(&'t AnalysedFunction, ParsedFunctionName)> {
    let parsed = ParsedFunctionName::parse(function).map_err(|err| anyhow!(err))?;
    let mut functions = Vec::new();

    for export in component.metadata.exports() {
        match export {
            AnalysedExport::Instance(interface) => {
                if matches!(parsed.site().interface_name(), Some(name) if name == interface.name) {
                    for function in &interface.functions {
                        if parsed.function().function_name() == function.name {
                            functions.push(function);
                        }
                    }
                }
            }
            AnalysedExport::Function(ref f @ AnalysedFunction { name, .. }) => {
                if parsed.site() == &ParsedFunctionSite::Global
                    && &parsed.function().function_name() == name
                {
                    functions.push(f);
                }
            }
        }
    }

    if functions.len() > 1 {
        bail!(
            "Multiple function results with the same name ({}) declared",
            function
        )
    } else if let Some(func) = functions.first() {
        Ok((func, parsed))
    } else {
        bail!("Can't find function ({}) in component", function)
    }
}

pub fn function_result_types<'t>(
    component: &'t Component,
    function: &str,
) -> anyhow::Result<Vec<&'t AnalysedType>> {
    let (func, _) = resolve_function(component, function)?;

    Ok(func.result.iter().map(|r| &r.typ).collect())
}

pub fn function_params_types<'t>(
    component: &'t Component,
    function: &str,
) -> anyhow::Result<Vec<&'t AnalysedType>> {
    let (func, parsed) = resolve_function(component, function)?;

    if parsed.function().is_indexed_resource() {
        Ok(func.parameters.iter().skip(1).map(|r| &r.typ).collect())
    } else {
        Ok(func.parameters.iter().map(|r| &r.typ).collect())
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::model::component::render_exported_function;
    use golem_wasm_ast::analysis::analysed_type::{
        bool, case, chr, f32, f64, field, flags, handle, list, option, r#enum, record, result,
        result_err, result_ok, s16, s32, s64, s8, str, tuple, u16, u32, u64, u8, unit_case,
        variant,
    };
    use golem_wasm_ast::analysis::{
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
