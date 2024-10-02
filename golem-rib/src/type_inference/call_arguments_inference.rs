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

use crate::type_registry::FunctionTypeRegistry;
use crate::Expr;
use std::collections::VecDeque;

pub fn infer_call_arguments_type(
    expr: &mut Expr,
    function_type_registry: &FunctionTypeRegistry,
) -> Result<(), String> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);
    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Call(parsed_fn_name, args, inferred_type) => {
                internal::resolve_call_argument_types(
                    parsed_fn_name,
                    function_type_registry,
                    args,
                    inferred_type,
                )?;
            }
            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }

    Ok(())
}

mod internal {
    use crate::call_type::CallType;
    use crate::{
        Expr, FunctionTypeRegistry, InferredType, ParsedFunctionName, RegistryKey, RegistryValue,
    };
    use golem_wasm_ast::analysis::AnalysedType;
    use std::fmt::Display;

    pub(crate) fn resolve_call_argument_types(
        call_type: &mut CallType,
        function_type_registry: &FunctionTypeRegistry,
        args: &mut [Expr],
        inferred_type: &mut InferredType,
    ) -> Result<(), String> {
        match call_type {
            CallType::Function(dynamic_parsed_function_name) => {
                let parsed_function_static = dynamic_parsed_function_name.clone().to_static();
                let function = parsed_function_static.clone().function;
                if function.resource_name().is_some() {
                    let constructor_name = {
                        let raw_str = function.resource_name().ok_or("Resource name not found")?;
                        format!["[constructor]{}", raw_str]
                    };

                    let mut constructor_params: &mut Vec<Expr> = &mut vec![];

                    if let Some(resource_params) = dynamic_parsed_function_name
                        .function
                        .raw_resource_params_mut()
                    {
                        constructor_params = resource_params
                    }

                    let registry_key = RegistryKey::from_function_name(
                        &parsed_function_static.site,
                        constructor_name.as_str(),
                    );

                    // Infer the types of constructor parameter expressions
                    infer_types(
                        &FunctionNameInternal::ResourceConstructorName(constructor_name),
                        function_type_registry,
                        registry_key,
                        constructor_params,
                        inferred_type,
                    )?;

                    // Infer the types of resource method parameters
                    let resource_method_name = function.function_name();
                    let registry_key = RegistryKey::from_function_name(
                        &parsed_function_static.site,
                        resource_method_name.as_str(),
                    );

                    infer_types(
                        &FunctionNameInternal::ResourceMethodName(resource_method_name),
                        function_type_registry,
                        registry_key,
                        args,
                        inferred_type,
                    )
                } else {
                    let registry_key = RegistryKey::from_invocation_name(call_type);
                    infer_types(
                        &FunctionNameInternal::Fqn(parsed_function_static),
                        function_type_registry,
                        registry_key,
                        args,
                        inferred_type,
                    )
                }
            }

            CallType::EnumConstructor(_) => {
                if args.is_empty() {
                    Ok(())
                } else {
                    Err("Enum constructor does not take any arguments".to_string())
                }
            }

            CallType::VariantConstructor(variant_name) => {
                let registry_key = RegistryKey::FunctionName(variant_name.clone());
                infer_types(
                    &FunctionNameInternal::VariantName(variant_name.clone()),
                    function_type_registry,
                    registry_key,
                    args,
                    inferred_type,
                )
            }
        }
    }

    fn infer_types(
        function_name: &FunctionNameInternal,
        function_type_registry: &FunctionTypeRegistry,
        key: RegistryKey,
        args: &mut [Expr],
        inferred_type: &mut InferredType,
    ) -> Result<(), String> {
        if let Some(value) = function_type_registry.types.get(&key) {
            match value {
                RegistryValue::Value(_) => Ok(()),
                RegistryValue::Variant {
                    parameter_types,
                    variant_type,
                } => {
                    let parameter_types = parameter_types.clone();

                    if parameter_types.len() == args.len() {
                        tag_argument_types(args, &parameter_types)?;
                        *inferred_type = InferredType::from_variant_cases(variant_type);

                        Ok(())
                    } else {
                        Err(format!(
                            "Variant {} expects {} arguments, but {} were provided",
                            function_name,
                            parameter_types.len(),
                            args.len()
                        ))
                    }
                }
                RegistryValue::Function {
                    parameter_types,
                    return_types,
                } => {
                    let mut parameter_types = parameter_types.clone();

                    if let FunctionNameInternal::ResourceMethodName(_) = function_name {
                        if let Some(AnalysedType::Handle(_)) = parameter_types.first() {
                            parameter_types.remove(0);
                        }
                    }

                    if parameter_types.len() == args.len() {
                        tag_argument_types(args, &parameter_types)?;

                        *inferred_type = {
                            if return_types.len() == 1 {
                                return_types[0].clone().into()
                            } else {
                                InferredType::Sequence(
                                    return_types.iter().map(|t| t.clone().into()).collect(),
                                )
                            }
                        };

                        Ok(())
                    } else {
                        Err(format!(
                            "Function {} expects {} arguments, but {} were provided",
                            function_name,
                            parameter_types.len(),
                            args.len()
                        ))
                    }
                }
            }
        } else {
            Err(format!("Unknown function/variant call {}", function_name))
        }
    }

    enum FunctionNameInternal {
        ResourceConstructorName(String),
        ResourceMethodName(String),
        Fqn(ParsedFunctionName),
        VariantName(String),
    }

    impl Display for FunctionNameInternal {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                FunctionNameInternal::ResourceConstructorName(name) => {
                    write!(f, "{}", name)
                }
                FunctionNameInternal::ResourceMethodName(name) => {
                    write!(f, "{}", name)
                }
                FunctionNameInternal::Fqn(name) => {
                    write!(f, "{}", name)
                }
                FunctionNameInternal::VariantName(name) => {
                    write!(f, "{}", name)
                }
            }
        }
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

    fn tag_argument_types(
        args: &mut [Expr],
        parameter_types: &[AnalysedType],
    ) -> Result<(), String> {
        for (arg, param_type) in args.iter_mut().zip(parameter_types) {
            check_function_arguments(param_type, arg)?;
            arg.add_infer_type_mut(param_type.clone().into());
        }

        Ok(())
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
        expr.infer_call_arguments_type(&function_type_registry)
            .unwrap();

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
