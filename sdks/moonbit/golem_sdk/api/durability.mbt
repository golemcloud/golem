///|
/// The executor policy for a custom durable operation.
pub(all) enum DurableFunctionType {
  ReadLocal
  WriteLocal
  ReadRemote
  WriteRemote
  WriteRemoteBatched(UInt64?)
  WriteRemoteTransaction(UInt64?)
} derive(Show, Eq)

///|
/// Errors detected while replaying a custom durable operation.
pub suberror DurabilityError {
  ReplayMismatch(String)
} derive(Show, Eq)

///|
fn DurableFunctionType::to_raw(
  self : DurableFunctionType,
) -> @oplog.WrappedFunctionType {
  match self {
    ReadLocal => @oplog.WrappedFunctionType::ReadLocal
    WriteLocal => @oplog.WrappedFunctionType::WriteLocal
    ReadRemote => @oplog.WrappedFunctionType::ReadRemote
    WriteRemote => @oplog.WrappedFunctionType::WriteRemote
    WriteRemoteBatched(parent) =>
      @oplog.WrappedFunctionType::WriteRemoteBatched(parent)
    WriteRemoteTransaction(parent) =>
      @oplog.WrappedFunctionType::WriteRemoteTransaction(parent)
  }
}

///|
/// Runs a custom durable operation synchronously.
///
/// The body runs only for a live or incomplete invocation. A completed invocation returns its
/// persisted response without evaluating the body. If the body raises, the unfinished invocation
/// is dropped and normal recovery re-executes it.
pub fn[
  Request : @schema.IntoSchema,
  Response : @schema.IntoSchema + @schema.FromSchema,
] durable(
  iface : String,
  function : String,
  function_type : DurableFunctionType,
  request : Request,
  forced_commit? : Bool = false,
  body : () -> Response,
) -> Response raise {
  let function_name = durable_function_name(iface, function)
  @raw_durability.observe_function_call(iface, function)
  let request = @model.typed_schema_value_to_wit(
    @schema.try_into_typed_schema_value(request),
  )
  match
    @raw_durability.begin_custom_durable_invocation(
      function_name,
      request,
      function_type.to_raw(),
    ) {
    Live(invocation) => {
      let mut consumed = false
      defer (if !consumed { invocation.drop() })
      let result = body()
      let response = @model.typed_schema_value_to_wit(
        @schema.try_into_typed_schema_value(result),
      )
      consumed = true
      invocation.finish(response, forced_commit)
      result
    }
    Replayed(invocation) =>
      decode_durable_replay(invocation, function_name, function_type)
  }
}

///|
/// Runs a custom durable operation asynchronously.
///
/// The live invocation remains active while the asynchronous body is suspended. Replayed
/// invocations return immediately without evaluating the body.
pub async fn[
  Request : @schema.IntoSchema,
  Response : @schema.IntoSchema + @schema.FromSchema,
] durable_async(
  iface : String,
  function : String,
  function_type : DurableFunctionType,
  request : Request,
  forced_commit? : Bool = false,
  body : async () -> Response,
) -> Response {
  let function_name = durable_function_name(iface, function)
  @raw_durability.observe_function_call(iface, function)
  let request = @model.typed_schema_value_to_wit(
    @schema.try_into_typed_schema_value(request),
  )
  match
    @raw_durability.begin_custom_durable_invocation(
      function_name,
      request,
      function_type.to_raw(),
    ) {
    Live(invocation) => {
      let mut consumed = false
      defer (if !consumed { invocation.drop() })
      let result = body()
      let response = @model.typed_schema_value_to_wit(
        @schema.try_into_typed_schema_value(result),
      )
      consumed = true
      invocation.finish(response, forced_commit)
      result
    }
    Replayed(invocation) =>
      decode_durable_replay(invocation, function_name, function_type)
  }
}

///|
fn durable_function_name(iface : String, function : String) -> String {
  if iface == "" {
    function
  } else {
    iface + "::" + function
  }
}

///|
fn[Response : @schema.FromSchema] decode_durable_replay(
  invocation : @raw_durability.PersistedDurableFunctionInvocation,
  function_name : String,
  function_type : DurableFunctionType,
) -> Response raise {
  guard invocation.function_name == function_name else {
    raise DurabilityError::ReplayMismatch(
      "expected function '\{function_name}', oplog has '\{invocation.function_name}'",
    )
  }
  guard invocation.function_type == function_type.to_raw() else {
    raise DurabilityError::ReplayMismatch(
      "function type differs for '\{function_name}'",
    )
  }
  let response = @model.typed_schema_value_from_wit(invocation.response)
  @schema.from_value_as(response.value)
}
