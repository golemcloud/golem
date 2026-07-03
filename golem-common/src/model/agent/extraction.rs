// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::model::agent::AgentError;
use crate::model::parsed_function_name::ParsedFunctionName;
use crate::schema::agent::AgentTypeSchema;
use crate::schema::agent::wit::{decode_agent_error_rejecting_quota_with, decode_agent_type, wire};
use crate::schema::tool::Tool;
use crate::schema::tool::validation::validate_tool;
use crate::schema::tool::wit::{decode_tool, wire as tool_wire};
use anyhow::anyhow;
use golem_schema::schema::wit::{
    QuotaTokenHandleDropper, QuotaTokenHandleRep, SecretHandleDropper, SecretHandleRep,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, trace};
use wasmtime::component::types::{ComponentInstance, ComponentItem};
use wasmtime::component::{
    Component, Func, Instance, Linker, LinkerInstance, ResourceTable, ResourceType, Type,
};
use wasmtime::{AsContextMut, Engine, Store};
use wasmtime_wasi::cli::StdoutStream;
use wasmtime_wasi::p2::pipe;
use wasmtime_wasi::{IoCtx, IoData, IoView, WasiCtx, WasiCtxView, WasiView};

const AGENT_INTERFACE_NAME: &str = "golem:agent/guest@2.0.0";
const AGENT_FUNCTION_NAME: &str = "discover-agent-types";
const TOOL_INTERFACE_NAME: &str = "golem:tool/guest@0.1.0";
const TOOL_FUNCTION_NAME: &str = "discover-tools";

/// Metadata discovered from a WASM component in a single instantiation:
/// the agent types returned by `golem:agent/guest.discover-agent-types` and
/// the tools returned by `golem:tool/guest.discover-tools`. Either list is
/// empty when the component does not export the corresponding interface.
///
/// Serializes as `{"agentTypes": [...], "tools": [...]}`. Deserialization also
/// accepts a bare agent type array (with an empty tool list), the format
/// extracted-metadata JSON files used before tools were bundled into the
/// extraction.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedComponentMetadata {
    pub agent_types: Vec<AgentTypeSchema>,
    pub tools: Vec<Tool>,
}

impl<'de> Deserialize<'de> for ExtractedComponentMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Metadata {
            agent_types: Vec<AgentTypeSchema>,
            #[serde(default)]
            tools: Vec<Tool>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Metadata(Metadata),
            AgentTypes(Vec<AgentTypeSchema>),
        }

        Ok(match Repr::deserialize(deserializer)? {
            Repr::Metadata(metadata) => ExtractedComponentMetadata {
                agent_types: metadata.agent_types,
                tools: metadata.tools,
            },
            Repr::AgentTypes(agent_types) => ExtractedComponentMetadata {
                agent_types,
                tools: Vec::new(),
            },
        })
    }
}

/// Extracts the agent types and tools implemented by the given WASM component
/// using a single component instantiation.
///
/// Agent types come from `golem:agent/guest.discover-agent-types`; if the
/// component does not export that interface the extraction either fails
/// (`fail_on_missing_discover_method`) or yields an empty agent type list.
/// Tools come from `golem:tool/guest.discover-tools` and are always optional:
/// components without the tool guest interface yield an empty tool list.
///
/// Returns the schema-native [`AgentTypeSchema`] and [`Tool`] models. This is
/// the canonical extraction path: it does not downgrade to the legacy
/// `AgentType`, so it preserves capability types (`QuotaToken`, `Secret`) and
/// rich scalars that the legacy schema model cannot represent.
pub async fn extract_component_metadata_with_streams(
    wasm_path: &Path,
    stdout: Option<impl StdoutStream + 'static>,
    stderr: Option<impl StdoutStream + 'static>,
    fail_on_missing_discover_method: bool,
    enable_fs_cache: bool,
) -> anyhow::Result<ExtractedComponentMetadata> {
    extract_component_metadata_impl(
        wasm_path,
        stdout,
        stderr,
        fail_on_missing_discover_method,
        enable_fs_cache,
        true,
    )
    .await
}

async fn extract_component_metadata_impl(
    wasm_path: &Path,
    stdout: Option<impl StdoutStream + 'static>,
    stderr: Option<impl StdoutStream + 'static>,
    fail_on_missing_discover_method: bool,
    enable_fs_cache: bool,
    include_tools: bool,
) -> anyhow::Result<ExtractedComponentMetadata> {
    let mut config = wasmtime::Config::default();
    config.wasm_multi_value(true);
    config.wasm_component_model(true);
    config.epoch_interruption(true);
    config.consume_fuel(true);
    config.wasm_backtrace_details(wasmtime::WasmBacktraceDetails::Enable);

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
    store.set_fuel(u64::MAX)?;
    store.set_epoch_deadline(u64::MAX);

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

    let agent_types = if let Some(func) = find_exported_function(
        &mut store,
        &instance,
        AGENT_INTERFACE_NAME,
        AGENT_FUNCTION_NAME,
    ) {
        discover_agent_types(&mut store, func).await?
    } else if fail_on_missing_discover_method {
        return Err(anyhow!(
            "Function {AGENT_FUNCTION_NAME} not found in interface {AGENT_INTERFACE_NAME}"
        ));
    } else {
        Vec::new()
    };

    let tools = if !include_tools {
        Vec::new()
    } else if let Some(func) = find_exported_function(
        &mut store,
        &instance,
        TOOL_INTERFACE_NAME,
        TOOL_FUNCTION_NAME,
    ) {
        discover_tools(&mut store, func).await?
    } else {
        Vec::new()
    };

    Ok(ExtractedComponentMetadata { agent_types, tools })
}

/// Same as [`extract_component_metadata_with_streams`], but extracts only the
/// agent types: `golem:tool/guest.discover-tools` is not called, so invalid
/// tool metadata does not fail agent-type-only extraction.
pub async fn extract_agent_type_schemas_with_streams(
    wasm_path: &Path,
    stdout: Option<impl StdoutStream + 'static>,
    stderr: Option<impl StdoutStream + 'static>,
    fail_on_missing_discover_method: bool,
    enable_fs_cache: bool,
) -> anyhow::Result<Vec<AgentTypeSchema>> {
    Ok(extract_component_metadata_impl(
        wasm_path,
        stdout,
        stderr,
        fail_on_missing_discover_method,
        enable_fs_cache,
        false,
    )
    .await?
    .agent_types)
}

/// Same as [`extract_agent_type_schemas_with_streams`], but inherits stdout and
/// stderr from the current process.
pub async fn extract_agent_type_schemas(
    wasm_path: &Path,
    fail_on_missing_discover_method: bool,
    enable_fs_cache: bool,
) -> anyhow::Result<Vec<AgentTypeSchema>> {
    extract_agent_type_schemas_with_streams(
        wasm_path,
        None::<pipe::MemoryOutputPipe>,
        None::<pipe::MemoryOutputPipe>,
        fail_on_missing_discover_method,
        enable_fs_cache,
    )
    .await
}

async fn discover_agent_types(
    store: &mut Store<Host>,
    func: Func,
) -> anyhow::Result<Vec<AgentTypeSchema>> {
    let typed_func = func
        .typed::<(), (Result<Vec<wire::AgentType>, wire::AgentError>,)>(&mut *store)
        .map_err(|e| {
            anyhow::anyhow!(
        "The component's golem:agent/guest interface does not match the expected type signature. \
         This usually means the golem-rust (or golem-ts) SDK version used to build the component \
         is incompatible with this version of golem-cli. \
         Try updating the SDK dependency or setting GOLEM_RUST_PATH / GOLEM_TS_PACKAGES_PATH \
         to point to a compatible local SDK: {e}"
    )
        })?;
    let results = typed_func.call_async(&mut *store, ()).await?;

    match results.0 {
        Ok(results) => {
            let mut agent_types: Vec<AgentTypeSchema> = Vec::with_capacity(results.len());
            for wire_type in results {
                let schema = decode_agent_type(&wire_type)
                    .map_err(|e| anyhow!("Failed to decode discovered agent type: {e:?}"))?;
                schema.validate().map_err(|e| {
                    anyhow!("Invalid agent type returned by discover-agent-types: {e}")
                })?;
                agent_types.push(schema);
            }
            trace!("Discovered agent types: {:#?}", agent_types);
            Ok(agent_types)
        }
        Err(agent_error) => {
            let agent_error: AgentError =
                decode_agent_error_rejecting_quota_with(agent_error, store.data_mut())
                    .map_err(|e| anyhow!("Failed to decode discovered agent error: {e:?}"))?;
            error!("Error while discovering agent types: {agent_error}");
            Err(anyhow!(agent_error.to_string()))
        }
    }
}

async fn discover_tools(store: &mut Store<Host>, func: Func) -> anyhow::Result<Vec<Tool>> {
    let typed_func = func
        .typed::<(), (Result<Vec<tool_wire::Tool>, tool_wire::ToolError>,)>(&mut *store)
        .map_err(|e| {
            anyhow::anyhow!(
        "The component's golem:tool/guest interface does not match the expected type signature. \
         This usually means the golem-rust (or golem-ts) SDK version used to build the component \
         is incompatible with this version of golem-cli. \
         Try updating the SDK dependency or setting GOLEM_RUST_PATH / GOLEM_TS_PACKAGES_PATH \
         to point to a compatible local SDK: {e}"
    )
        })?;
    let results = typed_func.call_async(&mut *store, ()).await?;

    match results.0 {
        Ok(results) => {
            let mut tools: Vec<Tool> = Vec::with_capacity(results.len());
            for wire_tool in results {
                let tool = decode_tool(&wire_tool)
                    .map_err(|e| anyhow!("Failed to decode discovered tool: {e}"))?;
                if let Err(errors) = validate_tool(&tool) {
                    let errors = errors
                        .iter()
                        .map(|e| e.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(anyhow!("Invalid tool returned by discover-tools: {errors}"));
                }
                tools.push(tool);
            }
            trace!("Discovered tools: {:#?}", tools);
            Ok(tools)
        }
        Err(tool_error) => {
            let message = format_wire_tool_error(&tool_error);
            error!("Error while discovering tools: {message}");
            Err(anyhow!(message))
        }
    }
}

/// Renders a wire `tool-error` returned by `discover-tools` as a message.
/// Custom error payloads are not decoded: they are self-contained
/// `typed-schema-value`s that may reference host resources, which discovery
/// does not support.
fn format_wire_tool_error(error: &tool_wire::ToolError) -> String {
    match error {
        tool_wire::ToolError::InvalidToolName(name) => format!("invalid tool name `{name}`"),
        tool_wire::ToolError::InvalidCommandPath(path) => {
            format!("invalid command path `{}`", path.join(" "))
        }
        tool_wire::ToolError::InvalidInput(message) => format!("invalid input: {message}"),
        tool_wire::ToolError::ConstraintViolation(message) => {
            format!("constraint violation: {message}")
        }
        tool_wire::ToolError::InvalidResult(message) => format!("invalid result: {message}"),
        tool_wire::ToolError::CustomError(_) => "custom tool error".to_string(),
    }
}

fn find_exported_function(
    mut store: impl AsContextMut,
    instance: &Instance,
    interface_name: &str,
    function_name: &str,
) -> Option<Func> {
    let (_, exported_instance_id) = instance.get_export(&mut store, None, interface_name)?;
    let (_, func_id) =
        instance.get_export(&mut store, Some(&exported_instance_id), function_name)?;
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

    fn io_data(&mut self) -> IoData<'_> {
        IoData {
            table: Arc::get_mut(&mut self.table)
                .expect("ResourceTable is shared and cannot be borrowed mutably")
                .get_mut()
                .expect("ResourceTable mutex must never fail"),
            io_ctx: Arc::get_mut(&mut self.io)
                .expect("IoCtx is shared and cannot be borrowed mutably")
                .get_mut()
                .expect("IoCtx mutex must never fail"),
        }
    }
}

impl WasiView for Host {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: Arc::get_mut(&mut self.wasi)
                .expect("WasiCtx is shared and cannot be borrowed mutably")
                .get_mut()
                .expect("WasiCtx mutex must never fail"),
            table: Arc::get_mut(&mut self.table)
                .expect("ResourceTable is shared and cannot be borrowed mutably")
                .get_mut()
                .expect("ResourceTable mutex must never fail"),
            io_ctx: Arc::get_mut(&mut self.io)
                .expect("IoCtx is shared and cannot be borrowed mutably")
                .get_mut()
                .expect("IoCtx mutex must never fail"),
        }
    }
}

impl QuotaTokenHandleDropper for Host {
    fn drop_quota_token_handle(
        &mut self,
        handle: wasmtime::component::Resource<QuotaTokenHandleRep>,
    ) {
        let _ = self.table().delete(handle);
    }
}

impl SecretHandleDropper for Host {
    fn drop_secret_handle(&mut self, handle: wasmtime::component::Resource<SecretHandleRep>) {
        let _ = self.table().delete(handle);
    }
}

/// Whether the resource `resource_name` imported from interface
/// `interface_name` is the opaque `golem:core/types.quota-token`. It is matched
/// both at its defining interface and at the `golem:quota/types` interface that
/// re-exports (`use`s) it, so every linker instance binds it to the same host
/// resource type expected by the generated `wire` bindings.
fn is_quota_token_resource(interface_name: &str, resource_name: &str) -> bool {
    resource_name == "quota-token"
        && matches!(
            interface_name,
            "golem:core/types@2.0.0" | "golem:quota/types@1.5.0"
        )
}

/// Whether the resource `resource_name` imported from interface
/// `interface_name` is the opaque `golem:core/types.secret`. Like
/// `quota-token`, it can appear transitively inside schema value trees and must
/// use the same host resource representation as the generated wire bindings.
fn is_secret_resource(interface_name: &str, resource_name: &str) -> bool {
    resource_name == "secret"
        && matches!(
            interface_name,
            "golem:core/types@2.0.0" | "golem:secrets/types@0.1.0" | "golem:secrets/reveal@0.1.0"
        )
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
                    if is_quota_token_resource(&name, &inner_name) {
                        // The `quota-token` resource appears transitively in the
                        // `discover-agent-types` result type (via
                        // `agent-error` -> `typed-schema-value` ->
                        // `schema-value-tree` -> `quota-token-handle`). The host
                        // calls that export through the generated `wire` bindings,
                        // which map this resource to [`QuotaTokenHandleRep`]. The
                        // resource type registered here must use the same host
                        // type or `Func::typed` rejects the signature with a
                        // resource type mismatch. Agent discovery never produces
                        // or consumes real quota tokens, so a dummy registration
                        // with a no-op destructor is sufficient.
                        instance.resource(
                            &inner_name,
                            ResourceType::host::<QuotaTokenHandleRep>(),
                            |_store, _rep| Ok(()),
                        )?;
                    } else if is_secret_resource(&name, &inner_name) {
                        instance.resource(
                            &inner_name,
                            ResourceType::host::<SecretHandleRep>(),
                            |_store, _rep| Ok(()),
                        )?;
                    } else if &inner_name != "pollable"
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
                move |_store, _func_type, _params, _results| {
                    let function_name = function.name.clone();
                    Box::new(async move {
                        error!(
                            "External function called during component metadata discovery: {function_name}",
                        );
                        Err(wasmtime::Error::msg(format!(
                            "External function called during component metadata discovery: {function_name}"
                        )))
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
    use super::ExtractedComponentMetadata;
    use test_r::test;

    #[test]
    fn deserializes_component_metadata_object() {
        let metadata: ExtractedComponentMetadata =
            serde_json::from_str(r#"{"agentTypes": [], "tools": []}"#).unwrap();
        assert!(metadata.agent_types.is_empty());
        assert!(metadata.tools.is_empty());
    }

    #[test]
    fn deserializes_component_metadata_object_without_tools() {
        let metadata: ExtractedComponentMetadata =
            serde_json::from_str(r#"{"agentTypes": []}"#).unwrap();
        assert!(metadata.agent_types.is_empty());
        assert!(metadata.tools.is_empty());
    }

    #[test]
    fn deserializes_legacy_agent_type_array() {
        let metadata: ExtractedComponentMetadata = serde_json::from_str("[]").unwrap();
        assert!(metadata.agent_types.is_empty());
        assert!(metadata.tools.is_empty());
    }

    #[test]
    fn rejects_empty_object() {
        assert!(serde_json::from_str::<ExtractedComponentMetadata>("{}").is_err());
    }

    #[test]
    fn rejects_object_with_wrong_field_casing() {
        assert!(
            serde_json::from_str::<ExtractedComponentMetadata>(r#"{"agent_types": []}"#).is_err()
        );
    }

    #[test]
    fn serializes_as_camel_case_object() {
        let metadata = ExtractedComponentMetadata {
            agent_types: Vec::new(),
            tools: Vec::new(),
        };
        assert_eq!(
            serde_json::to_string(&metadata).unwrap(),
            r#"{"agentTypes":[],"tools":[]}"#
        );
    }
}
