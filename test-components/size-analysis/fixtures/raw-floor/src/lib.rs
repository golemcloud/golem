#![allow(dead_code)]

mod golem_agentic {
    wit_bindgen::generate!({
        path: "../../../../sdks/rust/golem-rust/wit",
        world: "golem-agentic",
        generate_all,
        pub_export_macro: true,
    });
    pub use __export_golem_agentic_impl as export_golem_agentic;
}
mod save_snapshot {
    wit_bindgen::generate!({
        path: "../../../../sdks/rust/golem-rust/wit",
        world: "golem-rust-save-snapshot",
        generate_all,
        pub_export_macro: true,
    });
    pub use __export_golem_rust_save_snapshot_impl as export_save_snapshot;
}
mod load_snapshot {
    wit_bindgen::generate!({
        path: "../../../../sdks/rust/golem-rust/wit",
        world: "golem-rust-load-snapshot",
        generate_all,
        pub_export_macro: true,
    });
    pub use __export_golem_rust_load_snapshot_impl as export_load_snapshot;
}

use golem_agentic::exports::golem::{
    agent::guest as agent,
    tool::{guest as tool, tool_middleware_guest as middleware},
};
use load_snapshot::exports::golem::api::load_snapshot as load;
use save_snapshot::exports::golem::api::save_snapshot as save;

// Exercise the production raw dispatch without pulling in SDK registries,
// schema conversion, reflection, or application code.
#[path = "../../../../../sdks/rust/golem-rust/src/agentic/exports/raw.rs"]
mod raw;

#[cfg(feature = "tool")]
mod active {
    use super::*;
    use golem_agentic::golem::{
        agent::common::Principal,
        core::types::TypedSchemaValue,
        tool::streams::{ByteStreamItem, ToolStdoutWriter},
    };
    use wit_bindgen::rt::async_support::StreamReader;
    struct Component;

    #[ctor::ctor]
    fn install() {
        raw::tool_exports::install::<Component>();
    }

    impl tool::Guest for Component {
        fn discover_tools() -> Result<Vec<tool::Tool>, tool::ToolError> {
            Ok(Vec::new())
        }
        fn get_tool(name: String) -> Result<tool::Tool, tool::ToolError> {
            Err(tool::ToolError::InvalidToolName(name))
        }
        async fn invoke(
            _: String,
            _: Vec<String>,
            _: TypedSchemaValue,
            _: Option<StreamReader<ByteStreamItem>>,
            _: Option<ToolStdoutWriter>,
            _: Principal,
        ) -> Result<tool::InvocationResult, tool::ToolError> {
            Err(tool::ToolError::InvalidToolName(String::new()))
        }
    }
}
