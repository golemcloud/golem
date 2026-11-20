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
// Calls the worker service's runtime API under
// `/v1/runtime/envs/{environment}/agent-types/{type}` over
// `moonbitlang/async/http`, with JSON bodies built from the core `Json` model.
// All calls are `async` and run on the `moonbitlang/async` event loop.
//
// The runtime API names the environment by ID, and a [Configuration] names it
// by the application and environment names. [environment_id] resolves the ID
// with `GET /v1/me/token` and `GET /v1/me/visible-environments`, keeps the
// match in the account of the token, and caches it.

///|
let environment_ids : Map[(String, String, String, String), String] = Map([])

///|
/// Percent-encodes `value` for a URL path segment or query value.
fn url_component(value : String) -> String {
  let output = StringBuilder()
  for byte in @utf8.encode(value) {
    let c = byte.to_int()
    if (c >= 'a' && c <= 'z') ||
      (c >= 'A' && c <= 'Z') ||
      (c >= '0' && c <= '9') ||
      c == '-' ||
      c == '.' ||
      c == '_' ||
      c == '~' {
      output.write_char(c.unsafe_to_char())
    } else {
      let hex = "0123456789ABCDEF"
      output.write_char('%')
      output.write_char(hex.code_unit_at(c >> 4).to_int().unsafe_to_char())
      output.write_char(hex.code_unit_at(c & 15).to_int().unsafe_to_char())
    }
  }
  output.to_string()
}

///|
/// An environment lookup worth another attempt. `EnvironmentUnavailable` is an
/// answer with HTTP 429 or 5xx: the server could not serve the lookup now.
/// `EnvironmentNotAnswered` holds the error of an exchange that ended without a
/// complete answer, whatever part of it did arrive.
priv suberror EnvironmentUnavailable {
  EnvironmentUnavailable(String)
  EnvironmentNotAnswered(Error)
}

///|
/// Sends the request and returns the status code and the body of the answer.
async fn exchange(
  configuration : Configuration,
  path : String,
  body : String?,
) -> (Int, Bytes) {
  let server = configuration.server
  let url = server.url() + path
  let headers : @http.Headers = Map([])
  headers["Authorization"] = "Bearer " + server.token()
  let (resp, data) = match body {
    Some(body) => {
      headers["Content-Type"] = "application/json"
      @http.post(url, body, headers~)
    }
    None => @http.get(url, headers~)
  }
  (resp.code, data.binary())
}

///|
/// The text of a failed answer for an error message. Bytes that are not UTF-8
/// are replaced, so the body never hides the status.
fn error_text(body : Bytes) -> String {
  @utf8.decode_lossy(body)
}

///|
/// The JSON of a successful answer to an `endpoint` request, or the error of a
/// failed one.
fn answer_json(endpoint : String, code : Int, body : Bytes) -> Json raise {
  if code >= 200 && code < 300 {
    return @json.parse(@utf8.decode(body))
  }
  let text = error_text(body)
  if code == 404 {
    raise NotFound("Golem \{endpoint} request failed with HTTP 404: \{text}")
  } else if code == 409 {
    raise Conflict("Golem \{endpoint} request failed with HTTP 409: \{text}")
  } else {
    raise BridgeError(
      "Golem \{endpoint} request failed with HTTP \{code}: \{text}",
    )
  }
}

///|
async fn send(
  configuration : Configuration,
  endpoint : String,
  path : String,
  body : String?,
) -> Json raise {
  let (code, answer) = exchange(configuration, path, body)
  answer_json(endpoint, code, answer)
}

///|
/// A `GET` of the environment lookup. An exchange without a complete answer,
/// unless it was cancelled, and an answer with HTTP 429 or 5xx raise
/// [EnvironmentUnavailable].
async fn lookup_get(configuration : Configuration, path : String) -> Json raise {
  let (code, body) = exchange(configuration, path, None) catch {
    error => {
      if @async.is_being_cancelled() {
        raise error
      }
      raise EnvironmentNotAnswered(error)
    }
  }
  if code == 429 || code >= 500 {
    raise EnvironmentUnavailable(
      "Golem resolve-environment request failed with HTTP \{code}: \{error_text(body)}",
    )
  }
  answer_json("resolve-environment", code, body)
}

///|
/// The ID of the configured environment of the configured application in the
/// account of the server token. A successful resolution is cached for the
/// server, token and names. An environment missing from the account raises
/// [BridgeError::NotFound].
pub async fn environment_id(configuration : Configuration) -> String raise {
  resolve_environment_id(configuration) catch {
    EnvironmentUnavailable(message) => raise BridgeError(message)
    EnvironmentNotAnswered(error) => raise error
    error => raise error
  }
}

///|
/// Resolves the ID like [environment_id], but raises [EnvironmentUnavailable]
/// for a lookup worth another attempt.
async fn resolve_environment_id(configuration : Configuration) -> String raise {
  let server = configuration.server
  let key = (
    server.url(),
    server.token(),
    configuration.app_name,
    configuration.env_name,
  )
  match environment_ids.get(key) {
    Some(id) => return id
    None => ()
  }
  let token = lookup_get(configuration, "/v1/me/token")
  let account_id = decode_token_account_id(token)
  let query = "app_name=\{url_component(configuration.app_name)}&env_name=\{url_component(configuration.env_name)}"
  let environments = lookup_get(
    configuration,
    "/v1/me/visible-environments?\{query}",
  )
  match
    decode_visible_environment_id(
      environments,
      account_id,
      configuration.app_name,
      configuration.env_name,
    ) {
    Some(id) => {
      environment_ids[key] = id
      id
    }
    None =>
      raise NotFound(
        "Environment '\{configuration.env_name}' of application '\{configuration.app_name}' does not exist in the account of the token",
      )
  }
}

///|
async fn agent_type_path(
  configuration : Configuration,
  agent_type_name : String,
  suffix : String,
) -> String raise {
  let env = environment_id(configuration)
  "/v1/runtime/envs/\{env}/agent-types/\{url_component(agent_type_name)}/\{suffix}"
}

///|
/// Creates an agent and returns after its initialization. A keyed create of a
/// current key raises [BridgeError::Conflict].
pub async fn create_agent(
  configuration : Configuration,
  agent_type_name : String,
  parameters : SchemaValue,
  keyed~ : Bool,
  config~ : Array[AgentConfigEntry],
  env~ : Array[(String, String)],
) -> AgentReference raise {
  let body = render_create_agent_request(parameters, config, env, keyed)
  let path = agent_type_path(configuration, agent_type_name, "agents")
  decode_agent_reference(send(configuration, "create-agent", path, Some(body)))
}

///|
/// Returns the keyed agent of the parameters, and creates it when it is
/// missing.
pub async fn upsert_agent(
  configuration : Configuration,
  agent_type_name : String,
  parameters : SchemaValue,
) -> AgentReference raise {
  let body = render_agent_key_request(parameters)
  let path = agent_type_path(configuration, agent_type_name, "agents/upsert")
  decode_agent_reference(send(configuration, "upsert-agent", path, Some(body)))
}

///|
/// The existing agent of the type with the ID. It never creates the agent: an
/// unknown ID, or the ID of an agent of another type, raises
/// [BridgeError::NotFound].
pub async fn get_agent(
  configuration : Configuration,
  agent_type_name : String,
  agent_id : AgentId,
) -> AgentReference raise {
  let env = environment_id(configuration)
  let json = send(
    configuration,
    "get-agent",
    "/v1/runtime/envs/\{env}/agents/\{agent_id}",
    None,
  )
  let (type_name, fingerprint) = decode_agent_metadata(json)
  if type_name != Some(agent_type_name) {
    raise NotFound("agent \{agent_id} is not a \{agent_type_name} agent")
  }
  { agent_id, agent_fingerprint: fingerprint }
}

///|
/// The ID of the resolved agent. A client of `upsert` asks the server the
/// first time, and the server creates the agent when it is missing.
pub async fn agent_id(resolved : ResolvedAgent) -> AgentId raise {
  match resolved.known_agent_id {
    Some(id) => id
    None => {
      let reference = upsert_agent(
        resolved.configuration,
        resolved.agent_type_name,
        resolved.keyed_parameters(),
      )
      resolved.known_agent_id = Some(reference.agent_id)
      reference.agent_id
    }
  }
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
  let body = match resolved.target {
    Some(target) =>
      render_agent_invocation_request(
        Some(target.agent_id),
        None,
        method_name,
        method_parameters,
        mode,
        schedule_at,
        Some(target.agent_fingerprint),
      )
    None =>
      render_agent_invocation_request(
        None,
        Some(resolved.keyed_parameters()),
        method_name,
        method_parameters,
        mode,
        schedule_at,
        None,
      )
  }
  let path = agent_type_path(configuration, resolved.agent_type_name, "invoke")
  decode_agent_invocation_result(
    send(configuration, "invoke-agent", path, Some(body)),
  )
}
