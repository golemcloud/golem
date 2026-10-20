// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::mcp::agent_mcp_resource::McpResourceUri;
use crate::mcp::agent_mcp_tool::AgentMcpTool;
use crate::mcp::serving::{McpServing, ServeKind, ServeRefusal, Serving};
use crate::mcp::{McpCapabilityLookup, McpDeployment, invoke};
use crate::service::worker::WorkerService;
use golem_service_base::mcp::CompiledMcp;
use poem::http;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, model::*, service::RequestContext};
use std::sync::Arc;

/// Serves the MCP requests of one deployment domain. It keeps no state between requests: every
/// request is served with the deployment that the [`McpServing`] in its extensions names.
#[derive(Clone)]
pub struct GolemAgentMcpServer {
    mcp_definitions_lookup: Arc<dyn McpCapabilityLookup>,
    worker_service: Arc<WorkerService>,
}

impl GolemAgentMcpServer {
    pub fn new(
        mcp_definitions_lookup: Arc<dyn McpCapabilityLookup>,
        worker_service: Arc<WorkerService>,
    ) -> Self {
        Self {
            mcp_definitions_lookup,
            worker_service,
        }
    }

    pub async fn invoke_tool(
        &self,
        args_map: JsonObject,
        mcp_tool: &AgentMcpTool,
    ) -> Result<CallToolResult, ErrorData> {
        invoke::tool::invoke_tool(args_map, mcp_tool, &self.worker_service, None).await
    }

    async fn run_tool(
        &self,
        request: CallToolRequestParams,
        deployment: Arc<McpDeployment>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let compiled = &deployment.compiled;
        if let Some(export) = compiled
            .tools
            .iter()
            .find(|tool| tool.mcp_name == request.name)
        {
            let (input, stdin) = export
                .parse_arguments(request.arguments.unwrap_or_default())
                .map_err(|error| ErrorData::invalid_params(error, None))?;
            let result = invoke::native_tool::invoke(
                &self.worker_service,
                compiled,
                export,
                input,
                stdin,
                context.ct.clone(),
                None,
            )
            .await;
            // A cancelled call failed because of its cancel, not because the deployment changed.
            if result.is_err() && !context.ct.is_cancelled() {
                self.mcp_definitions_lookup
                    .invalidate(&compiled.domain)
                    .await;
                if let Ok(current) = self.mcp_definitions_lookup.get(&compiled.domain).await
                    && !same_deployment(compiled, &current.compiled)
                {
                    let _ = context.peer.notify_tool_list_changed().await;
                    return Err(changed_tool_set());
                }
            }
            return result.map(Into::into);
        }
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        deployment.capabilities()?.router.call(tcc).await
    }

    async fn run_resource(
        &self,
        uri: String,
        deployment: &McpDeployment,
    ) -> Result<ReadResourceResponse, McpError> {
        let resource_registry = &deployment.capabilities()?.resources;

        if let Some(resource) = resource_registry.get_static(&uri) {
            return invoke::resource::invoke_resource(
                &self.worker_service,
                resource,
                &uri,
                None,
                None,
            )
            .await
            .map(Into::into);
        }

        let parsed_resource_uri = McpResourceUri::parse(&uri)
            .map_err(|e| McpError::invalid_params(format!("Invalid resource URI: {e}"), None))?;

        if let Some((resource, params)) =
            resource_registry.extract_mcp_resource_with_input(&parsed_resource_uri)
        {
            return invoke::resource::invoke_resource(
                &self.worker_service,
                resource,
                &uri,
                Some(params),
                None,
            )
            .await
            .map(Into::into);
        }

        Err(McpError::invalid_params(
            format!("Resource not found for URI: {}", uri),
            None,
        ))
    }
}

/// The deployment to serve the request with, from the request's [`McpServing`]. A refusal is
/// turned into the client's error here, the one place for every handler method.
async fn serve(
    context: &RequestContext<RoleServer>,
    kind: ServeKind,
) -> Result<Serving, ErrorData> {
    let serving = context
        .extensions
        .get::<http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Arc<dyn McpServing>>())
        .cloned()
        .ok_or_else(|| {
            ErrorData::invalid_params("MCP request has no authenticated deployment", None)
        })?;
    match serving.serve(kind, &context.ct).await {
        Ok(serving) => Ok(serving),
        Err(ServeRefusal::ToolSetChanged) => {
            let _ = context.peer.notify_tool_list_changed().await;
            Err(changed_tool_set())
        }
        Err(ServeRefusal::Gone) => Err(ErrorData::invalid_request("MCP session ended", None)),
        Err(ServeRefusal::Unavailable) => Err(ErrorData::internal_error(
            "MCP session could not be read",
            None,
        )),
    }
}

/// Runs `work` until it completes. The serving's `while_running` work runs next to it; it never
/// completes.
async fn while_serving<T>(
    serving: Serving,
    work: impl Future<Output = Result<T, ErrorData>>,
) -> Result<T, ErrorData> {
    tokio::select! {
        result = work => result,
        never = serving.while_running => match never {},
    }
}

fn same_deployment(left: &CompiledMcp, right: &CompiledMcp) -> bool {
    left.domain == right.domain
        && left.environment_id == right.environment_id
        && left.deployment_revision == right.deployment_revision
}

fn changed_tool_set() -> ErrorData {
    ErrorData::invalid_params("tool set changed; list tools again", None)
}

#[allow(deprecated)]
impl ServerHandler for GolemAgentMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_prompts()
                .enable_resources()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2025_03_26)
    }

    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        std::borrow::Cow::Borrowed(ProtocolVersion::known_up_to(&ProtocolVersion::V_2025_03_26))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let serving = serve(&context, ServeKind::List).await?;
        let tools = serving.deployment.capabilities()?.tools.to_vec();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let serving = serve(&context, ServeKind::Invoke).await?;
        let deployment = serving.deployment.clone();
        while_serving(serving, self.run_tool(request, deployment, context)).await
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let serving = serve(&context, ServeKind::List).await?;
        let resource_list = serving
            .deployment
            .capabilities()?
            .resources
            .list_static_resources();

        tracing::info!("Listing {} static resources", resource_list.len());

        Ok(ListResourcesResult::with_all_items(resource_list))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        let serving = serve(&context, ServeKind::List).await?;
        let resource_templates = serving
            .deployment
            .capabilities()?
            .resources
            .list_resource_templates();

        tracing::info!("Listing {} resource templates", resource_templates.len());

        Ok(ListResourceTemplatesResult::with_all_items(
            resource_templates,
        ))
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let serving = serve(&context, ServeKind::List).await?;
        let prompt_list = serving.deployment.capabilities()?.prompts.list_prompts();

        tracing::info!("Listing {} prompts", prompt_list.len());

        Ok(ListPromptsResult::with_all_items(prompt_list))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        let serving = serve(&context, ServeKind::Invoke).await?;
        let deployment = serving.deployment.clone();
        let prompt = async {
            deployment
                .capabilities()?
                .prompts
                .get_by_name(&request.name)
                .map(|p| p.get_prompt_result().into())
                .ok_or_else(|| {
                    McpError::invalid_params(format!("Prompt not found: {}", request.name), None)
                })
        };
        while_serving(serving, prompt).await
    }

    async fn read_resource(
        &self,
        ReadResourceRequestParams { uri, .. }: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let serving = serve(&context, ServeKind::Invoke).await?;
        let deployment = serving.deployment.clone();
        while_serving(serving, self.run_resource(uri, &deployment)).await
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        serve(&context, ServeKind::Initialize).await?;
        Ok(self.get_info())
    }
}

#[cfg(test)]
mod tests {
    use crate::mcp::deployment::agent_capabilities_from_deployment;
    use golem_common::model::Empty;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::{
        AgentMode, AgentTypeName, RegisteredAgentTypeImplementer, Snapshotting,
    };
    use golem_common::model::application::ApplicationName;
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use golem_common::schema::{
        AgentConstructorSchema, AgentMethodSchema, AgentTypeKind, AgentTypeSchema, InputSchema,
        OutputSchema, RegisteredAgentTypeSchema, SchemaGraph,
    };
    use golem_service_base::mcp::CompiledMcp;
    use test_r::test;

    fn agent_type(kind: AgentTypeKind, name: &str) -> RegisteredAgentTypeSchema {
        RegisteredAgentTypeSchema {
            agent_type: AgentTypeSchema {
                kind,
                type_name: AgentTypeName(name.to_string()),
                description: String::new(),
                source_language: String::new(),
                schema: SchemaGraph::empty(),
                constructor: AgentConstructorSchema {
                    name: None,
                    description: String::new(),
                    prompt_hint: Some("constructor prompt".to_string()),
                    input_schema: InputSchema::Parameters(vec![]),
                },
                methods: vec![AgentMethodSchema {
                    name: "call".to_string(),
                    description: String::new(),
                    prompt_hint: Some("method prompt".to_string()),
                    input_schema: InputSchema::Parameters(vec![]),
                    output_schema: OutputSchema::Unit,
                    http_endpoint: vec![],
                    read_only: None,
                }],
                dependencies: vec![],
                mode: AgentMode::Durable,
                http_mount: None,
                snapshotting: Snapshotting::Disabled(Empty {}),
                config: vec![],
            },
            implemented_by: RegisteredAgentTypeImplementer {
                component_id: ComponentId::new(),
                component_revision: ComponentRevision::INITIAL,
                component_name: "component".to_string(),
                account_id: AccountId::new(),
                account_email: AccountEmail::new("owner@example.com"),
            },
        }
    }

    fn deployment(agent_type: RegisteredAgentTypeSchema) -> CompiledMcp {
        CompiledMcp {
            account_id: AccountId::new(),
            account_email: AccountEmail::new("owner@example.com"),
            environment_id: EnvironmentId::new(),
            application_name: ApplicationName("app".to_string()),
            environment_name: EnvironmentName("dev".to_string()),
            deployment_revision: DeploymentRevision::INITIAL,
            domain: Domain("mcp.example.com".to_string()),
            security_scheme_name: None,
            security_scheme: None,
            registered_agent_types: vec![agent_type],
            tools: vec![],
        }
    }

    #[test]
    fn tooling_corpus_controls_mcp_agent_discovery_by_kind() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
        ))
        .unwrap();

        for id in ["tooling-router-discovery", "tooling-name-not-kind"] {
            let case = corpus["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|case| case["id"] == id)
                .unwrap();
            let kind = match case["input"]["kind"].as_str().unwrap() {
                "regular" => AgentTypeKind::Regular,
                "http-router" => AgentTypeKind::HttpRouter,
                other => panic!("{id}: unsupported agent kind {other}"),
            };
            let name = case["input"]["name"].as_str().unwrap_or("Router");
            let capabilities =
                agent_capabilities_from_deployment(&deployment(agent_type(kind, name)));
            let included = !capabilities.tools.is_empty()
                || !capabilities.resources.is_empty()
                || !capabilities.prompts.is_empty();

            assert_eq!(
                included,
                case["expect"]["included"].as_bool().unwrap(),
                "{id}"
            );
        }
    }

    mod serving {
        use super::super::GolemAgentMcpServer;
        use crate::config::McpConfig;
        use crate::mcp::serving::{McpServing, ServeKind, ServeRefusal, Serving, StatelessServing};
        use crate::mcp::{
            InvocationHarness, McpCapabilitiesLookupError, McpCapabilityLookup, McpDeployment,
            mcp_http_transport_config,
        };
        use golem_common::model::agent::{AgentMode, InvocationFreshnessDisposition};
        use golem_common::model::deployment::DeploymentRevision;
        use golem_common::model::domain_registration::Domain;
        use poem::endpoint::{BoxEndpoint, TowerCompatExt};
        use poem::http::{Method, StatusCode};
        use poem::{Endpoint, EndpointExt, Request};
        use rmcp::ServerHandler;
        use rmcp::model::ProtocolVersion;
        use rmcp::transport::StreamableHttpService;
        use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
        use serde_json::{Value, json};
        use std::sync::Arc;
        use test_r::test;
        use tokio_util::sync::CancellationToken;

        struct NoDeployments;

        #[async_trait::async_trait]
        impl McpCapabilityLookup for NoDeployments {
            async fn get(
                &self,
                domain: &Domain,
            ) -> Result<Arc<McpDeployment>, McpCapabilitiesLookupError> {
                Err(McpCapabilitiesLookupError::UnknownSite(domain.clone()))
            }
            async fn invalidate(&self, _: &Domain) {}
            async fn invalidate_all(&self) {}
        }

        /// Refuses every request: the client listed another tool set.
        struct ToolSetChanged;

        #[async_trait::async_trait]
        impl McpServing for ToolSetChanged {
            async fn serve(
                &self,
                _: ServeKind,
                _: &CancellationToken,
            ) -> Result<Serving, ServeRefusal> {
                Err(ServeRefusal::ToolSetChanged)
            }
        }

        fn server(harness: &InvocationHarness) -> GolemAgentMcpServer {
            GolemAgentMcpServer::new(Arc::new(NoDeployments), harness.worker_service.clone())
        }

        /// The stateless rmcp service over Golem's handler, with no session layer in front.
        fn endpoint(harness: &InvocationHarness) -> BoxEndpoint<'static> {
            let worker_service = harness.worker_service.clone();
            StreamableHttpService::new(
                move || {
                    Ok(GolemAgentMcpServer::new(
                        Arc::new(NoDeployments),
                        worker_service.clone(),
                    ))
                },
                Arc::new(NeverSessionManager::default()),
                mcp_http_transport_config(&McpConfig::default()),
            )
            .compat()
            .map_to_response()
            .boxed()
        }

        fn post(body: Value, serving: Arc<dyn McpServing>) -> Request {
            let mut request = Request::builder()
                .method(Method::POST)
                .uri_str("/mcp")
                .header("host", "tools.example.com")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .body(serde_json::to_vec(&body).unwrap());
            request.extensions_mut().insert(serving);
            request
        }

        /// The JSON-RPC messages of a streamed response.
        async fn messages(endpoint: &BoxEndpoint<'static>, request: Request) -> Vec<Value> {
            let response = endpoint.call(request).await.unwrap();
            let status = response.status();
            let text = response.into_body().into_string().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{text}");
            text.lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim)
                .filter(|data| !data.is_empty())
                .map(|data| serde_json::from_str(data).unwrap())
                .collect()
        }

        fn list(id: i64) -> Value {
            json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"})
        }

        fn call(id: i64, name: &str) -> Value {
            json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                   "params": {"name": name, "arguments": {}}})
        }

        fn stateless(harness: &InvocationHarness, native: bool) -> Arc<dyn McpServing> {
            Arc::new(StatelessServing(Arc::new(McpDeployment::new(
                harness.compiled_mcp("tools.example.com", DeploymentRevision::INITIAL, native),
            ))))
        }

        #[test]
        async fn the_handler_serves_lists_and_calls_without_the_session_layer() {
            let harness = InvocationHarness::with_run_method(AgentMode::Durable);
            let endpoint = endpoint(&harness);
            let serving = stateless(&harness, false);

            let listed = messages(&endpoint, post(list(1), serving.clone())).await;
            assert_eq!(listed[0]["result"]["tools"][0]["name"], "mcp-agent-run");
            let called = messages(&endpoint, post(call(2, "mcp-agent-run"), serving)).await;
            assert!(called[0].get("result").is_some(), "{called:?}");
        }

        #[test]
        async fn tool_set_changed_gives_the_error_and_tools_list_changed() {
            let harness = InvocationHarness::with_run_method(AgentMode::Durable);
            let refused = messages(
                &endpoint(&harness),
                post(call(1, "mcp-agent-run"), Arc::new(ToolSetChanged)),
            )
            .await;
            assert_eq!(refused.len(), 2, "{refused:?}");
            assert_eq!(refused[0]["method"], "notifications/tools/list_changed");
            assert_eq!(
                refused[1]["error"]["message"],
                "tool set changed; list tools again"
            );
        }

        #[test]
        async fn a_call_without_a_key_keeps_todays_freshness() {
            let harness = InvocationHarness::with_run_method(AgentMode::Ephemeral);
            let endpoint = endpoint(&harness);
            let serving = stateless(&harness, true);

            messages(&endpoint, post(call(1, "mcp-agent-run"), serving.clone())).await;
            messages(&endpoint, post(call(2, "mcp-agent-run"), serving.clone())).await;
            messages(&endpoint, post(call(3, "native"), serving.clone())).await;
            messages(&endpoint, post(call(4, "native"), serving)).await;

            let contexts = harness.contexts.lock().unwrap();
            assert_eq!(contexts.len(), 2);
            assert!(
                contexts
                    .iter()
                    .all(|context| context.freshness == InvocationFreshnessDisposition::KnownFresh),
                "an ephemeral call with no key is known to be fresh"
            );
            assert_ne!(contexts[0].idempotency_key, contexts[1].idempotency_key);
            let native_keys = harness.native_session_keys();
            assert_eq!(native_keys.len(), 2);
            assert!(native_keys.iter().all(|key| !key.is_empty()));
            assert_ne!(
                native_keys[0], native_keys[1],
                "each native call has a fresh key"
            );
        }

        #[test]
        fn get_tool_returns_none() {
            let harness = InvocationHarness::with_run_method(AgentMode::Durable);
            assert!(server(&harness).get_tool("mcp-agent-run").is_none());
        }

        #[test]
        fn supported_protocol_versions_stay_up_to_2025_03_26() {
            let harness = InvocationHarness::with_run_method(AgentMode::Durable);
            assert_eq!(
                server(&harness).supported_protocol_versions().as_ref(),
                ProtocolVersion::known_up_to(&ProtocolVersion::V_2025_03_26)
            );
        }
    }
}
