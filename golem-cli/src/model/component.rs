use crate::cloud::ProjectId;
use crate::model::GolemError;
use golem_client::model::{
    ComponentMetadata, Export, ExportFunction, ExportInstance, FunctionResult, NameOptionTypePair,
    NameTypePair, ProtectedComponentId, ResourceMode, Type, TypeEnum, TypeFlags, TypeRecord,
    TypeTuple, TypeVariant, UserComponentId, VersionedComponentId,
};
use golem_wasm_ast::wave::DisplayNamedFunc;
use rib::{ParsedFunctionName, ParsedFunctionSite};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::model::wave::{func_to_analysed, function_wave_compatible};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub versioned_component_id: VersionedComponentId,
    pub user_component_id: UserComponentId,
    pub protected_component_id: ProtectedComponentId,
    pub component_name: String,
    pub component_size: u64,
    pub metadata: ComponentMetadata,
    pub project_id: Option<ProjectId>,
}

impl From<golem_client::model::Component> for Component {
    fn from(value: golem_client::model::Component) -> Self {
        let golem_client::model::Component {
            versioned_component_id,
            user_component_id,
            protected_component_id,
            component_name,
            component_size,
            metadata,
        } = value;

        Component {
            versioned_component_id,
            user_component_id,
            protected_component_id,
            component_name,
            component_size,
            metadata,
            project_id: None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentView {
    pub component_id: String,
    pub component_version: u64,
    pub component_name: String,
    pub component_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub project_id: Option<ProjectId>,
    pub exports: Vec<String>,
}

impl From<Component> for ComponentView {
    fn from(value: Component) -> Self {
        (&value).into()
    }
}

impl From<&Component> for ComponentView {
    fn from(value: &Component) -> Self {
        ComponentView {
            component_id: value.versioned_component_id.component_id.to_string(),
            component_version: value.versioned_component_id.version,
            component_name: value.component_name.to_string(),
            component_size: value.component_size,
            project_id: value.project_id,
            exports: value
                .metadata
                .exports
                .iter()
                .flat_map(|exp| match exp {
                    Export::Instance(ExportInstance { name, functions }) => {
                        let fs: Vec<String> = functions
                            .iter()
                            .map(|f| show_exported_function(&format!("{name}/"), f))
                            .collect();
                        fs
                    }
                    Export::Function(f) => {
                        vec![show_exported_function("", f)]
                    }
                })
                .collect(),
        }
    }
}

fn render_type(typ: &Type) -> String {
    match typ {
        Type::Variant(TypeVariant { cases }) => {
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
        Type::Result(boxed) => {
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
        Type::Option(boxed) => format!("option<{}>", render_type(&boxed.inner)),
        Type::Enum(TypeEnum { cases }) => format!("enum {{ {} }}", cases.join(", ")),
        Type::Flags(TypeFlags { cases }) => format!("flags {{ {} }}", cases.join(", ")),
        Type::Record(TypeRecord { cases }) => {
            let pairs: Vec<String> = cases
                .iter()
                .map(|NameTypePair { name, typ }| format!("{name}: {}", render_type(typ)))
                .collect();

            format!("record {{ {} }}", pairs.join(", "))
        }
        Type::Tuple(TypeTuple { items }) => {
            let typs: Vec<String> = items.iter().map(render_type).collect();
            format!("tuple<{}>", typs.join(", "))
        }
        Type::List(boxed) => format!("list<{}>", render_type(&boxed.inner)),
        Type::Str { .. } => "string".to_string(),
        Type::Chr { .. } => "char".to_string(),
        Type::F64 { .. } => "float64".to_string(),
        Type::F32 { .. } => "float32".to_string(),
        Type::U64 { .. } => "u64".to_string(),
        Type::S64 { .. } => "s64".to_string(),
        Type::U32 { .. } => "u32".to_string(),
        Type::S32 { .. } => "s32".to_string(),
        Type::U16 { .. } => "u16".to_string(),
        Type::S16 { .. } => "s16".to_string(),
        Type::U8 { .. } => "u8".to_string(),
        Type::S8 { .. } => "s8".to_string(),
        Type::Bool { .. } => "bool".to_string(),
        Type::Handle(handle) => match handle.mode {
            ResourceMode::Borrowed => format!("&handle<{}>", handle.resource_id),
            ResourceMode::Owned => format!("handle<{}>", handle.resource_id),
        },
    }
}

fn render_result(r: &FunctionResult) -> String {
    render_type(&r.typ)
}

fn show_exported_function(prefix: &str, f: &ExportFunction) -> String {
    if function_wave_compatible(f) {
        let name = &f.name;

        DisplayNamedFunc {
            name: format!("{prefix}{name}"),
            func: func_to_analysed(f),
        }
        .to_string()
    } else {
        custom_show_exported_function(prefix, f)
    }
}

fn custom_show_exported_function(prefix: &str, f: &ExportFunction) -> String {
    let name = &f.name;
    let params = f
        .parameters
        .iter()
        .map(|p| format!("{}: {}", p.name, render_type(&p.typ)))
        .collect::<Vec<String>>()
        .join(", ");

    let results = f.results.iter().map(render_result).collect::<Vec<String>>();

    let res_str = results.join(", ");

    if results.is_empty() {
        format!("{prefix}{name}({params})")
    } else if results.len() == 1 {
        format!("{prefix}{name}({params}) -> {res_str}")
    } else {
        format!("{prefix}{name}({params}) -> ({res_str})")
    }
}

fn resolve_function<'t>(
    component: &'t Component,
    function: &str,
) -> Result<(&'t ExportFunction, ParsedFunctionName), GolemError> {
    let parsed = ParsedFunctionName::parse(function).map_err(GolemError)?;
    let mut functions = Vec::new();

    for export in &component.metadata.exports {
        match export {
            Export::Instance(interface) => {
                if matches!(parsed.site().interface_name(), Some(name) if name == interface.name) {
                    for function in &interface.functions {
                        if parsed.function().function_name() == function.name {
                            functions.push(function);
                        }
                    }
                }
            }
            Export::Function(ref f @ ExportFunction { name, .. }) => {
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
) -> Result<Vec<&'t Type>, GolemError> {
    let (func, _) = resolve_function(component, function)?;

    Ok(func.results.iter().map(|r| &r.typ).collect())
}

pub fn function_params_types<'t>(
    component: &'t Component,
    function: &str,
) -> Result<Vec<&'t Type>, GolemError> {
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
        ExportFunction, FunctionParameter, FunctionResult, NameOptionTypePair, NameTypePair,
        ResourceMode, Type, TypeBool, TypeChr, TypeEnum, TypeF32, TypeF64, TypeFlags, TypeHandle,
        TypeList, TypeOption, TypeRecord, TypeResult, TypeS16, TypeS32, TypeS64, TypeS8, TypeStr,
        TypeTuple, TypeU16, TypeU32, TypeU64, TypeU8, TypeVariant,
    };

    #[test]
    fn show_exported_function_handles_type_handle() {
        let f = ExportFunction {
            name: "n".to_string(),
            parameters: vec![],
            results: vec![FunctionResult {
                name: None,
                typ: Type::Handle(TypeHandle {
                    resource_id: 1,
                    mode: ResourceMode::Borrowed,
                }),
            }],
        };
        let repr = show_exported_function("", &f);

        assert_eq!(repr, "n() -> &handle<1>")
    }

    #[test]
    fn show_no_results_wave() {
        let f = ExportFunction {
            name: "abc".to_string(),
            parameters: vec![],
            results: vec![],
        };

        let repr = show_exported_function("", &f);

        assert_eq!(repr, "abc()")
    }

    #[test]
    fn show_no_results_custom() {
        let f = ExportFunction {
            name: "abc".to_string(),
            parameters: vec![FunctionParameter {
                name: "n".to_string(),
                typ: Type::Handle(TypeHandle {
                    resource_id: 1,
                    mode: ResourceMode::Owned,
                }),
            }],
            results: vec![],
        };

        let repr = show_exported_function("", &f);

        assert_eq!(repr, "abc(n: handle<1>)")
    }

    #[test]
    fn show_result_wave() {
        let f = ExportFunction {
            name: "abc".to_string(),
            parameters: vec![],
            results: vec![FunctionResult {
                name: None,
                typ: type_bool(),
            }],
        };

        let repr = show_exported_function("", &f);

        assert_eq!(repr, "abc() -> bool")
    }

    #[test]
    fn show_result_custom() {
        let f = ExportFunction {
            name: "abc".to_string(),
            parameters: vec![],
            results: vec![FunctionResult {
                name: None,
                typ: Type::Handle(TypeHandle {
                    resource_id: 1,
                    mode: ResourceMode::Owned,
                }),
            }],
        };

        let repr = show_exported_function("", &f);

        assert_eq!(repr, "abc() -> handle<1>")
    }

    #[test]
    fn show_params_and_results_wave() {
        let f = ExportFunction {
            name: "abc".to_string(),
            parameters: vec![
                FunctionParameter {
                    name: "n1".to_string(),
                    typ: type_bool(),
                },
                FunctionParameter {
                    name: "n2".to_string(),
                    typ: type_bool(),
                },
            ],
            results: vec![
                FunctionResult {
                    name: Some("r1".to_string()),
                    typ: type_bool(),
                },
                FunctionResult {
                    name: None,
                    typ: type_bool(),
                },
            ],
        };

        let repr = show_exported_function("", &f);

        assert_eq!(repr, "abc(n1: bool, n2: bool) -> (bool, bool)")
    }

    #[test]
    fn show_params_and_results_custom() {
        let f = ExportFunction {
            name: "abc".to_string(),
            parameters: vec![
                FunctionParameter {
                    name: "n1".to_string(),
                    typ: type_bool(),
                },
                FunctionParameter {
                    name: "n2".to_string(),
                    typ: Type::Handle(TypeHandle {
                        resource_id: 1,
                        mode: ResourceMode::Owned,
                    }),
                },
            ],
            results: vec![
                FunctionResult {
                    name: Some("r1".to_string()),
                    typ: type_bool(),
                },
                FunctionResult {
                    name: None,
                    typ: type_bool(),
                },
            ],
        };

        let repr = show_exported_function("", &f);

        assert_eq!(repr, "abc(n1: bool, n2: handle<1>) -> (bool, bool)")
    }

    fn ensure_same_export(typ: Type, expected: &str) {
        let expected_wave = format!("wn() -> {expected}");
        let expected_custom = format!("cn() -> tuple<handle<1>, {expected}>");

        let wave_f = ExportFunction {
            name: "wn".to_string(),
            parameters: vec![],
            results: vec![FunctionResult {
                name: None,
                typ: typ.clone(),
            }],
        };
        let wave_res = show_exported_function("", &wave_f);
        assert_eq!(wave_res, expected_wave);

        let custom_f = ExportFunction {
            name: "cn".to_string(),
            parameters: vec![],
            results: vec![FunctionResult {
                name: None,
                typ: Type::Tuple(TypeTuple {
                    items: vec![
                        Type::Handle(TypeHandle {
                            resource_id: 1,
                            mode: ResourceMode::Owned,
                        }),
                        typ,
                    ],
                }),
            }],
        };
        let custom_res = show_exported_function("", &custom_f);
        assert_eq!(custom_res, expected_custom);
    }

    #[test]
    fn same_export_for_variant() {
        ensure_same_export(Type::Variant(TypeVariant { cases: vec![] }), "variant {  }");
        ensure_same_export(
            Type::Variant(TypeVariant {
                cases: vec![NameOptionTypePair {
                    name: "v1".to_string(),
                    typ: Some(type_bool()),
                }],
            }),
            "variant { v1(bool) }",
        );
        ensure_same_export(
            Type::Variant(TypeVariant {
                cases: vec![
                    NameOptionTypePair {
                        name: "v1".to_string(),
                        typ: Some(Type::Bool(TypeBool {})),
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

    fn type_bool() -> Type {
        Type::Bool(TypeBool {})
    }

    #[test]
    fn same_export_for_result() {
        ensure_same_export(
            Type::Result(Box::new(TypeResult {
                ok: None,
                err: None,
            })),
            "result",
        );
        ensure_same_export(
            Type::Result(Box::new(TypeResult {
                ok: Some(type_bool()),
                err: None,
            })),
            "result<bool>",
        );
        ensure_same_export(
            Type::Result(Box::new(TypeResult {
                ok: None,
                err: Some(type_bool()),
            })),
            "result<_, bool>",
        );
        ensure_same_export(
            Type::Result(Box::new(TypeResult {
                ok: Some(type_bool()),
                err: Some(type_bool()),
            })),
            "result<bool, bool>",
        );
    }

    #[test]
    fn same_export_for_option() {
        ensure_same_export(
            Type::Option(Box::new(TypeOption { inner: type_bool() })),
            "option<bool>",
        )
    }

    #[test]
    fn same_export_for_enum() {
        ensure_same_export(Type::Enum(TypeEnum { cases: vec![] }), "enum {  }");
        ensure_same_export(
            Type::Enum(TypeEnum {
                cases: vec!["a".to_string()],
            }),
            "enum { a }",
        );
        ensure_same_export(
            Type::Enum(TypeEnum {
                cases: vec!["a".to_string(), "b".to_string()],
            }),
            "enum { a, b }",
        );
    }

    #[test]
    fn same_export_for_flags() {
        ensure_same_export(Type::Flags(TypeFlags { cases: vec![] }), "flags {  }");
        ensure_same_export(
            Type::Flags(TypeFlags {
                cases: vec!["a".to_string()],
            }),
            "flags { a }",
        );
        ensure_same_export(
            Type::Flags(TypeFlags {
                cases: vec!["a".to_string(), "b".to_string()],
            }),
            "flags { a, b }",
        );
    }

    #[test]
    fn same_export_for_record() {
        ensure_same_export(Type::Record(TypeRecord { cases: vec![] }), "record {  }");
        ensure_same_export(
            Type::Record(TypeRecord {
                cases: vec![NameTypePair {
                    name: "n1".to_string(),
                    typ: type_bool(),
                }],
            }),
            "record { n1: bool }",
        );
        ensure_same_export(
            Type::Record(TypeRecord {
                cases: vec![
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
        ensure_same_export(Type::Tuple(TypeTuple { items: vec![] }), "tuple<>");
        ensure_same_export(
            Type::Tuple(TypeTuple {
                items: vec![type_bool()],
            }),
            "tuple<bool>",
        );
        ensure_same_export(
            Type::Tuple(TypeTuple {
                items: vec![type_bool(), type_bool()],
            }),
            "tuple<bool, bool>",
        );
    }

    #[test]
    fn same_export_for_list() {
        ensure_same_export(
            Type::List(Box::new(TypeList { inner: type_bool() })),
            "list<bool>",
        )
    }

    #[test]
    fn same_export_for_str() {
        ensure_same_export(Type::Str(TypeStr {}), "string")
    }

    #[test]
    fn same_export_for_chr() {
        ensure_same_export(Type::Chr(TypeChr {}), "char")
    }

    #[test]
    fn same_export_for_f64() {
        ensure_same_export(Type::F64(TypeF64 {}), "float64")
    }

    #[test]
    fn same_export_for_f32() {
        ensure_same_export(Type::F32(TypeF32 {}), "float32")
    }

    #[test]
    fn same_export_for_u64() {
        ensure_same_export(Type::U64(TypeU64 {}), "u64")
    }

    #[test]
    fn same_export_for_s64() {
        ensure_same_export(Type::S64(TypeS64 {}), "s64")
    }

    #[test]
    fn same_export_for_u32() {
        ensure_same_export(Type::U32(TypeU32 {}), "u32")
    }

    #[test]
    fn same_export_for_s32() {
        ensure_same_export(Type::S32(TypeS32 {}), "s32")
    }

    #[test]
    fn same_export_for_u16() {
        ensure_same_export(Type::U16(TypeU16 {}), "u16")
    }

    #[test]
    fn same_export_for_s16() {
        ensure_same_export(Type::S16(TypeS16 {}), "s16")
    }

    #[test]
    fn same_export_for_u8() {
        ensure_same_export(Type::U8(TypeU8 {}), "u8")
    }

    #[test]
    fn same_export_for_s8() {
        ensure_same_export(Type::S8(TypeS8 {}), "s8")
    }

    #[test]
    fn same_export_for_bool() {
        ensure_same_export(type_bool(), "bool")
    }
}
