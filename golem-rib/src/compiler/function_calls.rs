use golem_wasm_ast::analysis::AnalysedType;
use crate::{FunctionTypeRegistry, InferredExpr, RegistryKey, RegistryValue};

// An easier data type to deal with extracting the metadata
// of the original metadata in a compiled Rib.
// While a type registry is concerned with every types
// this is narrow down version of the real function invokes
// used in Rib.
#[derive(Clone, Debug)]
pub struct FunctionCallsInRib {
    function_calls: Vec<FunctionCallInRib>
}

#[derive(Debug, Clone)]
pub struct FunctionCallInRib {
    pub function_key: RegistryKey,
    pub parameter_types: Vec<AnalysedType>,
    pub return_types: Vec<AnalysedType>
}

impl FunctionCallInRib {
    pub fn from_inferred_expr(inferred_expr: &InferredExpr, original_type_registry: &FunctionTypeRegistry) -> Result<FunctionCallsInRib, String> {
        let worker_invoke_registry_keys =
            inferred_expr.worker_invoke_registry_keys();
        let type_registry_subset =
            original_type_registry.get_from_keys(worker_invoke_registry_keys);
        let mut function_calls = vec![];

        for (key, value) in type_registry_subset.types {
              if let  RegistryValue::Function  {parameter_types, return_types } = value {
                  let function_call_in_rib = FunctionCallInRib {
                      function_key: key,
                      parameter_types,
                      return_types
                  };
                  function_calls.push(function_call_in_rib)
              } else {
                  return Err("Internal Error: Function Calls should have parameter types and return types".to_string())
              }
        }

        Ok(FunctionCallsInRib {
            function_calls
        })
    }
}