pub use golem_tool_metadata::extended_tool_type::*;

use crate::agentic::Schema;
use crate::schema::SchemaGraph;
use crate::schema::tool::wit::wire;

pub type Tool = wire::Tool;

pub fn tool_value_schema<T: Schema>(position: &str) -> Result<SchemaGraph, ToolBuildError> {
    T::get_type()
        .get_schema_graph()
        .ok_or_else(|| ToolBuildError::AutoInjectedToolParameter(position.to_string()))
}

pub fn encode_schema_value_default(
    value: &crate::SchemaValue,
) -> Result<crate::schema::wit::wire::SchemaValueTree, ToolBuildError> {
    crate::schema::wit::encode_value(value)
        .map_err(|error| ToolBuildError::EncodeError(error.to_string()))
}
