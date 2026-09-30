#[cfg(not(feature = "tool-only"))]
mod bindings {
    wit_bindgen::generate!({
        path: "../../../../sdks/rust/golem-rust/wit",
        inline: "package size:floor; world floor { include golem:rust/golem-agentic; export golem:api/save-snapshot@1.5.0; export golem:api/load-snapshot@1.5.0; }",
        world: "size:floor/floor",
        generate_all,
    });
}

#[cfg(feature = "tool-only")]
mod bindings {
    wit_bindgen::generate!({
        path: "../../../../sdks/rust/golem-rust/wit",
        inline: "package size:floor; world floor { include golem:rust/golem-rust; export golem:tool/guest@0.1.0; }",
        world: "size:floor/floor",
        generate_all,
    });
}

use bindings::golem::tool::{
    common::{InvocationResult, Tool, ToolError},
    streams::{ByteStreamItem, ToolStdoutWriter},
};
use bindings::golem::{agent::common::Principal, core::types::TypedSchemaValue};
use wit_bindgen::rt::async_support::StreamReader;

struct Component;
bindings::export!(Component with_types_in bindings);

impl bindings::exports::golem::tool::guest::Guest for Component {
    fn discover_tools() -> Result<Vec<Tool>, ToolError> {
        Ok(Vec::new())
    }
    fn get_tool(_: String) -> Result<Tool, ToolError> {
        Err(ToolError::InvalidToolName(String::new()))
    }
    async fn invoke(
        _: String,
        _: Vec<String>,
        _: TypedSchemaValue,
        _: Option<StreamReader<ByteStreamItem>>,
        _: Option<ToolStdoutWriter>,
        _: Principal,
    ) -> Result<InvocationResult, ToolError> {
        Err(ToolError::InvalidToolName(String::new()))
    }
}

#[cfg(not(feature = "tool-only"))]
mod unified {
    use super::*;
    use bindings::golem::agent::common::{AgentError, AgentType};
    use bindings::golem::api::host::Snapshot;
    use bindings::golem::core::types::SchemaValueTree;
    use bindings::golem::tool::{common::ToolMiddleware, underlying::UnderlyingTool};

    impl bindings::exports::golem::api::save_snapshot::Guest for Component {
        async fn save() -> Snapshot {
            Snapshot {
                payload: Vec::new(),
                mime_type: String::new(),
            }
        }
    }

    impl bindings::exports::golem::api::load_snapshot::Guest for Component {
        async fn load(_: Snapshot) -> Result<(), String> {
            Ok(())
        }
    }

    impl bindings::exports::golem::agent::guest::Guest for Component {
        async fn initialize(_: String, _: SchemaValueTree, _: Principal) -> Result<(), AgentError> {
            Err(AgentError::InvalidType(String::new()))
        }
        async fn invoke(
            _: String,
            _: SchemaValueTree,
            _: Principal,
        ) -> Result<Option<SchemaValueTree>, AgentError> {
            Err(AgentError::InvalidType(String::new()))
        }
        fn discover_agent_types() -> Result<Vec<AgentType>, AgentError> {
            Ok(Vec::new())
        }
        fn get_definition() -> AgentType {
            panic!("no agent")
        }
    }

    impl bindings::exports::golem::tool::tool_middleware_guest::Guest for Component {
        fn discover_tool_middlewares() -> Result<Vec<ToolMiddleware>, ToolError> {
            Ok(Vec::new())
        }
        fn get_tool_middleware(_: String) -> Result<ToolMiddleware, ToolError> {
            Err(ToolError::InvalidToolName(String::new()))
        }
        async fn invoke_tool_middleware(
            _: String,
            _: String,
            _: Tool,
            _: TypedSchemaValue,
            _: Vec<String>,
            _: TypedSchemaValue,
            _: Option<StreamReader<ByteStreamItem>>,
            _: Option<ToolStdoutWriter>,
            _: Principal,
            _: UnderlyingTool,
        ) -> Result<InvocationResult, ToolError> {
            Err(ToolError::InvalidToolName(String::new()))
        }
    }
}
