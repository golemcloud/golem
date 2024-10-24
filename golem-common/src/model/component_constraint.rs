use bincode::{Decode, Encode};
use golem_api_grpc::proto::golem::component::FunctionConstraint as FunctionConstraintProto;
use golem_wasm_ast::analysis::AnalysedType;
use rib::{RegistryKey, WorkerFunctionInRibMetadata, WorkerFunctionsInRib};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// This is very similar to WorkerFunctionsInRib data structure, however
// it adds the total number of usages for each function in that component
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionConstraintCollection {
    pub function_usages: Vec<FunctionConstraint>,
}

impl TryFrom<golem_api_grpc::proto::golem::component::FunctionConstraintCollection>
    for FunctionConstraintCollection
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::FunctionConstraintCollection,
    ) -> Result<Self, Self::Error> {
        let collection = FunctionConstraintCollection {
            function_usages: value
                .constraints
                .iter()
                .map(|x| FunctionConstraint::try_from(x.clone()))
                .collect::<Result<_, _>>()?,
        };

        Ok(collection)
    }
}

impl From<FunctionConstraintCollection>
    for golem_api_grpc::proto::golem::component::FunctionConstraintCollection
{
    fn from(value: FunctionConstraintCollection) -> Self {
        golem_api_grpc::proto::golem::component::FunctionConstraintCollection {
            constraints: value
                .function_usages
                .iter()
                .map(|x| {
                    golem_api_grpc::proto::golem::component::FunctionConstraint::from(x.clone())
                })
                .collect(),
        }
    }
}

impl From<FunctionConstraintCollection> for WorkerFunctionsInRib {
    fn from(value: FunctionConstraintCollection) -> Self {
        WorkerFunctionsInRib {
            function_calls: value
                .function_usages
                .iter()
                .map(|x| rib::WorkerFunctionInRibMetadata::from(x.clone()))
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
            .map(FunctionConstraint::from_worker_function_in_rib)
            .collect::<Vec<_>>();

        FunctionConstraintCollection {
            function_usages: functions,
        }
    }
    pub fn try_merge(
        worker_functions: Vec<FunctionConstraintCollection>,
    ) -> Result<FunctionConstraintCollection, String> {
        let mut merged_function_calls: HashMap<RegistryKey, FunctionConstraint> = HashMap::new();

        for wf in worker_functions {
            for call in wf.function_usages {
                match merged_function_calls.get_mut(&call.function_key) {
                    Some(existing_call) => {
                        // Check for parameter type conflicts
                        if existing_call.parameter_types != call.parameter_types {
                            return Err(format!(
                                "Parameter type conflict for function key {:?}: {:?} vs {:?}",
                                call.function_key,
                                existing_call.parameter_types,
                                call.parameter_types
                            ));
                        }

                        // Check for return type conflicts
                        if existing_call.return_types != call.return_types {
                            return Err(format!(
                                "Return type conflict for function key {:?}: {:?} vs {:?}",
                                call.function_key, existing_call.return_types, call.return_types
                            ));
                        }

                        // Update usage_count instead of overwriting
                        existing_call.usage_count =
                            existing_call.usage_count.saturating_add(call.usage_count);
                    }
                    None => {
                        // Insert if no conflict is found
                        merged_function_calls.insert(call.function_key.clone(), call);
                    }
                }
            }
        }

        let mut merged_function_calls_vec: Vec<FunctionConstraint> =
            merged_function_calls.into_values().collect();

        merged_function_calls_vec.sort_by(|a, b| a.function_key.cmp(&b.function_key));

        Ok(FunctionConstraintCollection {
            function_usages: merged_function_calls_vec,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub struct FunctionConstraint {
    pub function_key: RegistryKey,
    pub parameter_types: Vec<AnalysedType>,
    pub return_types: Vec<AnalysedType>,
    pub usage_count: u32,
}

impl From<FunctionConstraint> for WorkerFunctionInRibMetadata {
    fn from(value: FunctionConstraint) -> Self {
        WorkerFunctionInRibMetadata {
            function_key: value.function_key.clone(),
            parameter_types: value.parameter_types.clone(),
            return_types: value.return_types.clone(),
        }
    }
}

impl FunctionConstraint {
    pub fn from_worker_function_in_rib(
        worker_function_rib: &WorkerFunctionInRibMetadata,
    ) -> FunctionConstraint {
        FunctionConstraint {
            function_key: worker_function_rib.function_key.clone(),
            parameter_types: worker_function_rib.parameter_types.clone(),
            return_types: worker_function_rib.return_types.clone(),
            usage_count: 1,
        }
    }

    pub fn increment_usage(&self) -> FunctionConstraint {
        FunctionConstraint {
            function_key: self.function_key.clone(),
            parameter_types: self.parameter_types.clone(),
            return_types: self.return_types.clone(),
            usage_count: self.usage_count + 1,
        }
    }
}

impl TryFrom<FunctionConstraintProto> for FunctionConstraint {
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
            function_key,
            return_types,
            parameter_types,
            usage_count,
        })
    }
}

impl From<FunctionConstraint> for FunctionConstraintProto {
    fn from(value: FunctionConstraint) -> Self {
        let registry_key = value.function_key.into();

        FunctionConstraintProto {
            function_key: Some(registry_key),
            parameter_types: value.parameter_types.iter().map(|x| x.into()).collect(),
            return_types: value.return_types.iter().map(|x| x.into()).collect(),
            usage_count: value.usage_count,
        }
    }
}
