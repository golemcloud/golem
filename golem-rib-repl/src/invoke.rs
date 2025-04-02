use async_trait::async_trait;
use golem_common::model::ComponentId;
use golem_wasm_rpc::ValueAndType;
use rib::{EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName};

#[async_trait]
pub trait WorkerFunctionInvoke {
    async fn invoke(
        &self,
        component_id: ComponentId,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
    ) -> Result<ValueAndType, String>;
}
