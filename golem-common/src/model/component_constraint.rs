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

use super::ComponentId;
use golem_wasm::analysis::AnalysedType;
use rib::{FunctionName, WorkerFunctionType, WorkerFunctionsInRib};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentConstraints {
    pub component_id: ComponentId,
    pub constraints: FunctionConstraints,
}

impl ComponentConstraints {
    pub fn function_signatures(&self) -> Vec<FunctionSignature> {
        let constraints = &self.constraints;

        constraints
            .constraints
            .iter()
            .map(|x| x.function_signature.clone())
            .collect()
    }
}

impl ComponentConstraints {
    pub fn init(
        component_id: &ComponentId,
        worker_functions_in_rib: WorkerFunctionsInRib,
    ) -> ComponentConstraints {
        ComponentConstraints {
            component_id: *component_id,
            constraints: FunctionConstraints {
                constraints: worker_functions_in_rib
                    .function_calls
                    .iter()
                    .map(FunctionUsageConstraint::from_worker_function_type)
                    .collect(),
            },
        }
    }
}

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
        let mut merged_function_calls: HashMap<FunctionName, FunctionUsageConstraint> =
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
                        if existing_constraint.return_type() != constraint_usage.return_type() {
                            return Err(format!(
                                "Return type conflict for function key {:?}: {:?} vs {:?}",
                                constraint_usage.function_key(),
                                existing_constraint.return_type(),
                                constraint_usage.return_type()
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
    function_name: FunctionName,
    parameter_types: Vec<AnalysedType>,
    return_type: Option<AnalysedType>,
}

impl FunctionSignature {
    pub fn new(
        function_key: FunctionName,
        parameter_types: Vec<AnalysedType>,
        return_type: Option<AnalysedType>,
    ) -> Self {
        FunctionSignature {
            function_name: function_key,
            parameter_types,
            return_type,
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
            function_name: value.function_signature.function_name.clone(),
            parameter_types: value.function_signature.parameter_types.clone(),
            return_type: value.function_signature.return_type.clone(),
        }
    }
}

impl FunctionUsageConstraint {
    pub fn function_key(&self) -> &FunctionName {
        &self.function_signature.function_name
    }

    pub fn parameter_types(&self) -> &Vec<AnalysedType> {
        &self.function_signature.parameter_types
    }

    pub fn return_type(&self) -> &Option<AnalysedType> {
        &self.function_signature.return_type
    }

    pub fn from_worker_function_type(
        worker_function_type: &WorkerFunctionType,
    ) -> FunctionUsageConstraint {
        FunctionUsageConstraint {
            function_signature: FunctionSignature {
                function_name: worker_function_type.function_name.clone(),
                parameter_types: worker_function_type.parameter_types.clone(),
                return_type: worker_function_type.return_type.clone(),
            },
            usage_count: 1,
        }
    }

    fn decrement_usage_count(&mut self) {
        self.usage_count -= 1;
    }
}

// mod protobuf {
//     use crate::model::component_constraint::{
//         FunctionConstraints, FunctionSignature, FunctionUsageConstraint,
//     };
//     use golem_api_grpc::proto::golem::component::FunctionConstraint as FunctionConstraintProto;
//     use golem_api_grpc::proto::golem::component::FunctionConstraintCollection as FunctionConstraintCollectionProto;
//     use golem_wasm::analysis::AnalysedType;
//     use rib::FunctionName;

//     impl TryFrom<golem_api_grpc::proto::golem::component::FunctionConstraintCollection>
//         for FunctionConstraints
//     {
//         type Error = String;

//         fn try_from(
//             value: golem_api_grpc::proto::golem::component::FunctionConstraintCollection,
//         ) -> Result<Self, Self::Error> {
//             let collection = FunctionConstraints {
//                 constraints: value
//                     .constraints
//                     .iter()
//                     .map(|constraint_proto| {
//                         FunctionUsageConstraint::try_from(constraint_proto.clone())
//                     })
//                     .collect::<Result<_, _>>()?,
//             };

//             Ok(collection)
//         }
//     }

//     impl From<FunctionConstraints> for FunctionConstraintCollectionProto {
//         fn from(value: FunctionConstraints) -> Self {
//             FunctionConstraintCollectionProto {
//                 constraints: value
//                     .constraints
//                     .iter()
//                     .map(|function_constraint| {
//                         FunctionConstraintProto::from(function_constraint.clone())
//                     })
//                     .collect(),
//             }
//         }
//     }

//     impl TryFrom<FunctionConstraintProto> for FunctionUsageConstraint {
//         type Error = String;

//         fn try_from(value: FunctionConstraintProto) -> Result<Self, Self::Error> {
//             let return_type = value
//                 .return_type
//                 .as_ref()
//                 .map(AnalysedType::try_from)
//                 .transpose()?;

//             let parameter_types = value
//                 .parameter_types
//                 .iter()
//                 .map(AnalysedType::try_from)
//                 .collect::<Result<_, _>>()?;

//             let function_name_proto = value
//                 .function_key
//                 .and_then(|x| x.function_name)
//                 .ok_or("Function key missing")?;

//             let function_key = FunctionName::try_from(function_name_proto)?;

//             let usage_count = value.usage_count;

//             Ok(Self {
//                 function_signature: FunctionSignature {
//                     function_name: function_key,
//                     parameter_types,
//                     return_type,
//                 },
//                 usage_count,
//             })
//         }
//     }

//     impl From<FunctionUsageConstraint> for FunctionConstraintProto {
//         fn from(value: FunctionUsageConstraint) -> Self {
//             let function_name = rib::proto::golem::rib::function_name_type::FunctionName::from(
//                 value.function_signature.clone().function_name,
//             );

//             let function_name_type = rib::proto::golem::rib::FunctionNameType {
//                 function_name: Some(function_name),
//             };

//             FunctionConstraintProto {
//                 function_key: Some(function_name_type),
//                 parameter_types: value
//                     .parameter_types()
//                     .iter()
//                     .map(|analysed_type| analysed_type.into())
//                     .collect(),
//                 return_type: value
//                     .return_type()
//                     .as_ref()
//                     .map(|analysed_type| analysed_type.into()),
//                 usage_count: value.usage_count,
//             }
//         }
//     }
// }
