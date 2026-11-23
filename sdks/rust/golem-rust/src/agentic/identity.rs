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
    AgentConfigValue, CreateOptions, GolemReflectError, agentic_rpc_error_to_reflect,
    rpc_error_to_reflect,
};
use crate::bindings::golem::agent::{common, host};
use crate::golem_agentic::golem::agent::{common as agentic_common, host as agentic_host};
use crate::schema::wit::wire::SchemaValueTree;
use crate::{AgentId, SchemaValue};

/// The panic text for an environment variable that Golem sets but that the agent cannot read.
/// The executor shows such a variable to the agent only when the agent's permission card allows
/// the agent to read it.
pub(crate) fn golem_env_var_not_readable(name: &str) -> String {
    format!(
        "{name} is not readable: the agent's permission card must allow env(?agent) @ ?agent : read : {name}"
    )
}

fn encode_constructor(constructor: &SchemaValue) -> Result<SchemaValueTree, GolemReflectError> {
    crate::encode_schema_value(constructor)
        .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))
}

pub(crate) fn encode_config(
    config: Vec<AgentConfigValue>,
) -> Result<Vec<common::TypedAgentConfigValue>, GolemReflectError> {
    config
        .into_iter()
        .map(|value| {
            Ok(common::TypedAgentConfigValue {
                path: value.path,
                value: crate::encode_typed_schema_value_owned(value.value)
                    .map_err(|error| GolemReflectError::SchemaEncode(error.to_string()))?,
            })
        })
        .collect()
}

pub(crate) fn upsert(
    type_name: &str,
    constructor: &SchemaValue,
) -> Result<host::WasmRpc, GolemReflectError> {
    host::WasmRpc::upsert(type_name, encode_constructor(constructor)?).map_err(rpc_error_to_reflect)
}

fn host_create_options(
    options: CreateOptions<Vec<common::TypedAgentConfigValue>>,
) -> host::CreateOptions {
    host::CreateOptions {
        keyed: options.keyed,
        agent_config: options.config,
        env: options.env,
    }
}

pub(crate) async fn create(
    type_name: &str,
    constructor: &SchemaValue,
    options: CreateOptions<Vec<common::TypedAgentConfigValue>>,
) -> Result<host::WasmRpc, GolemReflectError> {
    let created = host::WasmRpc::create(
        type_name.to_string(),
        encode_constructor(constructor)?,
        host_create_options(options),
    )
    .await
    .map_err(rpc_error_to_reflect)?;
    Ok(created.client)
}

pub(crate) fn get_by_id(
    type_name: &str,
    agent_id: &AgentId,
) -> Result<host::WasmRpc, GolemReflectError> {
    host::WasmRpc::get_by_id(type_name, agent_id.into()).map_err(rpc_error_to_reflect)
}

pub(crate) async fn agent_id(rpc: &host::WasmRpc) -> Result<AgentId, GolemReflectError> {
    rpc.agent_id()
        .await
        .map(Into::into)
        .map_err(rpc_error_to_reflect)
}

pub(crate) fn lookup(
    type_name: &str,
    constructor: &SchemaValue,
) -> Result<Option<AgentId>, GolemReflectError> {
    host::lookup_agent_id(type_name, encode_constructor(constructor)?)
        .map(|agent_id| agent_id.map(Into::into))
        .map_err(rpc_error_to_reflect)
}

/// The name of the running agent's type, as the host sets it in `GOLEM_AGENT_TYPE`.
pub(crate) fn current_agent_type() -> Option<String> {
    wasip3::cli::environment::get_environment()
        .into_iter()
        .find_map(|(key, value)| (key == "GOLEM_AGENT_TYPE").then_some(value))
}

#[doc(hidden)]
pub fn __bind_self(type_name: &str) -> Result<agentic_host::WasmRpc, GolemReflectError> {
    let bound_type_name = current_agent_type().ok_or_else(|| {
        GolemReflectError::InvalidType("the running agent has no agent type".to_string())
    })?;
    if bound_type_name != type_name {
        return Err(GolemReflectError::InvalidType(format!(
            "full client expects `{type_name}`, identity is `{bound_type_name}`"
        )));
    }
    let (_, rpc) = agentic_host::WasmRpc::bind_self().map_err(agentic_rpc_error_to_reflect)?;
    Ok(rpc)
}

#[doc(hidden)]
pub fn __upsert_agent(
    type_name: &str,
    constructor: SchemaValueTree,
) -> Result<agentic_host::WasmRpc, GolemReflectError> {
    agentic_host::WasmRpc::upsert(type_name, constructor).map_err(agentic_rpc_error_to_reflect)
}

#[doc(hidden)]
pub fn __upsert_agent_or_panic(
    type_name: &str,
    constructor: SchemaValueTree,
) -> agentic_host::WasmRpc {
    agentic_host::WasmRpc::upsert(type_name, constructor)
        .unwrap_or_else(|error| panic!("Internal Error: Failed to connect to agent: {error:?}"))
}

fn agentic_host_create_options(
    options: CreateOptions<Vec<agentic_common::TypedAgentConfigValue>>,
) -> agentic_host::CreateOptions {
    agentic_host::CreateOptions {
        keyed: options.keyed,
        agent_config: options.config,
        env: options.env,
    }
}

#[doc(hidden)]
pub async fn __create_agent(
    type_name: &str,
    constructor: SchemaValueTree,
    options: CreateOptions<Vec<agentic_common::TypedAgentConfigValue>>,
) -> Result<agentic_host::WasmRpc, GolemReflectError> {
    let created = agentic_host::WasmRpc::create(
        type_name.to_string(),
        constructor,
        agentic_host_create_options(options),
    )
    .await
    .map_err(agentic_rpc_error_to_reflect)?;
    Ok(created.client)
}

#[doc(hidden)]
pub fn __get_agent_by_id(
    type_name: &str,
    agent_id: &AgentId,
) -> Result<agentic_host::WasmRpc, GolemReflectError> {
    agentic_host::WasmRpc::get_by_id(type_name, agent_id.into())
        .map_err(agentic_rpc_error_to_reflect)
}

#[doc(hidden)]
pub async fn __agent_id(rpc: &agentic_host::WasmRpc) -> Result<AgentId, GolemReflectError> {
    rpc.agent_id()
        .await
        .map(Into::into)
        .map_err(agentic_rpc_error_to_reflect)
}

#[cfg(test)]
mod tests {
    use super::{AgentConfigValue, CreateOptions, encode_config};
    use crate::agentic::reflection::{
        GolemReflectError, agentic_rpc_error_to_reflect, rpc_error_to_reflect,
    };
    use crate::bindings::golem::agent::host;
    use crate::golem_agentic::golem::agent::{common as agentic_common, host as agentic_host};
    use crate::schema::{SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue};
    use test_r::test;

    fn options<C>(keyed: bool, config: C) -> CreateOptions<C> {
        CreateOptions {
            keyed,
            config,
            env: vec![
                ("REGION".to_string(), "eu".to_string()),
                ("MODE".to_string(), "test".to_string()),
            ],
        }
    }

    fn string_config(path: &str, value: &str) -> AgentConfigValue {
        AgentConfigValue {
            path: vec!["model".to_string(), path.to_string()],
            value: TypedSchemaValue::new(
                SchemaGraph {
                    defs: Vec::new(),
                    root: SchemaType::string(),
                },
                SchemaValue::String(value.to_string()),
            ),
        }
    }

    #[test]
    fn create_options_reach_the_host_with_keyed_config_and_env() {
        for keyed in [true, false] {
            let config = encode_config(vec![string_config("name", "fast")]).unwrap();
            let raw = super::host_create_options(options(keyed, config));
            assert_eq!(raw.keyed, keyed);
            assert_eq!(
                raw.env,
                vec![
                    ("REGION".to_string(), "eu".to_string()),
                    ("MODE".to_string(), "test".to_string()),
                ]
            );
            assert_eq!(raw.agent_config.len(), 1);
            assert_eq!(
                raw.agent_config[0].path,
                vec!["model".to_string(), "name".to_string()]
            );
            let value = crate::decode_typed_schema_value(&raw.agent_config[0].value).unwrap();
            assert_eq!(value.value(), &SchemaValue::String("fast".to_string()));
        }
    }

    #[test]
    fn generated_client_create_options_reach_the_host_with_keyed_config_and_env() {
        for keyed in [true, false] {
            let config = vec![agentic_common::TypedAgentConfigValue {
                path: vec!["limit".to_string()],
                value: crate::schema::wit::wire::TypedSchemaValue {
                    graph: crate::schema::wit::direct::schema::<u32>(),
                    value: crate::schema::wit::direct::encode(&7u32).unwrap(),
                },
            }];
            let mut raw = super::agentic_host_create_options(options(keyed, config));
            assert_eq!(raw.keyed, keyed);
            assert_eq!(raw.env.len(), 2);
            assert_eq!(raw.env[1], ("MODE".to_string(), "test".to_string()));
            assert_eq!(raw.agent_config.len(), 1);
            let entry = raw.agent_config.remove(0);
            assert_eq!(entry.path, vec!["limit".to_string()]);
            assert_eq!(
                crate::schema::wit::direct::decode::<u32>(entry.value.value).unwrap(),
                7
            );
        }
    }

    #[test]
    fn default_create_options_create_an_agent_without_a_key() {
        let raw = super::host_create_options(CreateOptions::default());
        assert!(!raw.keyed);
        assert!(raw.agent_config.is_empty());
        assert!(raw.env.is_empty());
    }

    #[test]
    fn conflict_and_not_found_are_typed_errors() {
        assert!(matches!(
            rpc_error_to_reflect(host::RpcError::Conflict("key is current".to_string())),
            GolemReflectError::Conflict(message) if message == "key is current"
        ));
        assert!(matches!(
            rpc_error_to_reflect(host::RpcError::NotFound("no agent".to_string())),
            GolemReflectError::RemoteNotFound(message) if message == "no agent"
        ));
        assert!(matches!(
            agentic_rpc_error_to_reflect(agentic_host::RpcError::Conflict(
                "key is current".to_string()
            )),
            GolemReflectError::Conflict(message) if message == "key is current"
        ));
        assert!(matches!(
            agentic_rpc_error_to_reflect(agentic_host::RpcError::NotFound(
                "wrong type".to_string()
            )),
            GolemReflectError::RemoteNotFound(message) if message == "wrong type"
        ));
    }
}
