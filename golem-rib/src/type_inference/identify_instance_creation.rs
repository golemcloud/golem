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

use crate::rib_type_error::RibTypeError;
use crate::{Expr, FunctionTypeRegistry};

// Handling the following and making sure the types are inferred fully at this stage.
// The expr `Call` will still be expr `Call` itself but CallType will be worker instance creation
// or resource creation
// instance;
// instance[foo]
// instance("worker-name")
// instance[foo]("worker-name")
pub fn identify_instance_creation(
    expr: &mut Expr,
    function_type_registry: &FunctionTypeRegistry,
) -> Result<(), RibTypeError> {
    internal::search_for_invalid_instance_declarations(expr)?;
    internal::identify_instance_creation_with_worker(expr, function_type_registry)
}

mod internal {
    use crate::call_type::{CallType, InstanceCreationType};
    use crate::instance_type::InstanceType;
    use crate::rib_type_error::RibTypeError;
    use crate::type_parameter::TypeParameter;
    use crate::type_registry::FunctionTypeRegistry;
    use crate::{
        CustomError, Expr, ExprVisitor, FunctionCallError, InferredType, ParsedFunctionReference,
        TypeInternal, TypeOrigin,
    };

    pub(crate) fn search_for_invalid_instance_declarations(
        expr: &mut Expr,
    ) -> Result<(), RibTypeError> {
        let mut visitor = ExprVisitor::bottom_up(expr);

        while let Some(expr) = visitor.pop_front() {
            match expr {
                Expr::Let {
                    variable_id, expr, ..
                } => {
                    if variable_id.name() == "instance" {
                        return Err(CustomError::new(
                            expr,
                            "`instance` is a reserved keyword and cannot be used as a variable.",
                        )
                        .into());
                    }
                }
                Expr::Identifier { variable_id, .. } => {
                    if variable_id.name() == "instance" && variable_id.is_global() {
                        let err = CustomError::new(
                            expr,
                             "`instance` is a reserved keyword"
                        ).with_help_message(
                            "use `instance()` instead of `instance` to create an ephemeral worker instance."
                        ).with_help_message(
                            "for a durable worker, use `instance(\"foo\")` where `\"foo\"` is the worker name"
                        );

                        return Err(err.into());
                    }
                }

                _ => {}
            }
        }

        Ok(())
    }

    // Identifying instance creations out of all parsed function calls.
    // Note that before any global variable related inference stages,
    // this has to go in first to disambiguate global variables with instance creations
    pub(crate) fn identify_instance_creation_with_worker(
        expr: &mut Expr,
        function_type_registry: &FunctionTypeRegistry,
    ) -> Result<(), RibTypeError> {
        let mut visitor = ExprVisitor::bottom_up(expr);

        while let Some(expr) = visitor.pop_back() {
            if let Expr::Call {
                call_type,
                generic_type_parameter,
                args,
                inferred_type,
                source_span,
                type_annotation,
            } = expr
            {
                let type_parameter = generic_type_parameter
                    .as_ref()
                    .map(|gtp| {
                        TypeParameter::from_text(&gtp.value).map_err(|err| {
                            FunctionCallError::invalid_generic_type_parameter(&gtp.value, err)
                        })
                    })
                    .transpose()?;

                let instance_creation_type = get_instance_creation_details(call_type, args);

                if let Some(instance_creation_details) = instance_creation_type {
                    let worker_name = instance_creation_details.worker_name().cloned();

                    *call_type = CallType::InstanceCreation(instance_creation_details);

                    let new_instance_type = InstanceType::from(
                        function_type_registry,
                        worker_name.as_ref(),
                        type_parameter,
                    )
                    .map_err(|err| {
                        RibTypeError::from(CustomError::new(
                            &Expr::Call {
                                call_type: call_type.clone(),
                                generic_type_parameter: generic_type_parameter.clone(),
                                args: args.clone(),
                                inferred_type: InferredType::unknown(),
                                source_span: source_span.clone(),
                                type_annotation: type_annotation.clone(),
                            },
                            format!("failed to create instance: {}", err),
                        ))
                    })?;

                    *inferred_type = InferredType::new(
                        TypeInternal::Instance {
                            instance_type: Box::new(new_instance_type),
                        },
                        TypeOrigin::NoOrigin,
                    );
                }
            }
        }

        Ok(())
    }

    fn get_instance_creation_details(
        call_type: &CallType,
        args: &[Expr],
    ) -> Option<InstanceCreationType> {
        match call_type {
            CallType::Function { function_name, .. } => {
                let function_name = function_name.to_parsed_function_name().function;
                match function_name {
                    ParsedFunctionReference::Function { function } if function == "instance" => {
                        let optional_worker_name_expression = args.first();
                        Some(InstanceCreationType::Worker {
                            worker_name: optional_worker_name_expression
                                .map(|x| Box::new(x.clone())),
                        })
                    }

                    _ => None,
                }
            }
            CallType::InstanceCreation(instance_creation_type) => {
                Some(instance_creation_type.clone())
            }
            CallType::VariantConstructor(_) => None,
            CallType::EnumConstructor(_) => None,
        }
    }
}
