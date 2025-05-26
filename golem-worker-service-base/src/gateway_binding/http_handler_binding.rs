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

use super::{IdempotencyKeyCompiled, WorkerNameCompiled};
use golem_common::model::component::VersionedComponentId;
use golem_wasm_ast::analysis::AnalysedExport;
use rib::{Expr, RibCompilationError};

#[derive(Debug, Clone, PartialEq)]
pub struct HttpHandlerBinding {
    pub component_id: VersionedComponentId,
    pub worker_name: Option<Expr>,
    pub idempotency_key: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpHandlerBindingCompiled {
    pub component_id: VersionedComponentId,
    pub worker_name_compiled: Option<WorkerNameCompiled>,
    pub idempotency_key_compiled: Option<IdempotencyKeyCompiled>,
}

impl HttpHandlerBindingCompiled {
    pub fn from_raw_http_handler_binding(
        http_handler_binding: &HttpHandlerBinding,
        export_metadata: &[AnalysedExport],
    ) -> Result<Self, RibCompilationError> {
        let worker_name_compiled: Option<WorkerNameCompiled> = http_handler_binding
            .worker_name
            .clone()
            .map(|worker_name_expr| {
                WorkerNameCompiled::from_worker_name(&worker_name_expr, export_metadata)
            })
            .transpose()?;

        let idempotency_key_compiled = match &http_handler_binding.idempotency_key {
            Some(idempotency_key) => Some(IdempotencyKeyCompiled::from_idempotency_key(
                idempotency_key,
                export_metadata,
            )?),
            None => None,
        };

        Ok(HttpHandlerBindingCompiled {
            component_id: http_handler_binding.component_id.clone(),
            worker_name_compiled,
            idempotency_key_compiled,
        })
    }
}

impl From<HttpHandlerBindingCompiled> for HttpHandlerBinding {
    fn from(value: HttpHandlerBindingCompiled) -> Self {
        let worker_binding = value.clone();

        HttpHandlerBinding {
            component_id: worker_binding.component_id,
            worker_name: worker_binding
                .worker_name_compiled
                .map(|compiled| compiled.worker_name),
            idempotency_key: worker_binding
                .idempotency_key_compiled
                .map(|compiled| compiled.idempotency_key),
        }
    }
}
