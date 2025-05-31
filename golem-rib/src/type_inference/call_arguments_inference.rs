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

use crate::type_registry::FunctionTypeRegistry;
use crate::{Expr, ExprVisitor, FunctionCallError};

// Resolving function arguments and return types based on function type registry
// If the function call is a mere instance creation, then the return type i
pub fn infer_function_call_types(
    expr: &mut Expr,
    function_type_registry: &FunctionTypeRegistry,
) -> Result<(), FunctionCallError> {
    let mut visitor = ExprVisitor::bottom_up(expr);
    while let Some(expr) = visitor.pop_back() {
        let expr_copied = expr.clone();

        if let Expr::Call {
            call_type,
            args,
            inferred_type,
            ..
        } = expr
        {
            internal::resolve_call_argument_types(
                &expr_copied,
                call_type,
                function_type_registry,
                args,
                inferred_type,
            )?;
        }
    }

    Ok(())
}

mod internal {
    use crate::call_type::{CallType, InstanceCreationType};
    use crate::inferred_type::TypeOrigin;
    use crate::type_inference::GetTypeHint;
    use crate::{
        ActualType, DynamicParsedFunctionName, ExpectedType, Expr, FunctionCallError,
        FunctionTypeRegistry, InferredType, RegistryKey, RegistryValue, TypeMismatchError,
    };
    use golem_wasm_ast::analysis::AnalysedType;
    use std::fmt::Display;

    pub(crate) fn resolve_call_argument_types(
        original_expr: &Expr,
        call_type: &mut CallType,
        function_type_registry: &FunctionTypeRegistry,
        args: &mut [Expr],
        function_result_inferred_type: &mut InferredType,
    ) -> Result<(), FunctionCallError> {
        let cloned = call_type.clone();

        match call_type {
            CallType::InstanceCreation(instance) => match instance {
                InstanceCreationType::Worker { .. } => {
                    for arg in args.iter_mut() {
                        arg.add_infer_type_mut(InferredType::string());
                    }

                    Ok(())
                }

                InstanceCreationType::Resource { resource_name, .. } => {
                    let resource_constructor_with_prefix =
                        format!["[constructor]{}", resource_name.resource_name];
                    let interface =
                        match (&resource_name.package_name, &resource_name.interface_name) {
                            (Some(package_name), Some(interface_name)) => {
                                Some(format!("{}/{}", package_name, interface_name))
                            }
                            (None, Some(interface_name)) => Some(interface_name.to_string()),
                            _ => None,
                        };

                    let registry_key = match interface {
                        None => RegistryKey::FunctionName(resource_constructor_with_prefix),
                        Some(interface) => RegistryKey::FunctionNameWithInterface {
                            interface_name: interface.to_string(),
                            function_name: resource_constructor_with_prefix,
                        },
                    };

                    infer_resource_constructor_arguments(
                        original_expr,
                        &registry_key,
                        Some(args),
                        function_type_registry,
                    )?;

                    Ok(())
                }
            },

            CallType::Function { function_name, .. } => {
                let resource_constructor_registry_key =
                    RegistryKey::resource_constructor_registry_key(function_name);

                match resource_constructor_registry_key {
                    Some(resource_constructor_name) => handle_function_with_resource(
                        original_expr,
                        &resource_constructor_name,
                        function_name,
                        function_type_registry,
                        function_result_inferred_type,
                        args,
                    ),
                    None => {
                        let registry_key = RegistryKey::from_call_type(&cloned).ok_or(
                            FunctionCallError::InvalidFunctionCall {
                                function_name: function_name.to_string(),
                                expr: original_expr.clone(),
                                message: "unknown function".to_string(),
                            },
                        )?;

                        infer_args_and_result_type(
                            original_expr,
                            &FunctionDetails::Fqn(function_name.clone()),
                            function_type_registry,
                            &registry_key,
                            args,
                            Some(function_result_inferred_type),
                        )
                    }
                }
            }

            CallType::EnumConstructor(name) => {
                if args.is_empty() {
                    Ok(())
                } else {
                    Err(FunctionCallError::ArgumentSizeMisMatch {
                        function_name: name.to_string(),
                        expr: original_expr.clone(),
                        expected: 0,
                        provided: args.len(),
                    })
                }
            }

            CallType::VariantConstructor(variant_name) => {
                let registry_key = RegistryKey::FunctionName(variant_name.clone());
                infer_args_and_result_type(
                    original_expr,
                    &FunctionDetails::VariantName(variant_name.clone()),
                    function_type_registry,
                    &registry_key,
                    args,
                    Some(function_result_inferred_type),
                )
            }
        }
    }

    fn handle_function_with_resource(
        original_expr: &Expr,
        resource_constructor_registry_key: &RegistryKey,
        dynamic_parsed_function_name: &mut DynamicParsedFunctionName,
        function_type_registry: &FunctionTypeRegistry,
        function_result_inferred_type: &mut InferredType,
        resource_method_args: &mut [Expr],
    ) -> Result<(), FunctionCallError> {
        // Infer the resource constructors
        infer_resource_constructor_arguments(
            original_expr,
            resource_constructor_registry_key,
            dynamic_parsed_function_name.raw_resource_params_mut(),
            function_type_registry,
        )?;

        let resource_method_registry_key =
            RegistryKey::fqn_registry_key(dynamic_parsed_function_name);

        // Infer the resource arguments
        infer_resource_method_arguments(
            original_expr,
            &resource_method_registry_key,
            dynamic_parsed_function_name,
            function_type_registry,
            resource_method_args,
            function_result_inferred_type,
        )
    }

    fn infer_resource_method_arguments(
        original_expr: &Expr,
        resource_method_registry_key: &RegistryKey,
        dynamic_parsed_function_name: &mut DynamicParsedFunctionName,
        function_type_registry: &FunctionTypeRegistry,
        resource_method_args: &mut [Expr],
        function_result_inferred_type: &mut InferredType,
    ) -> Result<(), FunctionCallError> {
        // Infer the types of resource method parameters
        let resource_method_name_in_metadata =
            dynamic_parsed_function_name.function_name_with_prefix_identifiers();

        let resource_constructor_name = dynamic_parsed_function_name
            .resource_name_simplified()
            .unwrap_or_default();

        infer_args_and_result_type(
            original_expr,
            &FunctionDetails::ResourceMethodName {
                resource_name: resource_constructor_name,
                resource_method_name: resource_method_name_in_metadata,
            },
            function_type_registry,
            resource_method_registry_key,
            resource_method_args,
            Some(function_result_inferred_type),
        )
    }

    fn infer_resource_constructor_arguments(
        original_expr: &Expr,
        resource_constructor_registry_key: &RegistryKey,
        raw_resource_parameters: Option<&mut [Expr]>,
        function_type_registry: &FunctionTypeRegistry,
    ) -> Result<(), FunctionCallError> {
        let mut constructor_params: &mut [Expr] = &mut [];

        if let Some(resource_params) = raw_resource_parameters {
            constructor_params = resource_params
        }

        // Infer the types of constructor parameter expressions
        infer_args_and_result_type(
            original_expr,
            &FunctionDetails::ResourceConstructorName {
                resource_constructor_name: resource_constructor_registry_key.get_function_name(),
            },
            function_type_registry,
            resource_constructor_registry_key,
            constructor_params,
            None,
        )
    }

    fn infer_args_and_result_type(
        original_expr: &Expr,
        function_name: &FunctionDetails,
        function_type_registry: &FunctionTypeRegistry,
        key: &RegistryKey,
        args: &mut [Expr],
        function_result_inferred_type: Option<&mut InferredType>,
    ) -> Result<(), FunctionCallError> {
        if let Some(value) = function_type_registry.types.get(key) {
            match value {
                RegistryValue::Value(_) => Ok(()),
                RegistryValue::Variant {
                    parameter_types,
                    variant_type,
                } => {
                    let parameter_types = parameter_types.clone();

                    if parameter_types.len() == args.len() {
                        tag_argument_types(original_expr, function_name, args, &parameter_types)?;

                        if let Some(function_result_type) = function_result_inferred_type {
                            *function_result_type = InferredType::from_type_variant(variant_type);
                        }

                        Ok(())
                    } else {
                        Err(FunctionCallError::ArgumentSizeMisMatch {
                            function_name: function_name.name(),
                            expr: original_expr.clone(),
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
                        tag_argument_types(original_expr, function_name, args, &parameter_types)?;

                        if let Some(function_result_type) = function_result_inferred_type {
                            *function_result_type = {
                                if return_types.len() == 1 {
                                    return_types.first().unwrap().into()
                                } else {
                                    InferredType::sequence(
                                        return_types.iter().map(|t| t.into()).collect(),
                                    )
                                }
                            }
                        };

                        Ok(())
                    } else {
                        Err(FunctionCallError::ArgumentSizeMisMatch {
                            function_name: function_name.name(),
                            expr: original_expr.clone(),
                            expected: parameter_types.len(),
                            provided: args.len(),
                        })
                    }
                }
            }
        } else {
            Err(FunctionCallError::InvalidFunctionCall {
                function_name: function_name.to_string(),
                expr: original_expr.clone(),
                message: "unknown function".to_string(),
            })
        }
    }

    #[derive(Clone)]
    enum FunctionDetails {
        ResourceConstructorName {
            resource_constructor_name: String,
        },
        ResourceMethodName {
            resource_name: String,
            resource_method_name: String,
        },
        Fqn(DynamicParsedFunctionName),
        VariantName(String),
    }

    impl FunctionDetails {
        pub fn name(&self) -> String {
            match self {
                FunctionDetails::ResourceConstructorName {
                    resource_constructor_name,
                } => resource_constructor_name.replace("[constructor]", ""),
                FunctionDetails::ResourceMethodName {
                    resource_name,
                    resource_method_name,
                } => {
                    let resource_constructor_prefix = format!("[method]{}.", resource_name);
                    resource_method_name.replace(&resource_constructor_prefix, "")
                }
                FunctionDetails::Fqn(fqn) => fqn.function.name_pretty(),
                FunctionDetails::VariantName(name) => name.clone(),
            }
        }
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
        function_call_expr: &Expr,
        function_name: &FunctionDetails,
        expected: &AnalysedType,
        provided: &Expr,
    ) -> Result<(), FunctionCallError> {
        let is_valid = if provided.inferred_type().is_unknown() {
            true
        } else {
            provided.inferred_type().get_type_hint().get_type_kind()
                == expected.get_type_hint().get_type_kind()
        };

        if is_valid {
            Ok(())
        } else {
            Err(FunctionCallError::TypeMisMatch {
                function_name: function_name.name(),
                argument: provided.clone(),
                error: TypeMismatchError {
                    expr_with_wrong_type: provided.clone(),
                    parent_expr: Some(function_call_expr.clone()),
                    expected_type: ExpectedType::AnalysedType(expected.clone()),
                    actual_type: ActualType::Inferred(provided.inferred_type().clone()),
                    field_path: Default::default(),
                    additional_error_detail: vec![],
                },
            })
        }
    }

    fn tag_argument_types(
        function_call_expr: &Expr,
        function_name: &FunctionDetails,
        args: &mut [Expr],
        parameter_types: &[AnalysedType],
    ) -> Result<(), FunctionCallError> {
        for (arg, param_type) in args.iter_mut().zip(parameter_types) {
            check_function_arguments(function_call_expr, function_name, param_type, arg)?;
            arg.add_infer_type_mut(
                InferredType::from(param_type).add_origin(TypeOrigin::Declared(arg.source_span())),
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod function_parameters_inference_tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::function_name::{DynamicParsedFunctionName, DynamicParsedFunctionReference};
    use crate::rib_source_span::SourceSpan;
    use crate::type_registry::FunctionTypeRegistry;
    use crate::{Expr, InferredType, ParsedFunctionSite};
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
        expr.infer_function_call_types(&function_type_registry)
            .unwrap();

        let let_binding = Expr::let_binding("x", Expr::number(BigDecimal::from(1)), None);

        let call_expr = Expr::call_worker_function(
            DynamicParsedFunctionName {
                site: ParsedFunctionSite::Global,
                function: DynamicParsedFunctionReference::Function {
                    function: "foo".to_string(),
                },
            },
            None,
            None,
            vec![Expr::identifier_global("x", None).with_inferred_type(InferredType::u64())],
        )
        .with_inferred_type(InferredType::sequence(vec![]));

        let expected = Expr::ExprBlock {
            exprs: vec![let_binding, call_expr],
            inferred_type: InferredType::unknown(),
            source_span: SourceSpan::default(),
            type_annotation: None,
        };

        assert_eq!(expr, expected);
    }
}
