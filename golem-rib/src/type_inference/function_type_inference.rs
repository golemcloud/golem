// Copyright 2024 Golem Cloud
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

use crate::type_registry::{FunctionTypeRegistry, RegistryKey, RegistryValue};
use crate::{Expr, InferredType};
use std::collections::VecDeque;
use crate::call_type::CallType;

pub fn infer_function_types(
    expr: &mut Expr,
    function_type_registry: &FunctionTypeRegistry,
) -> Result<(), String> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);
    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Call(parsed_fn_name, args, inferred_type) => {
                internal::resolve_call_expressions(parsed_fn_name, function_type_registry, args, inferred_type)?;
            }
            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }

    Ok(())
}

mod internal {
    use crate::{DynamicParsedFunctionName, Expr, FunctionTypeRegistry, InferredType, RegistryKey, RegistryValue};
    use golem_wasm_ast::analysis::AnalysedType;
    use crate::call_type::CallType;

    pub(crate) fn resolve_call_expressions(call_type: &mut CallType, function_type_registry: &FunctionTypeRegistry, args: &mut Vec<Expr>, inferred_type: &mut InferredType) -> Result<(), String>{
        match call_type {
            CallType::Function(dynamic_parsed_function_name) => {
                let parsed_function_static = dynamic_parsed_function_name.clone().to_static();
                let function = parsed_function_static.function;
                let indexed_resource = function.is_indexed_resource();

                if indexed_resource {
                    // Inferring th types of the resource parameters
                    let constructor = {
                        let raw_str = function.resource_name().ok_or("Resource name not found")?;
                        format!["[constructor]{}", raw_str]
                    };

                    let mut constructor_params =
                        dynamic_parsed_function_name
                            .function
                            .raw_resource_params().ok_or("Resource params not found")?;

                    let registry_key = RegistryKey::from_function_name(&parsed_function_static.site, constructor.as_str());

                    infer_types(constructor.as_str(), function_type_registry, registry_key, &mut constructor_params, inferred_type, false)?;

                    // Inferring the types of the final method in the resource
                    let resource_method_name = function.function_name();
                    let registry_key = RegistryKey::from_function_name(&parsed_function_static.site, resource_method_name.as_str());

                    infer_types(resource_method_name.as_str(), function_type_registry, registry_key, args, inferred_type, true)
                }

                else {
                    let registry_key = RegistryKey::from_invocation_name(call_type);

                    infer_types(function.function_name().as_str(), function_type_registry, registry_key, args, inferred_type, false)
                }
            }

            // This will never happen unless variant identification phase happens before functions identification phase
           _ => panic!("Enum constructor not supported"),
        }
    }

    pub(crate) fn infer_types(function_name: &str, function_type_registry: &FunctionTypeRegistry, key: RegistryKey, args: &mut Vec<Expr>, inferred_type: &mut InferredType, is_resource_method: bool) -> Result<(), String> {
        if let Some(value) = function_type_registry.types.get(&key) {
            match value {
                RegistryValue::Value(_) => {}
                RegistryValue::Function {
                    parameter_types,
                    return_types,
                } => {
                    let mut parameter_types = parameter_types.clone();
                    if is_resource_method {
                        parameter_types = parameter_types.iter().filter(|t| match t {
                            AnalysedType::Handle(_) => false,
                            _ => true,
                        }).cloned().collect();
                    }
                    if parameter_types.len() == args.len() {
                        for (arg, param_type) in args.iter_mut().zip(parameter_types) {
                            check_function_arguments(&param_type, arg)?;
                            arg.add_infer_type_mut(param_type.clone().into());
                            arg.push_types_down()?
                        }

                        *inferred_type = {
                            if return_types.len() == 1 {
                                return_types[0].clone().into()
                            } else {
                                InferredType::Sequence(
                                    return_types.iter().map(|t| t.clone().into()).collect(),
                                )
                            }
                        }

                    } else {
                        return Err(format!(
                            "Function {} expects {} arguments, but {} were provided",
                            function_name,
                            parameter_types.len(),
                            args.len()
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    // A preliminary check of the arguments passed before  typ inference
    pub(crate) fn check_function_arguments(
        expected: &AnalysedType,
        passed: &Expr,
    ) -> Result<(), String> {
        let valid_possibilities = passed.is_identifier()
            || passed.is_select_field()
            || passed.is_select_index()
            || passed.is_select_field()
            || passed.is_match_expr()
            || passed.is_if_else();

        match expected {
            AnalysedType::U32(_) => {
                if valid_possibilities || passed.is_number() {
                    Ok(())
                } else {
                    Err(format!("Expected U32, but found {:?}", passed))
                }
            }

            AnalysedType::U64(_) => {
                if valid_possibilities || passed.is_number() {
                    Ok(())
                } else {
                    Err(format!("Expected U64, but found {:?}", passed))
                }
            }

            AnalysedType::Variant(_) => {
                if valid_possibilities || passed.is_number() {
                    Ok(())
                } else {
                    Err(format!("Expected Variant, but found {:?}", passed))
                }
            }

            AnalysedType::Result(_) => {
                if valid_possibilities || passed.is_result() {
                    Ok(())
                } else {
                    Err(format!("Expected Result, but found {:?}", passed))
                }
            }
            AnalysedType::Option(_) => {
                if valid_possibilities || passed.is_option() {
                    Ok(())
                } else {
                    Err(format!("Expected Option, but found {:?}", passed))
                }
            }
            AnalysedType::Enum(_) => {
                if valid_possibilities {
                    Ok(())
                } else {
                    Err(format!("Expected Enum, but found {:?}", passed))
                }
            }
            AnalysedType::Flags(_) => {
                if valid_possibilities || passed.is_flags() {
                    Ok(())
                } else {
                    Err(format!("Expected Flags, but found {:?}", passed))
                }
            }
            AnalysedType::Record(_) => {
                if valid_possibilities || passed.is_record() {
                    Ok(())
                } else {
                    Err(format!("Expected Record, but found {:?}", passed))
                }
            }
            AnalysedType::Tuple(_) => {
                if valid_possibilities || passed.is_tuple() {
                    Ok(())
                } else {
                    Err(format!("Expected Tuple, but found {:?}", passed))
                }
            }
            AnalysedType::List(_) => {
                if valid_possibilities || passed.is_list() {
                    Ok(())
                } else {
                    Err(format!("Expected List, but found {:?}", passed))
                }
            }
            AnalysedType::Str(_) => {
                if valid_possibilities || passed.is_concat() || passed.is_literal() {
                    Ok(())
                } else {
                    Err(format!("Expected Str, but found {:?}", passed))
                }
            }
            // TODO?
            AnalysedType::Chr(_) => {
                if valid_possibilities || passed.is_literal() {
                    Ok(())
                } else {
                    Err(format!("Expected Chr, but found {:?}", passed))
                }
            }
            AnalysedType::F64(_) => {
                if valid_possibilities || passed.is_number() {
                    Ok(())
                } else {
                    Err(format!("Expected F64, but found {:?}", passed))
                }
            }
            AnalysedType::F32(_) => {
                if valid_possibilities || passed.is_number() {
                    Ok(())
                } else {
                    Err(format!("Expected F32, but found {:?}", passed))
                }
            }
            AnalysedType::S64(_) => {
                if valid_possibilities || passed.is_number() {
                    Ok(())
                } else {
                    Err(format!("Expected S64, but found {:?}", passed))
                }
            }
            AnalysedType::S32(_) => {
                if valid_possibilities || passed.is_number() {
                    Ok(())
                } else {
                    Err(format!("Expected S32, but found {:?}", passed))
                }
            }
            AnalysedType::U16(_) => {
                if valid_possibilities || passed.is_number() {
                    Ok(())
                } else {
                    Err(format!("Expected U16, but found {:?}", passed))
                }
            }
            AnalysedType::S16(_) => {
                if valid_possibilities || passed.is_number() {
                    Ok(())
                } else {
                    Err(format!("Expected S16, but found {:?}", passed))
                }
            }
            AnalysedType::U8(_) => {
                if valid_possibilities || passed.is_number() {
                    Ok(())
                } else {
                    Err(format!("Expected U8, but found {:?}", passed))
                }
            }
            AnalysedType::S8(_) => {
                if valid_possibilities || passed.is_number() {
                    Ok(())
                } else {
                    Err(format!("Expected S8, but found {:?}", passed))
                }
            }
            AnalysedType::Bool(_) => {
                if valid_possibilities || passed.is_boolean() || passed.is_comparison() {
                    Ok(())
                } else {
                    Err(format!("Expected Bool, but found {:?}", passed))
                }
            }
            AnalysedType::Handle(_) => {
                if valid_possibilities {
                    Ok(())
                } else {
                    Err(format!("Expected Handle, but found {:?}", passed))
                }
            }
        }
    }
}

#[cfg(test)]
mod function_parameters_inference_tests {
    use crate::call_type::CallType;
    use crate::function_name::{DynamicParsedFunctionName, DynamicParsedFunctionReference};
    use crate::type_registry::FunctionTypeRegistry;
    use crate::{Expr, InferredType, ParsedFunctionSite, VariableId};
    use golem_wasm_ast::analysis::{
        AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedType, TypeU32, TypeU64,
    };

    fn get_function_type_registry() -> FunctionTypeRegistry {
        let metadata = vec![
            AnalysedExport::Function(AnalysedFunction {
                name: "foo".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "my_parameter".to_string(),
                    typ: AnalysedType::U64(TypeU64),
                }],
                results: vec![],
            }),
            AnalysedExport::Function(AnalysedFunction {
                name: "baz".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "my_parameter".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                }],
                results: vec![],
            }),
        ];
        FunctionTypeRegistry::from_export_metadata(&metadata)
    }

    #[test]
    fn test_infer_function_types() {
        let rib_expr = r#"
          let x = 1;
          foo(x)
        "#;

        let function_type_registry = get_function_type_registry();

        let mut expr = Expr::from_text(rib_expr).unwrap();
        expr.infer_function_types(&function_type_registry).unwrap();

        let let_binding = Expr::let_binding("x", Expr::number(1f64));

        let call_expr = Expr::Call(
            CallType::Function(DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            }),
            vec![Expr::Identifier(
                VariableId::global("x".to_string()),
                InferredType::U64, // Call argument's types are updated
            )],
            InferredType::Sequence(vec![]), // Call Expressions return type is updated
        );

        let expected = Expr::Multiple(vec![let_binding, call_expr], InferredType::Unknown);

        assert_eq!(expr, expected);
    }
}
