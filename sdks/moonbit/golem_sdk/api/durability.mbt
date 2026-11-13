///|
/// The executor policy for a custom durable operation.
pub(all) enum DurableFunctionType {
  ReadLocal
  WriteLocal
  ReadRemote
  WriteRemote
} derive(Debug, Eq)

///|
/// Errors detected while replaying a custom durable operation.
pub suberror DurabilityError {
  ReplayMismatch(String)
} derive(Debug, Eq)

///|
fn DurableFunctionType::to_raw(
  self : DurableFunctionType,
) -> @oplog.WrappedFunctionType {
  match self {
    ReadLocal => @oplog.WrappedFunctionType::ReadLocal
    WriteLocal => @oplog.WrappedFunctionType::WriteLocal
    ReadRemote => @oplog.WrappedFunctionType::ReadRemote
    WriteRemote => @oplog.WrappedFunctionType::WriteRemote
  }
}

///|
/// Runs a custom durable operation synchronously.
///
/// The body runs only for a live or incomplete invocation. A completed invocation returns its
/// persisted response without evaluating the body. If the body raises, the unfinished invocation
/// is dropped without triggering a retry. If recovery later replays the incomplete invocation,
/// the body runs again.
///
/// The body cannot suspend. A task spawned in it aborts the invocation when the body returns.
pub fn[
  Request : @schema.IntoSchema,
  Response : @schema.IntoSchema + @schema.FromSchema,
] durable(
  iface : String,
  function : String,
  function_type : DurableFunctionType,
  request : Request,
  forced_commit~ : Bool = false,
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
      install_task_context_hooks()
      let frame = @async_core.push_task_context_frame(capture_caller=false)
      let mut consumed = false
      defer {
        if !consumed {
          invocation.drop()
        }
        @async_core.pop_task_context_frame(frame)
      }
      let result = body()
      frame.check_joined()
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
///
/// The invocation's scope follows the task that runs the body: other tasks of the same agent
/// invocation, and their host calls, are not attributed to it while the body waits. Tasks spawned
/// in the body, including stream and future producers, inherit the scope. Join spawned tasks
/// before the body returns; a task still running when the body exits aborts the invocation. The
/// operation waits for the stream and future producers the body started, and cancels them when
/// the body fails.
pub async fn[
  Request : @schema.IntoSchema,
  Response : @schema.IntoSchema + @schema.FromSchema,
] durable_async(
  iface : String,
  function : String,
  function_type : DurableFunctionType,
  request : Request,
  forced_commit~ : Bool = false,
  body : async () -> Response,
) -> Response {
  let function_name = durable_function_name(iface, function)
  @raw_durability.observe_function_call(iface, function)
  let request = @model.typed_schema_value_to_wit(
    @schema.try_into_typed_schema_value(request),
  )
  match
    @raw_durability.begin_custom_durable_invocation_async(
      function_name,
      request,
      function_type.to_raw(),
    ) {
    Live(invocation) => {
      install_task_context_hooks()
      let frame = @async_core.push_task_context_frame(capture_caller=true)
      invocation.enter()
      let mut finished = false
      defer {
        if !finished {
          @async_core.protect_from_cancel(() => {
            frame.settle(cancel_producers=true)
          })
        }
        invocation.drop()
        @async_core.pop_task_context_frame(frame)
      }
      let result = body()
      frame.settle()
      let response = @model.typed_schema_value_to_wit(
        @schema.try_into_typed_schema_value(result),
      )
      invocation.finish_async(response, forced_commit)
      finished = true
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
  let response = @model_host.typed_schema_value_from_wit(invocation.response)
  @schema.from_value_as(response.value)
}

///|
let task_context_hooks_installed : Ref[Bool] = Ref(false)

///|
/// Lets the async runtime switch the custom invocation context of the current
/// task whenever it switches coroutines.
fn install_task_context_hooks() -> Unit {
  guard !task_context_hooks_installed.val else { return }
  @async_core.install_task_context_hooks(
    exchange=next => {
      let @raw_durability.CustomInvocationContext(previous) = @raw_durability.exchange_custom_invocation_context(
        next.map(handle => @raw_durability.CustomInvocationContext(handle)),
      )
      previous
    },
    drop=handle => @raw_durability.CustomInvocationContext(handle).drop(),
  )
  task_context_hooks_installed.val = true
}
