use golem_rust::{ToolError, tool_definition};

#[derive(Debug, ToolError)]
pub enum PublicError {
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Rejected(String),
}

#[derive(Debug, ToolError)]
pub enum BackendError {
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Failed(String),
}

#[tool_definition(version = "1.2.3")]
pub trait PublicEcho {
    fn echo(&self, value: String) -> Result<String, PublicError>;
}

#[tool_definition(version = "4.5.6")]
pub trait BackendEcho {
    fn execute(&self, encoded: String) -> Result<u64, BackendError>;
}
