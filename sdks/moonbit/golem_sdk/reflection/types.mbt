///|
/// Failure reported while discovering, packing, or invoking an agent through
/// the reflection APIs.
pub(all) suberror ReflectError {
  Discovery(String)
  Identity(String)
  Schema(String)
  /// Canonical JSON was rejected. `path` locates the rejected value from the
  /// packed root through record field names, variant case names, `ok`/`err`
  /// result tags, and decimal list, tuple, flags, and map-entry indexes; a
  /// map entry adds `0` for its key or `1` for its value.
  Json(String, path~ : Array[String])
  Remote(AgentRpcError)
} derive(Debug, Eq)

///|
/// Structured RPC and remote-agent failures retained by every reflected,
/// caller-defined static, and fully dynamic agent client.
pub(all) enum AgentRpcError {
  Protocol(String)
  Denied(String)
  NotFound(String)
  RemoteInternal(String)
  InvalidInput(String)
  InvalidMethod(String)
  InvalidType(String)
  InvalidAgentId(String)
  Custom(@model.TypedSchemaValue)
  /// A keyed create found an agent that already has the key.
  Conflict(String)
  /// The awaited invocation was withdrawn before it started.
  Cancelled(String)
  /// A cancellation found the invocation already running.
  CannotCancelRunning(String)
} derive(Debug, Eq)

///|
pub(all) enum AgentMode {
  Durable
  Ephemeral
} derive(Debug, Eq)

///|
/// The final identity of one invocation. For an ephemeral agent type,
/// `agent_id` is the ID of the one-shot instance that ran it.
pub(all) struct InvocationMetadata {
  agent_id : @schema.AgentId
  invocation_id : @rpc.InvocationId
} derive(Debug, Eq)

///|
/// The result of an awaited invocation together with its final identity.
pub(all) struct InvocationOutput[T] {
  metadata : InvocationMetadata
  value : T
} derive(Debug, Eq)

///|
fn agent_error_message(error : @common.AgentError) -> String {
  match error {
    InvalidInput(message) => "invalid input: " + message
    InvalidMethod(message) => "invalid method: " + message
    InvalidType(message) => "invalid type: " + message
    InvalidAgentId(message) => "invalid agent id: " + message
    CustomError(_) => "custom remote agent error"
  }
}

///|
fn rpc_error(error : @agentHost.RpcError) -> ReflectError {
  Remote(
    match error {
      ProtocolError(message) => AgentRpcError::Protocol(message)
      Denied(message) => AgentRpcError::Denied(message)
      NotFound(message) => AgentRpcError::NotFound(message)
      RemoteInternalError(message) => AgentRpcError::RemoteInternal(message)
      Conflict(message) => AgentRpcError::Conflict(message)
      Cancelled(message) => AgentRpcError::Cancelled(message)
      CannotCancelRunning(message) =>
        AgentRpcError::CannotCancelRunning(message)
      RemoteAgentError(error) =>
        match error {
          InvalidInput(message) => AgentRpcError::InvalidInput(message)
          InvalidMethod(message) => AgentRpcError::InvalidMethod(message)
          InvalidType(message) => AgentRpcError::InvalidType(message)
          InvalidAgentId(message) => AgentRpcError::InvalidAgentId(message)
          CustomError(payload) => {
            let decoded = @model_host.typed_schema_value_from_wit(payload) catch {
              error =>
                return Remote(
                  AgentRpcError::RemoteInternal(
                    "failed to decode custom remote agent error: \{repr(error)}",
                  ),
                )
            }
            AgentRpcError::Custom(decoded)
          }
        }
    },
  )
}
