use async_trait::async_trait;
use golem_wasm_rpc::ValueAndType;

#[async_trait]
pub trait RibFunctionInvoke {
    async fn invoke(
        &self,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
    ) -> Result<ValueAndType, String>;
}

pub struct EvaluatedFqFn(pub String);

#[derive(Clone)]
pub struct EvaluatedWorkerName(pub String);

pub struct EvaluatedFnArgs(pub Vec<ValueAndType>);
