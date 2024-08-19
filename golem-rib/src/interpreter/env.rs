use crate::interpreter::result::RibInterpreterResult;
use crate::{ParsedFunctionName, VariableId};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub struct InterpreterEnv {
    pub env: HashMap<EnvironmentKey, RibInterpreterResult>,
    pub call_worker_function_async: RibFunctionInvoke,
}

pub type RibFunctionInvoke = Arc<
    dyn Fn(
            ParsedFunctionName,
            Vec<TypeAnnotatedValue>,
        ) -> Pin<Box<dyn Future<Output = Result<TypeAnnotatedValue, String>> + Send>>
        + Send
        + Sync,
>;

impl Default for InterpreterEnv {
    fn default() -> Self {
        InterpreterEnv {
            env: HashMap::new(),
            call_worker_function_async: internal::default_worker_invoke_async(),
        }
    }
}

impl InterpreterEnv {
    pub fn new(
        env: HashMap<EnvironmentKey, RibInterpreterResult>,
        call_worker_function_async: RibFunctionInvoke,
    ) -> Self {
        InterpreterEnv {
            env,
            call_worker_function_async,
        }
    }

    pub fn invoke_worker_function_async(
        &self,
        function_name: ParsedFunctionName,
        args: Vec<TypeAnnotatedValue>,
    ) -> Pin<Box<dyn Future<Output = Result<TypeAnnotatedValue, String>> + Send>> {
        (self.call_worker_function_async)(function_name, args)
    }

    pub fn from_input(env: HashMap<String, TypeAnnotatedValue>) -> Self {
        let env = env
            .into_iter()
            .map(|(k, v)| (EnvironmentKey::from_global(k), RibInterpreterResult::Val(v)))
            .collect();

        InterpreterEnv {
            env,
            call_worker_function_async: internal::default_worker_invoke_async(),
        }
    }

    pub fn insert(&mut self, key: EnvironmentKey, value: RibInterpreterResult) {
        self.env.insert(key, value);
    }

    pub fn lookup(&self, key: &EnvironmentKey) -> Option<RibInterpreterResult> {
        self.env.get(key).cloned()
    }
}

#[derive(Debug, Hash, Eq, PartialEq, Clone)]
pub struct EnvironmentKey {
    pub variable_id: VariableId,
}

impl EnvironmentKey {
    pub fn from(variable_id: VariableId) -> Self {
        EnvironmentKey { variable_id }
    }

    pub fn from_global(key: String) -> Self {
        EnvironmentKey {
            variable_id: VariableId::global(key),
        }
    }
}

mod internal {
    use crate::interpreter::env::RibFunctionInvoke;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::TypedTuple;
    use std::sync::Arc;

    pub(crate) fn default_worker_invoke_async() -> RibFunctionInvoke {
        Arc::new(|_, _| {
            Box::pin(async {
                Ok(TypeAnnotatedValue::Tuple(TypedTuple {
                    typ: vec![],
                    value: vec![],
                }))
            })
        })
    }
}
