///|
/// Failure reported while discovering, packing, or invoking an agent through
/// the reflection APIs.
pub(all) suberror ReflectError {
  Discovery(String)
  Identity(String)
  Schema(String)
  Json(String)
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
} derive(Debug, Eq)

///|
pub(all) enum AgentMode {
  Durable
  Ephemeral
} derive(Debug, Eq)

///|
/// A reusable agent identity. Ephemeral invocation addresses do not have one
/// before invocation; their final identity is returned in invocation metadata.
pub(all) struct ParsedAgentId {
  value : String
} derive(Debug, Eq)

///|
pub(all) struct ParsedAgentIdParts {
  type_name : String
  constructor_value : @model.SchemaValue
  phantom_id : @types.Uuid?
} derive(Debug, Eq)

///|
pub(all) struct InvocationMetadata {
  agent_id : ParsedAgentId
  idempotency_key : String
} derive(Debug, Eq)

///|
pub(all) struct Invocation[T] {
  metadata : InvocationMetadata
  value : T
} derive(Debug, Eq)

///|
pub(all) struct ScheduledInvocation {
  metadata : InvocationMetadata
  cancellation_token : @agentHost.CancellationToken
}

///|
/// Both identities produced when attempting to create a durable phantom.
pub(all) struct CreatedPhantomClient {
  agent_id : ParsedAgentId
  phantom_id : @types.Uuid
  client : DynamicAgentClient
}

///|
pub fn ScheduledInvocation::cancel(self : ScheduledInvocation) -> Unit {
  self.cancellation_token.cancel()
}

///|
pub fn ScheduledInvocation::drop(self : ScheduledInvocation) -> Unit {
  self.cancellation_token.drop()
}

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
