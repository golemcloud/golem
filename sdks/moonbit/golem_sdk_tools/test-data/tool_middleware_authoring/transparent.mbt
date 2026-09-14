/// A file tool shape used by transparent middleware.
#derive.tool("file")
struct FileTool {}

pub fn FileTool::read(path : String) -> String {
  path
}

/// Restricts paths while preserving the file tool shape.
#derive.tool_middleware("path-policy", presented="FileTool", alias="safe-files", alias="restricted-files")
struct PathPolicy {}

pub async fn PathPolicy::read(
  underlying : FileToolUnderlying,
  path : String,
) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  underlying.read(path)
}
