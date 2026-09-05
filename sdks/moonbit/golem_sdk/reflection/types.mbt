///|
/// Failure reported while discovering, packing, or invoking an agent through
/// the reflection APIs.
pub(all) suberror ReflectError {
  Discovery(String)
  Identity(String)
  Schema(String)
  Json(String)
  Remote(String)
} derive(Debug, Eq)

///|
pub(all) enum AgentMode {
  Durable
  Ephemeral
} derive(Debug, Eq)

///|
/// A reusable agent identity. Ephemeral invocation addresses do not have one
/// before invocation; their final identity is returned in invocation metadata.
pub(all) struct AgentId {
  component_id : @types.ComponentId
  value : String
} derive(Debug, Eq)

///|
pub(all) struct AgentIdParts {
  type_name : String
  constructor_value : @model.SchemaValue
  phantom_id : @types.Uuid?
} derive(Debug, Eq)

///|
pub(all) struct InvocationMetadata {
  agent_id : AgentId
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
  agent_id : AgentId
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
  match error {
    ProtocolError(message) => Remote("RPC protocol error: " + message)
    Denied(message) => Remote("RPC denied: " + message)
    NotFound(message) => Remote("RPC target not found: " + message)
    RemoteInternalError(message) => Remote("remote internal error: " + message)
    RemoteAgentError(error) => Remote(agent_error_message(error))
  }
}
