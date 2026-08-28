/// An operation that can be retried by middleware.
#derive.tool("unstable")
struct UnstableTool {}

pub fn UnstableTool::execute(value : String) -> String {
  value
}

/// Retries the next inner layer once after a failure.
#derive.tool_middleware("retry-once", presented="UnstableTool")
struct RetryOnce {}

pub async fn RetryOnce::execute(
  underlying : UnstableToolUnderlying,
  value : String,
) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  match underlying.execute(value) {
    Ok(result) => Ok(result)
    Err(_) => underlying.execute(value)
  }
}
