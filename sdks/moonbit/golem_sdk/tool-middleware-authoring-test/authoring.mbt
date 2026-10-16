///|
#derive.tool("presented")
#warnings("-struct_never_constructed")
struct Presented {}

///|
#derive.golem_schema
pub(all) enum RunError {
  #derive.error(kind="runtime-error")
  Broken(String)
}

///|
pub fn Presented::unit() -> Unit {
  ()
}

///|
pub fn Presented::value(value : String) -> Int {
  value.length()
}

///|
pub fn Presented::retry(value : String) -> Int {
  value.length()
}

///|
pub fn Presented::stdout(output : @asyncCore.Sink[Byte]) -> Unit {
  ignore(output)
}

///|
pub fn Presented::abandon_stdout(output : @asyncCore.Sink[Byte]) -> Unit {
  ignore(output)
}

///|
pub fn Presented::value_stdout(
  value : String,
  output : @asyncCore.Sink[Byte],
) -> Int {
  ignore(output)
  value.length()
}

///|
pub fn Presented::io(
  input : @asyncCore.Stream[Byte],
  output : @asyncCore.Sink[Byte],
  principal : @tool.Principal,
) -> Result[String, RunError] {
  ignore(input)
  ignore(output)
  ignore(principal)
  Ok("done")
}

///|
#derive.command(subtree="Nested")
pub fn Presented::admin(verbose : Bool) -> Unit {
  ignore(verbose)
}

///|
#derive.tool("nested")
#warnings("-struct_never_constructed")
struct Nested {}

///|
pub fn Nested::run(name : String) -> String {
  name
}

///|
#derive.tool_middleware("policy", version="1.2.3", presented="Presented")
#warnings("-struct_never_constructed")
struct Policy {}

///|
async fn[T, E] collect_started_stdout(
  invocation : @toolMiddleware.UnderlyingInvocation[T, E],
) -> Result[(T, @asyncCore.Stream[Byte]), @toolMiddleware.ToolInvokeError[E]] {
  let source = match invocation.stdout() {
    Some(source) => source
    None =>
      return match invocation.get() {
        Err(error) => Err(error)
        Ok(_) =>
          Err(
            @toolMiddleware.ToolInvokeError::InvalidResult(
              "tool result did not contain declared stdout stream",
            ),
          )
      }
  }
  @asyncCore.with_task_group(async fn(group) {
    let collected = group.spawn(() => {
      let bytes : Array[Byte] = []
      defer source.drop()
      try {
        for ;; {
          match source.read(65536) {
            Some(chunk) => bytes.append(chunk)
            None => break
          }
        }
        Ok(bytes)
      } catch {
        error =>
          Err(@toolMiddleware.ToolInvokeError::InvalidResult(repr(error)))
      }
    })
    let result = invocation.get()
    let bytes = match collected.wait() {
      Ok(bytes) => bytes
      Err(error) => return Err(error)
    }
    match result {
      Err(error) => Err(error)
      Ok(result) =>
        Ok(
          (
            result,
            @asyncCore.Stream::produce(async fn(sink) {
              ignore(sink.write_all(bytes[:]))
              sink.close()
            }),
          ),
        )
    }
  })
}

///|
pub async fn Policy::unit(
  underlying : PresentedUnderlying,
) -> Result[Unit, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  underlying.unit()
}

///|
pub async fn Policy::value(
  underlying : PresentedUnderlying,
  value : String,
) -> Result[Int, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  underlying.value(value)
}

///|
pub async fn Policy::retry(
  underlying : PresentedUnderlying,
  value : String,
) -> Result[Int, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  match underlying.retry(value) {
    Ok(_) => underlying.retry(value)
    Err(error) => Err(error)
  }
}

///|
pub async fn Policy::stdout(
  underlying : PresentedUnderlying,
) -> Result[
  @asyncCore.Stream[Byte],
  @toolMiddleware.ToolInvokeError[@tool.NoToolError],
] {
  match underlying.start_stdout() {
    Err(error) => Err(error)
    Ok(invocation) =>
      match collect_started_stdout(invocation) {
        Ok((_, stdout)) => Ok(stdout)
        Err(error) => Err(error)
      }
  }
}

///|
pub async fn Policy::abandon_stdout(
  underlying : PresentedUnderlying,
) -> Result[
  @asyncCore.Stream[Byte],
  @toolMiddleware.ToolInvokeError[@tool.NoToolError],
] {
  match underlying.start_abandon_stdout() {
    Err(error) => Err(error)
    Ok(invocation) =>
      match collect_started_stdout(invocation) {
        Ok(_) =>
          Err(
            @toolMiddleware.ToolInvokeError::InvalidResult("abandoned stdout"),
          )
        Err(error) => Err(error)
      }
  }
}

///|
pub async fn Policy::value_stdout(
  underlying : PresentedUnderlying,
  value : String,
) -> Result[
  (Int, @asyncCore.Stream[Byte]),
  @toolMiddleware.ToolInvokeError[@tool.NoToolError],
] {
  match underlying.start_value_stdout(value) {
    Err(error) => Err(error)
    Ok(invocation) => collect_started_stdout(invocation)
  }
}

///|
pub async fn Policy::io(
  underlying : PresentedUnderlying,
  input : @toolMiddleware.MiddlewareStdin,
  principal : @tool.Principal,
) -> Result[
  (String, @asyncCore.Stream[Byte]),
  @toolMiddleware.ToolInvokeError[RunError],
] {
  ignore(principal)
  match underlying.start_io(input) {
    Err(error) => Err(error)
    Ok(invocation) => collect_started_stdout(invocation)
  }
}

///|
pub async fn Policy::admin__run(
  underlying : PresentedUnderlying,
  verbose : Bool,
  name : String,
) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  underlying.admin__run(verbose, name)
}

///|
#derive.tool("storage")
#warnings("-struct_never_constructed")
struct Storage {}

///|
pub fn Storage::fetch(key : String) -> String {
  key
}

///|
#derive.tool_middleware("adapter", version="2.0.0", presented="Presented", expected="Storage")
#warnings("-struct_never_constructed")
struct Adapter {}

///|
#warnings("-unused_async")
pub async fn Adapter::unit(
  underlying : StorageUnderlying,
) -> Result[Unit, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  ignore(underlying)
  Ok(())
}

///|
pub async fn Adapter::value(
  underlying : StorageUnderlying,
  value : String,
) -> Result[Int, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  match underlying.fetch(value) {
    Ok(value) => Ok(value.length())
    Err(error) => Err(error)
  }
}

///|
pub async fn Adapter::retry(
  underlying : StorageUnderlying,
  value : String,
) -> Result[Int, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  match underlying.fetch(value) {
    Ok(value) => Ok(value.length())
    Err(error) => Err(error)
  }
}

///|
#warnings("-unused_async")
pub async fn Adapter::stdout(
  underlying : StorageUnderlying,
) -> Result[
  @asyncCore.Stream[Byte],
  @toolMiddleware.ToolInvokeError[@tool.NoToolError],
] {
  ignore(underlying)
  abort("compile fixture")
}

///|
#warnings("-unused_async")
pub async fn Adapter::abandon_stdout(
  underlying : StorageUnderlying,
) -> Result[
  @asyncCore.Stream[Byte],
  @toolMiddleware.ToolInvokeError[@tool.NoToolError],
] {
  ignore(underlying)
  abort("compile fixture")
}

///|
#warnings("-unused_async")
pub async fn Adapter::value_stdout(
  underlying : StorageUnderlying,
  value : String,
) -> Result[
  (Int, @asyncCore.Stream[Byte]),
  @toolMiddleware.ToolInvokeError[@tool.NoToolError],
] {
  ignore(underlying)
  ignore(value)
  abort("compile fixture")
}

///|
#warnings("-unused_async")
pub async fn Adapter::io(
  underlying : StorageUnderlying,
  input : @toolMiddleware.MiddlewareStdin,
  principal : @tool.Principal,
) -> Result[
  (String, @asyncCore.Stream[Byte]),
  @toolMiddleware.ToolInvokeError[RunError],
] {
  ignore(underlying)
  ignore(input)
  ignore(principal)
  abort("compile fixture")
}

///|
pub async fn Adapter::admin__run(
  underlying : StorageUnderlying,
  verbose : Bool,
  name : String,
) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  ignore(verbose)
  underlying.fetch(name)
}

///|
#derive.universal_tool_middleware("audit", version="3.1.4")
pub async fn audit(
  invocation : @toolMiddleware.RawToolInvocation,
  underlying : @toolMiddleware.UnderlyingTool,
) -> Result[
  @toolMiddleware.RawInvocationResult,
  @toolMiddleware.ToolInvokeError[@toolMiddleware.RawTypedSchemaValue],
] {
  ignore(invocation.tool_name())
  ignore(invocation.tool_metadata())
  ignore(invocation.principal())
  underlying.invoke_raw(
    invocation.command_path(),
    invocation.input(),
    invocation.stdin(),
  )
}

///|
pub fn register_generated_middlewares_for_tests() -> Unit raise {
  __golem_register_tool_middlewares()
}

///|
pub fn generated_input_for_tests(
  command_path : Array[String],
  params : Array[(String, @model.SchemaValue)],
) -> @types.TypedSchemaValue raise {
  let def = __golem_middleware_tool_def_Presented()
  let index = match def.command_index_by_path(command_path) {
    Some(index) => index
    None => raise Failure::Failure("unknown generated test command")
  }
  let input = @tool.build_canonical_input(
    def.canonical_input_model(index),
    params,
  )
  @model.typed_schema_value_to_wit(input)
}

///|
pub fn run_error_payload_for_tests(
  message : String,
) -> @types.TypedSchemaValue raise {
  let (_, payload) = match RunError::to_error_payload_value(Broken(message)) {
    Ok(named) => named
    Err(message) => raise Failure::Failure(message)
  }
  @model.typed_schema_value_to_wit(payload)
}

///|
pub fn decode_run_error_payload_for_tests(
  payload : @types.TypedSchemaValue,
) -> RunError raise {
  let payload = @model_host.typed_schema_value_from_wit(payload)
  match RunError::from_error_payload_value("broken", payload) {
    Ok(error) => error
    Err(message) => raise Failure::Failure(message)
  }
}
