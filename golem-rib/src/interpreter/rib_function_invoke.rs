// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{ComponentDependencyKey, InstructionId};
use async_trait::async_trait;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::ValueAndType;

#[async_trait]
pub trait RibComponentFunctionInvoke {
    async fn invoke(
        &self,
        component_dependency_key: ComponentDependencyKey,
        instruction_id: &InstructionId,
        worker_name: Option<EvaluatedWorkerName>,
        function_name: EvaluatedFqFn,
        args: EvaluatedFnArgs,
        return_type: Option<AnalysedType>,
    ) -> RibFunctionInvokeResult;
}

pub type RibFunctionInvokeResult =
    Result<Option<ValueAndType>, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
pub struct EvaluatedFqFn(pub String);

#[derive(Clone)]
pub struct EvaluatedWorkerName(pub String);

pub struct EvaluatedFnArgs(pub Vec<ValueAndType>);
