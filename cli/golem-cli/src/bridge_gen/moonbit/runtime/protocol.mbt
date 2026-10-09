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
  value : Json
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
  idempotency_key : String
  result : SchemaValue?
  component_revision : UInt64?
}

///|
/// A decoded invocation value together with the final one-shot agent identity.
pub(all) struct InvocationResponse[T] {
  agent_id : AgentId
  idempotency_key : String
  value : T
  component_revision : UInt64?
}

///|
/// Metadata returned for a triggered or scheduled one-shot invocation.
pub(all) struct InvocationReceipt {
  agent_id : AgentId
  idempotency_key : String
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
  config : Array[AgentConfigEntry]
  agent_id : AgentId?
}

///|
fn encode_config_entry(entry : AgentConfigEntry) -> Json {
  let path : Array[Json] = entry.path.map(Json::string)
  obj([("path", Json::array(path)), ("value", entry.value)])
}

///|
fn write_request_fields(
  output : StringBuilder,
  app_name : String,
  env_name : String,
  agent_type_name : String,
  parameters : SchemaValue,
  phantom_id : String?,
  config : Array[AgentConfigEntry],
) -> Unit {
  output.write_string("{\"appName\":")
  output.write_string(Json::string(app_name).stringify())
  output.write_string(",\"envName\":")
  output.write_string(Json::string(env_name).stringify())
  output.write_string(",\"agentTypeName\":")
  output.write_string(Json::string(agent_type_name).stringify())
  output.write_string(",\"parameters\":")
  schema_value_write_json(parameters, output)
  match phantom_id {
    Some(id) => {
      output.write_string(",\"phantomId\":")
      output.write_string(Json::string(id).stringify())
    }
    None => ()
  }
  output.write_string(",\"config\":[")
  for index, entry in config {
    if index != 0 {
      output.write_char(',')
    }
    output.write_string(encode_config_entry(entry).stringify())
  }
  output.write_char(']')
}

///|
fn render_create_agent_request(
  app_name : String,
  env_name : String,
  agent_type_name : String,
  parameters : SchemaValue,
  phantom_id : String?,
  config : Array[AgentConfigEntry],
) -> String {
  let output = StringBuilder()
  write_request_fields(
    output, app_name, env_name, agent_type_name, parameters, phantom_id, config,
  )
  output.write_char('}')
  output.to_string()
}

///|
fn render_agent_invocation_request(
  app_name : String,
  env_name : String,
  agent_type_name : String,
  parameters : SchemaValue,
  phantom_id : String?,
  config : Array[AgentConfigEntry],
  method_name : String,
  method_parameters : SchemaValue,
  mode : String,
  schedule_at : String?,
  idempotency_key : String?,
) -> String {
  let output = StringBuilder()
  write_request_fields(
    output, app_name, env_name, agent_type_name, parameters, phantom_id, config,
  )
  output.write_string(",\"methodName\":")
  output.write_string(Json::string(method_name).stringify())
  output.write_string(",\"methodParameters\":")
  schema_value_write_json(method_parameters, output)
  output.write_string(",\"mode\":")
  output.write_string(Json::string(mode).stringify())
  for pair in [("scheduleAt", schedule_at), ("idempotencyKey", idempotency_key)] {
    let (name, value) = pair
    match value {
      Some(value) => {
        output <+ ",\"\{name}\":"
        output.write_string(Json::string(value).stringify())
      }
      None => ()
    }
  }
  output.write_char('}')
  output.to_string()
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
      let graph = get_field(typed_m, "graph")
      ignore(expect_object(graph))
      let codec = public_value_codec(graph.stringify())
      Some(codec.decode(get_field(typed_m, "value")))
    }
    None => None
  }
  {
    agent_id: AgentId::from_json(get_field(m, "agentId")),
    idempotency_key: expect_string(get_field(m, "idempotencyKey")),
    result,
    component_revision: optional_u64(m, "componentRevision"),
  }
}
