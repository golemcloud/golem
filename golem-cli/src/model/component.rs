// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::cloud::ProjectId;
use crate::model::wave::function_wave_compatible;
use crate::model::GolemError;
use chrono::{DateTime, Utc};
use golem_client::model::{
    AnalysedType, ComponentMetadata, ComponentType, InitialComponentFile, VersionedComponentId,
};
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_common::model::trim_date::TrimDateTime;
use golem_common::model::ComponentId;
use golem_common::uri::oss::urn::ComponentUrn;
use golem_wasm_ast::analysis::wave::DisplayNamedFunc;
use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunction, AnalysedFunctionResult, AnalysedInstance,
    AnalysedResourceMode, NameOptionTypePair, NameTypePair, TypeEnum, TypeFlags, TypeRecord,
    TypeTuple, TypeVariant,
};
use rib::{ParsedFunctionName, ParsedFunctionSite};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tracing::info;

#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    pub versioned_component_id: VersionedComponentId,
    pub component_name: String,
    pub component_size: u64,
    pub component_type: ComponentType,
    pub metadata: ComponentMetadata,
    pub project_id: Option<ProjectId>,
    pub created_at: Option<DateTime<Utc>>,
    pub files: Vec<InitialComponentFile>,
}

impl From<golem_client::model::Component> for Component {
    fn from(value: golem_client::model::Component) -> Self {
        let golem_client::model::Component {
            versioned_component_id,
            component_name,
            component_size,
            component_type,
            metadata,
            created_at,
            files,
            installed_plugins: _installed_plugins,
        } = value;

        Component {
            versioned_component_id,
            component_name,
            component_size,
            component_type: component_type.unwrap_or(ComponentType::Durable),
            metadata,
            project_id: None,
            created_at,
            files,
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
    pub component_urn: ComponentUrn,
    pub component_version: u64,
    pub component_name: String,
    pub component_size: u64,
    pub created_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub project_id: Option<ProjectId>,
    pub exports: Vec<String>,
    pub dynamic_linking: BTreeMap<String, BTreeMap<String, String>>,
}

impl TrimDateTime for ComponentView {
    fn trim_date_time_ms(self) -> Self {
        Self {
            created_at: self.created_at.trim_date_time_ms(),
            ..self
        }
    }
}

impl From<Component> for ComponentView {
    fn from(value: Component) -> Self {
        (&value).into()
    }
}

impl From<&Component> for ComponentView {
    fn from(value: &Component) -> Self {
        ComponentView {
            component_urn: ComponentUrn {
                id: ComponentId(value.versioned_component_id.component_id),
            },
            component_version: value.versioned_component_id.version,
            component_name: value.component_name.to_string(),
            component_size: value.component_size,
            created_at: value.created_at,
            project_id: value.project_id,
            exports: value
                .metadata
                .exports
                .iter()
                .flat_map(|exp| match exp {
                    AnalysedExport::Instance(AnalysedInstance { name, functions }) => {
                        let fs: Vec<String> = functions
                            .iter()
                            .map(|f| show_exported_function(Some(name), f))
                            .collect();
                        fs
                    }
                    AnalysedExport::Function(f) => {
                        vec![show_exported_function(None, f)]
                    }
                })
                .collect(),
            dynamic_linking: value
                .metadata
                .dynamic_linking
                .iter()
                .map(|(name, link)| {
                    (
                        name.clone(),
                        match link {
                            DynamicLinkedInstance::WasmRpc(links) => links
                                .target_interface_name
                                .iter()
                                .map(|(resource, interface)| (resource.clone(), interface.clone()))
                                .collect::<BTreeMap<String, String>>(),
                        },
                    )
                })
                .collect(),
        }
    }
}

fn render_type(typ: &AnalysedType) -> String {
    match typ {
        AnalysedType::Variant(TypeVariant { cases }) => {
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
        AnalysedType::Enum(TypeEnum { cases }) => format!("enum {{ {} }}", cases.join(", ")),
        AnalysedType::Flags(TypeFlags { names }) => format!("flags {{ {} }}", names.join(", ")),
        AnalysedType::Record(TypeRecord { fields }) => {
            let pairs: Vec<String> = fields
                .iter()
                .map(|NameTypePair { name, typ }| format!("{name}: {}", render_type(typ)))
                .collect();

            format!("record {{ {} }}", pairs.join(", "))
        }
        AnalysedType::Tuple(TypeTuple { items }) => {
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

fn render_result(r: &AnalysedFunctionResult) -> String {
    render_type(&r.typ)
}

pub fn show_exported_function(prefix: Option<&str>, f: &AnalysedFunction) -> String {
    if function_wave_compatible(f) {
        DisplayNamedFunc {
            name: format_function_name(prefix, &f.name),
            func: f.clone(),
        }
        .to_string()
    } else {
        custom_show_exported_function(prefix, f)
    }
}

fn custom_show_exported_function(prefix: Option<&str>, f: &AnalysedFunction) -> String {
    let params = f
        .parameters
        .iter()
        .map(|p| format!("{}: {}", p.name, render_type(&p.typ)))
        .collect::<Vec<String>>()
        .join(", ");

    let results = f.results.iter().map(render_result).collect::<Vec<String>>();

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
) -> Result<(&'t AnalysedFunction, ParsedFunctionName), GolemError> {
    let parsed = ParsedFunctionName::parse(function).map_err(GolemError)?;
    let mut functions = Vec::new();

    for export in &component.metadata.exports {
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
        info!("Multiple function with the same name '{function}' declared");

        Err(GolemError(
            "Multiple function results with the same name declared".to_string(),
        ))
    } else if let Some(func) = functions.first() {
        Ok((func, parsed))
    } else {
        info!("No function '{function}' declared for component");

        Err(GolemError("Can't find function in component".to_string()))
    }
}

pub fn function_result_types<'t>(
    component: &'t Component,
    function: &str,
) -> Result<Vec<&'t AnalysedType>, GolemError> {
    let (func, _) = resolve_function(component, function)?;

    Ok(func.results.iter().map(|r| &r.typ).collect())
}

pub fn function_params_types<'t>(
    component: &'t Component,
    function: &str,
) -> Result<Vec<&'t AnalysedType>, GolemError> {
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

    use crate::model::component::show_exported_function;
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
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: handle(AnalysedResourceId(1), AnalysedResourceMode::Borrowed),
            }],
        };
        let repr = show_exported_function(None, &f);

        assert_eq!(repr, "n() -> &handle<1>")
    }

    #[test]
    fn show_no_results_wave() {
        let f = AnalysedFunction {
            name: "abc".to_string(),
            parameters: vec![],
            results: vec![],
        };

        let repr = show_exported_function(None, &f);

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
            results: vec![],
        };

        let repr = show_exported_function(None, &f);

        assert_eq!(repr, "abc(n: handle<1>)")
    }

    #[test]
    fn show_result_wave() {
        let f = AnalysedFunction {
            name: "abc".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: bool(),
            }],
        };

        let repr = show_exported_function(None, &f);

        assert_eq!(repr, "abc() -> bool")
    }

    #[test]
    fn show_result_custom() {
        let f = AnalysedFunction {
            name: "abc".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: handle(AnalysedResourceId(1), AnalysedResourceMode::Owned),
            }],
        };

        let repr = show_exported_function(None, &f);

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
            results: vec![
                AnalysedFunctionResult {
                    name: Some("r1".to_string()),
                    typ: bool(),
                },
                AnalysedFunctionResult {
                    name: None,
                    typ: bool(),
                },
            ],
        };

        let repr = show_exported_function(None, &f);

        assert_eq!(repr, "abc(n1: bool, n2: bool) -> (bool, bool)")
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
            results: vec![
                AnalysedFunctionResult {
                    name: Some("r1".to_string()),
                    typ: bool(),
                },
                AnalysedFunctionResult {
                    name: None,
                    typ: bool(),
                },
            ],
        };

        let repr = show_exported_function(None, &f);

        assert_eq!(repr, "abc(n1: bool, n2: handle<1>) -> (bool, bool)")
    }

    fn ensure_same_export(typ: AnalysedType, expected: &str) {
        let expected_wave = format!("wn() -> {expected}");
        let expected_custom = format!("cn() -> tuple<handle<1>, {expected}>");

        let wave_f = AnalysedFunction {
            name: "wn".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: typ.clone(),
            }],
        };
        let wave_res = show_exported_function(None, &wave_f);
        assert_eq!(wave_res, expected_wave);

        let custom_f = AnalysedFunction {
            name: "cn".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: tuple(vec![
                    handle(AnalysedResourceId(1), AnalysedResourceMode::Owned),
                    typ,
                ]),
            }],
        };
        let custom_res = show_exported_function(None, &custom_f);
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
