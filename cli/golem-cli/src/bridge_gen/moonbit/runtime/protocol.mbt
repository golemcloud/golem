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
// the server's OpenAPI `CreateAgentRequest` / `AgentKeyRequest` /
// `AgentInvocationRequest` / `AgentReference` / `AgentInvocationResult`
// (camelCase fields), the same contract the Rust and TypeScript bridges use.

///|
/// A single agent configuration override entry of a create request.
pub(all) struct AgentConfigEntry {
  path : Array[String]
  value : Json
}

///|
/// An agent and the incarnation of it that a create, upsert or lookup reached.
/// `agent_fingerprint` identifies the incarnation.
pub(all) struct AgentReference {
  agent_id : AgentId
  agent_fingerprint : String
} derive(Debug, Eq)

///|
pub extend AgentReference with Debug::{to_repr}

///|
pub extend AgentReference with Eq::{equal, not_equal}

///|
/// Response of an invoke request.
pub(all) struct AgentInvocationResult {
  agent_id : AgentId
  idempotency_key : String
  result : SchemaValue?
  component_revision : UInt64?
}

///|
/// A decoded invocation value together with the ID of the agent that ran it.
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
///
/// A client of `upsert` has the constructor `parameters` and no `target`; its
/// invocations name the keyed agent by them. A client of `create` or
/// `get_by_id` has a `target`, and its invocations name the agent by its ID and
/// fingerprint. Only the keyed agent of a keyed create also keeps its
/// `parameters`, which streaming invocations need because they name the agent
/// by its key.
pub struct ResolvedAgent {
  configuration : Configuration
  agent_type_name : String
  parameters : SchemaValue?
  target : AgentReference?
  priv mut known_agent_id : AgentId?
}

///|
pub fn ResolvedAgent::new(
  configuration : Configuration,
  agent_type_name : String,
  parameters~ : SchemaValue?,
  target~ : AgentReference?,
) -> ResolvedAgent {
  {
    configuration,
    agent_type_name,
    parameters,
    target,
    known_agent_id: target.map(reference => reference.agent_id),
  }
}

///|
/// The constructor parameters that name the keyed agent of a streaming
/// invocation.
fn ResolvedAgent::keyed_parameters(self : ResolvedAgent) -> SchemaValue raise {
  match self.parameters {
    Some(parameters) => parameters
    None =>
      raise BridgeError(
        "A streaming invocation names its \{self.agent_type_name} agent by its key: use a client of upsert, or of a keyed create",
      )
  }
}

///|
fn encode_config_entry(entry : AgentConfigEntry) -> Json {
  let path : Array[Json] = entry.path.map(Json::string)
  obj([("path", Json::array(path)), ("value", entry.value)])
}

///|
fn render_create_agent_request(
  parameters : SchemaValue,
  config : Array[AgentConfigEntry],
  env : Array[(String, String)],
  keyed : Bool,
) -> String raise {
  schema_value_check_text(parameters)
  let output = StringBuilder()
  output.write_string("{\"parameters\":")
  schema_value_write_json(parameters, output)
  output.write_string(",\"config\":[")
  for index, entry in config {
    if index != 0 {
      output.write_char(',')
    }
    output.write_string(encode_config_entry(entry).stringify())
  }
  output.write_string("],\"env\":")
  output.write_string(
    obj(env.map(entry => (entry.0, Json::string(entry.1)))).stringify(),
  )
  output.write_string(",\"keyed\":")
  output.write_string(Json::boolean(keyed).stringify())
  output.write_char('}')
  output.to_string()
}

///|
/// Body of an upsert or lookup request: the constructor parameters.
fn render_agent_key_request(parameters : SchemaValue) -> String raise {
  schema_value_check_text(parameters)
  let output = StringBuilder()
  output.write_string("{\"parameters\":")
  schema_value_write_json(parameters, output)
  output.write_char('}')
  output.to_string()
}

///|
/// Body of an invoke request. It names the agent by exactly one of `agent_id`
/// or `parameters`; with `expected_fingerprint`, a missing agent or another
/// incarnation fails with HTTP 404.
fn render_agent_invocation_request(
  agent_id : AgentId?,
  parameters : SchemaValue?,
  method_name : String,
  method_parameters : SchemaValue,
  mode : String,
  schedule_at : String?,
  expected_fingerprint : String?,
) -> String raise {
  if (agent_id is Some(_)) == (parameters is Some(_)) {
    raise BridgeError(
      "An invocation names its agent by exactly one of agentId or parameters",
    )
  }
  schema_value_check_text(method_parameters)
  let output = StringBuilder()
  output.write_char('{')
  match agent_id {
    Some(id) => {
      output.write_string("\"agentId\":")
      output.write_string(Json::string(id.to_string()).stringify())
      output.write_char(',')
    }
    None => ()
  }
  match parameters {
    Some(parameters) => {
      schema_value_check_text(parameters)
      output.write_string("\"parameters\":")
      schema_value_write_json(parameters, output)
      output.write_char(',')
    }
    None => ()
  }
  output.write_string("\"method\":")
  output.write_string(Json::string(method_name).stringify())
  output.write_string(",\"methodParameters\":")
  schema_value_write_json(method_parameters, output)
  output.write_string(",\"mode\":")
  output.write_string(Json::string(mode).stringify())
  for pair in [
    ("scheduleAt", schedule_at),
    ("expectedFingerprint", expected_fingerprint),
  ] {
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
fn decode_agent_reference(json : Json) -> AgentReference raise {
  let m = expect_object(json)
  {
    agent_id: AgentId::from_json(get_field(m, "agentId")),
    agent_fingerprint: expect_string(get_field(m, "agentFingerprint")),
  }
}

///|
/// The agent type name and the fingerprint in the server's `AgentMetadataDto`
/// of `GET /v1/runtime/envs/{env}/agents/{id}`.
fn decode_agent_metadata(json : Json) -> (String?, String) raise {
  let m = expect_object(json)
  let type_name = match field_opt(m, "agentTypeName") {
    Some(name) => Some(expect_string(name))
    None => None
  }
  (type_name, expect_string(get_field(m, "fingerprint")))
}

///|
/// The account ID of the token of `GET /v1/me/token`.
fn decode_token_account_id(json : Json) -> String raise {
  expect_string(get_field(expect_object(json), "accountId"))
}

///|
/// The environment ID of the page of `GET /v1/me/visible-environments` whose
/// entry is in `account_id` and has the application and environment names.
fn decode_visible_environment_id(
  json : Json,
  account_id : String,
  app_name : String,
  env_name : String,
) -> String? raise {
  fn string_at(json : Json, outer : String, inner : String) -> String raise {
    expect_string(get_field(expect_object(get_field(expect_object(json), outer)), inner))
  }
  for entry in expect_array(get_field(expect_object(json), "values")) {
    if string_at(entry, "account", "id") == account_id &&
      string_at(entry, "application", "name") == app_name &&
      string_at(entry, "environment", "name") == env_name {
      return Some(string_at(entry, "environment", "id"))
    }
  }
  None
}

///|
fn optional_u64(m : Map[String, Json], name : String) -> UInt64? raise {
  match field_opt(m, name) {
    Some(field) => Some(@string.parse_uint64(number_literal(field)))
    None => None
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
