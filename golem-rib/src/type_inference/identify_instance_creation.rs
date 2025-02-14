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

use crate::call_type::CallType;
use crate::instance_type::InstanceType;
use crate::type_registry::FunctionTypeRegistry;
use crate::{DynamicParsedFunctionName, Expr, InferredType};
use std::collections::VecDeque;

// Identifying instance creations out of all parsed function calls.
// Note that before any global variable related inference stages,
// this has to go in first to disambiguate global variables with instance creations
pub fn identify_instance_creation(
    expr: &mut Expr,
    function_type_registry: &FunctionTypeRegistry,
) -> Result<(), String> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);
    while let Some(expr) = queue.pop_back() {
        match expr {
            // We discard the generic parameter when identifying instance creation as we think the context of Rib doesn't deal with packages across components to identify which component, as of now
            // In a component metadata, all we infer is the list of functions and it can be a mix of different package names and interfaces. Example:
            // Exports:
            //   app:component-b-exports/app-component-b-api.{add}(value: u64) // Function that's part of the main package app:component-b-exports (which in actual WIT is app:component-b) and interface called api
            //   app:component-b-exports/app-component-b-api.{get}() -> u64 // Function that's part of the main package app:component-b-exports (which in actual WIT is app:component-b) and interface called api
            //   wasi:clocks/monotonic-clock@0.2.0.{now}() -> u64 // Function from a different package-interface
            //   wasi:clocks/monotonic-clock@0.2.0.{resolution}() -> u64 // Function from a different package-interface
            //   wasi:clocks/monotonic-clock@0.2.0.{subscribe-instant}(when: u64) -> handle<0> // Function from a different package-interface
            //   wasi:clocks/monotonic-clock@0.2.0.{subscribe-duration}(when: u64) -> handle<0> // Function from a different package-interface
            //   app:component-b-exports/app-component-b-inline-functions.{run}() -> u64 // A top level function but part of a package and a generated interface
            Expr::Call(call_type, _, args, inferred_type) => {
                let instance_creation_details =
                    internal::get_instance_creation_details(call_type, args.clone());
                // We change the call_type to instance creation which hardly does anything during interpretation
                if let Some(instance_creation_details) = instance_creation_details {
                    *call_type = CallType::InstanceCreation(instance_creation_details.clone());
                    let new_instance_type = InstanceType::from(
                        instance_creation_details.component_id(),
                        function_type_registry.clone(),
                        instance_creation_details.worker_name(),
                    )?;
                    *inferred_type = InferredType::Instance {
                        instance_type: new_instance_type,
                    }
                }
            }

            // While `instance("worker-name")` will be parsed as a function call,
            // `instance` will be regarded as an identifier, while that itself should also be a function call.
            // Such that, all function calls of `instance` and `instance("worker-name")` will be inferred
            // as an InstanceType creation. In other words, the Rib parser is kept devoid of the knowledge
            // of the semantics of variables of "instance" or InstanceType creatio
            Expr::Identifier(variable_id, _, _) => {
                if variable_id.name() == "instance" {
                    *expr = Expr::call(DynamicParsedFunctionName::parse("instance")?, None, vec![]);
                }
            }

            _ => expr.visit_children_mut_bottom_up(&mut queue),
        }
    }

    Ok(())
}

mod internal {
    use crate::call_type::{CallType, InstanceCreationType};

    use crate::{Expr, ParsedFunctionReference};

    pub(crate) fn get_instance_creation_details(
        call_type: &CallType,
        args: Vec<Expr>,
    ) -> Option<InstanceCreationType> {
        match call_type {
            CallType::Function(function_name) => {
                let function_name = function_name.to_parsed_function_name().function;
                match function_name {
                    ParsedFunctionReference::Function { function } if function == "instance" => {
                        let optional_worker_name_expression = args.first();
                        match optional_worker_name_expression {
                            None => {
                                Some(InstanceCreationType::Ephemeral {
                                    component_id: "component_id_to_be_provided".to_string(), // TODO: This is a placeholder
                                })
                            }
                            Some(worker_name_expr) => {
                                Some(InstanceCreationType::Durable {
                                    worker_name: Box::new(worker_name_expr.clone()),
                                    component_id: "component_id_to_be_provided".to_string(), // TODO: This is a placeholder
                                })
                            }
                        }
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
