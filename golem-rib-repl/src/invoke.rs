use crate::embedded_executor::EmbeddedWorkerExecutor;
use async_trait::async_trait;
use golem_common::base_model::TargetWorkerId;
use golem_common::model::ComponentId;
use golem_test_framework::dsl::TestDslUnsafe;
use golem_wasm_rpc::ValueAndType;
use rib::{EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName};
use std::sync::Arc;

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

pub struct DefaultWorkerFunctionInvoke {
    embedded_worker_executor: Arc<EmbeddedWorkerExecutor>,
}
impl<'a> DefaultWorkerFunctionInvoke {
    pub fn new(embedded_worker_executor: Arc<EmbeddedWorkerExecutor>) -> Self {
        Self {
            embedded_worker_executor,
        }
    }
}

#[async_trait]
impl WorkerFunctionInvoke for DefaultWorkerFunctionInvoke {
    async fn invoke(
        &self,
        component_id: ComponentId,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
    ) -> Result<ValueAndType, String> {
        let target_worker_id = worker_name
            .map(|w| TargetWorkerId {
                component_id: component_id.clone(),
                worker_name: Some(w.0),
            })
            .unwrap_or_else(|| TargetWorkerId {
                component_id,
                worker_name: None,
            });

        let function_name = function_name.0;

        self.embedded_worker_executor
            .invoke_and_await_typed(target_worker_id, function_name.as_str(), args.0)
            .await
            .map_err(|e| format!("Failed to invoke function: {:?}", e))
    }
}
