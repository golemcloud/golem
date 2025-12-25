// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use async_trait::async_trait;
use golem_cli::context::Context;
use golem_cli::mcp_server_service::Tools;
use golem_client::api::{ComponentClient, EnvironmentClient};
use golem_client::model::{ComponentDto, Components, RegisteredAgentType, RegisteredAgentTypes};
use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision, ProtectedComponentId};
use golem_common::model::environment::EnvironmentId;
use rmcp::{Request, Service};
use golem_cli::client::GolemClients;
use golem_cli::config::{ClientConfig, AuthenticationConfigWithSource};
use golem_common::model::account::AccountId;
use golem_common::model::auth::TokenSecret;
use golem_cli::command_handler::{AppCommandHandler, ComponentCommandHandler, Handlers, EnvironmentCommandHandler};
use golem_cli::command::GolemCliGlobalFlags;
use golem_cli::model::format::Format;
use anyhow::{anyhow, Result};
use golem_cli::command::shared_args::DeployArgs;
use golem_cli::config::NamedProfile;
use golem_cli::model::environment::EnvironmentReference;
use golem_cli::model::environment::{ResolvedEnvironmentIdentity, ServerEnvironment};
use golem_common::model::deployment::CurrentDeployment;


// Mock ComponentClient for testing
struct MockComponentClient {
    should_fail_component_list: bool,
    should_fail_get_deployment_component: bool,
    should_fail_get_component_revision: bool,
}

impl MockComponentClient {
    fn new(
        should_fail_component_list: bool,
        should_fail_get_deployment_component: bool,
        should_fail_get_component_revision: bool,
    ) -> Self {
        Self {
            should_fail_component_list,
            should_fail_get_deployment_component,
            should_fail_get_component_revision,
        }
    }
}

#[async_trait]
impl ComponentClient for MockComponentClient {
    async fn get_components(
        &self,
        _component_name: Option<&str>,
    ) -> Result<Components, golem_client::Error> {
        unimplemented!()
    }

    async fn get_latest_component(
        &self,
        _component_name: &str,
    ) -> Result<golem_client::model::Component, golem_client::Error> {
        unimplemented!()
    }

    async fn get_component_by_id(
        &self,
        _component_id: &str,
    ) -> Result<golem_client::model::Component, golem_client::Error> {
        unimplemented!()
    }

    async fn get_deployment_components(
        &self,
        _deployment_id: &str,
        _component_name: Option<&str>,
    ) -> Result<Vec<ComponentDto>, golem_client::Error> {
        if self.should_fail_component_list {
            Err(golem_client::Error::InternalError {
                uri: "/components".to_string(),
                method: http::Method::GET,
                payload: Some("Forced failure for testing".to_string()),
            })
        } else {
            Ok(vec![
                ComponentDto {
                    id: ComponentId("a5347275-4347-430a-9528-7634c44b360b".to_string()),
                    name: ComponentName("component1".to_string()),
                    revision: ComponentRevision(1),
                    size: 123,
                    protected_id: ProtectedComponentId("protected_id_1".to_string()),
                },
                ComponentDto {
                    id: ComponentId("b6347275-4347-430a-9528-7634c44b360c".to_string()),
                    name: ComponentName("component2".to_string()),
                    revision: ComponentRevision(2),
                    size: 456,
                    protected_id: ProtectedComponentId("protected_id_2".to_string()),
                },
            ])
        }
    }

    async fn get_deployment_component(
        &self,
        _deployment_id: &str,
        component_name: &str,
    ) -> Result<ComponentDto, golem_client::Error> {
        if self.should_fail_get_deployment_component {
            Err(golem_client::Error::InternalError {
                uri: format!("/components/{}", component_name),
                method: http::Method::GET,
                payload: Some("Forced failure for testing".to_string()),
            })
        } else if component_name == "component1" {
            Ok(ComponentDto {
                id: ComponentId("a5347275-4347-430a-9528-7634c44b360b".to_string()),
                name: ComponentName("component1".to_string()),
                revision: ComponentRevision(1),
                size: 123,
                protected_id: ProtectedComponentId("protected_id_1".to_string()),
            })
        } else {
            Err(golem_client::Error::NotFound {
                uri: format!("/components/{}", component_name),
                method: http::Method::GET,
                payload: None,
            })
        }
    }

    async fn get_component_revision(
        &self,
        component_id: &str,
        revision: golem_client::model::ComponentRevision,
    ) -> Result<ComponentDto, golem_client::Error> {
        if self.should_fail_get_component_revision {
            Err(golem_client::Error::InternalError {
                uri: format!("/components/{}/revisions/{}", component_id, revision.0),
                method: http::Method::GET,
                payload: Some("Forced failure for testing".to_string()),
            })
        } else if component_id == "a5347275-4347-430a-9528-7634c44b360b" && revision.0 == 1 {
            Ok(ComponentDto {
                id: ComponentId("a5347275-4347-430a-9528-7634c44b360b".to_string()),
                name: ComponentName("component1".to_string()),
                revision: ComponentRevision(1),
                size: 123,
                protected_id: ProtectedComponentId("protected_id_1".to_string()),
            })
        } else {
            Err(golem_client::Error::NotFound {
                uri: format!("/components/{}/revisions/{}", component_id, revision.0),
                method: http::Method::GET,
                payload: None,
            })
        }
    }
}

// Mock EnvironmentClient for testing
struct MockEnvironmentClient;

#[async_trait]
impl EnvironmentClient for MockEnvironmentClient {
    async fn list_deployment_agent_types(
        &self,
        _environment_id: &str,
        _deployment_id: golem_client::model::DeploymentRevision,
    ) -> Result<RegisteredAgentTypes, golem_client::Error> {
        Ok(RegisteredAgentTypes {
            values: vec![
                RegisteredAgentType {
                    agent_type_id: "agent-type-1".to_string(),
                    name: "AgentType1".to_string(),
                },
                RegisteredAgentType {
                    agent_type_id: "agent-type-2".to_string(),
                    name: "AgentType2".to_string(),
                },
            ],
        })
    }

    async fn get_deployment_components(
        &self,
        _environment_id: &str,
        _deployment_id: golem_client::model::DeploymentRevision,
    ) -> Result<Components, golem_client::Error> {
        unimplemented!()
    }
    
    // Remaining methods unimplemented as they are not needed for this test
    async fn list_deployments(&self, _environment_id: &str, _version: Option<&str>) -> Result<golem_client::model::Deployments, golem_client::Error> {
        unimplemented!()
    }
    async fn get_deployment_summary(&self, _environment_id: &str, _revision: golem_client::model::DeploymentRevision) -> Result<golem_client::model::Deployment, golem_client::Error> {
        unimplemented!()
    }
    async fn deploy_environment(&self, _environment_id: &str, _request: &golem_client::model::DeploymentCreation) -> Result<CurrentDeployment, golem_client::Error> {
        unimplemented!()
    }
    async fn rollback_environment(&self, _environment_id: &str, _request: &golem_client::model::DeploymentRollback) -> Result<CurrentDeployment, golem_client::Error> {
        unimplemented!()
    }
    async fn get_environment_deployment_plan(&self, _environment_id: &str) -> Result<golem_client::model::Deployment, golem_client::Error> {
        unimplemented!()
    }
    async fn get_component_definition_for_environment(&self, _environment_id: &str, _component_id: &str) -> Result<golem_client::model::ComponentDefinition, golem_client::Error> {
        unimplemented!()
    }
    async fn get_http_api_definition_for_environment(&self, _environment_id: &str, _api_definition_id: &str) -> Result<golem_client::model::HttpApiDefinition, golem_client::Error> {
        unimplemented!()
    }
    async fn get_http_api_deployment_for_environment(&self, _environment_id: &str, _http_api_deployment_id: &str) -> Result<golem_client::model::HttpApiDeployment, golem_client::Error> {
        unimplemented!()
    }
}


// A mock GolemClients struct that holds our mock clients
struct MockGolemClients {
    pub component: Arc<dyn ComponentClient + Send + Sync>,
    pub environment: Arc<dyn EnvironmentClient + Send + Sync>,
    // Add other clients as needed for future tests
}

impl MockGolemClients {
    pub fn new(
        component_client: Arc<dyn ComponentClient + Send + Sync>,
        environment_client: Arc<dyn EnvironmentClient + Send + Sync>,
    ) -> Self {
        Self {
            component: component_client,
            environment: environment_client,
        }
    }

    pub fn account_id(&self) -> &AccountId {
        unimplemented!()
    }

    pub fn auth_token(&self) -> &TokenSecret {
        unimplemented!()
    }
}


// A mock Context that provides our mock GolemClients
struct MockContext {
    mock_golem_clients: Arc<MockGolemClients>,
    // Other fields if necessary for future tests that use context directly
    config_dir: PathBuf,
    format: Format,
    help_mode: bool,
    deploy_args: DeployArgs,
    profile: NamedProfile,
    environment_reference: Option<EnvironmentReference>,
    // Only current_deployment_revision_or_default_warn needs this
    manifest_environment: Option<golem_cli::model::environment::SelectedManifestEnvironment>,

}

impl MockContext {
    pub fn new(mock_golem_clients: Arc<MockGolemClients>) -> Self {
        Self {
            mock_golem_clients,
            config_dir: PathBuf::from("."),
            format: Format::Text,
            help_mode: false,
            deploy_args: DeployArgs::none(),
            profile: NamedProfile::default(),
            environment_reference: None,
            manifest_environment: Some(golem_cli::model::environment::SelectedManifestEnvironment {
                application_name: "test".to_string().into(),
                environment_name: "test".to_string().into(),
                environment: Default::default(),
            }),
        }
    }

    pub async fn golem_clients(&self) -> Result<&MockGolemClients, anyhow::Error> {
        Ok(&self.mock_golem_clients)
    }

    pub fn golem_clients_mut(&self) -> Result<&MockGolemClients, anyhow::Error> {
        Ok(&self.mock_golem_clients)
    }

    // Minimal implementation of methods required by the Handlers trait or other parts of the system
    pub fn config_dir(&self) -> &Path { &self.config_dir }
    pub fn format(&self) -> Format { self.format }
    pub fn yes(&self) -> bool { false }
    pub fn dev_mode(&self) -> bool { false }
    pub fn show_sensitive(&self) -> bool { false }
    pub fn deploy_args(&self) -> &DeployArgs { &self.deploy_args }
    pub fn server_no_limit_change(&self) -> bool { false }
    pub fn should_colorize(&self) -> bool { false }
    pub async fn silence_app_context_init(&self) {}
    pub fn environment_reference(&self) -> Option<&EnvironmentReference> { self.environment_reference.as_ref() }
    pub fn manifest_environment(&self) -> Option<&golem_cli::model::environment::SelectedManifestEnvironment> {
        self.manifest_environment.as_ref()
    }
    pub fn manifest_environment_deployment_options(&self) -> Option<&golem_cli::model::app_raw::DeploymentOptions> {
        unimplemented!()
    }
    pub fn caches(&self) -> &golem_cli::context::Caches { unimplemented!() }
    pub fn http_batch_size(&self) -> u64 { 0 }
    pub fn http_parallelism(&self) -> usize { 0 }
    pub async fn account_id(&self) -> Result<AccountId> { Ok(AccountId("test-account".to_string())) }
    pub async fn auth_token(&self) -> Result<TokenSecret> { Ok(TokenSecret::from("test-token".to_string())) }
    fn auth_config(&self) -> Result<AuthenticationConfigWithSource> { unimplemented!() }

    pub fn app_handler(&self) -> AppCommandHandler {
        AppCommandHandler::new(Arc::new(self.clone()))
    }
    pub fn component_handler(&self) -> ComponentCommandHandler {
        ComponentCommandHandler::new(Arc::new(self.clone()))
    }
    pub fn environment_handler(&self) -> EnvironmentCommandHandler {
        EnvironmentCommandHandler::new(Arc::new(self.clone()))
    }
}

// Implement Clone manually for MockContext
impl Clone for MockContext {
    fn clone(&self) -> Self {
        MockContext {
            mock_golem_clients: self.mock_golem_clients.clone(),
            config_dir: self.config_dir.clone(),
            format: self.format.clone(),
            help_mode: self.help_mode,
            deploy_args: self.deploy_args.clone(),
            profile: self.profile.clone(),
            environment_reference: self.environment_reference.clone(),
            manifest_environment: self.manifest_environment.clone(),
        }
    }
}


fn create_mock_context(
    component_client: Arc<dyn ComponentClient + Send + Sync>,
    environment_client: Arc<dyn EnvironmentClient + Send + Sync>,
) -> Arc<Context> {
    let mock_golem_clients = Arc::new(MockGolemClients::new(component_client, environment_client));
    Arc::new(MockContext::new(mock_golem_clients).into())
}

// Convert MockContext to Context to use it in Tools.
// This is safe because Tools only uses a subset of Context's methods
// that we have mocked in MockContext.
impl From<MockContext> for Context {
    fn from(mock: MockContext) -> Self {
        let global_flags = GolemCliGlobalFlags::default();
        let client_config = ClientConfig::default();

        let environment = ResolvedEnvironmentIdentity {
            environment_id: EnvironmentId("test-env".to_string()),
            environment_name: "test".to_string().into(),
            server_environment: ServerEnvironment {
                current_deployment: Some(CurrentDeployment {
                    environment_id: EnvironmentId("test-env".to_string()),
                    application_id: "test-app".to_string().into(),
                    deployment_revision: 0.into(),
                    version: "test".to_string().into(),
                    created_at: chrono::Utc::now(),
                }),
                ..Default::default()
            }
        };

        Context {
            config_dir: mock.config_dir,
            format: mock.format,
            help_mode: mock.help_mode,
            deploy_args: mock.deploy_args,
            profile: mock.profile,
            environment_reference: mock.environment_reference,
            manifest_environment: mock.manifest_environment,
            manifest_environment_deployment_options: None, // Not needed for these tests
            app_context_config: None, // Not needed for these tests
            http_batch_size: 0,
            http_parallelism: 0,
            auth_token_override: None,
            client_config,
            yes: false,
            show_sensitive: false,
            dev_mode: false,
            server_no_limit_change: false,
            should_colorize: false,
            template_sdk_overrides: Default::default(),
            template_group: Default::default(),
            file_download_client: reqwest::Client::new(), // Dummy client
            golem_clients: tokio::sync::OnceCell::new(),
            templates: std::sync::OnceLock::new(),
            selected_context_logging: std::sync::OnceLock::new(),
            app_context_state: tokio::sync::RwLock::new(golem_cli::context::ApplicationContextState::new(false, golem_cli::model::app::ApplicationSourceMode::None)),
            rib_repl_state: tokio::sync::RwLock::default(),
            caches: Default::default(),
        }
    }
}


#[tokio::test]
async fn test_list_components() {
    let mock_component_client = Arc::new(MockComponentClient::new(false, false, false));
    let mock_environment_client = Arc::new(MockEnvironmentClient);
    let ctx = create_mock_context(mock_component_client.clone(), mock_environment_client.clone());

    let tools = Tools::new(ctx.clone());

    let req = Request {
        tool_name: "list_components".to_string(),
        parameters: serde_json::Value::Null,
    };

    let result = tools.call(req).await.unwrap();

    let components: Vec<ComponentDto> = serde_json::from_value(result).unwrap();

    assert_eq!(components.len(), 2);
    assert_eq!(components[0].name.0, "component1");
    assert_eq!(components[1].name.0, "component2");
}

#[tokio::test]
async fn test_list_agent_types() {
    let mock_component_client = Arc::new(MockComponentClient::new(false, false, false));
    let mock_environment_client = Arc::new(MockEnvironmentClient);
    let ctx = create_mock_context(mock_component_client.clone(), mock_environment_client.clone());

    let tools = Tools::new(ctx.clone());

    let req = Request {
        tool_name: "list_agent_types".to_string(),
        parameters: serde_json::Value::Null,
    };

    let result = tools.call(req).await.unwrap();

    let agent_types: Vec<RegisteredAgentType> = serde_json::from_value(result).unwrap();

    assert_eq!(agent_types.len(), 2);
    assert_eq!(agent_types[0].name, "AgentType1");
    assert_eq!(agent_types[1].name, "AgentType2");
}

#[tokio::test]
async fn test_get_component() {
    let mock_component_client = Arc::new(MockComponentClient::new(false, false, false));
    let mock_environment_client = Arc::new(MockEnvironmentClient);
    let ctx = create_mock_context(mock_component_client.clone(), mock_environment_client.clone());

    let tools = Tools::new(ctx.clone());

    let req = Request {
        tool_name: "get_component".to_string(),
        parameters: serde_json::json!({
            "component_name": "component1",
            "revision": null
        }),
    };

    let result = tools.call(req).await.unwrap();

    let component: Option<ComponentDto> = serde_json::from_value(result).unwrap();

    assert!(component.is_some());
    assert_eq!(component.unwrap().name.0, "component1");

    let req = Request {
        tool_name: "get_component".to_string(),
        parameters: serde_json::json!({
            "component_name": "non_existent_component",
            "revision": null
        }),
    };

    let result = tools.call(req).await.unwrap();
    let component: Option<ComponentDto> = serde_json::from_value(result).unwrap();
    assert!(component.is_none());
}

#[tokio::test]
async fn test_list_components_error() {
    let mock_component_client = Arc::new(MockComponentClient::new(true, false, false)); // Force component list to fail
    let mock_environment_client = Arc::new(MockEnvironmentClient);
    let ctx = create_mock_context(mock_component_client.clone(), mock_environment_client.clone());

    let tools = Tools::new(ctx.clone());

    let req = Request {
        tool_name: "list_components".to_string(),
        parameters: serde_json::Value::Null,
    };

    let result = tools.call(req).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Forced failure for testing"));
}

#[tokio::test]
async fn test_get_component_error() {
    let mock_component_client = Arc::new(MockComponentClient::new(false, true, false)); // Force get_deployment_component to fail
    let mock_environment_client = Arc::new(MockEnvironmentClient);
    let ctx = create_mock_context(mock_component_client.clone(), mock_environment_client.clone());

    let tools = Tools::new(ctx.clone());

    let req = Request {
        tool_name: "get_component".to_string(),
        parameters: serde_json::json!({
            "component_name": "component1",
            "revision": null
        }),
    };

    let result = tools.call(req).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Forced failure for testing"));
}

#[tokio::test]
async fn test_get_component_with_revision() {
    let mock_component_client = Arc::new(MockComponentClient::new(false, false, false));
    let mock_environment_client = Arc::new(MockEnvironmentClient);
    let ctx = create_mock_context(mock_component_client.clone(), mock_environment_client.clone());

    let tools = Tools::new(ctx.clone());

    let req = Request {
        tool_name: "get_component".to_string(),
        parameters: serde_json::json!({
            "component_name": "component1",
            "revision": 1
        }),
    };

    let result = tools.call(req).await.unwrap();

    let component: Option<ComponentDto> = serde_json::from_value(result).unwrap();

    assert!(component.is_some());
    let component = component.unwrap();
    assert_eq!(component.name.0, "component1");
    assert_eq!(component.revision.0, 1);
}

#[tokio::test]
async fn test_describe_component() {
    let mock_component_client = Arc::new(MockComponentClient::new(false, false, false));
    let mock_environment_client = Arc::new(MockEnvironmentClient);
    let ctx = create_mock_context(mock_component_client.clone(), mock_environment_client.clone());

    let tools = Tools::new(ctx.clone());

    let req = Request {
        tool_name: "describe_component".to_string(),
        parameters: serde_json::json!({
            "component_name": "component1",
            "revision": null
        }),
    };

    let result = tools.call(req).await.unwrap();

    let components: Vec<ComponentDto> = serde_json::from_value(result).unwrap();

    assert_eq!(components.len(), 1);
    assert_eq!(components[0].name.0, "component1");
}

#[tokio::test]
async fn test_get_golem_yaml() {
    let mock_component_client = Arc::new(MockComponentClient::new(false, false, false));
    let mock_environment_client = Arc::new(MockEnvironmentClient);
    let ctx = create_mock_context(mock_component_client.clone(), mock_environment_client.clone());

    let tools = Tools::new(ctx.clone());

    let content = "components: []";
    std::fs::write("golem.yaml", content).unwrap();

    let req = Request {
        tool_name: "get_golem_yaml".to_string(),
        parameters: serde_json::Value::Null,
    };

    let result = tools.call(req).await.unwrap();

    let yaml_content: String = serde_json::from_value(result).unwrap();

    assert_eq!(yaml_content, content);

    std::fs::remove_file("golem.yaml").unwrap();
}
}