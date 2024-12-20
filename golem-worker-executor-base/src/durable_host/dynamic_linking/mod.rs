use crate::durable_host::wasm_rpc::{UrnExtensions, WasmRpcEntryPayload};
use crate::durable_host::DurableWorkerCtx;
use crate::services::component::ComponentMetadata;
use crate::services::rpc::RpcDemand;
use crate::workerctx::{DynamicLinking, WorkerCtx};
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::component_metadata::{DynamicLinkedInstance, DynamicLinkedWasmRpc};
use golem_common::model::OwnedWorkerId;
use golem_wasm_rpc::golem::rpc::types::{FutureInvokeResult, HostFutureInvokeResult};
use golem_wasm_rpc::wasmtime::{decode_param, encode_output};
use golem_wasm_rpc::{HostWasmRpc, Uri, Value, WasmRpcEntry, WitValue};
use itertools::Itertools;
use rib::{ParsedFunctionName, ParsedFunctionReference};
use std::collections::HashMap;
use tracing::debug;
use wasmtime::component::types::{ComponentItem, Field};
use wasmtime::component::{Component, Linker, Resource, ResourceType, Type, Val};
use wasmtime::{AsContextMut, Engine, StoreContextMut};
use wasmtime_wasi::WasiView;
// TODO: support multiple different dynamic linkers

#[async_trait]
impl<Ctx: WorkerCtx + HostWasmRpc + HostFutureInvokeResult> DynamicLinking<Ctx>
    for DurableWorkerCtx<Ctx>
{
    fn link(
        &mut self,
        engine: &Engine,
        linker: &mut Linker<Ctx>,
        component: &Component,
        component_metadata: &ComponentMetadata,
    ) -> anyhow::Result<()> {
        let mut root = linker.root();

        // TODO >
        let mut component_metadata = component_metadata.clone();
        component_metadata.dynamic_linking.insert(
            "auction:auction-stub/stub-auction".to_string(),
            DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                target_interface_name: "auction:auction/api".to_string(),
            }),
        );
        component_metadata.dynamic_linking.insert(
            "rpc:counters-stub/stub-counters".to_string(),
            DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                target_interface_name: "rpc:counters/api".to_string(),
            }),
        );
        component_metadata.dynamic_linking.insert(
            "rpc:ephemeral-stub/stub-ephemeral".to_string(),
            DynamicLinkedInstance::WasmRpc(DynamicLinkedWasmRpc {
                target_interface_name: "rpc:ephemeral/api".to_string(),
            }),
        );
        // TODO <

        let component_type = component.component_type();
        for (name, item) in component_type.imports(engine) {
            let name = name.to_string();
            debug!("Import {name}: {item:?}");
            match item {
                ComponentItem::ComponentFunc(_) => {
                    debug!("MUST LINK COMPONENT FUNC {name}");
                }
                ComponentItem::CoreFunc(_) => {
                    debug!("MUST LINK CORE FUNC {name}");
                }
                ComponentItem::Module(_) => {
                    debug!("MUST LINK MODULE {name}");
                }
                ComponentItem::Component(_) => {
                    debug!("MUST LINK COMPONENT {name}");
                }
                ComponentItem::ComponentInstance(ref inst) => {
                    match component_metadata.dynamic_linking.get(&name.to_string()) {
                        Some(DynamicLinkedInstance::WasmRpc(rpc_metadata)) => {
                            debug!("NAME == {name}");
                            let mut instance = root.instance(&name)?;
                            let mut resources: HashMap<(String, String), Vec<MethodInfo>> =
                                HashMap::new();
                            let mut functions = Vec::new();

                            for (inner_name, inner_item) in inst.exports(engine) {
                                let name = name.to_owned();
                                let inner_name = inner_name.to_owned();
                                debug!("Instance {name} export {inner_name}: {inner_item:?}");

                                match inner_item {
                                    ComponentItem::ComponentFunc(fun) => {
                                        let param_types: Vec<Type> = fun.params().collect();
                                        let result_types: Vec<Type> = fun.results().collect();

                                        let function_name = ParsedFunctionName::parse(format!(
                                            "{name}.{{{inner_name}}}"
                                        ))
                                        .map_err(|err| anyhow!(err))?; // TODO: proper error

                                        if let Some(resource_name) =
                                            function_name.function.resource_name()
                                        {
                                            let methods = resources
                                                .entry((name.clone(), resource_name.clone()))
                                                .or_default();
                                            methods.push(MethodInfo {
                                                method_name: inner_name.clone(),
                                                params: param_types.clone(),
                                                results: result_types.clone(),
                                            });
                                        }

                                        functions.push(FunctionInfo {
                                            name: function_name,
                                            params: param_types,
                                            results: result_types,
                                        });
                                    }
                                    ComponentItem::CoreFunc(_) => {}
                                    ComponentItem::Module(_) => {}
                                    ComponentItem::Component(component) => {
                                        debug!("MUST LINK COMPONENT {inner_name} {component:?}");
                                    }
                                    ComponentItem::ComponentInstance(instance) => {
                                        debug!("MUST LINK COMPONENT INSTANCE {inner_name} {instance:?}");
                                    }
                                    ComponentItem::Type(_) => {}
                                    ComponentItem::Resource(_resource) => {
                                        resources.entry((name, inner_name)).or_default();
                                    }
                                }
                            }

                            let mut resource_types = HashMap::new();
                            for ((interface_name, resource_name), methods) in resources {
                                let resource_type = DynamicRpcResource::analyse(
                                    &interface_name,
                                    &resource_name,
                                    &methods,
                                    rpc_metadata,
                                )?;

                                if let Some(resource_type) = &resource_type {
                                    resource_types.insert(
                                        (interface_name.clone(), resource_name.clone()),
                                        resource_type.clone(),
                                    );
                                }

                                match resource_type {
                                    Some(DynamicRpcResource::InvokeResult) => {
                                        debug!("LINKING FUTURE INVOKE RESULT {resource_name}");
                                        instance.resource(
                                            &resource_name,
                                            ResourceType::host::<FutureInvokeResult>(),
                                            |_store, _rep| Ok(()),
                                        )?;
                                    }
                                    Some(DynamicRpcResource::Stub)
                                    | Some(DynamicRpcResource::ResourceStub) => {
                                        debug!("LINKING RESOURCE {resource_name}");
                                        let interface_name_clone =
                                            rpc_metadata.target_interface_name.clone();
                                        let resource_name_clone = resource_name.clone();

                                        instance.resource_async(
                                            &resource_name,
                                            ResourceType::host::<WasmRpcEntry>(),
                                            move |store, rep| {
                                                let interface_name = interface_name_clone.clone();
                                                let resource_name = resource_name_clone.clone();

                                                Box::new(async move {
                                                    Self::drop_linked_resource(
                                                        store,
                                                        rep,
                                                        &interface_name,
                                                        &resource_name,
                                                    )
                                                    .await
                                                })
                                            },
                                        )?;
                                    }
                                    None => {
                                        debug!("NOT LINKING RESOURCE {resource_name}");
                                    }
                                }
                            }

                            for function in functions {
                                let call_type = DynamicRpcCall::analyse(
                                    &function.name,
                                    &function.params,
                                    &function.results,
                                    rpc_metadata,
                                    &resource_types,
                                )?;
                                if let Some(call_type) = call_type {
                                    let name2 = name.clone();
                                    let inner_name2 = function.name.function.function_name();
                                    instance.func_new_async(
                                        // TODO: instrument async closure
                                        &function.name.function.function_name(),
                                        move |store, params, results| {
                                            let name = name2.clone();
                                            let inner_name = inner_name2.clone();
                                            let param_types = function.params.clone();
                                            let result_types = function.results.clone();
                                            let call_type = call_type.clone();
                                            Box::new(async move {
                                                Self::dynamic_function_call(
                                                    store,
                                                    &name,
                                                    &inner_name,
                                                    params,
                                                    &param_types,
                                                    results,
                                                    &result_types,
                                                    &call_type,
                                                )
                                                .await?;
                                                // TODO: failures here must be somehow handled
                                                Ok(())
                                            })
                                        },
                                    )?;
                                    debug!("LINKED {name} export {}", function.name);
                                } else {
                                    debug!("NO CALL TYPE FOR {name} export {}", function.name);
                                }
                            }
                        }
                        None => {
                            debug!("NO DYNAMIC LINKING INFORMATION FOR {name}");
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

// TODO: these helpers probably should not be directly living in DurableWorkerCtx
impl<Ctx: WorkerCtx + HostWasmRpc + HostFutureInvokeResult> DurableWorkerCtx<Ctx> {
    async fn dynamic_function_call(
        mut store: impl AsContextMut<Data = Ctx> + Send,
        interface_name: &str,
        function_name: &str,
        params: &[Val],
        param_types: &[Type],
        results: &mut [Val],
        result_types: &[Type],
        call_type: &DynamicRpcCall,
    ) -> anyhow::Result<()> {
        let mut store = store.as_context_mut();
        debug!(
            "Instance {interface_name} export {function_name} called XXX {} params {} results",
            params.len(),
            results.len()
        );

        match call_type {
            DynamicRpcCall::GlobalStubConstructor => {
                // Simple stub interface constructor

                let target_worker_urn = params[0].clone();
                let (remote_worker_id, demand) =
                    Self::create_rpc_target(&mut store, target_worker_urn).await?;

                let handle = {
                    let mut wasi = store.data_mut().as_wasi_view();
                    let table = wasi.table();
                    table.push(WasmRpcEntry {
                        payload: Box::new(WasmRpcEntryPayload::Interface {
                            demand,
                            remote_worker_id,
                        }),
                    })?
                };
                results[0] = Val::Resource(handle.try_into_resource_any(store)?);
            }
            DynamicRpcCall::ResourceStubConstructor {
                stub_constructor_name,
                target_constructor_name,
            } => {
                // Resource stub constructor

                // First parameter is the target uri
                // Rest of the parameters must be sent to the remote constructor

                let target_worker_urn = params[0].clone();
                let (remote_worker_id, demand) =
                    Self::create_rpc_target(&mut store, target_worker_urn.clone()).await?;

                // First creating a resource for invoking the constructor (to avoid having to make a special case)
                let handle = {
                    let mut wasi = store.data_mut().as_wasi_view();
                    let table = wasi.table();
                    table.push(WasmRpcEntry {
                        payload: Box::new(WasmRpcEntryPayload::Interface {
                            demand,
                            remote_worker_id,
                        }),
                    })?
                };
                let temp_handle = handle.rep();

                let constructor_result = Self::remote_invoke_and_wait(
                    stub_constructor_name,
                    target_constructor_name,
                    params,
                    param_types,
                    &mut store,
                    handle,
                )
                .await?;

                // TODO: extract and clean up
                let (resource_uri, resource_id) = if let Value::Tuple(values) = constructor_result {
                    if values.len() == 1 {
                        if let Value::Handle { uri, resource_id } =
                            values.into_iter().next().unwrap()
                        {
                            (Uri { value: uri }, resource_id)
                        } else {
                            return Err(anyhow!(
                                "Invalid constructor result: single handle expected"
                            ));
                        }
                    } else {
                        return Err(anyhow!(
                            "Invalid constructor result: single handle expected"
                        ));
                    }
                } else {
                    return Err(anyhow!(
                        "Invalid constructor result: single handle expected"
                    ));
                };

                let (remote_worker_id, demand) =
                    Self::create_rpc_target(&mut store, target_worker_urn).await?;

                let handle = {
                    let mut wasi = store.data_mut().as_wasi_view();
                    let table = wasi.table();

                    let temp_handle: Resource<WasmRpcEntry> = Resource::new_own(temp_handle);
                    table.delete(temp_handle)?; // Removing the temporary handle

                    table.push(WasmRpcEntry {
                        payload: Box::new(WasmRpcEntryPayload::Resource {
                            demand,
                            remote_worker_id,
                            resource_uri,
                            resource_id,
                        }),
                    })?
                };
                results[0] = Val::Resource(handle.try_into_resource_any(store)?);
            }
            DynamicRpcCall::BlockingFunctionCall {
                stub_function_name,
                target_function_name,
            } => {
                // Simple stub interface method
                debug!(
                    "{function_name} handle={:?}, rest={:?}",
                    params[0],
                    params.iter().skip(1).collect::<Vec<_>>()
                );

                let handle = match params[0] {
                    Val::Resource(handle) => handle,
                    _ => return Err(anyhow!("Invalid handle parameter")),
                };
                let handle: Resource<WasmRpcEntry> = handle.try_into_resource(&mut store)?;
                {
                    let mut wasi = store.data_mut().as_wasi_view();
                    let entry = wasi.table().get(&handle)?;
                    let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
                    debug!("CALLING {function_name} ON {}", payload.remote_worker_id());
                }

                let result = Self::remote_invoke_and_wait(
                    stub_function_name,
                    target_function_name,
                    params,
                    param_types,
                    &mut store,
                    handle,
                )
                .await?;
                Self::value_result_to_wasmtime_vals(result, results, result_types, &mut store)
                    .await?;
            }
            DynamicRpcCall::FireAndForgetFunctionCall {
                stub_function_name,
                target_function_name,
            } => {
                // Fire-and-forget stub interface method
                debug!(
                    "FNF {function_name} handle={:?}, rest={:?}",
                    params[0],
                    params.iter().skip(1).collect::<Vec<_>>()
                );

                let handle = match params[0] {
                    Val::Resource(handle) => handle,
                    _ => return Err(anyhow!("Invalid handle parameter")),
                };
                let handle: Resource<WasmRpcEntry> = handle.try_into_resource(&mut store)?;
                {
                    let mut wasi = store.data_mut().as_wasi_view();
                    let entry = wasi.table().get(&handle)?;
                    let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
                    debug!("CALLING {function_name} ON {}", payload.remote_worker_id());
                }

                Self::remote_invoke(
                    stub_function_name,
                    target_function_name,
                    params,
                    param_types,
                    &mut store,
                    handle,
                )
                .await?;
            }
            DynamicRpcCall::AsyncFunctionCall {
                stub_function_name,
                target_function_name,
            } => {
                // Async stub interface method
                debug!(
                    "ASYNC {function_name} handle={:?}, rest={:?}",
                    params[0],
                    params.iter().skip(1).collect::<Vec<_>>()
                );

                let handle = match params[0] {
                    Val::Resource(handle) => handle,
                    _ => return Err(anyhow!("Invalid handle parameter")),
                };
                let handle: Resource<WasmRpcEntry> = handle.try_into_resource(&mut store)?;
                {
                    let mut wasi = store.data_mut().as_wasi_view();
                    let entry = wasi.table().get(&handle)?;
                    let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
                    debug!("CALLING {function_name} ON {}", payload.remote_worker_id());
                }

                let result = Self::remote_async_invoke_and_await(
                    stub_function_name,
                    target_function_name,
                    params,
                    param_types,
                    &mut store,
                    handle,
                )
                .await?;

                Self::value_result_to_wasmtime_vals(result, results, result_types, &mut store)
                    .await?;
            }
            DynamicRpcCall::FutureInvokeResultSubscribe => {
                let handle = match params[0] {
                    Val::Resource(handle) => handle,
                    _ => return Err(anyhow!("Invalid handle parameter")),
                };
                let handle: Resource<FutureInvokeResult> = handle.try_into_resource(&mut store)?;
                let pollable = store.data_mut().subscribe(handle).await?;
                let pollable_any = pollable.try_into_resource_any(&mut store)?;
                let resource_id = store.data_mut().add(pollable_any).await;

                let value_result = Value::Tuple(vec![Value::Handle {
                    uri: store.data().self_uri().value,
                    resource_id,
                }]);
                Self::value_result_to_wasmtime_vals(
                    value_result,
                    results,
                    result_types,
                    &mut store,
                )
                .await?;
            }
            DynamicRpcCall::FutureInvokeResultGet => {
                let handle = match params[0] {
                    Val::Resource(handle) => handle,
                    _ => return Err(anyhow!("Invalid handle parameter")),
                };
                let handle: Resource<FutureInvokeResult> = handle.try_into_resource(&mut store)?;
                let result = HostFutureInvokeResult::get(store.data_mut(), handle).await?;

                // NOTE: we are currently failing on RpcError instead of passing it to the caller, as the generated stub interface requires
                let value_result = Value::Tuple(vec![match result {
                    None => Value::Option(None),
                    Some(Ok(value)) => {
                        let value: Value = value.into();
                        match value {
                            Value::Tuple(items) if items.len() == 1 => {
                                Value::Option(Some(Box::new(items.into_iter().next().unwrap())))
                            }
                            _ => Err(anyhow!("Invalid future invoke result value"))?, // TODO: better error
                        }
                    }
                    Some(Err(err)) => Err(anyhow!("RPC invocation failed with {err:?}"))?, // TODO: more information into the error
                }]);

                Self::value_result_to_wasmtime_vals(
                    value_result,
                    results,
                    result_types,
                    &mut store,
                )
                .await?;
            }
        }

        Ok(())
    }

    async fn drop_linked_resource(
        mut store: StoreContextMut<'_, Ctx>,
        rep: u32,
        interface_name: &str,
        resource_name: &str,
    ) -> anyhow::Result<()> {
        let must_drop = {
            let mut wasi = store.data_mut().as_wasi_view();
            let table = wasi.table();
            let entry: &WasmRpcEntry = table.get_any_mut(rep)?.downcast_ref().unwrap(); // TODO: error handling
            let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();

            debug!("DROPPING RESOURCE {payload:?}");

            matches!(payload, WasmRpcEntryPayload::Resource { .. })
        };
        if must_drop {
            let resource: Resource<WasmRpcEntry> = Resource::new_own(rep);

            let function_name = format!("{interface_name}.{{{resource_name}.drop}}");
            let _ = store
                .data_mut()
                .invoke_and_await(resource, function_name, vec![])
                .await?;
        }
        Ok(())
    }

    // TODO: stub_function_name can probably be removed
    async fn remote_invoke_and_wait(
        stub_function_name: &ParsedFunctionName,
        target_function_name: &ParsedFunctionName,
        params: &[Val],
        param_types: &[Type],
        store: &mut StoreContextMut<'_, Ctx>,
        handle: Resource<WasmRpcEntry>,
    ) -> anyhow::Result<Value> {
        let mut wit_value_params = Vec::new();
        for (param, typ) in params.iter().zip(param_types).skip(1) {
            let value: Value = encode_output(param, typ, store.data_mut())
                .await
                .map_err(|err| anyhow!(format!("{err:?}")))?; // TODO: proper error
            let wit_value: WitValue = value.into();
            wit_value_params.push(wit_value);
        }

        debug!(
                "CALLING {stub_function_name} as {target_function_name} with parameters {wit_value_params:?}",
            );

        let wit_value_result = store
            .data_mut()
            .invoke_and_await(handle, target_function_name.to_string(), wit_value_params)
            .await??;

        debug!(
            "CALLING {stub_function_name} RESULTED IN {:?}",
            wit_value_result
        );

        let value_result: Value = wit_value_result.into();
        Ok(value_result)
    }

    // TODO: stub_function_name can probably be removed
    async fn remote_async_invoke_and_await(
        stub_function_name: &ParsedFunctionName,
        target_function_name: &ParsedFunctionName,
        params: &[Val],
        param_types: &[Type],
        store: &mut StoreContextMut<'_, Ctx>,
        handle: Resource<WasmRpcEntry>,
    ) -> anyhow::Result<Value> {
        let mut wit_value_params = Vec::new();
        for (param, typ) in params.iter().zip(param_types).skip(1) {
            let value: Value = encode_output(param, typ, store.data_mut())
                .await
                .map_err(|err| anyhow!(format!("{err:?}")))?; // TODO: proper error
            let wit_value: WitValue = value.into();
            wit_value_params.push(wit_value);
        }

        debug!(
                "CALLING {stub_function_name} as {target_function_name} with parameters {wit_value_params:?}",
            );

        let invoke_result_resource = store
            .data_mut()
            .async_invoke_and_await(handle, target_function_name.to_string(), wit_value_params)
            .await?;

        let invoke_result_resource_any =
            invoke_result_resource.try_into_resource_any(&mut *store)?;
        let resource_id = store.data_mut().add(invoke_result_resource_any).await;

        let value_result: Value = Value::Tuple(vec![Value::Handle {
            uri: store.data().self_uri().value,
            resource_id,
        }]);
        Ok(value_result)
    }

    // TODO: stub_function_name can probably be removed
    async fn remote_invoke(
        stub_function_name: &ParsedFunctionName,
        target_function_name: &ParsedFunctionName,
        params: &[Val],
        param_types: &[Type],
        store: &mut StoreContextMut<'_, Ctx>,
        handle: Resource<WasmRpcEntry>,
    ) -> anyhow::Result<()> {
        let mut wit_value_params = Vec::new();
        for (param, typ) in params.iter().zip(param_types).skip(1) {
            let value: Value = encode_output(param, typ, store.data_mut())
                .await
                .map_err(|err| anyhow!(format!("{err:?}")))?; // TODO: proper error
            let wit_value: WitValue = value.into();
            wit_value_params.push(wit_value);
        }

        debug!(
                "CALLING {stub_function_name} as {target_function_name} with parameters {wit_value_params:?}",
            );

        store
            .data_mut()
            .invoke(handle, target_function_name.to_string(), wit_value_params)
            .await??;

        Ok(())
    }

    async fn value_result_to_wasmtime_vals(
        value_result: Value,
        results: &mut [Val],
        result_types: &[Type],
        store: &mut StoreContextMut<'_, Ctx>,
    ) -> anyhow::Result<()> {
        match value_result {
            Value::Tuple(values) | Value::Record(values) => {
                for (idx, (value, typ)) in values.iter().zip(result_types).enumerate() {
                    let result = decode_param(value, typ, store.data_mut())
                        .await
                        .map_err(|err| anyhow!(format!("{err:?}")))?; // TODO: proper error
                    results[idx] = result.val;
                }
            }
            _ => {
                return Err(anyhow!(
                    "Unexpected result value {value_result:?}, expected tuple or record"
                ));
            }
        }

        Ok(())
    }
}

// TODO: these helpers probably should not be directly living in DurableWorkerCtx
impl<Ctx: WorkerCtx + HostWasmRpc> DurableWorkerCtx<Ctx> {
    async fn create_rpc_target(
        store: &mut StoreContextMut<'_, Ctx>,
        target_worker_urn: Val,
    ) -> anyhow::Result<(OwnedWorkerId, Box<dyn RpcDemand>)> {
        let worker_urn = match target_worker_urn {
            Val::Record(ref record) => {
                let mut target = None;
                for (key, val) in record.iter() {
                    if key == "value" {
                        if let Val::String(s) = val {
                            target = Some(s.clone());
                        }
                    }
                }
                target
            }
            _ => None,
        };

        let (remote_worker_id, demand) = if let Some(location) = worker_urn {
            let uri = Uri {
                value: location.clone(),
            };
            match uri.parse_as_golem_urn() {
                Some((remote_worker_id, None)) => {
                    let remote_worker_id = store
                        .data_mut()
                        .generate_unique_local_worker_id(remote_worker_id)
                        .await?;

                    let remote_worker_id = OwnedWorkerId::new(
                        &store.data().owned_worker_id().account_id,
                        &remote_worker_id,
                    );
                    let demand = store.data().rpc().create_demand(&remote_worker_id).await;
                    (remote_worker_id, demand)
                }
                _ => {
                    return Err(anyhow!(
                        "Invalid URI: {}. Must be urn:worker:component-id/worker-name",
                        location
                    ))
                }
            }
        } else {
            return Err(anyhow!("Missing or invalid worker URN parameter")); // TODO: more details;
        };
        Ok((remote_worker_id, demand))
    }
}

#[derive(Clone)]
enum DynamicRpcCall {
    GlobalStubConstructor,
    ResourceStubConstructor {
        stub_constructor_name: ParsedFunctionName,
        target_constructor_name: ParsedFunctionName,
    },
    BlockingFunctionCall {
        stub_function_name: ParsedFunctionName,
        target_function_name: ParsedFunctionName,
    },
    FireAndForgetFunctionCall {
        stub_function_name: ParsedFunctionName,
        target_function_name: ParsedFunctionName,
    },
    AsyncFunctionCall {
        stub_function_name: ParsedFunctionName,
        target_function_name: ParsedFunctionName,
    },
    FutureInvokeResultSubscribe,
    FutureInvokeResultGet,
}

impl DynamicRpcCall {
    pub fn analyse(
        stub_name: &ParsedFunctionName,
        _param_types: &[Type],
        result_types: &[Type],
        rpc_metadata: &DynamicLinkedWasmRpc,
        resource_types: &HashMap<(String, String), DynamicRpcResource>,
    ) -> anyhow::Result<Option<DynamicRpcCall>> {
        if let Some(resource_name) = stub_name.is_constructor() {
            match resource_types.get(&(
                stub_name.site.interface_name().unwrap_or_default(),
                resource_name.to_string(),
            )) {
                Some(DynamicRpcResource::Stub) => Ok(Some(DynamicRpcCall::GlobalStubConstructor)),
                Some(DynamicRpcResource::ResourceStub) => {
                    let target_constructor_name = ParsedFunctionName {
                        site: rpc_metadata.target_site().map_err(|err| anyhow!(err))?, // TODO: proper error
                        function: ParsedFunctionReference::RawResourceConstructor {
                            resource: resource_name.to_string(),
                        },
                    };

                    Ok(Some(DynamicRpcCall::ResourceStubConstructor {
                        stub_constructor_name: stub_name.clone(),
                        target_constructor_name,
                    }))
                }
                _ => Ok(None),
            }
        } else if let Some(resource_name) = stub_name.is_method() {
            match resource_types.get(&(
                stub_name.site.interface_name().unwrap_or_default(),
                resource_name.to_string(),
            )) {
                Some(DynamicRpcResource::InvokeResult) => {
                    if stub_name.function.resource_method_name() == Some("subscribe".to_string()) {
                        Ok(Some(DynamicRpcCall::FutureInvokeResultSubscribe))
                    } else if stub_name.function.resource_method_name() == Some("get".to_string()) {
                        Ok(Some(DynamicRpcCall::FutureInvokeResultGet))
                    } else {
                        Ok(None)
                    }
                }
                Some(stub) => {
                    let method_name = stub_name.function.resource_method_name().unwrap(); // safe because of stub_name.is_method()
                    let blocking = method_name.starts_with("blocking-");
                    let target_method_name = if blocking {
                        method_name
                            .strip_prefix("blocking-")
                            .unwrap_or(&method_name)
                    } else {
                        &method_name
                    };
                    let target_function = match stub {
                        DynamicRpcResource::Stub => ParsedFunctionReference::Function {
                            function: target_method_name.to_string(),
                        },
                        _ => ParsedFunctionReference::RawResourceMethod {
                            resource: resource_name.to_string(),
                            method: target_method_name.to_string(),
                        },
                    };

                    let target_function_name = ParsedFunctionName {
                        site: rpc_metadata.target_site().map_err(|err| anyhow!(err))?, // TODO: proper error
                        function: target_function,
                    };

                    if blocking {
                        Ok(Some(DynamicRpcCall::BlockingFunctionCall {
                            stub_function_name: stub_name.clone(),
                            target_function_name,
                        }))
                    } else {
                        debug!("ASYNC FUNCTION RESULT TYPES: {result_types:?}");
                        if !result_types.is_empty() {
                            Ok(Some(DynamicRpcCall::AsyncFunctionCall {
                                stub_function_name: stub_name.clone(),
                                target_function_name,
                            }))
                        } else {
                            Ok(Some(DynamicRpcCall::FireAndForgetFunctionCall {
                                stub_function_name: stub_name.clone(),
                                target_function_name,
                            }))
                        }
                    }
                }
                None => Ok(None),
            }
        } else {
            // Unsupported item
            Ok(None)
        }
    }
}

#[derive(Clone)]
enum DynamicRpcResource {
    Stub,
    ResourceStub,
    InvokeResult,
}

impl DynamicRpcResource {
    pub fn analyse(
        _interface_name: &str, // TODO: remove
        resource_name: &str,
        methods: &[MethodInfo],
        rpc_metadata: &DynamicLinkedWasmRpc,
    ) -> anyhow::Result<Option<DynamicRpcResource>> {
        if resource_name == "pollable" {
            Ok(None)
        } else if Self::is_invoke_result(resource_name, methods) {
            Ok(Some(DynamicRpcResource::InvokeResult))
        } else if let Some(constructor) = methods
            .iter()
            .find_or_first(|m| m.method_name.contains("[constructor]"))
        {
            if Self::first_parameter_is_uri(&constructor.params) {
                if constructor.params.len() > 1 {
                    Ok(Some(DynamicRpcResource::ResourceStub))
                } else if rpc_metadata
                    .target_interface_name
                    .ends_with(&format!("/{resource_name}"))
                {
                    Ok(Some(DynamicRpcResource::Stub))
                } else {
                    Ok(Some(DynamicRpcResource::ResourceStub))
                }
            } else {
                // First constructor parameter is not an Uri => not a stub
                Ok(None)
            }
        } else {
            // No constructor => not a stub
            Ok(None)
        }
    }

    fn is_invoke_result(resource_name: &str, methods: &[MethodInfo]) -> bool {
        resource_name.starts_with("future-")
            && resource_name.ends_with("-result")
            && methods
                .iter()
                .filter_map(|m| m.method_name.split('.').last().map(|s| s.to_string()))
                .sorted()
                .collect::<Vec<_>>()
                == vec!["get".to_string(), "subscribe".to_string()]
            && {
                let subscribe = methods
                    .iter()
                    .find(|m| m.method_name.ends_with(".subscribe"))
                    .unwrap();
                subscribe.params.len() == 1
                    && matches!(subscribe.params[0], Type::Borrow(_))
                    && subscribe.results.len() == 1
                    && matches!(subscribe.results[0], Type::Own(_))
            }
    }

    fn first_parameter_is_uri(param_types: &[Type]) -> bool {
        if let Some(Type::Record(record)) = param_types.first() {
            let fields: Vec<Field> = record.fields().collect();
            fields.len() == 1 && matches!(fields[0].ty, Type::String) && fields[0].name == "value"
        } else {
            false
        }
    }
}

struct MethodInfo {
    method_name: String,
    params: Vec<Type>,
    results: Vec<Type>,
}

struct FunctionInfo {
    name: ParsedFunctionName,
    params: Vec<Type>,
    results: Vec<Type>,
}
