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

// JSON (de)serialization of the bridge REST protocol. The wire shapes mirror
// the server's OpenAPI `CreateAgentRequest` / `AgentInvocationRequest` /
// `CreateAgentResponse` / `AgentInvocationResult` (camelCase fields), the same
// contract the Rust and TypeScript bridges use.

///|
/// A single agent configuration override entry of a create-agent request.
pub(all) struct AgentConfigEntry {
  path : Array[String]
  value : SchemaValue
}

///|
/// Response of a `POST /v1/agents/create-agent` request.
pub(all) struct CreateAgentResponse {
  agent_id : AgentId
  component_revision : UInt64?
} derive(Debug)

///|
/// Response of a `POST /v1/agents/invoke-agent` request.
pub(all) struct AgentInvocationResult {
  agent_id : AgentId
  result : SchemaValue?
  component_revision : UInt64?
}

///|
/// A resolved agent: everything a generated remote client needs to issue method
/// invocations. The [Configuration] is captured at construction time so a
/// remote keeps targeting the server it was created against, even if the global
/// configuration is later changed.
pub(all) struct ResolvedAgent {
  configuration : Configuration
  agent_type_name : String
  parameters : SchemaValue
  phantom_id : String?
  agent_id : AgentId
} derive(Debug)

///|
fn encode_config_entry(entry : AgentConfigEntry) -> Json {
  let path : Array[Json] = entry.path.map(Json::string)
  obj([("path", Json::array(path)), ("value", schema_value_to_json(entry.value))])
}

///|
fn encode_create_agent_request(
  app_name : String,
  env_name : String,
  agent_type_name : String,
  parameters : SchemaValue,
  phantom_id : String?,
  config : Array[AgentConfigEntry],
) -> Json {
  let fields : Array[(String, Json)] = [
    ("appName", Json::string(app_name)),
    ("envName", Json::string(env_name)),
    ("agentTypeName", Json::string(agent_type_name)),
    ("parameters", schema_value_to_json(parameters)),
  ]
  match phantom_id {
    Some(id) => fields.push(("phantomId", Json::string(id)))
    None => ()
  }
  fields.push(("config", Json::array(config.map(encode_config_entry))))
  obj(fields)
}

///|
fn encode_agent_invocation_request(
  app_name : String,
  env_name : String,
  agent_type_name : String,
  parameters : SchemaValue,
  phantom_id : String?,
  method_name : String,
  method_parameters : SchemaValue,
  mode : String,
  schedule_at : String?,
  idempotency_key : String?,
) -> Json {
  let fields : Array[(String, Json)] = [
    ("appName", Json::string(app_name)),
    ("envName", Json::string(env_name)),
    ("agentTypeName", Json::string(agent_type_name)),
    ("parameters", schema_value_to_json(parameters)),
    ("methodName", Json::string(method_name)),
    ("methodParameters", schema_value_to_json(method_parameters)),
    ("mode", Json::string(mode)),
  ]
  match phantom_id {
    Some(id) => fields.push(("phantomId", Json::string(id)))
    None => ()
  }
  match schedule_at {
    Some(at) => fields.push(("scheduleAt", Json::string(at)))
    None => ()
  }
  match idempotency_key {
    Some(k) => fields.push(("idempotencyKey", Json::string(k)))
    None => ()
  }
  obj(fields)
}

///|
fn optional_u64(m : Map[String, Json], name : String) -> UInt64? raise {
  match field_opt(m, name) {
    Some(field) => Some(@string.parse_uint64(number_literal(field)))
    None => None
  }
}

///|
fn decode_create_agent_response(json : Json) -> CreateAgentResponse raise {
  let m = expect_object(json)
  {
    agent_id: AgentId::from_json(get_field(m, "agentId")),
    component_revision: optional_u64(m, "componentRevision"),
  }
}

///|
fn decode_agent_invocation_result(json : Json) -> AgentInvocationResult raise {
  let m = expect_object(json)
  let result = match field_opt(m, "result") {
    Some(typed) => {
      let typed_m = expect_object(typed)
      match field_opt(typed_m, "value") {
        Some(value) => Some(schema_value_from_json(value))
        None => None
      }
    }
    None => None
  }
  {
    agent_id: AgentId::from_json(get_field(m, "agentId")),
    result,
    component_revision: optional_u64(m, "componentRevision"),
  }
}
