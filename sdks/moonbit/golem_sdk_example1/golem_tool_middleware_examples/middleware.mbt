///|
/// The tool shape presented by the policy middleware.
#derive.tool("messages")
#warnings("-struct_never_constructed")
struct Messages {}

///|
pub fn Messages::send(message : String) -> String {
  message
}

///|
/// Forwards ordinary messages and rejects blocked messages without invoking the next layer.
#derive.tool_middleware("message-policy", presented="Messages")
#warnings("-struct_never_constructed")
struct MessagePolicy {}

///|
pub async fn MessagePolicy::send(
  underlying : MessagesUnderlying,
  message : String,
) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  if message.contains("blocked") {
    Err(
      @toolMiddleware.ToolInvokeError::ConstraintViolation(
        "messages containing 'blocked' are not allowed",
      ),
    )
  } else {
    underlying.send(message)
  }
}

///|
/// Observes metadata for every tool shape and forwards its carriers unchanged.
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
/// The public shape presented by the adapter.
#derive.tool("public-files")
#warnings("-struct_never_constructed")
struct PublicFiles {}

///|
pub fn PublicFiles::read(path : String) -> String {
  path
}

///|
/// The shape expected from the adapter's next inner layer.
#derive.tool("storage")
#warnings("-struct_never_constructed")
struct Storage {}

///|
pub fn Storage::fetch(key : String) -> String {
  key
}

///|
/// Adapts the public file shape to a differently shaped storage tool.
#derive.tool_middleware("file-adapter", presented="PublicFiles", expected="Storage")
#warnings("-struct_never_constructed")
struct FileAdapter {}

///|
pub async fn FileAdapter::read(
  underlying : StorageUnderlying,
  path : String,
) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  underlying.fetch("files/" + path)
}

///|
fn main {

}
