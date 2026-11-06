// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::reflection::{
    AgentConfigValue, GolemReflectError, ParsedAgentId, ParsedAgentIdParts, agent_error_to_reflect,
    agentic_agent_error_to_reflect, agentic_rpc_error_to_reflect, rpc_error_to_reflect,
};
use crate::bindings::golem::agent::{common, host};
use crate::golem_agentic::golem::agent::{common as agentic_common, host as agentic_host};
use crate::{SchemaValue, Uuid};

pub(crate) fn parse_agent_id(
    id: &str,
) -> Result<
    (
        String,
        agentic_host::TypedSchemaValue,
        Option<agentic_host::Uuid>,
    ),
    agentic_common::AgentError,
> {
    agentic_host::parse_agent_id(id)
}

pub(crate) fn parts(id: &str) -> Result<ParsedAgentIdParts, GolemReflectError> {
    let (type_name, typed, phantom_id) =
        host::parse_agent_id(id).map_err(agent_error_to_reflect)?;
    let typed = crate::decode_typed_schema_value(&typed)
        .map_err(|error| GolemReflectError::SchemaDecode(error.to_string()))?;
    Ok(ParsedAgentIdParts {
        type_name,
        constructor_value: typed.value().clone(),
        constructor_schema: typed.graph().clone(),
        phantom_id: phantom_id.map(crate::wire_uuid_to_schema),
    })
}

pub(crate) fn make_agent_id_value(
    type_name: &str,
    constructor: SchemaValue,
    phantom_id: Option<Uuid>,
) -> Result<ParsedAgentId, GolemReflectError> {
    let encoded = crate::encode_schema_value(&constructor)
        .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))?;
    let id = host::make_agent_id(
        type_name,
        encoded,
        phantom_id.map(crate::schema_uuid_to_wire),
    )
    .map_err(agent_error_to_reflect)?;
    Ok(ParsedAgentId::new(id))
}

pub(crate) fn connect(
    type_name: &str,
    constructor: &SchemaValue,
    phantom_id: Option<Uuid>,
    config: Vec<common::TypedAgentConfigValue>,
) -> Result<host::WasmRpc, GolemReflectError> {
    let constructor = crate::encode_schema_value(constructor)
        .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))?;
    host::WasmRpc::create(
        type_name,
        constructor,
        phantom_id.map(crate::schema_uuid_to_wire),
        config,
    )
    .map_err(rpc_error_to_reflect)
}

pub(crate) fn connect_with_config(
    type_name: &str,
    constructor: &SchemaValue,
    phantom_id: Option<Uuid>,
    config: Vec<AgentConfigValue>,
) -> Result<host::WasmRpc, GolemReflectError> {
    let config = config
        .into_iter()
        .map(|value| {
            Ok(common::TypedAgentConfigValue {
                path: value.path,
                value: crate::encode_typed_schema_value_owned(value.value)
                    .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))?,
            })
        })
        .collect::<Result<Vec<_>, GolemReflectError>>()?;
    connect(type_name, constructor, phantom_id, config)
}

// Every consuming host call gets a fresh envelope. Schema value trees can own
// affine quota-token handles, so even an equal constructor must be encoded again.
#[doc(hidden)]
pub fn __connect_agent(
    type_name: &str,
    mut encode: impl FnMut() -> Result<crate::schema::wit::wire::SchemaValueTree, GolemReflectError>,
    phantom_id: Option<agentic_host::Uuid>,
    config: impl FnOnce() -> Vec<agentic_common::TypedAgentConfigValue>,
) -> Result<(String, agentic_host::WasmRpc), GolemReflectError> {
    let id = agentic_host::make_agent_id(type_name, encode()?, phantom_id)
        .map_err(agentic_agent_error_to_reflect)?;
    let rpc = __agent_proxy(type_name, encode, phantom_id, config)?;
    Ok((id, rpc))
}

#[doc(hidden)]
pub fn __agent_proxy(
    type_name: &str,
    mut encode: impl FnMut() -> Result<crate::schema::wit::wire::SchemaValueTree, GolemReflectError>,
    phantom_id: Option<agentic_host::Uuid>,
    config: impl FnOnce() -> Vec<agentic_common::TypedAgentConfigValue>,
) -> Result<agentic_host::WasmRpc, GolemReflectError> {
    agentic_host::WasmRpc::create(type_name, encode()?, phantom_id, config())
        .map_err(agentic_rpc_error_to_reflect)
}

#[doc(hidden)]
pub fn __connect_agent_or_panic(
    type_name: &str,
    mut encode: impl FnMut() -> crate::schema::wit::wire::SchemaValueTree,
    phantom_id: Option<agentic_host::Uuid>,
    config: impl FnOnce() -> Vec<agentic_common::TypedAgentConfigValue>,
) -> (String, agentic_host::WasmRpc) {
    let id = agentic_host::make_agent_id(type_name, encode(), phantom_id)
        .expect("Internal Error: Failed to make agent id");
    let rpc = __agent_proxy_or_panic(type_name, encode, phantom_id, config);
    (id, rpc)
}

#[doc(hidden)]
pub fn __agent_proxy_or_panic(
    type_name: &str,
    mut encode: impl FnMut() -> crate::schema::wit::wire::SchemaValueTree,
    phantom_id: Option<agentic_host::Uuid>,
    config: impl FnOnce() -> Vec<agentic_common::TypedAgentConfigValue>,
) -> agentic_host::WasmRpc {
    agentic_host::WasmRpc::create(type_name, encode(), phantom_id, config())
        .unwrap_or_else(|error| panic!("Internal Error: Agent type not registered: {error:?}"))
}
