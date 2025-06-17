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

use crate::call_type::{CallType, InstanceCreationType, InstanceIdentifier};
use crate::rib_type_error::RibTypeError;
use crate::type_parameter::TypeParameter;
use crate::{
    DynamicParsedFunctionName, Expr, FunctionCallError, InferredType, TypeInternal, TypeName,
    TypeOrigin,
};
use crate::{FunctionName, InstanceType};
use std::collections::VecDeque;
use std::ops::Deref;

// This phase is responsible for identifying the worker function invocations
// such as `worker.foo("x, y, z")` or `cart-resource.add-item(..)` etc
// lazy method invocations are converted to actual Expr::Call
pub fn infer_worker_function_invokes(expr: &mut Expr) -> Result<(), RibTypeError> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        if let Expr::InvokeMethodLazy {
            lhs,
            method,
            generic_type_parameter,
            args,
            source_span,
            type_annotation,
            ..
        } = expr
        {
            let inferred_type = lhs.inferred_type();

            match inferred_type.internal_type() {
                TypeInternal::Instance { instance_type } => {
                    let type_parameter = generic_type_parameter
                        .as_ref()
                        .map(|gtp| {
                            TypeParameter::from_text(&gtp.value).map_err(|err| {
                                FunctionCallError::invalid_generic_type_parameter(&gtp.value, err)
                            })
                        })
                        .transpose()?;

                    // This can be made optional component info to improve type inference
                    // with multiple possibilities of functions but complicates quite a bit
                    let (component, function) = instance_type
                        .get_function(method, type_parameter)
                        .map_err(|err| {
                            FunctionCallError::invalid_function_call(
                                method,
                                &Expr::InvokeMethodLazy {
                                    lhs: lhs.clone(),
                                    method: method.clone(),
                                    generic_type_parameter: generic_type_parameter.clone(),
                                    args: args.clone(),
                                    source_span: source_span.clone(),
                                    type_annotation: type_annotation.clone(),
                                    inferred_type: inferred_type.clone(),
                                },
                                err,
                            )
                        })?;

                    match function.function_name {
                        // TODO; verify if this assumption is true
                        // that user never need to call a variant function from an instance
                        // If we need to support instance.variant-name(),
                        // this needs to be implemented
                        FunctionName::Variant(_) => {}
                        FunctionName::Enum(_) => {}

                        FunctionName::Function(function_name) => {
                            let dynamic_parsed_function_name = function_name.to_string();
                            let dynamic_parsed_function_name = DynamicParsedFunctionName::parse(
                                dynamic_parsed_function_name.as_str(),
                            )
                            .map_err(|err| {
                                FunctionCallError::invalid_function_call(
                                    &dynamic_parsed_function_name,
                                    &Expr::InvokeMethodLazy {
                                        lhs: lhs.clone(),
                                        method: method.clone(),
                                        generic_type_parameter: generic_type_parameter.clone(),
                                        args: args.clone(),
                                        source_span: source_span.clone(),
                                        type_annotation: type_annotation.clone(),
                                        inferred_type: inferred_type.clone(),
                                    },
                                    format!("Invalid function name: {}", err),
                                )
                            })?;

                            // let x = instance();
                            // x is now fo the type instance
                            // when we bump into add.
                            // the interpretation of add should lookup the variable-id  which is optional
                            // or it should lookup the instance type itself which is already part of
                            // the call_type. If there is no variable-id, then no reuse, otherwise
                            // do what's in the instance type
                            // This implies the CallType::Function should take a variable-id representing the instance
                            // such that it can lookup or it can use InstanceType
                            // x.add("item");
                            let module = get_module_identifier(instance_type, lhs);

                            let new_call = Expr::call_worker_function(
                                dynamic_parsed_function_name,
                                None,
                                Some(module),
                                args.clone(),
                                Some(component),
                            )
                            .with_source_span(source_span.clone());
                            *expr = new_call;
                        }
                        FunctionName::ResourceConstructor(fully_qualified_resource_constructor) => {
                            let resource_instance_type = instance_type.get_resource_instance_type(
                                fully_qualified_resource_constructor.clone(),
                                args.clone(),
                                instance_type.worker_name(),
                            );

                            let new_inferred_type = InferredType::new(
                                TypeInternal::Instance {
                                    instance_type: Box::new(resource_instance_type),
                                },
                                TypeOrigin::NoOrigin,
                            );

                            let module = get_module_identifier(instance_type, lhs);

                            let new_call_type =
                                CallType::InstanceCreation(InstanceCreationType::WitResource {
                                    component_info: Some(component.clone()),
                                    module: Some(module),
                                    resource_name: fully_qualified_resource_constructor.clone(),
                                });

                            *expr = Expr::call(new_call_type, None, args.clone())
                                .with_inferred_type(new_inferred_type)
                                .with_source_span(source_span.clone());
                        }
                        // If resource method is called, we could convert to strict call
                        // however it can only be possible if the instance type of LHS is
                        // a resource type
                        FunctionName::ResourceMethod(resource_method) => {
                            match instance_type.deref() {
                                InstanceType::Resource {
                                    resource_method_dictionary: resource_method_dict,
                                    resource_constructor,
                                    ..
                                } => {
                                    let resource_method = resource_method_dict
                                        .map
                                        .get(&component)
                                        .ok_or(FunctionCallError::invalid_function_call(
                                            resource_method.method_name(),
                                            &Expr::InvokeMethodLazy {
                                                lhs: lhs.clone(),
                                                method: method.clone(),
                                                generic_type_parameter: generic_type_parameter
                                                    .clone(),
                                                args: args.clone(),
                                                source_span: source_span.clone(),
                                                type_annotation: type_annotation.clone(),
                                                inferred_type: inferred_type.clone(),
                                            },
                                            format!(
                                                "Resource method {} not found in resource {}",
                                                resource_method.method_name(),
                                                resource_constructor
                                            ),
                                        ))?
                                        .iter()
                                        .find(|(k, _)| k == &resource_method)
                                        .map(|(k, _)| k.clone())
                                        .ok_or(FunctionCallError::invalid_function_call(
                                            resource_method.method_name(),
                                            &Expr::InvokeMethodLazy {
                                                lhs: lhs.clone(),
                                                method: method.clone(),
                                                generic_type_parameter: generic_type_parameter
                                                    .clone(),
                                                args: args.clone(),
                                                source_span: source_span.clone(),
                                                type_annotation: type_annotation.clone(),
                                                inferred_type: inferred_type.clone(),
                                            },
                                            format!(
                                                "Resource method {} not found in resource {}",
                                                resource_method.method_name(),
                                                resource_constructor
                                            ),
                                        ))?;

                                    let dynamic_parsed_function_name = resource_method
                                        .dynamic_parsed_function_name()
                                        .map_err(|err| {
                                            FunctionCallError::invalid_function_call(
                                                resource_method.method_name(),
                                                &Expr::InvokeMethodLazy {
                                                    lhs: lhs.clone(),
                                                    method: method.clone(),
                                                    generic_type_parameter: generic_type_parameter
                                                        .clone(),
                                                    args: args.clone(),
                                                    source_span: source_span.clone(),
                                                    type_annotation: type_annotation.clone(),
                                                    inferred_type: inferred_type.clone(),
                                                },
                                                format!("Invalid function name: {}", err),
                                            )
                                        })?;

                                    let module = get_module_identifier(instance_type, lhs);

                                    let new_call = Expr::call_worker_function(
                                        dynamic_parsed_function_name,
                                        None,
                                        Some(module),
                                        args.clone(),
                                        Some(component),
                                    )
                                    .with_source_span(source_span.clone());

                                    *expr = new_call
                                }

                                _ => {
                                    return Err(FunctionCallError::InvalidResourceMethodCall {
                                        resource_method_name: resource_method
                                            .method_name()
                                            .to_string(),
                                        invalid_lhs: *lhs.deref().clone(),
                                    }
                                    .into());
                                }
                            }
                        }
                    }
                }
                // This implies, none of the phase identified `lhs` to be an instance-type yet.
                // Re-running (fix point) the same phase will help identify the instance type of `lhs`.
                // Hence, this phase is part of computing the fix-point of compiler type inference.
                TypeInternal::Unknown => {}
                _ => {
                    return Err(FunctionCallError::invalid_function_call(
                        method,
                        &Expr::InvokeMethodLazy {
                            lhs: lhs.clone(),
                            method: method.clone(),
                            generic_type_parameter: generic_type_parameter.clone(),
                            args: args.clone(),
                            source_span: source_span.clone(),
                            type_annotation: type_annotation.clone(),
                            inferred_type: inferred_type.clone(),
                        },
                        format!(
                            "invalid worker function invoke. Expected to be an instance type, found {}",
                            TypeName::try_from(inferred_type)
                                .map(|x| x.to_string())
                                .unwrap_or("Unknown".to_string())
                        )
                    ).into());
                }
            }
        }

        expr.visit_expr_nodes_lazy(&mut queue);
    }

    Ok(())
}

fn get_module_identifier(instance_type: &InstanceType, lhs: &Expr) -> InstanceIdentifier {
    let variable_id = match lhs {
        Expr::Identifier { variable_id, .. } => Some(variable_id),
        _ => None,
    };

    match instance_type {
        InstanceType::Resource {
            worker_name,
            resource_constructor,
            ..
        } => InstanceIdentifier::WitResource {
            variable_id: variable_id.cloned(),
            worker_name: worker_name.clone(),
            resource_name: resource_constructor.clone(),
        },
        instance_type => InstanceIdentifier::WitWorker {
            variable_id: variable_id.cloned(),
            worker_name: instance_type.worker_name(),
        },
    }
}
