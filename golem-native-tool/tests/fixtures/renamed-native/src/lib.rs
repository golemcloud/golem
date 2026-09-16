use native_sdk::{ToolError, tool_definition, tool_implementation};

#[derive(ToolError)]
enum Failure {
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Failed(String),
}

#[tool_definition]
trait RenamedTool {
    fn renamed_tool(&self, context: &mut (), value: String) -> Result<String, Failure>;
}

struct RenamedToolImpl;

#[tool_implementation]
impl RenamedTool for RenamedToolImpl {
    fn renamed_tool(&self, _context: &mut (), value: String) -> Result<String, Failure> {
        Ok(value)
    }
}

fn wrapper() -> impl native_sdk::NativeToolInvoker<(), native_sdk::HostError> {
    RenamedToolImpl.native_tool_invoker()
}
