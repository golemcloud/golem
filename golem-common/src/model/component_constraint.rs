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

use golem_wasm_ast::analysis::AnalysedType;
use rib::{RegistryKey, WorkerFunctionType, WorkerFunctionsInRib};
use std::collections::HashMap;
use itertools::Itertools;

// This is very similar to WorkerFunctionsInRib data structure in `rib`, however
// it adds more info that is specific to other golem services,
// such as the total number of usages for each function in that component.
// This forms the core of component constraints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionConstraintCollection {
    pub function_constraints: Vec<FunctionConstraintUsage>,
}

impl From<FunctionConstraintCollection> for WorkerFunctionsInRib {
    fn from(value: FunctionConstraintCollection) -> Self {
        WorkerFunctionsInRib {
            function_calls: value
                .function_constraints
                .iter()
                .map(|function_constraint| {
                    rib::WorkerFunctionType::from(function_constraint.clone())
                })
                .collect(),
        }
    }
}

impl FunctionConstraintCollection {
    pub fn from_worker_functions_in_rib(
        worker_functions_in_rib: &WorkerFunctionsInRib,
    ) -> FunctionConstraintCollection {
        let functions = worker_functions_in_rib
            .function_calls
            .iter()
            .map(FunctionConstraintUsage::from_worker_function_type)
            .collect::<Vec<_>>();

        FunctionConstraintCollection {
            function_constraints: functions,
        }
    }

    pub fn remove_constraints(&self, constraints_to_remove: &Vec<FunctionConstraint>) {
        let mut constraints = vec![];

      for constraint in self.function_constraints {
          if constraints_to_remove.contains(&constraint.constraint) {
              let mut constraint = constraint;
              constraint.decrement_usage_count();
              constraints.push(constraint);
          }
      }

    }

    pub fn try_merge(
        worker_functions: Vec<FunctionConstraintCollection>,
    ) -> Result<FunctionConstraintCollection, String> {
        let mut merged_function_calls: HashMap<RegistryKey, FunctionConstraintUsage> = HashMap::new();

        for wf in worker_functions {
            for constraint_usage in wf.function_constraints {
                match merged_function_calls.get_mut(constraint_usage.function_key()) {
                    Some(existing_constraint) => {
                        // Check for parameter type conflicts
                        if existing_constraint.parameter_types() != constraint_usage.parameter_types() {
                            return Err(format!(
                                "Parameter type conflict for function key {:?}: {:?} vs {:?}",
                                constraint_usage.function_key(),
                                existing_constraint.parameter_types(),
                                constraint_usage.parameter_types()
                            ));
                        }

                        // Check for return type conflicts
                        if existing_constraint.return_types() != constraint_usage.return_types() {
                            return Err(format!(
                                "Return type conflict for function key {:?}: {:?} vs {:?}",
                                constraint_usage.function_key(), existing_constraint.return_types(), constraint_usage.return_types()
                            ));
                        }

                        // Update usage_count instead of overwriting
                        existing_constraint.usage_count =
                            existing_constraint.usage_count.saturating_add(constraint_usage.usage_count);
                    }
                    None => {
                        // Insert if no conflict is found
                        merged_function_calls.insert(constraint_usage.function_key().clone(), constraint_usage);
                    }
                }
            }
        }

        let mut merged_function_calls_vec: Vec<FunctionConstraintUsage> =
            merged_function_calls.into_values().collect();

        merged_function_calls_vec.sort_by(|a, b| a.function_key().cmp(b.function_key()));

        Ok(FunctionConstraintCollection {
            function_constraints: merged_function_calls_vec,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionConstraint {
    pub function_key: RegistryKey,
    pub parameter_types: Vec<AnalysedType>,
    pub return_types: Vec<AnalysedType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionConstraintUsage {
    pub constraint: FunctionConstraint,
    pub usage_count: u32,
}

impl From<FunctionConstraintUsage> for WorkerFunctionType {
    fn from(value: FunctionConstraintUsage) -> Self {
        WorkerFunctionType {
            function_key: value.constraint.function_key.clone(),
            parameter_types: value.constraint.parameter_types.clone(),
            return_types: value.constraint.return_types.clone(),
        }
    }
}

impl FunctionConstraintUsage {

    pub fn function_key(&self) -> &RegistryKey {
        &self.constraint.function_key
    }

    pub fn parameter_types(&self) -> &Vec<AnalysedType> {
        &self.constraint.parameter_types
    }

    pub fn return_types(&self) -> &Vec<AnalysedType> {
        &self.constraint.return_types
    }


    pub fn from_worker_function_type(
        worker_function_type: &WorkerFunctionType,
    ) -> FunctionConstraintUsage {
        FunctionConstraintUsage {
            constraint: FunctionConstraint {
                function_key: worker_function_type.function_key.clone(),
                parameter_types: worker_function_type.parameter_types.clone(),
                return_types: worker_function_type.return_types.clone(),
            },
            usage_count: 1,
        }
    }

    fn decrement_usage_count(&mut self) {
        self.usage_count -= 1;
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::model::component_constraint::{FunctionConstraintUsage, FunctionConstraintCollection, FunctionConstraint};
    use golem_api_grpc::proto::golem::component::FunctionConstraint as FunctionConstraintProto;
    use golem_api_grpc::proto::golem::component::FunctionConstraintCollection as FunctionConstraintCollectionProto;
    use golem_wasm_ast::analysis::AnalysedType;
    use rib::RegistryKey;

    impl TryFrom<golem_api_grpc::proto::golem::component::FunctionConstraintCollection>
        for FunctionConstraintCollection
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::FunctionConstraintCollection,
        ) -> Result<Self, Self::Error> {
            let collection = FunctionConstraintCollection {
                function_constraints: value
                    .constraints
                    .iter()
                    .map(|constraint_proto| FunctionConstraintUsage::try_from(constraint_proto.clone()))
                    .collect::<Result<_, _>>()?,
            };

            Ok(collection)
        }
    }

    impl From<FunctionConstraintCollection> for FunctionConstraintCollectionProto {
        fn from(value: FunctionConstraintCollection) -> Self {
            FunctionConstraintCollectionProto {
                constraints: value
                    .function_constraints
                    .iter()
                    .map(|function_constraint| {
                        FunctionConstraintProto::from(function_constraint.clone())
                    })
                    .collect(),
            }
        }
    }

    impl TryFrom<FunctionConstraintProto> for FunctionConstraintUsage {
        type Error = String;

        fn try_from(value: FunctionConstraintProto) -> Result<Self, Self::Error> {
            let return_types = value
                .return_types
                .iter()
                .map(AnalysedType::try_from)
                .collect::<Result<_, _>>()?;

            let parameter_types = value
                .parameter_types
                .iter()
                .map(AnalysedType::try_from)
                .collect::<Result<_, _>>()?;

            let registry_key_proto = value.function_key.ok_or("Function key missing")?;
            let function_key = RegistryKey::try_from(registry_key_proto)?;
            let usage_count = value.usage_count;

            Ok(Self {
                constraint: FunctionConstraint {
                    function_key,
                    parameter_types,
                    return_types,
                },
                usage_count,
            })
        }
    }

    impl From<FunctionConstraintUsage> for FunctionConstraintProto {
        fn from(value: FunctionConstraintUsage) -> Self {
            let registry_key = value.function_key().into();

            FunctionConstraintProto {
                function_key: Some(registry_key),
                parameter_types: value
                    .parameter_types()
                    .iter()
                    .map(|analysed_type| analysed_type.into())
                    .collect(),
                return_types: value
                    .return_types()
                    .iter()
                    .map(|analysed_type| analysed_type.into())
                    .collect(),
                usage_count: value.usage_count,
            }
        }
    }
}
