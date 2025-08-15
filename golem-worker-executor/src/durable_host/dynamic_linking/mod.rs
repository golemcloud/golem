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

use crate::durable_host::dynamic_linking::wasm_rpc::dynamic_wasm_rpc_link;
use crate::durable_host::DurableWorkerCtx;
use crate::workerctx::{DynamicLinking, WorkerCtx};
use async_trait::async_trait;
use golem_common::model::component_metadata::DynamicLinkedInstance;
use golem_wasm_rpc::golem_rpc_0_2_x::types::HostFutureInvokeResult;
use golem_wasm_rpc::HostWasmRpc;
use wasmtime::component::types::ComponentItem;
use wasmtime::component::{Component, Linker};
use wasmtime::Engine;

mod wasm_rpc;

#[async_trait]
impl<Ctx: WorkerCtx + HostWasmRpc + HostFutureInvokeResult> DynamicLinking<Ctx>
    for DurableWorkerCtx<Ctx>
{
    fn link(
        &mut self,
        engine: &Engine,
        linker: &mut Linker<Ctx>,
        component: &Component,
        component_metadata: &golem_service_base::model::Component,
    ) -> anyhow::Result<()> {
        let mut root = linker.root();

        let component_type = component.component_type();
        for (name, item) in component_type.imports(engine) {
            let name = name.to_string();
            match item {
                ComponentItem::ComponentFunc(_) => {}
                ComponentItem::CoreFunc(_) => {}
                ComponentItem::Module(_) => {}
                ComponentItem::Component(_) => {}
                ComponentItem::ComponentInstance(ref inst) => {
                    match component_metadata
                        .metadata
                        .dynamic_linking()
                        .get(&name.to_string())
                    {
                        Some(DynamicLinkedInstance::WasmRpc(rpc_metadata)) => {
                            dynamic_wasm_rpc_link(&name, rpc_metadata, engine, &mut root, inst)?;
                        }
                        None => {
                            // Instance not marked for dynamic linking
                        }
                    }
                }
                ComponentItem::Type(_) => {}
                ComponentItem::Resource(_) => {}
            }
        }

        Ok(())
    }
}
