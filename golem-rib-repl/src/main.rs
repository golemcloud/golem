use async_trait::async_trait;
use crate::rib_repl::RibRepl;
use golem_wasm_rpc::ValueAndType;
use rib::{EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, RibFunctionInvoke};

mod dependency_manager;
mod history;
mod local;
mod result_printer;
mod rib_repl;
mod syntax_highlighter;

#[tokio::main]
async fn main() {
    let mut repl = RibRepl::new(
        None,
        Box::new(dependency_manager::DefaultRibDependencyManager),
        Box::new(TempRibFunctionInvoke),
        None,
        None,
    );
    repl.run().await;
}

struct TempRibFunctionInvoke;

#[async_trait]
impl RibFunctionInvoke for TempRibFunctionInvoke {
    async fn invoke(
        &self,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
    ) -> Result<ValueAndType, String> {
        todo!()
    }
}
