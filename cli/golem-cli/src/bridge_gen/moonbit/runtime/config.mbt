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

///|
/// Target Golem server for the generated bridge client. Each variant resolves
/// to a base worker-service URL and an authentication token.
pub(all) enum GolemServer {
  /// Local single-executable Golem server on `http://localhost:9881`.
  Local
  /// Golem Cloud (release region), with the given authentication token.
  Cloud(String)
  /// A custom worker-service deployment: `Custom(url, token)`.
  Custom(String, String)
} derive(Debug, Eq)

///|
let local_well_known_token : String = "5c832d93-ff85-4a8f-9803-513950fdfdb1"

///|
/// Base URL of the worker service, without a trailing slash.
pub fn GolemServer::url(self : GolemServer) -> String {
  match self {
    Local => "http://localhost:9881"
    Cloud(_) => "https://release.api.golem.cloud"
    Custom(url, _) => url.trim_end(chars="/").to_owned()
  }
}

///|
/// Bearer token used for the `Authorization` header.
pub fn GolemServer::token(self : GolemServer) -> String {
  match self {
    Local => local_well_known_token
    Cloud(token) => token
    Custom(_, token) => token
  }
}

///|
/// How long a streaming call keeps reconnecting without progress: a
/// connection that stays up for 5 seconds after the server accepted its start
/// or a resume.
pub(all) enum ReconnectPolicy {
  /// Reconnects until the caller cancels the call.
  Unlimited
  /// Fails with [BridgeError::Unreachable] once the given number of
  /// milliseconds pass without progress.
  Within(Int)
} derive(Debug, Eq)

///|
/// The reconnect policy of a configuration that does not set one: 5 minutes.
pub let default_reconnect_policy : ReconnectPolicy = Within(5 * 60 * 1000)

///|
/// Connection configuration shared by all generated bridge clients in this
/// project. Holds the target [GolemServer], the application/environment names
/// the agents live in, and the reconnect policy of streaming calls.
pub(all) struct Configuration {
  server : GolemServer
  app_name : String
  env_name : String
  reconnect : ReconnectPolicy
} derive(Debug, Eq)

///|
let config_cell : Ref[Configuration?] = { val: None }

///|
/// Configure all bridge clients in this project. Can be called repeatedly to
/// point the clients at a different server.
pub fn configure(
  server : GolemServer,
  app_name : String,
  env_name : String,
  reconnect? : ReconnectPolicy = default_reconnect_policy,
) -> Unit {
  config_cell.val = Some({ server, app_name, env_name, reconnect })
}

///|
/// Replace the active configuration wholesale.
pub fn set_configuration(configuration : Configuration) -> Unit {
  config_cell.val = Some(configuration)
}

///|
/// The active configuration, or a raised [BridgeError] if [configure] was
/// never called.
pub fn get_configuration() -> Configuration raise {
  match config_cell.val {
    Some(configuration) => configuration
    None =>
      raise BridgeError(
        "Golem bridge configuration has not been set. Call configure(...) first.",
      )
  }
}

///|
/// Whether a configuration has been set.
pub fn is_configured() -> Bool {
  config_cell.val is Some(_)
}
