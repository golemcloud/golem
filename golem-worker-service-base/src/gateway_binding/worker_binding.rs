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

use super::{IdempotencyKeyCompiled, InvocationContextCompiled, WorkerNameCompiled};
use crate::gateway_rib_compiler::DefaultWorkerServiceRibCompiler;
use crate::gateway_rib_compiler::WorkerServiceRibCompiler;
use golem_common::model::component::VersionedComponentId;
use golem_wasm_ast::analysis::AnalysedExport;
use rib::{
    Expr, RibByteCode, RibCompilationError, RibInputTypeInfo, RibOutputTypeInfo,
    WorkerFunctionsInRib,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerBinding {
    pub component_id: VersionedComponentId,
    pub worker_name: Option<Expr>,
    pub idempotency_key: Option<Expr>,
    pub response_mapping: ResponseMapping,
    pub invocation_context: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkerBindingCompiled {
    pub component_id: VersionedComponentId,
    pub worker_name_compiled: Option<WorkerNameCompiled>,
    pub idempotency_key_compiled: Option<IdempotencyKeyCompiled>,
    pub response_compiled: ResponseMappingCompiled,
    pub invocation_context_compiled: Option<InvocationContextCompiled>,
}

impl WorkerBindingCompiled {
    pub fn from_raw_worker_binding(
        gateway_worker_binding: &WorkerBinding,
        export_metadata: &[AnalysedExport],
    ) -> Result<Self, RibCompilationError> {
        let worker_name_compiled: Option<WorkerNameCompiled> = gateway_worker_binding
            .worker_name
            .clone()
            .map(|worker_name_expr| {
                WorkerNameCompiled::from_worker_name(&worker_name_expr, export_metadata)
            })
            .transpose()?;

        let idempotency_key_compiled = match &gateway_worker_binding.idempotency_key {
            Some(idempotency_key) => Some(IdempotencyKeyCompiled::from_idempotency_key(
                idempotency_key,
                export_metadata,
            )?),
            None => None,
        };
        let response_compiled = ResponseMappingCompiled::from_response_mapping(
            &gateway_worker_binding.response_mapping,
            export_metadata,
        )?;
        let invocation_context_compiled = match &gateway_worker_binding.invocation_context {
            Some(invocation_context) => Some(InvocationContextCompiled::from_invocation_context(
                invocation_context,
                export_metadata,
            )?),
            None => None,
        };

        Ok(WorkerBindingCompiled {
            component_id: gateway_worker_binding.component_id.clone(),
            worker_name_compiled,
            idempotency_key_compiled,
            response_compiled,
            invocation_context_compiled,
        })
    }
}

impl From<WorkerBindingCompiled> for WorkerBinding {
    fn from(value: WorkerBindingCompiled) -> Self {
        let worker_binding = value.clone();

        WorkerBinding {
            component_id: worker_binding.component_id,
            worker_name: worker_binding
                .worker_name_compiled
                .map(|compiled| compiled.worker_name),
            idempotency_key: worker_binding
                .idempotency_key_compiled
                .map(|compiled| compiled.idempotency_key),
            response_mapping: ResponseMapping(
                worker_binding.response_compiled.response_mapping_expr,
            ),
            invocation_context: worker_binding
                .invocation_context_compiled
                .map(|compiled| compiled.invocation_context),
        }
    }
}

// ResponseMapping will consist of actual logic such as invoking worker functions
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResponseMapping(pub Expr);

#[derive(Debug, Clone, PartialEq)]
pub struct ResponseMappingCompiled {
    pub response_mapping_expr: Expr,
    pub response_mapping_compiled: RibByteCode,
    pub rib_input: RibInputTypeInfo,
    pub worker_calls: Option<WorkerFunctionsInRib>,
    // Optional to keep backward compatibility
    pub rib_output: Option<RibOutputTypeInfo>,
}

impl ResponseMappingCompiled {
    pub fn from_response_mapping(
        response_mapping: &ResponseMapping,
        exports: &[AnalysedExport],
    ) -> Result<Self, RibCompilationError> {
        let response_compiled =
            DefaultWorkerServiceRibCompiler::compile(&response_mapping.0, exports)?;

        Ok(ResponseMappingCompiled {
            response_mapping_expr: response_mapping.0.clone(),
            response_mapping_compiled: response_compiled.byte_code,
            rib_input: response_compiled.rib_input_type_info,
            worker_calls: response_compiled.worker_invoke_calls,
            rib_output: response_compiled.rib_output_type_info,
        })
    }
}
