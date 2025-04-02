use async_trait::async_trait;
use golem_wasm_rpc::ValueAndType;
use rib::{EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName};
use uuid::Uuid;

#[async_trait]
pub trait WorkerFunctionInvoke {
    async fn invoke(
        &self,
        component_id: Uuid,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
    ) -> Result<ValueAndType, String>;
}
