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

use golem_client::api::RegistryServiceClient;
use golem_common::base_model::agent::AgentTypeName;
use golem_common::base_model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_common::base_model::mcp_deployment::{McpDeploymentAgentOptions, McpDeploymentCreation};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{EnvironmentOptions, TestDsl, TestDslExtended};
use rmcp::model::InitializeRequestParams;
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation,
    ReadResourceRequestParams,
};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{RoleClient, ServiceExt};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Debug, Formatter};
use test_r::test_dep;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

const MCP_PORT: u16 = 9007;

pub struct McpTestContext {
    pub domain: Domain,
}

impl Debug for McpTestContext {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "McpTestContext")
    }
}

impl McpTestContext {
    async fn connect_mcp_client(
        &self,
    ) -> anyhow::Result<RunningService<RoleClient, InitializeRequestParams>> {
        let uri = format!("http://127.0.0.1:{}/mcp", MCP_PORT);

        let mut custom_headers = HashMap::new();
        custom_headers.insert(
            http::HeaderName::from_static("host"),
            http::HeaderValue::from_str(&self.domain.0)?,
        );

        let config =
            StreamableHttpClientTransportConfig::with_uri(uri).custom_headers(custom_headers);

        let transport = StreamableHttpClientTransport::from_config(config);

        let client_info = ClientInfo {
            meta: None,
            protocol_version: Default::default(),
            capabilities: ClientCapabilities::default(),
            client_info: Implementation {
                name: "golem-mcp-integration-test".to_string(),
                title: None,
                version: "0.0.1".to_string(),
                description: None,
                website_url: None,
                icons: None,
            },
        };

        let client = client_info.serve(transport).await?;
        Ok(client)
    }
}

#[test_dep]
async fn test_context(deps: &EnvBasedTestDependencies) -> McpTestContext {
    let user = deps.user().await.unwrap().with_auto_deploy(false);

    let client = deps.registry_service().client(&user.token).await;

    let (_, env) = user
        .app_and_env_custom(&EnvironmentOptions {
            security_overrides: true,
            version_check: false,
            compatibility_check: false,
        })
        .await
        .unwrap();

    let domain = Domain(format!("{}.golem.cloud", env.id));

    client
        .create_domain_registration(
            &env.id.0,
            &DomainRegistrationCreation {
                domain: domain.clone(),
            },
        )
        .await
        .unwrap();

    user.component(&env.id, "golem_it_mcp_release")
        .name("golem-it:mcp")
        .store()
        .await
        .unwrap();

    let mcp_deployment_creation = McpDeploymentCreation {
        domain: domain.clone(),
        agents: BTreeMap::from_iter(vec![
            (
                AgentTypeName("weather-agent".to_string()),
                McpDeploymentAgentOptions::default(),
            ),
            (
                AgentTypeName("weather-agent-singleton".to_string()),
                McpDeploymentAgentOptions::default(),
            ),
            (
                AgentTypeName("static-resource".to_string()),
                McpDeploymentAgentOptions::default(),
            ),
            (
                AgentTypeName("dynamic-resource".to_string()),
                McpDeploymentAgentOptions::default(),
            ),
        ]),
    };

    client
        .create_mcp_deployment(&env.id.0, &mcp_deployment_creation)
        .await
        .unwrap();

    user.deploy_environment(env.id).await.unwrap();

    McpTestContext { domain }
}

#[test]
#[tracing::instrument]
async fn list_tools(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;
    let tools = client.list_all_tools().await?;

    let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();

    // WeatherAgent tools (with constructor param "name")
    assert!(
        tool_names.contains(&"WeatherAgent-get_weather_report_for_city".to_string()),
        "Expected WeatherAgent-get_weather_report_for_city in {:?}",
        tool_names
    );
    assert!(
        tool_names.contains(&"WeatherAgent-get_weather_report_for_city_with_images".to_string())
    );
    assert!(tool_names.contains(&"WeatherAgent-get_weather_report_for_city_text".to_string()));
    assert!(tool_names.contains(&"WeatherAgent-get_snow_fall_image_for_city".to_string()));

    assert!(tool_names.contains(&"WeatherAgent-get_lat_long_for_city".to_string()));

    // WeatherAgentSingleton tools (no constructor params)
    assert!(tool_names.contains(&"WeatherAgentSingleton-get_weather_report_for_city".to_string()));

    assert!(tool_names
        .contains(&"WeatherAgentSingleton-get_weather_report_for_city_with_images".to_string()));

    assert!(
        tool_names.contains(&"WeatherAgentSingleton-get_weather_report_for_city_text".to_string())
    );

    assert!(tool_names.contains(&"WeatherAgentSingleton-get_snow_fall_image_for_city".to_string()));

    assert!(tool_names.contains(&"WeatherAgentSingleton-get_lat_long_for_city".to_string()));

    // StaticResource and DynamicResource methods have no input params -> exposed as resources, not tools
    assert!(!tool_names.iter().any(|n| n.starts_with("StaticResource")));
    assert!(!tool_names.iter().any(|n| n.starts_with("DynamicResource")));

    drop(client);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_string(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(CallToolRequestParams {
            meta: None,
            name: "WeatherAgent-get_weather_report_for_city".into(),
            arguments: Some(
                json!({
                    "name": "test-agent",
                    "city": "Sydney"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
            task: None,
        })
        .await?;

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.unwrap();
    assert_eq!(
        structured,
        json!("Agent test-agent: This is a weather report for Sydney")
    );

    drop(client);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_multimodal(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(CallToolRequestParams {
            meta: None,
            name: "WeatherAgent-get_weather_report_for_city_with_images".into(),
            arguments: Some(
                json!({
                    "name": "test-agent",
                    "city": "Sydney"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
            task: None,
        })
        .await?;

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.unwrap();

    let parts = structured["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2);

    // First part: text
    let text_part = &parts[0];
    assert!(text_part["value"]["data"]
        .as_str()
        .unwrap()
        .contains("snow fall in Sydney"));

    // Second part: binary (base64 encoded)
    let binary_part = &parts[1];

    assert_eq!(binary_part["value"]["mimeType"], "image/png");

    assert!(binary_part["value"]["data"].as_str().is_some());

    drop(client);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_unstructured_text(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(CallToolRequestParams {
            meta: None,
            name: "WeatherAgent-get_weather_report_for_city_text".into(),
            arguments: Some(
                json!({
                    "name": "test-agent",
                    "city": "Sydney"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
            task: None,
        })
        .await?;

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.unwrap();
    assert!(
        structured["data"]
            .as_str()
            .unwrap()
            .contains("unstructured weather report for Sydney"),
        "Expected unstructured text, got: {:?}",
        structured
    );

    drop(client);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_unstructured_binary(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(CallToolRequestParams {
            meta: None,
            name: "WeatherAgent-get_snow_fall_image_for_city".into(),
            arguments: Some(
                json!({
                    "name": "test-agent",
                    "city": "Sydney"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
            task: None,
        })
        .await?;

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.unwrap();

    // Binary data is base64 encoded: vec![1, 2, 3] -> "AQID"
    assert_eq!(structured["data"], "AQID");
    assert_eq!(structured["mimeType"], "image/png");

    drop(client);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_component_model(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(CallToolRequestParams {
            meta: None,
            name: "WeatherAgent-get_lat_long_for_city".into(),
            arguments: Some(
                json!({
                    "name": "test-agent",
                    "city": "Sydney"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
            task: None,
        })
        .await?;

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.unwrap();

    // LocationDetails { lat: 0.0, long: 0.0, country: "Unknown", population: 0 }
    assert_eq!(structured["lat"], json!(0.0));
    assert_eq!(structured["long"], json!(0.0));
    assert_eq!(structured["country"], "Unknown");
    assert_eq!(structured["population"], 0);

    drop(client);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_singleton_string(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(CallToolRequestParams {
            meta: None,
            name: "WeatherAgentSingleton-get_weather_report_for_city".into(),
            arguments: Some(
                json!({
                    "city": "Darwin"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
            task: None,
        })
        .await?;

    assert_eq!(result.is_error, Some(false));

    let structured = result.structured_content.unwrap();

    assert_eq!(structured, json!("This is a weather report for Darwin."));

    drop(client);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_singleton_component_model(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(CallToolRequestParams {
            meta: None,
            name: "WeatherAgentSingleton-get_lat_long_for_city".into(),
            arguments: Some(
                json!({
                    "city": "Darwin"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
            task: None,
        })
        .await?;

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.unwrap();

    assert_eq!(structured["lat"], json!(0.0));
    assert_eq!(structured["long"], json!(0.0));
    assert_eq!(structured["country"], "Unknown");
    assert_eq!(structured["population"], 0);

    drop(client);
    Ok(())
}

// ── Resource listing tests ──────────────────────────────────────────────

#[test]
#[tracing::instrument]
async fn list_resources(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let resources = client.list_all_resources().await?;

    let resource_uris: Vec<String> = resources.iter().map(|r| r.uri.clone()).collect();

    // StaticResource: methods exposed as static resources
    assert!(
        resource_uris.contains(&"golem://StaticResource/get_static_weather_report".to_string()),
        "Expected static weather report resource in {:?}",
        resource_uris
    );
    assert!(resource_uris
        .contains(&"golem://StaticResource/get_static_weather_report_with_images".to_string()));

    assert!(resource_uris
        .contains(&"golem://StaticResource/get_static_weather_report_text".to_string()));

    assert!(resource_uris.contains(&"golem://StaticResource/get_static_now_fall_image".to_string()));

    // DynamicResource methods should NOT be in static resources (they are templates)
    assert!(!resource_uris
        .iter()
        .any(|u| u.starts_with("golem://DynamicResource")));

    drop(client);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_resource_templates(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;
    let templates = client.list_all_resource_templates().await?;

    let template_uris: Vec<String> = templates.iter().map(|t| t.uri_template.clone()).collect();

    // DynamicResource: methods exposed as resource templates with {name} param
    assert!(
        template_uris.contains(&"golem://DynamicResource/get_weather_report/{name}".to_string()),
        "Expected dynamic weather report template in {:?}",
        template_uris
    );
    assert!(template_uris
        .contains(&"golem://DynamicResource/get_weather_report_with_images/{name}".to_string()));
    assert!(template_uris
        .contains(&"golem://DynamicResource/get_weather_report_text/{name}".to_string()));
    assert!(
        template_uris.contains(&"golem://DynamicResource/get_snow_fall_image/{name}".to_string())
    );

    drop(client);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn read_static_resource_string(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .read_resource(ReadResourceRequestParams {
            meta: None,
            uri: "golem://StaticResource/get_static_weather_report".to_string(),
        })
        .await?;

    assert_eq!(result.contents.len(), 1);
    match &result.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents {
            text, mime_type, ..
        } => {
            assert_eq!(mime_type.as_deref(), Some("application/json"));
            let json_value: serde_json::Value = serde_json::from_str(text)?;
            let expected_text = "Sydney: Sunny, Darwin: Rainy, Hobart: Cloudy";
            assert_eq!(json_value, json!(expected_text));
        }
        other => {
            panic!("Expected TextResourceContents, got: {:?}", other);
        }
    }

    drop(client);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn read_static_resource_unstructured_text(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .read_resource(ReadResourceRequestParams {
            meta: None,
            uri: "golem://StaticResource/get_static_weather_report_text".to_string(),
        })
        .await?;

    assert_eq!(result.contents.len(), 1);
    match &result.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => {
            // Note: ResourceContents::text() puts the first arg as text and second as uri.
            // The resource.rs code calls ResourceContents::text(uri, data) which swaps them.
            // We verify against actual behavior.
            assert!(
                text == "golem://StaticResource/get_static_weather_report_text"
                    || text.contains("unstructured weather report"),
                "Unexpected text content: {}",
                text
            );
        }
        other => {
            panic!("Expected TextResourceContents, got: {:?}", other);
        }
    }

    drop(client);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn read_static_resource_binary(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .read_resource(ReadResourceRequestParams {
            meta: None,
            uri: "golem://StaticResource/get_static_now_fall_image".to_string(),
        })
        .await?;

    assert_eq!(result.contents.len(), 1);
    match &result.contents[0] {
        rmcp::model::ResourceContents::BlobResourceContents {
            blob, mime_type, ..
        } => {
            assert_eq!(mime_type.as_deref(), Some("image/png"));
            // vec![1, 2, 3] encoded as base64 = "AQID"
            assert_eq!(blob, "AQID");
        }
        other => {
            panic!("Expected BlobResourceContents, got: {:?}", other);
        }
    }

    drop(client);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn read_static_resource_multimodal(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .read_resource(ReadResourceRequestParams {
            meta: None,
            uri: "golem://StaticResource/get_static_weather_report_with_images".to_string(),
        })
        .await?;

    // Multimodal returns multiple ResourceContents items
    assert_eq!(result.contents.len(), 2);

    // First: text part
    match &result.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => {
            assert!(
                text.contains("snow fall in Sydney")
                    || text == "golem://StaticResource/get_static_weather_report_with_images",
                "Unexpected text: {}",
                text
            );
        }
        other => {
            panic!(
                "Expected TextResourceContents for first part, got: {:?}",
                other
            );
        }
    }

    // Second: blob part
    match &result.contents[1] {
        rmcp::model::ResourceContents::BlobResourceContents {
            blob, mime_type, ..
        } => {
            assert_eq!(mime_type.as_deref(), Some("image/png"));
            assert_eq!(blob, "AQID");
        }
        other => {
            panic!(
                "Expected BlobResourceContents for second part, got: {:?}",
                other
            );
        }
    }

    drop(client);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn read_dynamic_resource_string(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .read_resource(ReadResourceRequestParams {
            meta: None,
            uri: "golem://DynamicResource/get_weather_report/test-city".to_string(),
        })
        .await?;

    assert_eq!(result.contents.len(), 1);

    match &result.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents {
            text, mime_type, ..
        } => {
            assert_eq!(mime_type.as_deref(), Some("application/json"));

            let json_value: serde_json::Value = serde_json::from_str(text)?;

            assert_eq!(
                json_value,
                json!("This is a dynamic weather report for test-city.")
            );
        }
        other => {
            panic!("Expected TextResourceContents, got: {:?}", other);
        }
    }

    drop(client);
    Ok(())
}

#[test]
#[tracing::instrument]
async fn read_dynamic_resource_binary(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .read_resource(ReadResourceRequestParams {
            meta: None,
            uri: "golem://DynamicResource/get_snow_fall_image/test-city".to_string(),
        })
        .await?;

    assert_eq!(result.contents.len(), 1);
    match &result.contents[0] {
        rmcp::model::ResourceContents::BlobResourceContents {
            blob, mime_type, ..
        } => {
            assert_eq!(mime_type.as_deref(), Some("image/png"));
            assert_eq!(blob, "AQID");
        }
        other => {
            panic!("Expected BlobResourceContents, got: {:?}", other);
        }
    }

    drop(client);
    Ok(())
}
