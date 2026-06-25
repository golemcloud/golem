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

// The REST transport shared by all generated bridge clients in this project.
//
// Calls the worker service's `create-agent` and `invoke-agent` endpoints over
// `moonbitlang/async/http`, with JSON bodies built from the core `Json` model.
// All calls are `async` and run on the `moonbitlang/async` event loop.

///|
async fn send(
  configuration : Configuration,
  endpoint : String,
  body : String,
  idempotency_key : String?,
) -> Json raise {
  let server = configuration.server
  let url = server.url() + "/v1/agents/" + endpoint
  let headers : Map[String, String] = {}
  headers["Content-Type"] = "application/json"
  headers["Authorization"] = "Bearer " + server.token()
  match idempotency_key {
    Some(k) => headers["Idempotency-Key"] = k
    None => ()
  }
  let (resp, data) = @http.post(url, body, headers~)
  let text = data.text()
  if resp.code >= 200 && resp.code < 300 {
    @json.parse(text)
  } else {
    raise BridgeError(
      "Golem \{endpoint} request failed with HTTP \{resp.code}: \{text}",
    )
  }
}

///|
/// Creates (or gets) an agent and returns its identity.
pub async fn create_agent(
  configuration : Configuration,
  agent_type_name : String,
  parameters : SchemaValue,
  phantom_id : String?,
  config : Array[AgentConfigEntry],
) -> CreateAgentResponse raise {
  let body = encode_create_agent_request(
    configuration.app_name,
    configuration.env_name,
    agent_type_name,
    parameters,
    phantom_id,
    config,
  ).stringify()
  let json = send(configuration, "create-agent", body, None)
  decode_create_agent_response(json)
}

///|
/// Invokes a method on a resolved agent.
pub async fn invoke_agent(
  resolved : ResolvedAgent,
  method_name : String,
  method_parameters : SchemaValue,
  mode : String,
  schedule_at : String?,
) -> AgentInvocationResult raise {
  let configuration = resolved.configuration
  let body = encode_agent_invocation_request(
    configuration.app_name,
    configuration.env_name,
    resolved.agent_type_name,
    resolved.parameters,
    resolved.phantom_id,
    method_name,
    method_parameters,
    mode,
    schedule_at,
    None,
  ).stringify()
  let json = send(configuration, "invoke-agent", body, None)
  decode_agent_invocation_result(json)
}
