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

use crate::rib_type_error::RibTypeErrorInternal;
use crate::{ComponentDependencies, Expr};

// Handling the following and making sure the types are inferred fully at this stage.
// The expr `Call` will still be expr `Call` itself but CallType will be worker instance creation
// or resource creation
// instance;
// instance[foo]
// instance("worker-name")
// instance[foo]("worker-name")
pub fn identify_instance_creation(
    expr: &mut Expr,
    component_dependencies: &ComponentDependencies,
) -> Result<(), RibTypeErrorInternal> {
    internal::search_for_invalid_instance_declarations(expr)?;
    internal::identify_instance_creation_with_worker(expr, component_dependencies)
}

mod internal {
    use crate::call_type::{CallType, InstanceCreationType};
    use crate::instance_type::InstanceType;
    use crate::rib_type_error::RibTypeErrorInternal;
    use crate::type_parameter::TypeParameter;
    use crate::{
        ComponentDependencies, CustomError, Expr, ExprVisitor, FunctionCallError, InferredType,
        ParsedFunctionReference, TypeInternal, TypeOrigin,
    };

    pub(crate) fn search_for_invalid_instance_declarations(
        expr: &mut Expr,
    ) -> Result<(), RibTypeErrorInternal> {
        let mut visitor = ExprVisitor::bottom_up(expr);

        while let Some(expr) = visitor.pop_front() {
            match expr {
                Expr::Let {
                    variable_id, expr, ..
                } => {
                    if variable_id.name() == "instance" {
                        return Err(CustomError::new(
                            expr.source_span(),
                            "`instance` is a reserved keyword and cannot be used as a variable.",
                        )
                        .into());
                    }
                }
                Expr::Identifier { variable_id, .. } => {
                    if variable_id.name() == "instance" && variable_id.is_global() {
                        let err = CustomError::new(
                            expr.source_span(),
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
        component_dependency: &ComponentDependencies,
    ) -> Result<(), RibTypeErrorInternal> {
        let mut visitor = ExprVisitor::bottom_up(expr);

        while let Some(expr) = visitor.pop_back() {
            if let Expr::Call {
                call_type,
                generic_type_parameter,
                args,
                inferred_type,
                source_span,
                ..
            } = expr
            {
                let type_parameter = generic_type_parameter
                    .as_ref()
                    .map(|gtp| {
                        TypeParameter::from_text(&gtp.value).map_err(|err| {
                            FunctionCallError::invalid_generic_type_parameter(
                                &gtp.value,
                                err,
                                source_span.clone(),
                            )
                        })
                    })
                    .transpose()?;

                let instance_creation_type = get_instance_creation_details(
                    call_type,
                    type_parameter.clone(),
                    args,
                    component_dependency,
                )
                .map_err(|err| {
                    RibTypeErrorInternal::from(CustomError::new(
                        source_span.clone(),
                        format!("failed to get instance creation details: {err}"),
                    ))
                })?;

                if let Some(instance_creation_type) = instance_creation_type {
                    let worker_name = instance_creation_type.worker_name();

                    *call_type = CallType::InstanceCreation(instance_creation_type);

                    let new_instance_type = InstanceType::from(
                        component_dependency,
                        worker_name.as_ref(),
                        type_parameter,
                    )
                    .map_err(|err| {
                        RibTypeErrorInternal::from(CustomError::new(
                            source_span.clone(),
                            format!("failed to create instance: {err}"),
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
        type_parameter: Option<TypeParameter>,
        args: &[Expr],
        component_dependency: &ComponentDependencies,
    ) -> Result<Option<InstanceCreationType>, String> {
        match call_type {
            CallType::Function { function_name, .. } => {
                let function_name = function_name.to_parsed_function_name().function;
                match function_name {
                    ParsedFunctionReference::Function { function } if function == "instance" => {
                        let optional_worker_name_expression = args.first();

                        let instance_creation = component_dependency.get_worker_instance_type(
                            type_parameter,
                            optional_worker_name_expression.cloned(),
                        )?;

                        Ok(Some(instance_creation))
                    }

                    _ => Ok(None),
                }
            }
            CallType::InstanceCreation(instance_creation_type) => {
                Ok(Some(instance_creation_type.clone()))
            }
            CallType::VariantConstructor(_) => Ok(None),
            CallType::EnumConstructor(_) => Ok(None),
        }
    }
}
