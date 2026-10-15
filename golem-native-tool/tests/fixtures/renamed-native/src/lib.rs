use native_sdk::{ToolError, tool_definition, tool_implementation};

#[derive(ToolError)]
enum Failure {
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Failed(String),
}

#[tool_definition]
trait RenamedTool {
    fn renamed_tool(&self, context: &mut (), value: String) -> Result<String, Failure>;

    async fn stream(
        &self,
        context: &mut (),
        stdin: native_sdk::NativeToolStdin<()>,
        stdout: native_sdk::NativeToolOutput<()>,
    );
}

struct RenamedToolImpl;

#[tool_implementation]
impl RenamedTool for RenamedToolImpl {
    fn renamed_tool(&self, _context: &mut (), value: String) -> Result<String, Failure> {
        Ok(value)
    }

    async fn stream(
        &self,
        context: &mut (),
        mut stdin: native_sdk::NativeToolStdin<()>,
        mut stdout: native_sdk::NativeToolOutput<()>,
    ) {
        while let Some(bytes) = stdin.read(context).await {
            stdout.write(context, bytes.unwrap()).await.unwrap();
        }
        stdout.finish(context).await.unwrap();
    }
}

fn wrapper() -> impl native_sdk::NativeToolInvoker<(), native_sdk::HostError> {
    RenamedToolImpl.native_tool_invoker()
}
