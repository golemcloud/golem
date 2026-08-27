///|
priv enum RawCarrierState {
  Available
  TransferredLocal
  TransferredUnderlying
  TransferredFinal
  Dropped
} derive(Eq)

///|
priv struct RawTypedSchemaValueState {
  mut value : @types.TypedSchemaValue?
  resources : ResourceCarrierOwnership
  mut ownership : RawCarrierState
}

///|
/// Opaque zero-copy owner of one WIT typed-schema-value. Copies share a
/// take-once state, including when the value contains no resource handles.
pub struct RawTypedSchemaValue {
  priv context : OwnershipContext
  priv state : Ref[RawTypedSchemaValueState]
}

///|
fn raw_resource_candidates(
  value : @types.TypedSchemaValue,
) -> Array[ResourceCandidate] {
  let candidates : Array[ResourceCandidate] = []
  for node in value.value.value_nodes {
    match node {
      @types.SchemaValueNode::SecretValue(secret) => {
        let @types.Secret(raw_index) = secret
        candidates.push(
          resource_candidate(ResourceKind::Secret, raw_index, () => {
            secret.drop()
          }),
        )
      }
      @types.SchemaValueNode::QuotaTokenHandle(token) => {
        let @types.QuotaToken(raw_index) = token
        candidates.push(
          resource_candidate(ResourceKind::QuotaToken, raw_index, () => {
            token.drop()
          }),
        )
      }
      @types.SchemaValueNode::PermissionCardHandle(card) => {
        let @types.PermissionCard(raw_index) = card
        candidates.push(
          resource_candidate(ResourceKind::PermissionCard, raw_index, () => {
            card.drop()
          }),
        )
      }
      _ => ()
    }
  }
  candidates
}

///|
#doc(hidden)
pub fn OwnershipContext::own_raw_typed_schema_value(
  self : OwnershipContext,
  value : @types.TypedSchemaValue,
) -> RawTypedSchemaValue {
  let state : Ref[RawTypedSchemaValueState] = {
    val: {
      value: Some(value),
      resources: self.acquire_resources(raw_resource_candidates(value)),
      ownership: Available,
    },
  }
  let owned : RawTypedSchemaValue = { context: self, state }
  self.register_cleanup(() => owned.drop_if_available())
  owned
}

///|
fn RawTypedSchemaValue::drop_if_available(self : RawTypedSchemaValue) -> Unit {
  if self.state.val.ownership == Available {
    self.state.val.value = None
    self.state.val.ownership = Dropped
    ignore(self.state.val.resources.drop())
  }
}

///|
fn RawTypedSchemaValue::take(
  self : RawTypedSchemaValue,
  context : OwnershipContext,
  target : RawCarrierState,
) -> Result[@types.TypedSchemaValue, String] {
  if !physical_equal(self.context.state, context.state) {
    return Err("raw typed schema value belongs to another invocation")
  }
  if self.context.is_revoked() {
    self.drop_if_available()
    return Err("middleware invocation ownership has been revoked")
  }
  if self.state.val.ownership != Available {
    return Err("raw typed schema value is no longer available")
  }
  let value = match self.state.val.value {
    Some(value) => value
    None => return Err("raw typed schema value is no longer available")
  }
  let transition = match target {
    TransferredUnderlying => self.state.val.resources.transfer_underlying()
    TransferredFinal => self.state.val.resources.transfer_final()
    Available | TransferredLocal | Dropped =>
      abort("invalid raw typed value transfer target")
  }
  match transition {
    Err(message) => Err(message)
    Ok(_) => {
      self.state.val.value = None
      self.state.val.ownership = target
      Ok(value)
    }
  }
}

///|
fn RawTypedSchemaValue::decode_model_for_local(
  self : RawTypedSchemaValue,
  context : OwnershipContext,
) -> Result[@model.TypedSchemaValue, String] {
  if !physical_equal(self.context.state, context.state) {
    return Err("raw typed schema value belongs to another invocation")
  }
  if self.context.is_revoked() {
    self.drop_if_available()
    return Err("middleware invocation ownership has been revoked")
  }
  if self.state.val.ownership != Available {
    return Err("raw typed schema value is no longer available")
  }
  let value = match self.state.val.value {
    Some(value) => value
    None => return Err("raw typed schema value is no longer available")
  }
  match self.state.val.resources.check_available() {
    Err(message) => {
      self.drop_if_available()
      return Err(message)
    }
    Ok(_) => ()
  }
  match self.state.val.resources.transfer_local() {
    Err(message) => return Err(message)
    Ok(_) => ()
  }
  self.state.val.value = None
  self.state.val.ownership = TransferredLocal
  let decoded = @model.typed_schema_value_from_wit(value) catch {
    error => {
      @model.drop_wit_capabilities(value.value)
      return Err("failed to decode middleware input: \{error}")
    }
  }
  context.register_cleanup(fn() {
    @model.drop_owned_capabilities(decoded.value)
  })
  Ok(decoded)
}

///|
#doc(hidden)
pub fn RawTypedSchemaValue::take_for_underlying(
  self : RawTypedSchemaValue,
  context : OwnershipContext,
) -> Result[@types.TypedSchemaValue, String] {
  self.take(context, TransferredUnderlying)
}

///|
#doc(hidden)
pub fn RawTypedSchemaValue::take_for_final(
  self : RawTypedSchemaValue,
  context : OwnershipContext,
) -> Result[@types.TypedSchemaValue, String] {
  self.take(context, TransferredFinal)
}

///|
/// Releases this carrier if it has not already been forwarded or returned.
pub fn RawTypedSchemaValue::drop(
  self : RawTypedSchemaValue,
) -> Result[Unit, String] {
  if self.state.val.ownership != Available {
    return Err("raw typed schema value is no longer available")
  }
  self.drop_if_available()
  Ok(())
}

///|
/// Returns the resource-free schema graph without decoding or rebuilding it.
pub fn RawTypedSchemaValue::schema_graph(
  self : RawTypedSchemaValue,
) -> Result[@types.SchemaGraph, String] {
  match self.state.val.value {
    Some(value) if self.state.val.ownership == Available => Ok(value.graph)
    _ => Err("raw typed schema value is no longer available")
  }
}

///|
priv struct RawInvocationResultState {
  mut result : RawTypedSchemaValue?
  mut stdout : @async-core.Stream[Byte]?
  mut ownership : RawCarrierState
}

///|
/// Opaque zero-copy owner of an underlying invocation result and stdout.
pub struct RawInvocationResult {
  priv context : OwnershipContext
  priv state : Ref[RawInvocationResultState]
}

///|
fn OwnershipContext::own_raw_invocation_result(
  self : OwnershipContext,
  result : @toolCommon.InvocationResult,
) -> RawInvocationResult {
  let state : Ref[RawInvocationResultState] = {
    val: {
      result: result.result.map(value => self.own_raw_typed_schema_value(value)),
      stdout: result.stdout,
      ownership: Available,
    },
  }
  let owned : RawInvocationResult = { context: self, state }
  self.register_cleanup(() => owned.drop_if_available())
  owned
}

///|
fn RawInvocationResult::drop_if_available(self : RawInvocationResult) -> Unit {
  if self.state.val.ownership == Available {
    let result = self.state.val.result
    let stdout = self.state.val.stdout
    self.state.val.result = None
    self.state.val.stdout = None
    self.state.val.ownership = Dropped
    match result {
      Some(result) => ignore(result.drop())
      None => ()
    }
    match stdout {
      Some(stdout) => stdout.drop_sync()
      None => ()
    }
  }
}

///|
#doc(hidden)
pub fn RawInvocationResult::take_for_final(
  self : RawInvocationResult,
  context : OwnershipContext,
) -> Result[@toolCommon.InvocationResult, String] {
  if !physical_equal(self.context.state, context.state) {
    return Err("raw invocation result belongs to another invocation")
  }
  if self.context.is_revoked() {
    self.drop_if_available()
    return Err("middleware invocation ownership has been revoked")
  }
  if self.state.val.ownership != Available {
    return Err("raw invocation result is no longer available")
  }
  let result = match self.state.val.result {
    Some(result) =>
      match result.take_for_final(context) {
        Ok(value) => Some(value)
        Err(message) => return Err(message)
      }
    None => None
  }
  let stdout = self.state.val.stdout
  self.state.val.result = None
  self.state.val.stdout = None
  self.state.val.ownership = TransferredFinal
  Ok({ result, stdout })
}

///|
/// Releases a raw result that will not be returned by the middleware.
pub fn RawInvocationResult::drop(
  self : RawInvocationResult,
) -> Result[Unit, String] {
  if self.state.val.ownership != Available {
    return Err("raw invocation result is no longer available")
  }
  self.drop_if_available()
  Ok(())
}

///|
/// Runtime invocation context passed to universal middleware. Resource-bearing
/// input and stdin remain opaque take-once owners.
pub struct RawToolInvocation {
  priv context : OwnershipContext
  priv tool_name : String
  priv tool_metadata : @toolCommon.Tool
  priv command_path : Array[String]
  priv input : RawTypedSchemaValue
  priv stdin : MiddlewareStdin?
  priv principal : @agentCommon.Principal
}

///|
#doc(hidden)
pub fn RawToolInvocation::from_wit(
  context : OwnershipContext,
  tool_name : String,
  tool_metadata : @toolCommon.Tool,
  command_path : Array[String],
  input : @types.TypedSchemaValue,
  stdin : @async-core.Stream[Byte]?,
  principal : @agentCommon.Principal,
) -> RawToolInvocation {
  {
    context,
    tool_name,
    tool_metadata,
    command_path,
    input: context.own_raw_typed_schema_value(input),
    stdin: stdin.map(stream => context.own_stdin(stream)),
    principal,
  }
}

///|
pub fn RawToolInvocation::tool_name(self : RawToolInvocation) -> String {
  self.tool_name
}

///|
pub fn RawToolInvocation::tool_metadata(
  self : RawToolInvocation,
) -> @toolCommon.Tool {
  self.tool_metadata
}

///|
pub fn RawToolInvocation::command_path(
  self : RawToolInvocation,
) -> Array[String] {
  self.command_path.copy()
}

///|
pub fn RawToolInvocation::input(
  self : RawToolInvocation,
) -> RawTypedSchemaValue {
  self.input
}

///|
pub fn RawToolInvocation::stdin(self : RawToolInvocation) -> MiddlewareStdin? {
  self.stdin
}

///|
pub fn RawToolInvocation::principal(
  self : RawToolInvocation,
) -> @agentCommon.Principal {
  self.principal
}

///|
/// Creates a resource-free short-circuit result tied to this invocation.
pub fn RawToolInvocation::empty_result(
  self : RawToolInvocation,
) -> RawInvocationResult {
  self.context.own_raw_invocation_result({ result: None, stdout: None })
}

///|
#doc(hidden)
pub fn[E] RawToolInvocation::decode_input(
  self : RawToolInvocation,
  def : @toolCore.ToolDef,
  command_index : Int,
  context : String,
) -> Result[Array[@toolCore.CanonicalInputValue], ToolInvokeError[E]] {
  let typed = match self.input.decode_model_for_local(self.context) {
    Ok(typed) => typed
    Err(message) => return Err(InvalidInput(message))
  }
  Ok(@toolCore.decode_canonical_input_value(def, command_index, typed, context)) catch {
    @toolCommon.ToolError::InvalidToolName(name) => Err(InvalidToolName(name))
    @toolCommon.ToolError::InvalidCommandPath(path) =>
      Err(InvalidCommandPath(path))
    @toolCommon.ToolError::InvalidInput(message) => Err(InvalidInput(message))
    @toolCommon.ToolError::ConstraintViolation(message) =>
      Err(ConstraintViolation(message))
    @toolCommon.ToolError::InvalidResult(message) => Err(InvalidResult(message))
    @toolCommon.ToolError::CustomError(value) => {
      @model.drop_wit_capabilities(value.value)
      Err(
        InvalidResult("canonical input decoding returned a custom tool error"),
      )
    }
  }
}

///|
fn[T : @schema.IntoSchema] encode_middleware_result_value(
  value : T,
) -> Result[@types.TypedSchemaValue, String] {
  let typed = @schema.try_into_typed_schema_value(value) catch {
    error => return Err("failed to build middleware result schema: \{error}")
  }
  match @toolCore.encode_typed_input(typed) {
    Ok(encoded) => Ok(encoded)
    Err(message) => {
      @model.drop_owned_capabilities(typed.value)
      Err("failed to encode middleware result: \{message}")
    }
  }
}

///|
#doc(hidden)
pub fn[E] RawToolInvocation::typed_result_empty(
  self : RawToolInvocation,
) -> Result[RawInvocationResult, ToolInvokeError[E]] {
  Ok(self.context.own_raw_invocation_result({ result: None, stdout: None }))
}

///|
#doc(hidden)
pub fn[T : @schema.IntoSchema, E] RawToolInvocation::typed_result_value(
  self : RawToolInvocation,
  value : T,
) -> Result[RawInvocationResult, ToolInvokeError[E]] {
  match encode_middleware_result_value(value) {
    Ok(value) =>
      Ok(
        self.context.own_raw_invocation_result({
          result: Some(value),
          stdout: None,
        }),
      )
    Err(message) => Err(InvalidResult(message))
  }
}

///|
#doc(hidden)
pub fn[E] RawToolInvocation::typed_result_stdout(
  self : RawToolInvocation,
  stdout : @async-core.Stream[Byte],
) -> Result[RawInvocationResult, ToolInvokeError[E]] {
  Ok(
    self.context.own_raw_invocation_result({
      result: None,
      stdout: Some(stdout),
    }),
  )
}

///|
#doc(hidden)
pub fn[T : @schema.IntoSchema, E] RawToolInvocation::typed_result_value_with_stdout(
  self : RawToolInvocation,
  value : T,
  stdout : @async-core.Stream[Byte],
) -> Result[RawInvocationResult, ToolInvokeError[E]] {
  match encode_middleware_result_value(value) {
    Ok(value) =>
      Ok(
        self.context.own_raw_invocation_result({
          result: Some(value),
          stdout: Some(stdout),
        }),
      )
    Err(message) => {
      stdout.drop_sync()
      Err(InvalidResult(message))
    }
  }
}

///|
#doc(hidden)
pub fn[E] RawToolInvocation::encode_handler_error(
  self : RawToolInvocation,
  error : ToolInvokeError[E],
  encode_custom_error : (E) -> Result[@model.TypedSchemaValue, String],
) -> ToolInvokeError[RawTypedSchemaValue] {
  match error {
    InvalidToolName(name) => InvalidToolName(name)
    InvalidCommandPath(path) => InvalidCommandPath(path)
    InvalidInput(message) => InvalidInput(message)
    ConstraintViolation(message) => ConstraintViolation(message)
    InvalidResult(message) => InvalidResult(message)
    Tool(error) => {
      let typed = match encode_custom_error(error) {
        Ok(typed) => typed
        Err(message) => return InvalidResult(message)
      }
      match @toolCore.encode_typed_input(typed) {
        Ok(encoded) => Tool(self.context.own_raw_typed_schema_value(encoded))
        Err(message) => {
          @model.drop_owned_capabilities(typed.value)
          InvalidResult("failed to encode middleware custom error: \{message}")
        }
      }
    }
  }
}

///|
pub async fn UnderlyingTool::invoke_raw(
  self : UnderlyingTool,
  command_path : Array[String],
  input : RawTypedSchemaValue,
  stdin : MiddlewareStdin?,
) -> Result[RawInvocationResult, ToolInvokeError[RawTypedSchemaValue]] {
  match self.ownership.begin_call() {
    Err(error) => return Err(error)
    Ok(_) => ()
  }
  defer self.ownership.finish_call()
  match stdin {
    Some(stdin) =>
      match stdin.check_for_underlying(self.ownership) {
        Ok(_) => ()
        Err(error) => return Err(InvalidInput(error))
      }
    None => ()
  }
  let wire_input = match input.take_for_underlying(self.ownership) {
    Ok(value) => value
    Err(error) => return Err(InvalidInput(error))
  }
  let wire_stdin = match stdin {
    Some(stdin) =>
      match stdin.take_for_underlying() {
        Ok(stream) => Some(stream)
        Err(error) => return Err(InvalidInput(error))
      }
    None => None
  }
  match (self.invoke)(command_path.copy(), wire_input, wire_stdin) {
    Ok(result) => Ok(self.ownership.own_raw_invocation_result(result))
    Err(error) =>
      Err(
        match error {
          @toolCommon.ToolError::InvalidToolName(name) => InvalidToolName(name)
          @toolCommon.ToolError::InvalidCommandPath(path) =>
            InvalidCommandPath(path)
          @toolCommon.ToolError::InvalidInput(message) => InvalidInput(message)
          @toolCommon.ToolError::ConstraintViolation(message) =>
            ConstraintViolation(message)
          @toolCommon.ToolError::InvalidResult(message) =>
            InvalidResult(message)
          @toolCommon.ToolError::CustomError(value) =>
            Tool(self.ownership.own_raw_typed_schema_value(value))
        },
      )
  }
}

///|
#doc(hidden)
pub fn encode_raw_tool_error(
  error : ToolInvokeError[RawTypedSchemaValue],
  context : OwnershipContext,
) -> @toolCommon.ToolError {
  match error {
    InvalidToolName(name) => @toolCommon.ToolError::InvalidToolName(name)
    InvalidCommandPath(path) => @toolCommon.ToolError::InvalidCommandPath(path)
    InvalidInput(message) => @toolCommon.ToolError::InvalidInput(message)
    ConstraintViolation(message) =>
      @toolCommon.ToolError::ConstraintViolation(message)
    InvalidResult(message) => @toolCommon.ToolError::InvalidResult(message)
    Tool(value) =>
      match value.take_for_final(context) {
        Ok(value) => @toolCommon.ToolError::CustomError(value)
        Err(message) => @toolCommon.ToolError::InvalidResult(message)
      }
  }
}
