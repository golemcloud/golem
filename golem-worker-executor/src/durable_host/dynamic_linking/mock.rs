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

use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use golem_wasm::golem_rpc_0_2_x::types::HostFutureInvokeResult;
use golem_wasm::HostWasmRpc;
use rib::ParsedFunctionName;
use tracing::error;
use wasmtime::component::types::{ComponentInstance, ComponentItem};
use wasmtime::component::{LinkerInstance, ResourceType};
use wasmtime::Engine;

/// Temporary solution for mocking some dependencies
///
/// This way user components can permanently import a wide set of Golem-provided libraries,
/// without having to actually satisfy all these imports. Instead, the unsatisfied imports
/// will cause a runtime error instead of an instantiation failure.
///
/// We have to do this to avoid rebuilding the JS engine WASM for users - a temporary hack,
/// which is going to be replaced by a new way to compose libraries through a dynamic host interface.
pub fn should_mock_dependency(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "golem:embed/embed",
        "golem:exec/executor",
        "golem:exec/types",
        "golem:graph/connection",
        "golem:graph/errors",
        "golem:graph/schema",
        "golem:graph/transactions",
        "golem:graph/traversal",
        "golem:graph/types",
        "golem:graph/query",
        "golem:llm/llm",
        "golem:search/core",
        "golem:search/types",
        "golem:stt/languages",
        "golem:stt/transcription",
        "golem:stt/types",
        "golem:video-generation/advanced",
        "golem:video-generation/lip-sync",
        "golem:video-generation/types",
        "golem:video-generation/video-generation",
        "golem:web-search/types",
        "golem:web-search/web-search",
    ];

    if name.starts_with("golem:") {
        for prefix in PREFIXES {
            if name.starts_with(prefix) {
                return true;
            }
        }
    }
    false
}

pub fn mock_link<Ctx: WorkerCtx + HostWasmRpc + HostFutureInvokeResult>(
    name: &str,
    engine: &Engine,
    root: &mut LinkerInstance<Ctx>,
    inst: &ComponentInstance,
) -> anyhow::Result<()> {
    let mut instance = root.instance(name)?;
    let mut functions = Vec::new();

    for (inner_name, inner_item) in inst.exports(engine) {
        let name = name.to_owned();
        let inner_name = inner_name.to_owned();

        match inner_item {
            ComponentItem::ComponentFunc(_) => {
                let function_name = ParsedFunctionName::parse(format!(
                    "{name}.{{{inner_name}}}"
                ))
                    .map_err(|err| anyhow!(format!("Unexpected linking error: {name}.{{{inner_name}}} is not a valid function name: {err}")))?;

                functions.push(FunctionInfo {
                    name: function_name,
                });
            }
            ComponentItem::CoreFunc(_) => {}
            ComponentItem::Module(_) => {}
            ComponentItem::Component(_) => {}
            ComponentItem::ComponentInstance(_) => {}
            ComponentItem::Type(_) => {}
            ComponentItem::Resource(_resource) => {
                if &inner_name != "pollable"
                    && &inner_name != "wasi-io-pollable"
                    && &inner_name != "input-stream"
                    && &inner_name != "output-stream"
                {
                    // TODO: figure out how to do this properly
                    instance.resource(
                        &inner_name,
                        ResourceType::host::<ResourceEntry>(),
                        |_store, _rep| Ok(()),
                    )?;
                }
            }
        }
    }

    for function in functions {
        let name = name.to_string();
        instance.func_new_async(
            &function.name.function.function_name(),
            move |_store, _params, _results| {
                let name = name.clone();
                Box::new(async move {
                    let error_message = format!(
                        "Library {name} called without being linked with an implementation"
                    );
                    error!(error_message);
                    Err(anyhow!(error_message))
                })
            },
        )?;
    }

    Ok(())
}

struct FunctionInfo {
    name: ParsedFunctionName,
}

struct ResourceEntry;
