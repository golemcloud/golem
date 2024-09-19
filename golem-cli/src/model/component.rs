use crate::cloud::ProjectId;
use crate::model::conversions::analysed_function_client_to_model;
use crate::model::wave::function_wave_compatible;
use crate::model::GolemError;
use chrono::{DateTime, Utc};
use golem_client::model::{
    AnalysedExport, AnalysedFunction, AnalysedFunctionResult, AnalysedInstance,
    AnalysedResourceMode, AnalysedType, ComponentMetadata, ComponentType, NameOptionTypePair,
    NameTypePair, TypeEnum, TypeFlags, TypeRecord, TypeTuple, TypeVariant, VersionedComponentId,
};
use golem_common::model::trim_date::TrimDateTime;
use golem_common::model::ComponentId;
use golem_common::uri::oss::urn::ComponentUrn;
use golem_wasm_ast::analysis::wave::DisplayNamedFunc;
use rib::{ParsedFunctionName, ParsedFunctionSite};
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub versioned_component_id: VersionedComponentId,
    pub component_name: String,
    pub component_size: u64,
    pub component_type: ComponentType,
    pub metadata: ComponentMetadata,
    pub project_id: Option<ProjectId>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
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
        } = value;

        Component {
            versioned_component_id,
            component_name,
            component_size,
            component_type: component_type.unwrap_or(ComponentType::Durable),
            metadata,
            project_id: None,
            created_at,
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
            let ok_str = boxed.ok.as_ref().map(render_type);
            let err_str = boxed.err.as_ref().map(render_type);

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
        AnalysedType::F64 { .. } => "float64".to_string(),
        AnalysedType::F32 { .. } => "float32".to_string(),
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
            AnalysedResourceMode::Borrowed => format!("&handle<{}>", handle.resource_id),
            AnalysedResourceMode::Owned => format!("handle<{}>", handle.resource_id),
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
            func: analysed_function_client_to_model(f),
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
    use crate::model::component::show_exported_function;
    use golem_client::model::{
        AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult, AnalysedResourceMode,
        AnalysedType, NameOptionTypePair, NameTypePair, TypeBool, TypeChr, TypeEnum, TypeF32,
        TypeF64, TypeFlags, TypeHandle, TypeList, TypeOption, TypeRecord, TypeResult, TypeS16,
        TypeS32, TypeS64, TypeS8, TypeStr, TypeTuple, TypeU16, TypeU32, TypeU64, TypeU8,
        TypeVariant,
    };

    #[test]
    fn show_exported_function_handles_type_handle() {
        let f = AnalysedFunction {
            name: "n".to_string(),
            parameters: vec![],
            results: vec![AnalysedFunctionResult {
                name: None,
                typ: AnalysedType::Handle(TypeHandle {
                    resource_id: 1,
                    mode: AnalysedResourceMode::Borrowed,
                }),
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
                typ: AnalysedType::Handle(TypeHandle {
                    resource_id: 1,
                    mode: AnalysedResourceMode::Owned,
                }),
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
                typ: type_bool(),
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
                typ: AnalysedType::Handle(TypeHandle {
                    resource_id: 1,
                    mode: AnalysedResourceMode::Owned,
                }),
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
                    typ: type_bool(),
                },
                AnalysedFunctionParameter {
                    name: "n2".to_string(),
                    typ: type_bool(),
                },
            ],
            results: vec![
                AnalysedFunctionResult {
                    name: Some("r1".to_string()),
                    typ: type_bool(),
                },
                AnalysedFunctionResult {
                    name: None,
                    typ: type_bool(),
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
                    typ: type_bool(),
                },
                AnalysedFunctionParameter {
                    name: "n2".to_string(),
                    typ: AnalysedType::Handle(TypeHandle {
                        resource_id: 1,
                        mode: AnalysedResourceMode::Owned,
                    }),
                },
            ],
            results: vec![
                AnalysedFunctionResult {
                    name: Some("r1".to_string()),
                    typ: type_bool(),
                },
                AnalysedFunctionResult {
                    name: None,
                    typ: type_bool(),
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
                typ: AnalysedType::Tuple(TypeTuple {
                    items: vec![
                        AnalysedType::Handle(TypeHandle {
                            resource_id: 1,
                            mode: AnalysedResourceMode::Owned,
                        }),
                        typ,
                    ],
                }),
            }],
        };
        let custom_res = show_exported_function(None, &custom_f);
        assert_eq!(custom_res, expected_custom);
    }

    #[test]
    fn same_export_for_variant() {
        ensure_same_export(
            AnalysedType::Variant(TypeVariant { cases: vec![] }),
            "variant {  }",
        );
        ensure_same_export(
            AnalysedType::Variant(TypeVariant {
                cases: vec![NameOptionTypePair {
                    name: "v1".to_string(),
                    typ: Some(type_bool()),
                }],
            }),
            "variant { v1(bool) }",
        );
        ensure_same_export(
            AnalysedType::Variant(TypeVariant {
                cases: vec![
                    NameOptionTypePair {
                        name: "v1".to_string(),
                        typ: Some(AnalysedType::Bool(TypeBool {})),
                    },
                    NameOptionTypePair {
                        name: "v2".to_string(),
                        typ: None,
                    },
                ],
            }),
            "variant { v1(bool), v2 }",
        );
    }

    fn type_bool() -> AnalysedType {
        AnalysedType::Bool(TypeBool {})
    }

    #[test]
    fn same_export_for_result() {
        ensure_same_export(
            AnalysedType::Result(Box::new(TypeResult {
                ok: None,
                err: None,
            })),
            "result",
        );
        ensure_same_export(
            AnalysedType::Result(Box::new(TypeResult {
                ok: Some(type_bool()),
                err: None,
            })),
            "result<bool>",
        );
        ensure_same_export(
            AnalysedType::Result(Box::new(TypeResult {
                ok: None,
                err: Some(type_bool()),
            })),
            "result<_, bool>",
        );
        ensure_same_export(
            AnalysedType::Result(Box::new(TypeResult {
                ok: Some(type_bool()),
                err: Some(type_bool()),
            })),
            "result<bool, bool>",
        );
    }

    #[test]
    fn same_export_for_option() {
        ensure_same_export(
            AnalysedType::Option(Box::new(TypeOption { inner: type_bool() })),
            "option<bool>",
        )
    }

    #[test]
    fn same_export_for_enum() {
        ensure_same_export(AnalysedType::Enum(TypeEnum { cases: vec![] }), "enum {  }");
        ensure_same_export(
            AnalysedType::Enum(TypeEnum {
                cases: vec!["a".to_string()],
            }),
            "enum { a }",
        );
        ensure_same_export(
            AnalysedType::Enum(TypeEnum {
                cases: vec!["a".to_string(), "b".to_string()],
            }),
            "enum { a, b }",
        );
    }

    #[test]
    fn same_export_for_flags() {
        ensure_same_export(
            AnalysedType::Flags(TypeFlags { names: vec![] }),
            "flags {  }",
        );
        ensure_same_export(
            AnalysedType::Flags(TypeFlags {
                names: vec!["a".to_string()],
            }),
            "flags { a }",
        );
        ensure_same_export(
            AnalysedType::Flags(TypeFlags {
                names: vec!["a".to_string(), "b".to_string()],
            }),
            "flags { a, b }",
        );
    }

    #[test]
    fn same_export_for_record() {
        ensure_same_export(
            AnalysedType::Record(TypeRecord { fields: vec![] }),
            "record {  }",
        );
        ensure_same_export(
            AnalysedType::Record(TypeRecord {
                fields: vec![NameTypePair {
                    name: "n1".to_string(),
                    typ: type_bool(),
                }],
            }),
            "record { n1: bool }",
        );
        ensure_same_export(
            AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "n1".to_string(),
                        typ: type_bool(),
                    },
                    NameTypePair {
                        name: "n2".to_string(),
                        typ: type_bool(),
                    },
                ],
            }),
            "record { n1: bool, n2: bool }",
        );
    }

    #[test]
    fn same_export_for_tuple() {
        ensure_same_export(AnalysedType::Tuple(TypeTuple { items: vec![] }), "tuple<>");
        ensure_same_export(
            AnalysedType::Tuple(TypeTuple {
                items: vec![type_bool()],
            }),
            "tuple<bool>",
        );
        ensure_same_export(
            AnalysedType::Tuple(TypeTuple {
                items: vec![type_bool(), type_bool()],
            }),
            "tuple<bool, bool>",
        );
    }

    #[test]
    fn same_export_for_list() {
        ensure_same_export(
            AnalysedType::List(Box::new(TypeList { inner: type_bool() })),
            "list<bool>",
        )
    }

    #[test]
    fn same_export_for_str() {
        ensure_same_export(AnalysedType::Str(TypeStr {}), "string")
    }

    #[test]
    fn same_export_for_chr() {
        ensure_same_export(AnalysedType::Chr(TypeChr {}), "char")
    }

    #[test]
    fn same_export_for_f64() {
        ensure_same_export(AnalysedType::F64(TypeF64 {}), "float64")
    }

    #[test]
    fn same_export_for_f32() {
        ensure_same_export(AnalysedType::F32(TypeF32 {}), "float32")
    }

    #[test]
    fn same_export_for_u64() {
        ensure_same_export(AnalysedType::U64(TypeU64 {}), "u64")
    }

    #[test]
    fn same_export_for_s64() {
        ensure_same_export(AnalysedType::S64(TypeS64 {}), "s64")
    }

    #[test]
    fn same_export_for_u32() {
        ensure_same_export(AnalysedType::U32(TypeU32 {}), "u32")
    }

    #[test]
    fn same_export_for_s32() {
        ensure_same_export(AnalysedType::S32(TypeS32 {}), "s32")
    }

    #[test]
    fn same_export_for_u16() {
        ensure_same_export(AnalysedType::U16(TypeU16 {}), "u16")
    }

    #[test]
    fn same_export_for_s16() {
        ensure_same_export(AnalysedType::S16(TypeS16 {}), "s16")
    }

    #[test]
    fn same_export_for_u8() {
        ensure_same_export(AnalysedType::U8(TypeU8 {}), "u8")
    }

    #[test]
    fn same_export_for_s8() {
        ensure_same_export(AnalysedType::S8(TypeS8 {}), "s8")
    }

    #[test]
    fn same_export_for_bool() {
        ensure_same_export(type_bool(), "bool")
    }
}
