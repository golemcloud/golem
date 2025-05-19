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

use golem_wasm_ast::analysis::AnalysedType;
use rib::{RegistryKey, WorkerFunctionType, WorkerFunctionsInRib};
use std::collections::HashMap;

// This is very similar to WorkerFunctionsInRib data structure in `rib`, however
// it adds more info that is specific to other golem services,
// such as the total number of usages for each function in that component.
// This forms the core of component constraints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionConstraints {
    pub constraints: Vec<FunctionUsageConstraint>,
}

impl From<FunctionConstraints> for WorkerFunctionsInRib {
    fn from(value: FunctionConstraints) -> Self {
        WorkerFunctionsInRib {
            function_calls: value
                .constraints
                .iter()
                .map(|function_constraint| {
                    rib::WorkerFunctionType::from(function_constraint.clone())
                })
                .collect(),
        }
    }
}

impl FunctionConstraints {
    pub fn from_worker_functions_in_rib(
        worker_functions_in_rib: &WorkerFunctionsInRib,
    ) -> FunctionConstraints {
        let functions = worker_functions_in_rib
            .function_calls
            .iter()
            .map(FunctionUsageConstraint::from_worker_function_type)
            .collect::<Vec<_>>();

        FunctionConstraints {
            constraints: functions,
        }
    }

    pub fn remove_constraints(&self, constraints_to_remove: &[FunctionSignature]) -> Option<Self> {
        let mut constraints = vec![];

        for constraint in &self.constraints {
            if constraints_to_remove.contains(&constraint.function_signature) {
                let mut constraint = constraint.clone();
                constraint.decrement_usage_count();

                if constraint.usage_count > 0 {
                    constraints.push(constraint);
                }
            }
        }

        if self.constraints.is_empty() {
            None
        } else {
            Some(FunctionConstraints { constraints })
        }
    }

    pub fn try_merge(
        worker_functions: Vec<FunctionConstraints>,
    ) -> Result<FunctionConstraints, String> {
        let mut merged_function_calls: HashMap<RegistryKey, FunctionUsageConstraint> =
            HashMap::new();

        for wf in worker_functions {
            for constraint_usage in wf.constraints {
                match merged_function_calls.get_mut(constraint_usage.function_key()) {
                    Some(existing_constraint) => {
                        // Check for parameter type conflicts
                        if existing_constraint.parameter_types()
                            != constraint_usage.parameter_types()
                        {
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
                                constraint_usage.function_key(),
                                existing_constraint.return_types(),
                                constraint_usage.return_types()
                            ));
                        }

                        // Update usage_count instead of overwriting
                        existing_constraint.usage_count = existing_constraint
                            .usage_count
                            .saturating_add(constraint_usage.usage_count);
                    }
                    None => {
                        // get-cart-contents -> 1
                        merged_function_calls
                            .insert(constraint_usage.function_key().clone(), constraint_usage);
                    }
                }
            }
        }

        let mut merged_function_calls_vec: Vec<FunctionUsageConstraint> =
            merged_function_calls.into_values().collect();

        merged_function_calls_vec.sort_by(|a, b| a.function_key().cmp(b.function_key()));

        Ok(FunctionConstraints {
            constraints: merged_function_calls_vec,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSignature {
    function_key: RegistryKey,
    parameter_types: Vec<AnalysedType>,
    return_types: Vec<AnalysedType>,
}

impl FunctionSignature {
    pub fn new(
        function_key: RegistryKey,
        parameter_types: Vec<AnalysedType>,
        return_types: Vec<AnalysedType>,
    ) -> Self {
        FunctionSignature {
            function_key,
            parameter_types,
            return_types,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionUsageConstraint {
    pub function_signature: FunctionSignature,
    pub usage_count: u32,
}

// The worker-functions in a rib script that's deployed in a host
// becomes a function-usage-constraint to component-service.
impl From<FunctionUsageConstraint> for WorkerFunctionType {
    fn from(value: FunctionUsageConstraint) -> Self {
        WorkerFunctionType {
            function_key: value.function_signature.function_key.clone(),
            parameter_types: value.function_signature.parameter_types.clone(),
            return_types: value.function_signature.return_types.clone(),
        }
    }
}

impl FunctionUsageConstraint {
    pub fn function_key(&self) -> &RegistryKey {
        &self.function_signature.function_key
    }

    pub fn parameter_types(&self) -> &Vec<AnalysedType> {
        &self.function_signature.parameter_types
    }

    pub fn return_types(&self) -> &Vec<AnalysedType> {
        &self.function_signature.return_types
    }

    pub fn from_worker_function_type(
        worker_function_type: &WorkerFunctionType,
    ) -> FunctionUsageConstraint {
        FunctionUsageConstraint {
            function_signature: FunctionSignature {
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
    use crate::model::component_constraint::{
        FunctionConstraints, FunctionSignature, FunctionUsageConstraint,
    };
    use golem_api_grpc::proto::golem::component::FunctionConstraint as FunctionConstraintProto;
    use golem_api_grpc::proto::golem::component::FunctionConstraintCollection as FunctionConstraintCollectionProto;
    use golem_wasm_ast::analysis::AnalysedType;
    use rib::RegistryKey;

    impl TryFrom<golem_api_grpc::proto::golem::component::FunctionConstraintCollection>
        for FunctionConstraints
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::component::FunctionConstraintCollection,
        ) -> Result<Self, Self::Error> {
            let collection = FunctionConstraints {
                constraints: value
                    .constraints
                    .iter()
                    .map(|constraint_proto| {
                        FunctionUsageConstraint::try_from(constraint_proto.clone())
                    })
                    .collect::<Result<_, _>>()?,
            };

            Ok(collection)
        }
    }

    impl From<FunctionConstraints> for FunctionConstraintCollectionProto {
        fn from(value: FunctionConstraints) -> Self {
            FunctionConstraintCollectionProto {
                constraints: value
                    .constraints
                    .iter()
                    .map(|function_constraint| {
                        FunctionConstraintProto::from(function_constraint.clone())
                    })
                    .collect(),
            }
        }
    }

    impl TryFrom<FunctionConstraintProto> for FunctionUsageConstraint {
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
                function_signature: FunctionSignature {
                    function_key,
                    parameter_types,
                    return_types,
                },
                usage_count,
            })
        }
    }

    impl From<FunctionUsageConstraint> for FunctionConstraintProto {
        fn from(value: FunctionUsageConstraint) -> Self {
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
