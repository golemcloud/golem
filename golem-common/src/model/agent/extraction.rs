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

use crate::model::agent::{AgentError, AgentType};
use anyhow::anyhow;
use rib::ParsedFunctionName;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{debug, error};
use wasmtime::component::types::{ComponentInstance, ComponentItem};
use wasmtime::component::{
    Component, Func, Instance, Linker, LinkerInstance, ResourceTable, ResourceType, Type,
};
use wasmtime::{AsContextMut, Engine, Store};
use wasmtime_wasi::p2::{pipe, StdoutStream, WasiCtx, WasiView};
use wasmtime_wasi::{IoCtx, IoView};
use wit_parser::{PackageId, Resolve, WorldItem};

const INTERFACE_NAME: &str = "golem:agent/guest";
const FUNCTION_NAME: &str = "discover-agent-types";

/// Extracts the implemented agent types from the given WASM component, assuming it implements the `golem:agent/guest` interface.
/// Optionally fails if the component does not implement the agent interfaces, otherwise returns an empty agent type set for such components.
pub async fn extract_agent_types_with_streams(
    wasm_path: &Path,
    stdout: Option<impl StdoutStream + 'static>,
    stderr: Option<impl StdoutStream + 'static>,
    fail_on_missing_discover_method: bool,
    enable_fs_cache: bool,
) -> anyhow::Result<Vec<AgentType>> {
    let mut config = wasmtime::Config::default();
    config.async_support(true);
    config.wasm_component_model(true);

    if enable_fs_cache {
        config.cache(Some(
            wasmtime::Cache::new(wasmtime::CacheConfig::new()).expect("Failed to initialize cache"),
        ));
    }

    let engine = Engine::new(&config)?;
    let mut linker: Linker<Host> = Linker::new(&engine);
    linker.allow_shadowing(true);

    wasmtime_wasi::p2::add_to_linker_with_options_async(
        &mut linker,
        &wasmtime_wasi::p2::bindings::LinkOptions::default(),
    )?;

    let mut builder = WasiCtx::builder();

    if let Some(stdout) = stdout {
        builder.stdout(stdout);
    } else {
        builder.inherit_stdout();
    }

    if let Some(stderr) = stderr {
        builder.stderr(stderr);
    } else {
        builder.inherit_stderr();
    }

    let (wasi, io) = builder.env("RUST_BACKTRACE", "1").build();

    let host = Host {
        table: Arc::new(Mutex::new(ResourceTable::new())),
        wasi: Arc::new(Mutex::new(wasi)),
        io: Arc::new(Mutex::new(io)),
    };

    let component = Component::from_file(&engine, wasm_path)?;
    let mut store = Store::new(&engine, host);

    let mut linker_instance = linker.root();
    let component_type = component.component_type();
    for (name, item) in component_type.imports(&engine) {
        let name = name.to_string();
        match item {
            ComponentItem::ComponentFunc(_) => {}
            ComponentItem::CoreFunc(_) => {}
            ComponentItem::Module(_) => {}
            ComponentItem::Component(_) => {}
            ComponentItem::ComponentInstance(ref inst) => {
                dynamic_import(&name, &engine, &mut linker_instance, inst)?;
            }
            ComponentItem::Type(_) => {}
            ComponentItem::Resource(_) => {}
        }
    }

    debug!("Instantiating component");
    let instance = linker.instantiate_async(&mut store, &component).await?;

    let func = if let Some(func) = find_discover_function(&mut store, &instance) {
        func
    } else if fail_on_missing_discover_method {
        return Err(anyhow!(
            "Function {FUNCTION_NAME} not found in interface {INTERFACE_NAME}"
        ));
    } else {
        return Ok(Vec::new());
    };

    let typed_func = func.typed::<(), (
        Result<
            Vec<crate::model::agent::bindings::golem::agent::common::AgentType>,
            crate::model::agent::bindings::golem::agent::common::AgentError,
        >,
    )>(&mut store)?;
    let results = typed_func.call_async(&mut store, ()).await?;
    typed_func.post_return_async(&mut store).await?;

    match results.0 {
        Ok(results) => {
            let agent_types = results.into_iter().map(AgentType::from).collect();
            debug!("Discovered agent types: {:#?}", agent_types);
            Ok(agent_types)
        }
        Err(agent_error) => {
            let agent_error: AgentError = agent_error.into();
            error!("Error while discovering agent types: {agent_error}");
            Err(anyhow!(agent_error.to_string()))
        }
    }
}

/// Same as extract_agent_types_with_streams, but inherits stdout and stderr from the current process
pub async fn extract_agent_types(
    wasm_path: &Path,
    fail_on_missing_discover_method: bool,
    enable_fs_cache: bool,
) -> anyhow::Result<Vec<AgentType>> {
    extract_agent_types_with_streams(
        wasm_path,
        None::<pipe::MemoryOutputPipe>,
        None::<pipe::MemoryOutputPipe>,
        fail_on_missing_discover_method,
        enable_fs_cache,
    )
    .await
}

/// Checks if the given resolved component implements the `golem:agent/guest` interface.
pub fn is_agent(
    resolve: &Resolve,
    root_package_id: &PackageId,
    world: Option<&str>,
) -> anyhow::Result<bool> {
    let golem_agent_package = wit_parser::PackageName {
        namespace: "golem".to_string(),
        name: "agent".to_string(),
        version: None,
    };
    const GOLEM_AGENT_INTERFACE_NAME: &str = "guest";

    let world_id = resolve.select_world(*root_package_id, world)?;
    let world = resolve
        .worlds
        .get(world_id)
        .ok_or_else(|| anyhow!("Could not get {world_id:?}"))?;
    let world_name = &world.name;
    for (key, item) in &world.exports {
        if let WorldItem::Interface { id, .. } = &item {
            let interface = resolve.interfaces.get(*id).ok_or_else(|| {
                anyhow!("Could not get exported interface {key:?} exported from world {world_name}")
            })?;
            if let Some(interface_name) = interface.name.as_ref() {
                if interface_name == GOLEM_AGENT_INTERFACE_NAME {
                    if let Some(package_id) = &interface.package {
                        let package = resolve.packages.get(*package_id).ok_or_else(|| {
                            anyhow!(
                                "Could not get owner package of exported interface {interface_name}"
                            )
                        })?;

                        if package.name == golem_agent_package {
                            return Ok(true);
                        }
                    }
                }
            }
        }
    }

    Ok(false)
}

fn find_discover_function(mut store: impl AsContextMut, instance: &Instance) -> Option<Func> {
    let (_, exported_instance_id) = instance.get_export(&mut store, None, INTERFACE_NAME)?;
    let (_, func_id) =
        instance.get_export(&mut store, Some(&exported_instance_id), FUNCTION_NAME)?;
    let func = instance.get_func(&mut store, func_id)?;
    Some(func)
}

#[derive(Clone)]
struct Host {
    pub table: Arc<Mutex<ResourceTable>>,
    pub wasi: Arc<Mutex<WasiCtx>>,
    pub io: Arc<Mutex<IoCtx>>,
}

impl IoView for Host {
    fn table(&mut self) -> &mut ResourceTable {
        Arc::get_mut(&mut self.table)
            .expect("ResourceTable is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("ResourceTable mutex must never fail")
    }

    fn io_ctx(&mut self) -> &mut IoCtx {
        Arc::get_mut(&mut self.io)
            .expect("IoCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("IoCtx mutex must never fail")
    }
}

impl WasiView for Host {
    fn ctx(&mut self) -> &mut WasiCtx {
        Arc::get_mut(&mut self.wasi)
            .expect("WasiCtx is shared and cannot be borrowed mutably")
            .get_mut()
            .expect("WasiCtx mutex must never fail")
    }
}

fn dynamic_import(
    name: &str,
    engine: &Engine,
    root: &mut LinkerInstance<Host>,
    inst: &ComponentInstance,
) -> anyhow::Result<()> {
    if name.starts_with("wasi:cli")
        || name.starts_with("wasi:clocks")
        || name.starts_with("wasi:filesystem")
        || name.starts_with("wasi:io")
        || name.starts_with("wasi:random")
        || name.starts_with("wasi:sockets")
    {
        // These does not have to be mocked, we allow them through wasmtime-wasi
        Ok(())
    } else {
        let mut instance = root.instance(name)?;
        let mut functions = Vec::new();

        for (inner_name, inner_item) in inst.exports(engine) {
            let name = name.to_owned();
            let inner_name = inner_name.to_owned();

            match inner_item {
                ComponentItem::ComponentFunc(fun) => {
                    let param_types: Vec<Type> = fun.params().map(|(_, t)| t).collect();
                    let result_types: Vec<Type> = fun.results().collect();

                    let function_name = ParsedFunctionName::parse(format!(
                        "{name}.{{{inner_name}}}"
                    ))
                        .map_err(|err| anyhow!(format!("Unexpected linking error: {name}.{{{inner_name}}} is not a valid function name: {err}")))?;

                    functions.push(FunctionInfo {
                        name: function_name,
                        params: param_types,
                        results: result_types,
                    });
                }
                ComponentItem::CoreFunc(_) => {}
                ComponentItem::Module(_) => {}
                ComponentItem::Component(_) => {}
                ComponentItem::ComponentInstance(_) => {}
                ComponentItem::Type(_) => {}
                ComponentItem::Resource(_resource) => {
                    if &inner_name != "pollable"
                        && inner_name != "wasi-io-pollable"
                        && &inner_name != "input-stream"
                        && &inner_name != "output-stream"
                        && &inner_name != "incoming-value-async-body"
                        && &inner_name != "outgoing-value-body-async"
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
            instance.func_new_async(
                &function.name.function.function_name(),
                move |_store, _params, _results| {
                    let function_name = function.name.clone();
                    Box::new(async move {
                        error!(
                            "External function called in get-agent-definitions: {function_name}",
                        );
                        Err(anyhow!(
                            "External function called in get-agent-definitions: {function_name}"
                        ))
                    })
                },
            )?;
        }

        Ok(())
    }
}

#[allow(unused)]
struct MethodInfo {
    method_name: String,
    params: Vec<Type>,
    results: Vec<Type>,
}

#[allow(unused)]
struct FunctionInfo {
    name: ParsedFunctionName,
    params: Vec<Type>,
    results: Vec<Type>,
}

struct ResourceEntry;

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::assert;
    use std::path::PathBuf;
    use std::str::FromStr;
    use test_r::test;

    #[test]
    async fn can_extract_agent_types_from_component_with_dynamic_rpc() -> anyhow::Result<()> {
        let result = extract_agent_types(
            &PathBuf::from_str("../test-components/caller.wasm")?,
            false,
            false,
        )
        .await;
        assert!(let Ok(_) = result);
        Ok(())
    }
}
