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
) -> Result<(), String> {
    internal::search_for_invalid_instance_declarations(expr)?;
    internal::identify_instance_creation_with_worker(expr, function_type_registry)
}

mod internal {
    use crate::call_type::{CallType, InstanceCreationType};
    use crate::instance_type::InstanceType;
    use crate::type_parameter::TypeParameter;
    use crate::type_registry::FunctionTypeRegistry;
    use crate::{Expr, InferredType, ParsedFunctionReference};
    use std::collections::VecDeque;

    pub(crate) fn search_for_invalid_instance_declarations(expr: &mut Expr) -> Result<(), String> {
        let mut queue = VecDeque::new();
        queue.push_front(expr);
        while let Some(expr) = queue.pop_front() {
            match expr {
                Expr::Let(variable_id, _, expr, _) => {
                    queue.push_front(expr);

                    if variable_id.name() == "instance" {
                        return Err(
                            "`instance` is a reserved keyword and cannot be used as a variable."
                                .to_string(),
                        );
                    }
                }
                Expr::Identifier(variable_id, _, _) => {
                    if variable_id.name() == "instance" && variable_id.is_global() {
                        return Err(concat!(
                        "`instance` is a reserved keyword.\n ",
                        "note: Use `instance()` instead of `instance` to create an ephemeral worker instance.\n ",
                        "note: For a durable worker, use `instance(\"foo\")` where `\"foo\"` is the worker name"
                        ).to_string());
                    }
                }

                _ => expr.visit_children_mut_top_down(&mut queue),
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
    ) -> Result<(), String> {
        let mut queue = VecDeque::new();
        queue.push_back(expr);
        while let Some(expr) = queue.pop_back() {
            match expr {
                Expr::Call(call_type, generic_type_parameter, args, inferred_type) => {
                    let type_parameter = generic_type_parameter
                        .clone()
                        .map(|type_parameter| TypeParameter::from_str(&type_parameter.value))
                        .transpose()?;

                    let instance_creation_details =
                        get_instance_creation_details(call_type, args.clone());
                    // We change the call_type to instance creation which hardly does anything during interpretation
                    if let Some(instance_creation_details) = instance_creation_details {
                        *call_type = CallType::InstanceCreation(instance_creation_details.clone());
                        let new_instance_type = InstanceType::from(
                            function_type_registry.clone(),
                            instance_creation_details.worker_name(),
                            type_parameter,
                        )?;
                        *inferred_type = InferredType::Instance {
                            instance_type: Box::new(new_instance_type),
                        }
                    }
                }

                _ => expr.visit_children_mut_bottom_up(&mut queue),
            }
        }

        Ok(())
    }

    fn get_instance_creation_details(
        call_type: &CallType,
        args: Vec<Expr>,
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
            CallType::VariantConstructor(_) => None,
            CallType::EnumConstructor(_) => None,
            CallType::InstanceCreation(instance_creation_type) => {
                Some(instance_creation_type.clone())
            }
        }
    }
}
