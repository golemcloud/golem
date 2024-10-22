// Copyright 2024 Golem Cloud
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

use std::collections::HashMap;
use bincode::{Decode, Encode};
use crate::{FunctionTypeRegistry, InferredExpr, RegistryKey, RegistryValue};
use golem_api_grpc::proto::golem::rib::WorkerFunctionInRibMetadata as WorkerFunctionInRibMetadataProto;
use golem_api_grpc::proto::golem::rib::WorkerFunctionsInRib as WorkerFunctionsInRibProto;
use golem_wasm_ast::analysis::AnalysedType;
use serde::{Deserialize, Serialize};

// An easier data type that focus just the function calls,
// return types and parameter types, corresponding to a function
// that can also be a resource constructor, resource method, as well
// as a simple function name.
// These will not include variant or enum calls, that are originally
// tagged as functions. This is why we need a fully inferred Rib (fully compiled rib),
// which has specific details, along with original type registry to construct this data.
// These function calls are specifically worker invoke calls and nothing else.
// If Rib has inbuilt function support, that will not be included here either.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub struct WorkerFunctionsInRib {
    pub function_calls: Vec<WorkerFunctionInRibMetadata>,
}

impl WorkerFunctionsInRib {
    pub fn try_merge(worker_functions: Vec<WorkerFunctionsInRib>) -> Result<WorkerFunctionsInRib, String> {
        let mut merged_function_calls: HashMap<RegistryKey, WorkerFunctionInRibMetadata> = HashMap::new();

        for wf in worker_functions {
            for call in wf.function_calls {
                match merged_function_calls.get(&call.function_key) {
                    Some(existing_call) => {
                        // Check for parameter type conflicts
                        if existing_call.parameter_types != call.parameter_types {
                            return Err(format!(
                                "Parameter type conflict for function key {:?}: {:?} vs {:?}",
                                call.function_key, existing_call.parameter_types, call.parameter_types
                            ));
                        }

                        // Check for return type conflicts
                        if existing_call.return_types != call.return_types {
                            return Err(format!(
                                "Return type conflict for function key {:?}: {:?} vs {:?}",
                                call.function_key, existing_call.return_types, call.return_types
                            ));
                        }
                    }
                    None => {
                        // Insert if no conflict is found
                        merged_function_calls.insert(call.function_key.clone(), call);
                    }
                }
            }
        }

        let merged_function_calls_vec =
            merged_function_calls.into_iter().map(|(_, call)| call).collect();

        Ok(WorkerFunctionsInRib {
            function_calls: merged_function_calls_vec,
        })
    }

    pub fn from_inferred_expr(
        inferred_expr: &InferredExpr,
        original_type_registry: &FunctionTypeRegistry,
    ) -> Result<Option<WorkerFunctionsInRib>, String> {
        let worker_invoke_registry_keys = inferred_expr.worker_invoke_registry_keys();
        let type_registry_subset =
            original_type_registry.get_from_keys(worker_invoke_registry_keys);
        let mut function_calls = vec![];

        for (key, value) in type_registry_subset.types {
            if let RegistryValue::Function {
                parameter_types,
                return_types,
            } = value
            {
                let function_call_in_rib = WorkerFunctionInRibMetadata {
                    function_key: key,
                    parameter_types,
                    return_types,
                };
                function_calls.push(function_call_in_rib)
            } else {
                return Err(
                    "Internal Error: Function Calls should have parameter types and return types"
                        .to_string(),
                );
            }
        }

        if function_calls.is_empty() {
            Ok(None)
        } else {
            Ok(Some(WorkerFunctionsInRib { function_calls }))
        }
    }
}

impl TryFrom<WorkerFunctionsInRibProto> for WorkerFunctionsInRib {
    type Error = String;

    fn try_from(value: WorkerFunctionsInRibProto) -> Result<Self, Self::Error> {
        let function_calls_proto = value.function_calls;
        let function_calls = function_calls_proto
            .iter()
            .map(|x| WorkerFunctionInRibMetadata::try_from(x.clone()))
            .collect::<Result<_, _>>()?;
        Ok(Self { function_calls })
    }
}

impl From<WorkerFunctionsInRib> for WorkerFunctionsInRibProto {
    fn from(value: WorkerFunctionsInRib) -> Self {
        WorkerFunctionsInRibProto {
            function_calls: value
                .function_calls
                .iter()
                .map(|x| WorkerFunctionInRibMetadataProto::from(x.clone()))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub struct WorkerFunctionInRibMetadata {
    pub function_key: RegistryKey,
    pub parameter_types: Vec<AnalysedType>,
    pub return_types: Vec<AnalysedType>,
}

impl TryFrom<WorkerFunctionInRibMetadataProto> for WorkerFunctionInRibMetadata {
    type Error = String;

    fn try_from(value: WorkerFunctionInRibMetadataProto) -> Result<Self, Self::Error> {
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

        Ok(Self {
            function_key,
            return_types,
            parameter_types,
        })
    }
}

impl From<WorkerFunctionInRibMetadata> for WorkerFunctionInRibMetadataProto {
    fn from(value: WorkerFunctionInRibMetadata) -> Self {
        let registry_key = value.function_key.into();

        WorkerFunctionInRibMetadataProto {
            function_key: Some(registry_key),
            parameter_types: value.parameter_types.iter().map(|x| x.into()).collect(),
            return_types: value.return_types.iter().map(|x| x.into()).collect(),
        }
    }
}
