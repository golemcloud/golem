use crate::context::Context;
use crate::mcp::tools::AppTools;
use async_trait::async_trait;
use rust_mcp_sdk::schema::{
    schema_utils::CallToolError, CallToolRequest, CallToolResult, ListToolsRequest,
    ListToolsResult, RpcError,
};
use rust_mcp_sdk::{mcp_server::ServerHandler, McpServer};
use std::sync::Arc;

pub struct GolemMcpServerHandler {
    pub ctx: Arc<Context>,
}

#[async_trait]
impl ServerHandler for GolemMcpServerHandler {
    async fn handle_list_tools_request(
        &self,
        _request: ListToolsRequest,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ListToolsResult, RpcError> {
        Ok(ListToolsResult {
            meta: None,
            next_cursor: None,
            tools: AppTools::tools(),
        })
    }

    async fn handle_call_tool_request(
        &self,
        request: CallToolRequest,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        let tool_params: AppTools =
            AppTools::try_from(request.params).map_err(CallToolError::new)?;

        match tool_params {
            AppTools::BuildAppTool(build_app) => {
                let ctx = self.ctx.clone();
                let project_root = build_app.project_root.clone();
                let res = tokio::task::spawn_blocking(move || {
                    // This runs in a dedicated blocking thread pool
                    // Convert any non-Send error into a String so the closure's return
                    // type is Send across threads.

                    let prev = std::env::current_dir().ok();
                    let _ = std::env::set_current_dir(&project_root);
                    let result =
                        tokio::runtime::Handle::current().block_on(build_app.call_tool(ctx));
                    if let Some(prev_dir) = prev {
                        let _ = std::env::set_current_dir(prev_dir);
                    }
                    match result {
                        Ok(v) => Ok(v),
                        Err(e) => Err(format!("{}", e)),
                    }
                })
                .await
                .map_err(|e| CallToolError::new(e))?;

                match res {
                    Ok(v) => Ok(v),
                    Err(s) => Err(CallToolError::new(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        s,
                    ))),
                }
            }
            AppTools::DeployAppTool(deploy_app) => deploy_app.call_tool(),
            AppTools::CleanAppTool(clean_app) => clean_app.call_tool(),
        }
    }
}
