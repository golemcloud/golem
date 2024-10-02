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
    use std::fmt::{Display};
    use crate::type_inference::kind::{GetTypeKind, TypeKind};

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
                    let resource_name =  function.resource_name().ok_or("Resource name not found")?;

                    let constructor_name = {
                        format!["[constructor]{}", resource_name]
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
                    ).map_err(|e| match e {
                        ArgTypesInferenceError::UnknownFunction => {
                            format!("Unknown resource constructor call: `{}`. Resource `{}` doesn't exist" , parsed_function_static, resource_name)
                        }
                        ArgTypesInferenceError::ArgumentSizeMisMatch {
                            expected,
                            provided,
                        } => format!(
                            "Incorrect number of arguments for resource constructor `{}`. Expected {}, but provided {}",
                            resource_name, expected, provided
                        ),
                        ArgTypesInferenceError::TypeMisMatchError { expected, provided } => {
                            format!(
                                "Invalid arguments for resource constructor {}. Expected type {:?}, but provided {}",
                                resource_name, expected, provided
                            )
                        }
                    })?;

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
                    ).map_err(|e| match e {
                        ArgTypesInferenceError::UnknownFunction => {
                            format!("Invalid resource method call {}. `{}` doesn't exist in resource `{}`", parsed_function_static, parsed_function_static.function.resource_method_name().unwrap(), resource_name)
                        }
                        ArgTypesInferenceError::ArgumentSizeMisMatch {
                            expected,
                            provided,
                        } => format!(
                            "Incorrect number of arguments in resource method `{}`. Expected {}, but provided {}",
                            parsed_function_static, expected, provided
                        ),
                        ArgTypesInferenceError::TypeMisMatchError { expected, provided } => {
                            format!(
                                "Invalid arguments to resource method {}. Expected type {:?}, but provided {}",
                                parsed_function_static, expected, provided
                            )
                        }
                    })
                } else {
                    let registry_key = RegistryKey::from_invocation_name(call_type);
                    infer_types(
                        &FunctionNameInternal::Fqn(parsed_function_static.clone()),
                        function_type_registry,
                        registry_key,
                        args,
                        inferred_type,
                    ).map_err(|e| match e {
                        ArgTypesInferenceError::UnknownFunction => {
                            format!("Unknown function call: `{}`", parsed_function_static.function.function_name())
                        }
                        ArgTypesInferenceError::ArgumentSizeMisMatch {
                            expected,
                            provided,
                        } => format!(
                            "Incorrect number of arguments for function `{}`. Expected {}, but provided {}",
                            parsed_function_static, expected, provided
                        ),
                        ArgTypesInferenceError::TypeMisMatchError { expected, provided } => {
                            format!(
                                "Invalid argument types in function {}. Expected type {:?}, but provided {}",
                                parsed_function_static.function.function_name(), expected, provided
                            )
                        }
                    })
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
                ).map_err(|e| match e {
                    ArgTypesInferenceError::UnknownFunction => {
                        format!("Invalid variant constructor call: {}", variant_name)
                    }
                    ArgTypesInferenceError::ArgumentSizeMisMatch {
                        expected,
                        provided,
                    } => format!(
                        "Invalid variant construction: {}. Expected {} arguments, but provided {}",
                        variant_name, expected, provided
                    ),
                    ArgTypesInferenceError::TypeMisMatchError { expected, provided } => {
                        format!(
                            "Invalid type for {} construction arguments. Expected type {:?}, but provided {}",
                            variant_name, expected, provided
                        )
                    }
                })
            }
        }
    }
    enum ArgTypesInferenceError {
        UnknownFunction,
        ArgumentSizeMisMatch {
            expected: usize,
            provided: usize,
        },
        TypeMisMatchError {
            expected: AnalysedType,
            provided: TypeKind
        },
    }

    impl ArgTypesInferenceError {
        fn type_mismatch(expected: AnalysedType, provided: TypeKind) -> ArgTypesInferenceError {
            ArgTypesInferenceError::TypeMisMatchError {
                expected,
                provided
            }
        }
    }

    fn infer_types(
        function_name: &FunctionNameInternal,
        function_type_registry: &FunctionTypeRegistry,
        key: RegistryKey,
        args: &mut [Expr],
        inferred_type: &mut InferredType,
    ) -> Result<(), ArgTypesInferenceError> {
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
                        Err(ArgTypesInferenceError::ArgumentSizeMisMatch {
                            expected: parameter_types.len(),
                            provided: args.len(),
                        })
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
                        Err(ArgTypesInferenceError::ArgumentSizeMisMatch {
                            expected: parameter_types.len(),
                            provided: args.len(),
                        })
                    }
                }
            }
        } else {
            Err(ArgTypesInferenceError::UnknownFunction)
        }
    }

    #[derive(Clone)]
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
    fn check_function_arguments(
        expected: &AnalysedType,
        provided: &Expr,
    ) -> Result<(), ArgTypesInferenceError> {
        let is_valid = if provided.inferred_type().is_unknown() {
            true
        } else {
            provided.inferred_type().get_kind() == expected.get_kind()
        };

        if is_valid {
            Ok(())
        } else {
            Err(ArgTypesInferenceError::type_mismatch(expected.clone(), provided.inferred_type().get_kind()))
        }
    }

    fn tag_argument_types(
        args: &mut [Expr],
        parameter_types: &[AnalysedType],
    ) -> Result<(), ArgTypesInferenceError> {
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
