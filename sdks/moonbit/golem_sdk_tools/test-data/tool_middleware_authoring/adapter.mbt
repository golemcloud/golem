/// Public file operations presented by the middleware.
#derive.tool("public-files")
struct PublicFiles {}

pub fn PublicFiles::read(path : String) -> String {
  path
}

/// Storage operations expected from the next inner layer.
#derive.tool("storage")
struct Storage {}

pub fn Storage::fetch(key : String) -> String {
  key
}

/// Adapts the public file surface to the storage surface.
#derive.tool_middleware("file-adapter", presented="PublicFiles", expected="Storage")
struct FileAdapter {}

pub async fn FileAdapter::read(
  underlying : StorageUnderlying,
  path : String,
) -> Result[String, @toolMiddleware.ToolInvokeError[@tool.NoToolError]] {
  underlying.fetch("files/" + path)
}
