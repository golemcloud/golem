use crate::durable_host::wasm_rpc::{UrnExtensions, WasmRpcEntryPayload};
use crate::durable_host::DurableWorkerCtx;
use crate::services::rpc::RpcDemand;
use crate::workerctx::{DynamicLinking, WorkerCtx};
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::OwnedWorkerId;
use golem_wasm_rpc::wasmtime::{decode_param, encode_output};
use golem_wasm_rpc::{HostWasmRpc, Uri, Value, WasmRpcEntry, WitValue};
use rib::{ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite};
use tracing::debug;
use wasmtime::component::types::ComponentItem;
use wasmtime::component::{Component, Linker, Resource, ResourceType, Type, Val};
use wasmtime::{AsContextMut, Engine, StoreContextMut};
use wasmtime_wasi::WasiView;

// TODO: support multiple different dynamic linkers

#[async_trait]
impl<Ctx: WorkerCtx + HostWasmRpc> DynamicLinking<Ctx> for DurableWorkerCtx<Ctx> {
    fn link(
        &mut self,
        engine: &Engine,
        linker: &mut Linker<Ctx>,
        component: &Component,
    ) -> anyhow::Result<()> {
        let mut root = linker.root();

        for (name, item) in component.component_type().imports(&engine) {
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
                    if name == "auction:auction-stub/stub-auction"
                        || name == "auction:auction/api"
                        || name == "rpc:counters-stub/stub-counters"
                        || name == "rpc:counters/api"
                        || name == "rpc:ephemeral-stub/stub-ephemeral"
                    {
                        debug!("NAME == {name}");
                        let mut instance = root.instance(name)?;

                        for (ename, eitem) in inst.exports(&engine) {
                            let name = name.to_owned();
                            let ename = ename.to_owned();
                            debug!("Instance {name} export {ename}: {eitem:?}");

                            match eitem {
                                ComponentItem::ComponentFunc(fun) => {
                                    let name2 = name.clone();
                                    let ename2 = ename.clone();
                                    instance.func_new_async(
                                        // TODO: instrument async closure
                                        &ename.clone(),
                                        move |store, params, results| {
                                            let name = name2.clone();
                                            let ename = ename2.clone();
                                            let param_types: Vec<Type> = fun.params().collect();
                                            let result_types: Vec<Type> = fun.results().collect();
                                            Box::new(async move {
                                                Ctx::dynamic_function_call(
                                                    store,
                                                    &name,
                                                    &ename,
                                                    params,
                                                    &param_types,
                                                    results,
                                                    &result_types,
                                                )
                                                .await?;
                                                // TODO: failures here must be somehow handled
                                                Ok(())
                                            })
                                        },
                                    )?;
                                    debug!("LINKED {name} export {ename}");
                                }
                                ComponentItem::CoreFunc(_) => {}
                                ComponentItem::Module(_) => {}
                                ComponentItem::Component(component) => {
                                    debug!("MUST LINK COMPONENT {ename} {component:?}");
                                }
                                ComponentItem::ComponentInstance(instance) => {
                                    debug!("MUST LINK COMPONENT INSTANCE {ename} {instance:?}");
                                }
                                ComponentItem::Type(_) => {}
                                ComponentItem::Resource(resource) => {
                                    if ename != "pollable" {
                                        // TODO: ?? this should be 'if it is not already linked' but not way to check that
                                        debug!("LINKING RESOURCE {ename} {resource:?}");
                                        instance.resource(
                                            &ename,
                                            ResourceType::host::<WasmRpcEntry>(),
                                            Ctx::drop_linked_resource,
                                        )?;
                                    }
                                }
                            }
                        }
                    } else {
                        debug!("NAME NOT MATCHING: {name}");
                    }
                }
                ComponentItem::Type(_) => {}
                ComponentItem::Resource(_) => {}
            }
        }

        Ok(())
    }

    async fn dynamic_function_call(
        mut store: impl AsContextMut<Data = Ctx> + Send,
        interface_name: &str,
        function_name: &str,
        params: &[Val],
        param_types: &[Type],
        results: &mut [Val],
        result_types: &[Type],
    ) -> anyhow::Result<()> {
        let mut store = store.as_context_mut();
        debug!(
            "Instance {interface_name} export {function_name} called XXX {} params {} results",
            params.len(),
            results.len()
        );

        // TODO: add an enum with the call types (interface stub constructor, resource stub constructor, etc)
        // TODO: and detect which one it is based on metadata + type info

        if (interface_name == "auction:auction-stub/stub-auction"
            && function_name == "[constructor]api")
            || (interface_name == "rpc:counters-stub/stub-counters"
                && function_name == "[constructor]api")
        {
            // Simple stub interface constructor

            let target_worker_urn = params[0].clone();
            debug!("CREATING AUCTION STUB TARGETING WORKER {target_worker_urn:?}");
            // Record([("value", String("urn:worker:2a174805-bdd5-49e1-b1e8-124208123b4a/auction-5f0a94f1-1d14-4b65-8e6c-3a8fa3c24ea9"))])

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
        } else if (interface_name == "auction:auction-stub/stub-auction"
            && function_name == "[constructor]running-auction")
            || (interface_name == "rpc:counters-stub/stub-counters"
                && function_name == "[constructor]counter")
        {
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

            let constructor_result = Self::remote_invoke(
                &interface_name,
                &function_name,
                params,
                param_types,
                &mut store,
                handle,
            )
            .await?;

            // TODO: extract and clean up
            let (resource_uri, resource_id) = if let Value::Tuple(values) = constructor_result {
                if values.len() == 1 {
                    if let Value::Handle { uri, resource_id } = values.into_iter().next().unwrap() {
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
        } else if function_name.starts_with("[method]") {
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

            let result = Self::remote_invoke(
                &interface_name,
                &function_name,
                params,
                param_types,
                &mut store,
                handle,
            )
            .await?;
            Self::value_result_to_wasmtime_vals(result, results, result_types, &mut store).await?;
        }

        Ok(())
    }

    fn drop_linked_resource(mut store: StoreContextMut<'_, Ctx>, rep: u32) -> anyhow::Result<()> {
        let mut wasi = store.data_mut().as_wasi_view();
        let table = wasi.table();
        let entry: &WasmRpcEntry = table.get_any_mut(rep).unwrap().downcast_ref().unwrap(); // TODO: error handling
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        debug!("DROPPING RESOURCE {payload:?}");
        if let WasmRpcEntryPayload::Resource { .. } = payload {
            // TODO: remote drop
        }
        Ok(())
    }
}

// TODO: these helpers probably should not be directly living in DurableWorkerCtx
impl<Ctx: WorkerCtx + HostWasmRpc> DurableWorkerCtx<Ctx> {
    async fn remote_invoke(
        interface_name: &&str,
        function_name: &&str,
        params: &[Val],
        param_types: &[Type],
        store: &mut StoreContextMut<'_, Ctx>,
        handle: Resource<WasmRpcEntry>,
    ) -> anyhow::Result<Value> {
        let stub_function_name =
            ParsedFunctionName::parse(&format!("{interface_name}.{{{function_name}}}"))
                .map_err(|err| anyhow!(err))?; // TODO: proper error
        debug!("STUB FUNCTION NAME: {stub_function_name:?}");
        let target_function_name = ParsedFunctionName {
            site: if interface_name.starts_with("auction") {
                ParsedFunctionSite::PackagedInterface {
                    // TODO: this must come from component metadata linking information
                    namespace: "auction".to_string(),
                    package: "auction".to_string(),
                    interface: "api".to_string(),
                    version: None,
                }
            } else {
                ParsedFunctionSite::PackagedInterface {
                    namespace: "rpc".to_string(),
                    package: "counters".to_string(),
                    interface: "api".to_string(),
                    version: None,
                }
            },
            function: if let Some(resource) = stub_function_name.is_constructor() {
                ParsedFunctionReference::RawResourceConstructor {
                    resource: resource.to_string(),
                }
            } else {
                match &stub_function_name.function {
                    ParsedFunctionReference::RawResourceMethod { resource, method }
                        if resource == "counter" =>
                    {
                        ParsedFunctionReference::RawResourceMethod {
                            resource: resource.to_string(),
                            method: method
                                .strip_prefix("blocking-") // TODO: we also have to support the non-blocking variants
                                .unwrap()
                                .to_string(),
                        }
                    }
                    _ => ParsedFunctionReference::Function {
                        function: stub_function_name
                            .function
                            .resource_method_name()
                            .unwrap() // TODO: proper error
                            .strip_prefix("blocking-") // TODO: we also have to support the non-blocking variants
                            .unwrap()
                            .to_string(),
                    },
                }
            },
        };

        let mut wit_value_params = Vec::new();
        for (param, typ) in params.iter().zip(param_types).skip(1) {
            let value: Value = encode_output(param, typ, store.data_mut())
                .await
                .map_err(|err| anyhow!(format!("{err:?}")))?; // TODO: proper error
            let wit_value: WitValue = value.into();
            wit_value_params.push(wit_value);
        }

        debug!(
                "CALLING {function_name} as {target_function_name} with parameters {wit_value_params:?}",
            );

        // "auction:auction/api.{initialize}",
        let wit_value_result = store
            .data_mut()
            .invoke_and_await(handle, target_function_name.to_string(), wit_value_params)
            .await??;

        debug!("CALLING {function_name} RESULTED IN {:?}", wit_value_result);

        let value_result: Value = wit_value_result.into();
        Ok(value_result)
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
                    let result = decode_param(&value, &typ, store.data_mut())
                        .await
                        .map_err(|err| anyhow!(format!("{err:?}")))?; // TODO: proper error
                    results[idx] = result.val;
                    // TODO: do we have to do something with result.resources_to_drop here?
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
                        match val {
                            Val::String(s) => {
                                target = Some(s.clone());
                            }
                            _ => {}
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
