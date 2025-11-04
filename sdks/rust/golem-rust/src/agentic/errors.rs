use crate::golem_agentic::golem::agent::common::AgentError;
use crate::wasm_rpc::analysis::analysed_type::str;
use crate::wasm_rpc::{Value, ValueAndType};

pub fn custom_error(msg: impl ToString) -> AgentError {
    AgentError::CustomError(ValueAndType::new(Value::String(msg.to_string()), str()).into())
}

pub fn internal_error(msg: impl ToString) -> AgentError {
    custom_error(format!("Internal error: {}", msg.to_string()))
}

pub fn invalid_input_error(msg: impl ToString) -> AgentError {
    AgentError::InvalidInput(msg.to_string())
}

pub fn invalid_method_error(method_name: impl ToString) -> AgentError {
    AgentError::InvalidMethod(method_name.to_string())
}
