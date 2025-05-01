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

use anyhow::{anyhow, Context};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use golem_common::model::component_metadata::{DynamicLinkedRpcTargets, RpcTarget};
use golem_common::model::ComponentType;
use heck::ToLowerCamelCase;
use itertools::Itertools;
use rib::{ParsedFunctionName, ParsedFunctionReference};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::fmt::Display;
use wasmtime::component::{Type, Val};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum DynamicRpcCall {
    GlobalStubConstructor {
        component_name: String,
        component_type: ComponentType,
    },
    GlobalCustomConstructor {
        component_type: ComponentType,
    },
    ResourceStubConstructor {
        component_name: String,
        component_type: ComponentType,
        target_constructor_name: ParsedFunctionName,
    },
    ResourceCustomConstructor {
        component_type: ComponentType,
        target_constructor_name: ParsedFunctionName,
    },
    BlockingFunctionCall {
        target_function_name: ParsedFunctionName,
        component_name: String,
    },
    ScheduledFunctionCall {
        target_function_name: ParsedFunctionName,
    },
    FireAndForgetFunctionCall {
        target_function_name: ParsedFunctionName,
    },
    AsyncFunctionCall {
        target_function_name: ParsedFunctionName,
        component_name: String,
    },
    FutureInvokeResultSubscribe,
    FutureInvokeResultGet,
}

impl DynamicRpcCall {
    pub fn analyse<R: RpcTarget + Display, T: DynamicLinkedRpcTargets<R>>(
        stub_name: &ParsedFunctionName,
        _param_types: &[Type],
        result_types: &[Type],
        rpc_metadata: &T,
        resource_types: &HashMap<(String, String), DynamicRpcResource>,
    ) -> anyhow::Result<Option<DynamicRpcCall>> {
        fn context<R: RpcTarget + Display, T: DynamicLinkedRpcTargets<R>>(
            rpc_metadata: &T,
        ) -> String {
            format!(
                "Failed to get mapped target site ({}) from dynamic linking metadata",
                rpc_metadata
                    .targets()
                    .iter()
                    .map(|(k, v)| format!("{k}=>{v}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        }

        if let Some(resource_name) = stub_name.is_constructor() {
            match resource_types.get(&(
                stub_name.site.interface_name().unwrap_or_default(),
                resource_name.to_string(),
            )) {
                Some(DynamicRpcResource::Stub) => {
                    let target = rpc_metadata
                        .target(resource_name)
                        .map_err(|err: String| anyhow!(err))
                        .context(context(rpc_metadata))?;

                    Ok(Some(DynamicRpcCall::GlobalStubConstructor {
                        component_name: target.component_name(),
                        component_type: target.component_type(),
                    }))
                }
                Some(DynamicRpcResource::ResourceStub) => {
                    let target = rpc_metadata
                        .target(resource_name)
                        .map_err(|err: String| anyhow!(err))
                        .context(context(rpc_metadata))?;

                    Ok(Some(DynamicRpcCall::ResourceStubConstructor {
                        target_constructor_name: ParsedFunctionName {
                            site: target
                                .site()
                                .map_err(|err: String| anyhow!(err))
                                .context(context(rpc_metadata))?,
                            function: ParsedFunctionReference::RawResourceConstructor {
                                resource: resource_name.to_string(),
                            },
                        },
                        component_name: target.component_name(),
                        component_type: target.component_type(),
                    }))
                }
                _ => Ok(None),
            }
        } else if let Some(resource_name) = stub_name.is_method() {
            match resource_types.get(&(
                stub_name.site.interface_name().unwrap_or_default(),
                resource_name.to_string(),
            )) {
                Some(DynamicRpcResource::InvokeResult) => {
                    if stub_name.function.resource_method_name() == Some("subscribe".to_string()) {
                        Ok(Some(DynamicRpcCall::FutureInvokeResultSubscribe))
                    } else if stub_name.function.resource_method_name() == Some("get".to_string()) {
                        Ok(Some(DynamicRpcCall::FutureInvokeResultGet))
                    } else {
                        Ok(None)
                    }
                }
                Some(stub) => {
                    let method_name = stub_name.function.resource_method_name().unwrap(); // safe because of stub_name.is_method()

                    if stub_name.is_static_method().is_some() && method_name == "custom" {
                        match stub {
                            DynamicRpcResource::Stub => {
                                let target = rpc_metadata
                                    .target(resource_name)
                                    .map_err(|err| anyhow!(err))
                                    .context(context(rpc_metadata))?;

                                Ok(Some(DynamicRpcCall::GlobalCustomConstructor {
                                    component_type: target.component_type(),
                                }))
                            }
                            DynamicRpcResource::ResourceStub => {
                                let target = rpc_metadata
                                    .target(resource_name)
                                    .map_err(|err: String| anyhow!(err))
                                    .context(context(rpc_metadata))?;

                                Ok(Some(DynamicRpcCall::ResourceCustomConstructor {
                                    target_constructor_name: ParsedFunctionName {
                                        site: target
                                            .site()
                                            .map_err(|err: String| anyhow!(err))
                                            .context(context(rpc_metadata))?,
                                        function: ParsedFunctionReference::RawResourceConstructor {
                                            resource: resource_name.to_string(),
                                        },
                                    },
                                    component_type: target.component_type(),
                                }))
                            }
                            DynamicRpcResource::InvokeResult => {
                                unreachable!()
                            }
                        }
                    } else {
                        let blocking = method_name.starts_with("blocking-");
                        let scheduled = method_name.starts_with("schedule-");

                        let target_method_name = if blocking {
                            method_name
                                .strip_prefix("blocking-")
                                .unwrap_or(&method_name)
                        } else if scheduled {
                            method_name
                                .strip_prefix("schedule-")
                                .unwrap_or(&method_name)
                        } else {
                            &method_name
                        };

                        let target_function = match stub {
                            DynamicRpcResource::Stub => ParsedFunctionReference::Function {
                                function: target_method_name.to_string(),
                            },
                            _ => ParsedFunctionReference::RawResourceMethod {
                                resource: resource_name.to_string(),
                                method: target_method_name.to_string(),
                            },
                        };

                        let target = rpc_metadata
                            .target(resource_name)
                            .map_err(|err: String| anyhow!(err))
                            .context(context(rpc_metadata))?;

                        let target_function_name = ParsedFunctionName {
                            site: target
                                .site()
                                .map_err(|err: String| anyhow!(err))
                                .context(context(rpc_metadata))?,
                            function: target_function,
                        };

                        if blocking {
                            Ok(Some(DynamicRpcCall::BlockingFunctionCall {
                                target_function_name,
                                component_name: target.component_name(),
                            }))
                        } else if scheduled {
                            Ok(Some(DynamicRpcCall::ScheduledFunctionCall {
                                target_function_name,
                            }))
                        } else if !result_types.is_empty() {
                            Ok(Some(DynamicRpcCall::AsyncFunctionCall {
                                target_function_name,
                                component_name: target.component_name(),
                            }))
                        } else {
                            Ok(Some(DynamicRpcCall::FireAndForgetFunctionCall {
                                target_function_name,
                            }))
                        }
                    }
                }
                None => Ok(None),
            }
        } else {
            // Unsupported item
            Ok(None)
        }
    }
}

#[derive(Debug, Clone)]
pub enum DynamicRpcResource {
    Stub,
    ResourceStub,
    InvokeResult,
}

impl DynamicRpcResource {
    pub fn analyse<R: RpcTarget, T: DynamicLinkedRpcTargets<R>>(
        resource_name: &str,
        methods: &[MethodInfo],
        rpc_metadata: &T,
    ) -> anyhow::Result<Option<DynamicRpcResource>> {
        if resource_name == "pollable" {
            Ok(None)
        } else if Self::is_invoke_result(resource_name, methods) {
            Ok(Some(DynamicRpcResource::InvokeResult))
        } else if let Some(_constructor) = methods
            .iter()
            .find_or_first(|m| m.method_name.contains("[constructor]"))
        {
            match rpc_metadata.target(resource_name) {
                Ok(target) => {
                    if target
                        .interface_name()
                        .ends_with(&format!("/{resource_name}"))
                    {
                        Ok(Some(DynamicRpcResource::Stub))
                    } else {
                        Ok(Some(DynamicRpcResource::ResourceStub))
                    }
                }
                Err(err) => Err(anyhow!("{}", err)),
            }
        } else {
            // No constructor => not a stub
            Ok(None)
        }
    }

    fn is_invoke_result(resource_name: &str, methods: &[MethodInfo]) -> bool {
        resource_name.starts_with("future-")
            && resource_name.ends_with("-result")
            && methods
                .iter()
                .filter_map(|m| m.method_name.split('.').last().map(|s| s.to_string()))
                .sorted()
                .collect::<Vec<_>>()
                == vec!["get".to_string(), "subscribe".to_string()]
            && {
                let subscribe = methods
                    .iter()
                    .find(|m| m.method_name.ends_with(".subscribe"))
                    .unwrap();
                subscribe.params.len() == 1
                    && matches!(subscribe.params[0], Type::Borrow(_))
                    && subscribe.results.len() == 1
                    && matches!(subscribe.results[0], Type::Own(_))
            }
    }
}

pub struct MethodInfo {
    pub method_name: String,
    pub params: Vec<Type>,
    pub results: Vec<Type>,
}

pub struct FunctionInfo {
    pub name: ParsedFunctionName,
    pub params: Vec<Type>,
    pub results: Vec<Type>,
}

pub fn to_vals_(
    results_json_values: Vec<JsonValue>,
    results: &mut [Val],
    result_types: &[Type],
) -> anyhow::Result<()> {
    for (idx, (json_value, typ)) in results_json_values.iter().zip(result_types).enumerate() {
        results[idx] = json_to_val(json_value, typ);
    }
    Ok(())
}

pub fn to_json_values_(params: &[Val]) -> Result<Vec<JsonValue>, anyhow::Error> {
    let mut params_json_values: Vec<JsonValue> = Vec::new();
    for val in params.iter() {
        params_json_values.push(val_to_json(val));
    }
    Ok(params_json_values.clone())
}

fn val_to_json(val: &Val) -> JsonValue {
    match val {
        Val::Bool(b) => json!(b),
        Val::S8(n) => json!(n),
        Val::U8(n) => json!(n),
        Val::S16(n) => json!(n),
        Val::U16(n) => json!(n),
        Val::S32(n) => json!(n),
        Val::U32(n) => json!(n),
        Val::S64(n) => json!(n),
        Val::U64(n) => json!(n),
        Val::Float32(f) => json!(f),
        Val::Float64(f) => json!(f),
        Val::Char(c) => json!(c.to_string()),
        Val::String(s) => json!(s),
        Val::List(list) => {
            let items: Vec<JsonValue> = list.iter().map(val_to_json).collect();
            JsonValue::Array(items)
        }
        Val::Record(fields) => {
            let mut map = serde_json::Map::new();
            for (name, value) in fields {
                map.insert(name.to_string(), val_to_json(value));
            }
            JsonValue::Object(map)
        }
        Val::Tuple(items) => {
            let vec: Vec<JsonValue> = items.iter().map(val_to_json).collect();
            json!(vec)
        }
        Val::Variant(discriminant, value) => {
            let mut obj = serde_json::Map::new();
            obj.insert(
                discriminant.to_string(),
                value
                    .as_ref()
                    .map(|box_val| val_to_json(box_val))
                    .unwrap_or(json!(null)),
            );
            JsonValue::Object(obj)
        }
        Val::Enum(discriminant) => {
            json!(discriminant)
        }
        Val::Option(Some(inner)) => json!(val_to_json(inner)),
        Val::Option(None) => json!(null),
        Val::Result(r) => match r {
            Ok(v) => {
                let value_json = match v {
                    Some(value) => val_to_json(value),
                    None => JsonValue::Null,
                };
                json!({ "ok": value_json })
            }
            Err(e) => {
                let value_json = match e {
                    Some(value) => val_to_json(value),
                    None => JsonValue::Null,
                };
                json!({ "err": value_json })
            }
        },

        Val::Flags(flags) => {
            let names: Vec<String> = flags.iter().map(|name| name.to_string()).collect();
            json!(names)
        }
        Val::Resource(_) => todo!(),
    }
}

pub fn json_to_val(json: &JsonValue, ty: &Type) -> Val {
    match ty {
        Type::Bool => Val::Bool(json.as_bool().expect("Expected bool")),
        Type::S8 => Val::S8(json.as_i64().expect("Expected i64") as i8),
        Type::U8 => Val::U8(json.as_u64().expect("Expected u64") as u8),
        Type::S16 => Val::S16(json.as_i64().expect("Expected i64") as i16),
        Type::U16 => Val::U16(json.as_u64().expect("Expected u64") as u16),
        Type::S32 => Val::S32(json.as_i64().expect("Expected i64") as i32),
        Type::U32 => Val::U32(json.as_u64().expect("Expected u64") as u32),
        Type::S64 => {
            let value = match json {
                JsonValue::String(s) => s.parse::<i64>().expect("Failed to parse i64 from string"),
                JsonValue::Number(n) => n.as_i64().expect("Number is not a valid i64"),
                _ => panic!("Expected string or number for int64, got: {}", json),
            };
            Val::S64(value)
        }
        Type::U64 => Val::U64(json.as_u64().expect("Expected u64")),
        Type::Float32 => Val::Float32(json.as_f64().expect("Expected f64") as f32),
        Type::Float64 => Val::Float64(json.as_f64().expect("Expected f64")),
        Type::Char => {
            let s = json.as_str().expect("Expected string");
            assert!(s.chars().count() == 1, "Expected single character");
            Val::Char(s.chars().next().unwrap())
        }
        Type::String => Val::String(json.as_str().expect("Expected string").to_string()),
        Type::List(list) => match json {
            JsonValue::String(s) => match STANDARD.decode(s) {
                Ok(bytes) => {
                    let vals = bytes.iter().map(|u8| Val::U8(*u8)).collect();
                    Val::List(vals)
                }
                Err(_) => panic!("decode error"),
            },
            JsonValue::Array(arr) => {
                let vals = arr.iter().map(|j| json_to_val(j, &list.ty())).collect();
                Val::List(vals)
            }
            _ => panic!("Expected base64 string or array for bytes"),
        },
        Type::Record(record) => {
            let obj = json.as_object().expect("Expected object");
            let mut vals = vec![];

            record.fields().for_each(|field| {
                let value: &JsonValue = obj
                    .get(&field.name.to_lower_camel_case())
                    .unwrap_or_else(|| panic!("Missing field '{}'", field.name));
                vals.push((field.name.to_string(), json_to_val(value, &field.ty)));
            });
            Val::Record(vals)
        }
        Type::Tuple(items) => {
            let arr = json.as_array().expect("Expected array");
            assert_eq!(arr.len(), items.types().len(), "Tuple length mismatch");
            let mut vals = vec![];
            arr.iter().zip(items.types()).for_each(|(j, typ)| {
                vals.push(json_to_val(j, &typ));
            });
            Val::Tuple(vals)
        }
        Type::Variant(variants) => {
            let obj = json.as_object().expect("Expected object");
            assert_eq!(obj.len(), 1, "Variant object must have exactly one key");
            let (label, value) = obj.iter().next().unwrap();

            let (index, payload_ty) = variants
                .cases()
                .enumerate()
                .find(|(_, case)| case.name == label)
                .expect("Unknown variant tag");

            let val = match payload_ty.ty {
                Some(typ) => Some(Box::new(json_to_val(value, &typ))),
                None => {
                    if !value.is_null() {
                        panic!("Variant '{}' should have null payload", label);
                    }
                    None
                }
            };

            Val::Variant(index.to_string(), val)
        }
        Type::Enum(cases) => {
            let tag = json.as_str().expect("Expected enum string");
            let value = cases
                .names()
                .find(|s| *s == tag)
                .expect("Invalid enum string");

            Val::Enum(value.to_string())
        }
        Type::Option(inner_ty) => {
            if json.is_null() {
                Val::Option(None)
            } else {
                Val::Option(Some(Box::new(json_to_val(json, &inner_ty.ty()))))
            }
        }
        Type::Result(result_type) => {
            let obj = json
                .as_object()
                .expect("Expected object with 'Ok' or 'Err'");

            if let Some(ok_val) = obj.get("ok") {
                let val = match &result_type.ok() {
                    Some(ty) => Some(Box::new(json_to_val(ok_val, ty))),
                    None => {
                        assert!(
                            ok_val.is_null(),
                            "Expected null for Result::Ok with no payload"
                        );
                        None
                    }
                };
                Val::Result(Ok(val))
            } else if let Some(err_val) = obj.get("err") {
                let val = match &result_type.err() {
                    Some(ty) => Some(Box::new(json_to_val(err_val, ty))),
                    None => {
                        assert!(
                            err_val.is_null(),
                            "Expected null for Result::Err with no payload"
                        );
                        None
                    }
                };
                Val::Result(Err(val))
            } else {
                panic!("Expected object with either 'Ok' or 'Err' key");
            }
        }
        Type::Flags(_) => todo!(),
        Type::Own(_) => todo!(),
        Type::Borrow(_) => todo!(),
    }
}
