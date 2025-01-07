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
    use crate::type_inference::kind::GetTypeKind;
    use crate::{
        DynamicParsedFunctionName, Expr, FunctionTypeRegistry, InferredType, RegistryKey,
        RegistryValue,
    };
    use golem_wasm_ast::analysis::AnalysedType;
    use std::fmt::Display;

    pub(crate) fn resolve_call_argument_types(
        call_type: &mut CallType,
        function_type_registry: &FunctionTypeRegistry,
        args: &mut [Expr],
        function_result_inferred_type: &mut InferredType,
    ) -> Result<(), String> {
        let cloned = call_type.clone();

        match call_type {
            CallType::Function(dynamic_parsed_function_name) => {
                let resource_constructor_registry_key =
                    RegistryKey::resource_constructor_registry_key(dynamic_parsed_function_name);

                match resource_constructor_registry_key {
                    Some(resource_constructor_name) => handle_function_with_resource(
                        &resource_constructor_name,
                        dynamic_parsed_function_name,
                        function_type_registry,
                        function_result_inferred_type,
                        args,
                    ),
                    None => {
                        let registry_key = RegistryKey::from_call_type(&cloned);
                        infer_args_and_result_type(
                            &FunctionDetails::Fqn(dynamic_parsed_function_name.to_string()),
                            function_type_registry,
                            &registry_key,
                            args,
                            Some(function_result_inferred_type),
                        )
                        .map_err(|e| e.to_string())
                    }
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
                infer_args_and_result_type(
                    &FunctionDetails::VariantName(variant_name.clone()),
                    function_type_registry,
                    &registry_key,
                    args,
                    Some(function_result_inferred_type),
                )
                .map_err(|e| e.to_string())
            }
        }
    }

    // An internal error type for all possibilities of errors
    // when inferring the type of arguments
    enum FunctionArgsTypeInferenceError {
        UnknownFunction(FunctionDetails),
        ArgumentSizeMisMatch {
            function_type_internal: FunctionDetails,
            expected: usize,
            provided: usize,
        },
        TypeMisMatchError {
            function_type_internal: FunctionDetails,
            expected: AnalysedType,
            provided: Expr,
        },
    }

    impl FunctionArgsTypeInferenceError {
        fn type_mismatch(
            function_type_internal: FunctionDetails,
            expected: AnalysedType,
            provided: Expr,
        ) -> FunctionArgsTypeInferenceError {
            FunctionArgsTypeInferenceError::TypeMisMatchError {
                function_type_internal,
                expected,
                provided,
            }
        }
    }

    impl Display for FunctionArgsTypeInferenceError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                FunctionArgsTypeInferenceError::UnknownFunction(FunctionDetails::Fqn(
                    parsed_function_name,
                )) => {
                    write!(f, "Unknown function call: `{}`", parsed_function_name)
                }
                FunctionArgsTypeInferenceError::UnknownFunction(
                    FunctionDetails::ResourceMethodName {
                        fqn,
                        resource_constructor_name_pretty: resource_name_human,
                        resource_method_name_pretty: resource_method_name_human,
                        ..
                    },
                ) => {
                    write!(
                        f,
                        "Unknown resource method call `{}`. `{}` doesn't exist in resource `{}`",
                        fqn, resource_method_name_human, resource_name_human
                    )
                }
                FunctionArgsTypeInferenceError::UnknownFunction(
                    FunctionDetails::ResourceConstructorName {
                        fqn,
                        resource_constructor_name_pretty: resource_constructor_name_human,
                        ..
                    },
                ) => {
                    write!(
                        f,
                        "Unknown resource constructor call: `{}`. Resource `{}` doesn't exist",
                        fqn, resource_constructor_name_human
                    )
                }

                FunctionArgsTypeInferenceError::UnknownFunction(FunctionDetails::VariantName(
                    variant_name,
                )) => {
                    write!(f, "Invalid variant constructor call: {}", variant_name)
                }

                FunctionArgsTypeInferenceError::TypeMisMatchError {
                    function_type_internal,
                    expected,
                    provided,
                } => match function_type_internal {
                    FunctionDetails::ResourceConstructorName {
                        resource_constructor_name_pretty: resource_constructor_name_human,
                        ..
                    } => {
                        write!(f,"Invalid type for the argument in resource constructor `{}`. Expected type `{}`, but provided argument `{}` is a `{}`", resource_constructor_name_human, expected.get_type_kind(), provided, provided.inferred_type().get_type_kind())
                    }
                    FunctionDetails::ResourceMethodName { fqn, .. } => {
                        write!(f,"Invalid type for the argument in resource method `{}`. Expected type `{}`, but provided argument `{}` is a `{}`", fqn, expected.get_type_kind(), provided, provided.inferred_type().get_type_kind())
                    }
                    FunctionDetails::Fqn(fqn) => {
                        write!(f,"Invalid type for the argument in function `{}`. Expected type `{}`, but provided argument `{}` is a `{}`", fqn, expected.get_type_kind(), provided, provided.inferred_type().get_type_kind())
                    }
                    FunctionDetails::VariantName(str) => {
                        write!(f,"Invalid type for the argument in variant constructor `{}`. Expected type `{}`, but provided argument `{}` is a `{}`", str, expected.get_type_kind(), provided, provided.inferred_type().get_type_kind())
                    }
                },
                FunctionArgsTypeInferenceError::ArgumentSizeMisMatch {
                    function_type_internal,
                    expected,
                    provided,
                } => match function_type_internal {
                    FunctionDetails::ResourceConstructorName {
                        resource_constructor_name_pretty,
                        ..
                    } => {
                        write!(f, "Incorrect number of arguments for resource constructor `{}`. Expected {}, but provided {}", resource_constructor_name_pretty, expected, provided)
                    }
                    FunctionDetails::ResourceMethodName { fqn, .. } => {
                        write!(f, "Incorrect number of arguments in resource method `{}`. Expected {}, but provided {}", fqn, expected, provided)
                    }
                    FunctionDetails::Fqn(fqn) => {
                        write!(f, "Incorrect number of arguments for function `{}`. Expected {}, but provided {}", fqn, expected, provided)
                    }
                    FunctionDetails::VariantName(str) => {
                        write!(f, "Invalid number of arguments in variant `{}`. Expected {}, but provided {}", str, expected, provided)
                    }
                },
            }
        }
    }

    fn handle_function_with_resource(
        resource_constructor_registry_key: &RegistryKey,
        dynamic_parsed_function_name: &mut DynamicParsedFunctionName,
        function_type_registry: &FunctionTypeRegistry,
        function_result_inferred_type: &mut InferredType,
        resource_method_args: &mut [Expr],
    ) -> Result<(), String> {
        // Infer the resource constructors
        infer_resource_constructor_arguments(
            resource_constructor_registry_key,
            dynamic_parsed_function_name,
            function_type_registry,
        )?;

        let resource_method_registry_key =
            RegistryKey::fqn_registry_key(dynamic_parsed_function_name);

        // Infer the resource arguments
        infer_resource_method_arguments(
            &resource_method_registry_key,
            dynamic_parsed_function_name,
            function_type_registry,
            resource_method_args,
            function_result_inferred_type,
        )
    }

    fn infer_resource_method_arguments(
        resource_method_registry_key: &RegistryKey,
        dynamic_parsed_function_name: &mut DynamicParsedFunctionName,
        function_type_registry: &FunctionTypeRegistry,
        resource_method_args: &mut [Expr],
        function_result_inferred_type: &mut InferredType,
    ) -> Result<(), String> {
        // Infer the types of resource method parameters
        let resource_method_name_in_metadata =
            dynamic_parsed_function_name.function_name_with_prefix_identifiers();

        infer_args_and_result_type(
            &FunctionDetails::ResourceMethodName {
                fqn: dynamic_parsed_function_name.to_string(),
                resource_constructor_name_pretty: dynamic_parsed_function_name
                    .resource_name_simplified()
                    .unwrap_or_default(),
                resource_method_name_pretty: dynamic_parsed_function_name
                    .resource_method_name_simplified()
                    .unwrap_or_default(),
                resource_method_name: resource_method_name_in_metadata,
            },
            function_type_registry,
            resource_method_registry_key,
            resource_method_args,
            Some(function_result_inferred_type),
        )
        .map_err(|e| e.to_string())
    }

    fn infer_resource_constructor_arguments(
        resource_constructor_registry_key: &RegistryKey,
        dynamic_parsed_function_name: &mut DynamicParsedFunctionName,
        function_type_registry: &FunctionTypeRegistry,
    ) -> Result<(), String> {
        let fqn = dynamic_parsed_function_name.to_string();
        // Mainly for error reporting
        let resource_constructor_name_pretty = dynamic_parsed_function_name
            .resource_name_simplified()
            .unwrap_or_default();

        let mut constructor_params: &mut Vec<Expr> = &mut vec![];

        if let Some(resource_params) = dynamic_parsed_function_name.raw_resource_params_mut() {
            constructor_params = resource_params
        }

        // Infer the types of constructor parameter expressions
        infer_args_and_result_type(
            &FunctionDetails::ResourceConstructorName {
                fqn,
                resource_constructor_name_pretty,
                resource_constructor_name: resource_constructor_registry_key.get_function_name(),
            },
            function_type_registry,
            resource_constructor_registry_key,
            constructor_params,
            None,
        )
        .map_err(|e| e.to_string())
    }

    fn infer_args_and_result_type(
        function_name: &FunctionDetails,
        function_type_registry: &FunctionTypeRegistry,
        key: &RegistryKey,
        args: &mut [Expr],
        function_result_inferred_type: Option<&mut InferredType>,
    ) -> Result<(), FunctionArgsTypeInferenceError> {
        if let Some(value) = function_type_registry.types.get(key) {
            match value {
                RegistryValue::Value(_) => Ok(()),
                RegistryValue::Variant {
                    parameter_types,
                    variant_type,
                } => {
                    let parameter_types = parameter_types.clone();

                    if parameter_types.len() == args.len() {
                        tag_argument_types(function_name, args, &parameter_types)?;

                        if let Some(function_result_type) = function_result_inferred_type {
                            *function_result_type = InferredType::from_variant_cases(variant_type);
                        }

                        Ok(())
                    } else {
                        Err(FunctionArgsTypeInferenceError::ArgumentSizeMisMatch {
                            function_type_internal: function_name.clone(),
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

                    if let FunctionDetails::ResourceMethodName { .. } = function_name {
                        if let Some(AnalysedType::Handle(_)) = parameter_types.first() {
                            parameter_types.remove(0);
                        }
                    }

                    if parameter_types.len() == args.len() {
                        tag_argument_types(function_name, args, &parameter_types)?;

                        if let Some(function_result_type) = function_result_inferred_type {
                            *function_result_type = {
                                if return_types.len() == 1 {
                                    return_types[0].clone().into()
                                } else {
                                    InferredType::Sequence(
                                        return_types.iter().map(|t| t.clone().into()).collect(),
                                    )
                                }
                            }
                        };

                        Ok(())
                    } else {
                        Err(FunctionArgsTypeInferenceError::ArgumentSizeMisMatch {
                            function_type_internal: function_name.clone(),
                            expected: parameter_types.len(),
                            provided: args.len(),
                        })
                    }
                }
            }
        } else {
            Err(FunctionArgsTypeInferenceError::UnknownFunction(
                function_name.clone(),
            ))
        }
    }

    // An internal structure that has specific details
    // of the components of a function name, especially to handle
    // the resource constructors within a function name.
    #[derive(Clone)]
    enum FunctionDetails {
        ResourceConstructorName {
            fqn: String,
            resource_constructor_name_pretty: String,
            resource_constructor_name: String,
        },
        ResourceMethodName {
            fqn: String,
            resource_constructor_name_pretty: String,
            resource_method_name_pretty: String,
            resource_method_name: String,
        },
        Fqn(String),
        VariantName(String),
    }

    impl Display for FunctionDetails {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                FunctionDetails::ResourceConstructorName {
                    resource_constructor_name,
                    ..
                } => {
                    write!(f, "{}", resource_constructor_name)
                }
                FunctionDetails::ResourceMethodName {
                    resource_method_name,
                    ..
                } => {
                    write!(f, "{}", resource_method_name)
                }
                FunctionDetails::Fqn(fqn) => {
                    write!(f, "{}", fqn)
                }
                FunctionDetails::VariantName(name) => {
                    write!(f, "{}", name)
                }
            }
        }
    }

    // A preliminary check of the arguments passed before  typ inference
    fn check_function_arguments(
        function_name: &FunctionDetails,
        expected: &AnalysedType,
        provided: &Expr,
    ) -> Result<(), FunctionArgsTypeInferenceError> {
        let is_valid = if provided.inferred_type().is_unknown() {
            true
        } else {
            provided.inferred_type().get_type_kind() == expected.get_type_kind()
        };

        if is_valid {
            Ok(())
        } else {
            Err(FunctionArgsTypeInferenceError::type_mismatch(
                function_name.clone(),
                expected.clone(),
                provided.clone(),
            ))
        }
    }

    fn tag_argument_types(
        function_name: &FunctionDetails,
        args: &mut [Expr],
        parameter_types: &[AnalysedType],
    ) -> Result<(), FunctionArgsTypeInferenceError> {
        for (arg, param_type) in args.iter_mut().zip(parameter_types) {
            check_function_arguments(function_name, param_type, arg)?;
            arg.add_infer_type_mut(param_type.clone().into());
        }

        Ok(())
    }
}

#[cfg(test)]
mod function_parameters_inference_tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

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

        let let_binding = Expr::let_binding("x", Expr::untyped_number(BigDecimal::from(1)));

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

        let expected = Expr::ExprBlock(vec![let_binding, call_expr], InferredType::Unknown);

        assert_eq!(expr, expected);
    }
}
