use crate::InstructionId;
use async_trait::async_trait;
use golem_wasm_rpc::ValueAndType;

#[async_trait]
pub trait RibFunctionInvoke {
    async fn invoke(
        &self,
        instruction_id: &InstructionId,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
    ) -> RibFunctionInvokeResult;
}

pub type RibFunctionInvokeResult = Result<ValueAndType, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
pub struct EvaluatedFqFn(pub String);

#[derive(Clone)]
pub struct EvaluatedWorkerName(pub String);

pub struct EvaluatedFnArgs(pub Vec<ValueAndType>);
