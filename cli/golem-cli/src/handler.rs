use std::sync::Arc;

use async_trait::async_trait;

use rust_mcp_sdk::schema::{
    schema_utils::{ CallToolError, NotificationFromClient, RequestFromClient, ResultFromServer },
    ClientRequest,
    ListToolsResult,
    RpcError,
};
use rust_mcp_sdk::{
    mcp_server::{ enforce_compatible_protocol_version, ServerHandlerCore },
    McpServer,
};

use crate::tools::CoreTools;
use crate::resources;

pub struct MyServerHandler;

// To check out a list of all the methods in the trait that you can override, take a look at
// https://github.com/rust-mcp-stack/rust-mcp-sdk/blob/main/crates/rust-mcp-sdk/src/mcp_handlers/mcp_server_handler_core.rs
#[allow(unused)]
#[async_trait]
impl ServerHandlerCore for MyServerHandler {
    // Process incoming requests from the client
    async fn handle_request(
        &self,
        request: RequestFromClient,
        runtime: Arc<dyn McpServer>
    ) -> std::result::Result<ResultFromServer, RpcError> {
        let method_name = &request.method().to_owned();
        match request {
            //Handle client requests according to their specific type.
            RequestFromClient::ClientRequest(client_request) =>
                match client_request {
                    // Handle the initialization request
                    ClientRequest::InitializeRequest(initialize_request) => {
                        let mut server_info = runtime.server_info().to_owned();

                        if
                            let Some(updated_protocol_version) =
                                enforce_compatible_protocol_version(
                                    &initialize_request.params.protocol_version,
                                    &server_info.protocol_version
                                ).map_err(|err| {
                                    tracing::error!(
                                        "Incompatible protocol version :\nclient: {}\nserver: {}",
                                        &initialize_request.params.protocol_version,
                                        &server_info.protocol_version
                                    );
                                    RpcError::internal_error().with_message(err.to_string())
                                })?
                        {
                            server_info.protocol_version = updated_protocol_version;
                        }

                        return Ok(server_info.into());
                    }
                    // Handle ListToolsRequest, return list of available tools
                    ClientRequest::ListToolsRequest(_) =>
                        Ok(
                            (ListToolsResult {
                                meta: None,
                                next_cursor: None,
                                tools: CoreTools::tools(),
                            }).into()
                        ),

                    // Handles incoming CallToolRequest and processes it using the appropriate tool.
                    ClientRequest::CallToolRequest(request) => {
                        let tool_name = request.tool_name().to_string();

                        // Attempt to convert request parameters into CoreTools enum
                        let tool_params = CoreTools::try_from(request.params).map_err(|_|
                            CallToolError::unknown_tool(tool_name.clone())
                        )?;

                        // Match the tool variant and execute its corresponding logic
                        let result = match tool_params {
                            CoreTools::ListToolsTool(list_tools_tool) => {
                                list_tools_tool
                                    .call_tool().await
                                    .map_err(|err| {
                                        RpcError::internal_error().with_message(err.to_string())
                                    })?
                            }
                            CoreTools::CallToolTool(call_tool_tool) => {
                                call_tool_tool
                                    .call_tool().await
                                    .map_err(|err| {
                                        RpcError::internal_error().with_message(err.to_string())
                                    })?
                            }
                            CoreTools::ListResourcesTool(list_resources_tool) => {
                                list_resources_tool
                                    .call_tool().await
                                    .map_err(|err| {
                                        RpcError::internal_error().with_message(err.to_string())
                                    })?
                            }
                        };
                        Ok(result.into())
                    }

                    ClientRequest::ListResourcesRequest(_req) => {
                        // you can pass Some(cwd) if you want to honor a configured root
                        Ok(resources::list_resources_from_manifests(None).into())
                    }

                    ClientRequest::ReadResourceRequest(req) => {
                        match resources::read_manifest_resource(&req.params.uri) {
                            Some(res) => Ok(res.into()),
                            None =>
                                Err(
                                    RpcError::invalid_params().with_message(
                                        format!(
                                            "Unknown or unreadable resource URI: {}",
                                            req.params.uri
                                        )
                                    )
                                ),
                        }
                    }

                    // Return Method not found for any other requests
                    _ =>
                        Err(
                            RpcError::method_not_found().with_message(
                                format!("No handler is implemented for '{method_name}'.")
                            )
                        ),
                }
            // Handle custom requests
            RequestFromClient::CustomRequest(_) =>
                Err(
                    RpcError::method_not_found().with_message(
                        "No handler is implemented for custom requests.".to_string()
                    )
                ),
        }
    }

    // Process incoming client notifications
    async fn handle_notification(
        &self,
        notification: NotificationFromClient,
        _: Arc<dyn McpServer>
    ) -> std::result::Result<(), RpcError> {
        Ok(())
    }

    // Process incoming client errors
    async fn handle_error(
        &self,
        error: &RpcError,
        _: Arc<dyn McpServer>
    ) -> std::result::Result<(), RpcError> {
        Ok(())
    }
}
