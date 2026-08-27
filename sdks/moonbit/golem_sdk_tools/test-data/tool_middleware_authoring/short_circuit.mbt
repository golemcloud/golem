/// A protected operation.
#derive.tool("protected")
struct ProtectedTool {}

pub fn ProtectedTool::execute(value : String) -> String {
  value
}

/// Rejects an invocation without calling the still-declared expected tool.
#derive.tool_middleware("deny-policy", presented="ProtectedTool")
struct DenyPolicy {}

pub async fn DenyPolicy::execute(
  underlying : ProtectedToolUnderlying,
  value : String,
) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  ignore(underlying)
  Err(@toolMiddleware.ToolInvokeError::ConstraintViolation("denied: " + value))
}
