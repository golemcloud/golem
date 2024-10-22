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

use crate::{FunctionTypeRegistry, InferredExpr, RegistryKey, RegistryValue};
use golem_api_grpc::proto::golem::rib::WorkerInvokeCallInRib as WorkerInvokeCallInRibProto;
use golem_api_grpc::proto::golem::rib::WorkerInvokeCallsInRib as WorkerInvokeCallsInRibProto;
use golem_wasm_ast::analysis::AnalysedType;

// An easier data type that focus just the function calls,
// return types and parameter types, corresponding to a function
// that can also be a resource constructor, resource method, as well
// as a simple function name.
// These will not include variant or enum calls, that are originally
// tagged as functions. This is why we need a fully inferred Rib (fully compiled rib),
// which has specific details, along with original type registry to construct this data.
// These function calls are specifically worker invoke calls and nothing else.
// If Rib has inbuilt function support, that will not be included here either.
#[derive(Clone, Debug)]
pub struct WorkerInvokeCallsInRib {
    function_calls: Vec<WorkerInvokeCallInRib>,
}

impl WorkerInvokeCallsInRib {
    pub fn from_inferred_expr(
        inferred_expr: &InferredExpr,
        original_type_registry: &FunctionTypeRegistry,
    ) -> Result<Option<WorkerInvokeCallsInRib>, String> {
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
                let function_call_in_rib = WorkerInvokeCallInRib {
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
            Ok(Some(WorkerInvokeCallsInRib { function_calls }))
        }
    }
}

impl TryFrom<WorkerInvokeCallsInRibProto> for WorkerInvokeCallsInRib {
    type Error = String;

    fn try_from(value: WorkerInvokeCallsInRibProto) -> Result<Self, Self::Error> {
        let function_calls_proto = value.function_calls;
        let function_calls = function_calls_proto
            .iter()
            .map(|x| WorkerInvokeCallInRib::try_from(x.clone()))
            .collect::<Result<_, _>>()?;
        Ok(Self { function_calls })
    }
}

impl From<WorkerInvokeCallsInRib> for WorkerInvokeCallsInRibProto {
    fn from(value: WorkerInvokeCallsInRib) -> Self {
        WorkerInvokeCallsInRibProto {
            function_calls: value
                .function_calls
                .iter()
                .map(|x| WorkerInvokeCallInRibProto::from(x.clone()))
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkerInvokeCallInRib {
    pub function_key: RegistryKey,
    pub parameter_types: Vec<AnalysedType>,
    pub return_types: Vec<AnalysedType>,
}

impl TryFrom<WorkerInvokeCallInRibProto> for WorkerInvokeCallInRib {
    type Error = String;

    fn try_from(value: WorkerInvokeCallInRibProto) -> Result<Self, Self::Error> {
        let return_types = value
            .return_types
            .iter()
            .map(|x| AnalysedType::try_from(x))
            .collect::<Result<_, _>>()?;

        let parameter_types = value
            .parameter_types
            .iter()
            .map(|x| AnalysedType::try_from(x))
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

impl From<WorkerInvokeCallInRib> for WorkerInvokeCallInRibProto {
    fn from(value: WorkerInvokeCallInRib) -> Self {
        let registry_key = value.function_key.into();

        WorkerInvokeCallInRibProto {
            function_key: Some(registry_key),
            parameter_types: value.parameter_types.iter().map(|x| x.into()).collect(),
            return_types: value.return_types.iter().map(|x| x.into()).collect(),
        }
    }
}
