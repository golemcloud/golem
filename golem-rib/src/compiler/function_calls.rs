use golem_wasm_ast::analysis::AnalysedType;
use crate::{FunctionTypeRegistry, InferredExpr, RegistryKey, RegistryValue};

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
    function_calls: Vec<WorkerInvokeCallInRib>
}

impl WorkerInvokeCallsInRib {
    pub fn from_inferred_expr(inferred_expr: &InferredExpr, original_type_registry: &FunctionTypeRegistry) -> Result<WorkerInvokeCallsInRib, String> {
        let worker_invoke_registry_keys =
            inferred_expr.worker_invoke_registry_keys();
        let type_registry_subset =
            original_type_registry.get_from_keys(worker_invoke_registry_keys);
        let mut function_calls = vec![];

        for (key, value) in type_registry_subset.types {
            if let  RegistryValue::Function  {parameter_types, return_types } = value {
                let function_call_in_rib = WorkerInvokeCallInRib {
                    function_key: key,
                    parameter_types,
                    return_types
                };
                function_calls.push(function_call_in_rib)
            } else {
                return Err("Internal Error: Function Calls should have parameter types and return types".to_string())
            }
        }

        Ok(WorkerInvokeCallsInRib {
            function_calls
        })
    }
}

#[derive(Debug, Clone)]
pub struct WorkerInvokeCallInRib {
    pub function_key: RegistryKey,
    pub parameter_types: Vec<AnalysedType>,
    pub return_types: Vec<AnalysedType>
}
