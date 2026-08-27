///|
#derive.tool("presented")
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
struct Nested {}

///|
pub fn Nested::run(name : String) -> String {
  name
}

///|
#derive.tool_middleware("policy", presented="Presented")
struct Policy {}

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
  underlying.stdout()
}

///|
pub async fn Policy::abandon_stdout(
  underlying : PresentedUnderlying,
) -> Result[
  @asyncCore.Stream[Byte],
  @toolMiddleware.ToolInvokeError[@tool.NoToolError],
] {
  match underlying.abandon_stdout() {
    Ok(_) =>
      Err(@toolMiddleware.ToolInvokeError::InvalidResult("abandoned stdout"))
    Err(error) => Err(error)
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
  underlying.value_stdout(value)
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
  underlying.io(input)
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
struct Storage {}

///|
pub fn Storage::fetch(key : String) -> String {
  key
}

///|
#derive.tool_middleware("adapter", presented="Presented", expected="Storage")
struct Adapter {}

///|
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
#derive.universal_tool_middleware("audit")
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
  let payload = match RunError::to_error_payload_value(Broken(message)) {
    Ok(payload) => payload
    Err(message) => raise Failure::Failure(message)
  }
  @model.typed_schema_value_to_wit(payload)
}

///|
pub fn decode_run_error_payload_for_tests(
  payload : @types.TypedSchemaValue,
) -> RunError raise {
  let payload = @model.typed_schema_value_from_wit(payload)
  match RunError::from_error_payload_value(payload) {
    Ok(error) => error
    Err(message) => raise Failure::Failure(message)
  }
}
