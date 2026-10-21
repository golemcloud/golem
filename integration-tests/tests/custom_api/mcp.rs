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
use integration_tests::mcp_client::McpClient;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use test_r::{define_matrix_dimension, tag, test_dep};
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(
    #[tagged_as("postgres")]
    EnvBasedTestDependencies
);
inherit_test_dep!(
    #[tagged_as("sqlite")]
    EnvBasedTestDependencies
);

pub struct McpTestContext {
    pub domain: Domain,
    pub mcp_port: u16,
    pub custom_request_port: u16,
}

impl Debug for McpTestContext {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "McpTestContext")
    }
}

impl McpTestContext {
    async fn connect_mcp_client(&self) -> anyhow::Result<McpClient> {
        self.connect_mcp_client_on(self.mcp_port).await
    }

    async fn connect_mcp_client_on(&self, port: u16) -> anyhow::Result<McpClient> {
        McpClient::connect(reqwest::Client::new(), &self.mcp_url(port), &self.domain.0).await
    }

    fn mcp_url(&self, port: u16) -> String {
        format!("http://{}:{}/mcp", self.mcp_host(), port)
    }

    fn mcp_host(&self) -> &'static str {
        "127.0.0.1"
    }
}

async fn build_test_context(deps: &EnvBasedTestDependencies) -> McpTestContext {
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
        tools: BTreeMap::new(),
        agents: BTreeMap::from_iter(vec![
            (
                AgentTypeName("WeatherAgent".to_string()),
                McpDeploymentAgentOptions::default(),
            ),
            (
                AgentTypeName("WeatherAgentSingleton".to_string()),
                McpDeploymentAgentOptions::default(),
            ),
            (
                AgentTypeName("StaticResource".to_string()),
                McpDeploymentAgentOptions::default(),
            ),
            (
                AgentTypeName("DynamicResource".to_string()),
                McpDeploymentAgentOptions::default(),
            ),
        ]),
    };

    client
        .create_mcp_deployment(&env.id.0, &mcp_deployment_creation)
        .await
        .unwrap();

    user.deploy_environment(env.id).await.unwrap();

    McpTestContext {
        domain,
        mcp_port: deps.worker_service().mcp_port(),
        custom_request_port: deps.worker_service().custom_request_port(),
    }
}

#[test_dep(scope = PerWorker)]
async fn test_context(deps: &EnvBasedTestDependencies) -> McpTestContext {
    build_test_context(deps).await
}

#[test_dep(scope = PerWorker, tagged_as = "postgres")]
async fn test_context_postgres(
    #[tagged_as("postgres")] deps: &EnvBasedTestDependencies,
) -> McpTestContext {
    build_test_context(deps).await
}

#[test_dep(scope = PerWorker, tagged_as = "sqlite")]
async fn test_context_sqlite(
    #[tagged_as("sqlite")] deps: &EnvBasedTestDependencies,
) -> McpTestContext {
    build_test_context(deps).await
}

define_matrix_dimension!(db: McpTestContext -> "postgres", "sqlite");

#[test]
#[tracing::instrument]
async fn list_tools(#[dimension(db)] ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;
    let tools = client.list_tools().await?;

    let tool_names: Vec<String> = tools
        .iter()
        .filter_map(|t| t["name"].as_str().map(String::from))
        .collect();

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
    assert!(
        !tool_names.contains(&"WeatherAgent-stream_weather_report_for_city".to_string()),
        "Streaming methods must not be exported as tools: {:?}",
        tool_names
    );

    // WeatherAgentSingleton tools (no constructor params)
    assert!(tool_names.contains(&"WeatherAgentSingleton-get_weather_report_for_city".to_string()));
    assert!(
        tool_names
            .contains(&"WeatherAgentSingleton-get_weather_report_for_city_with_images".to_string())
    );
    assert!(
        tool_names.contains(&"WeatherAgentSingleton-get_weather_report_for_city_text".to_string())
    );
    assert!(tool_names.contains(&"WeatherAgentSingleton-get_snow_fall_image_for_city".to_string()));
    assert!(tool_names.contains(&"WeatherAgentSingleton-get_lat_long_for_city".to_string()));

    // StaticResource and DynamicResource methods are read-only and have no input params,
    // so they are exposed as resources, not tools.
    assert!(!tool_names.iter().any(|n| n.starts_with("StaticResource")));
    assert!(!tool_names.iter().any(|n| n.starts_with("DynamicResource")));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn mcp_deployment_is_served_on_the_custom_request_port(
    #[dimension(db)] ctx: &McpTestContext,
) -> anyhow::Result<()> {
    let tool_names = |tools: Vec<Value>| -> Vec<String> {
        let mut names: Vec<String> = tools
            .iter()
            .filter_map(|t| t["name"].as_str().map(String::from))
            .collect();
        names.sort();
        names
    };

    let on_mcp_port = tool_names(ctx.connect_mcp_client().await?.list_tools().await?);
    let on_custom_request_port = tool_names(
        ctx.connect_mcp_client_on(ctx.custom_request_port)
            .await?
            .list_tools()
            .await?,
    );
    assert!(!on_mcp_port.is_empty());
    assert_eq!(on_custom_request_port, on_mcp_port);

    let http = reqwest::Client::new();

    // An MCP domain serves only the MCP routes, even on the shared port.
    let response = http
        .get(format!(
            "http://{}:{}/some/http/route",
            ctx.mcp_host(),
            ctx.custom_request_port
        ))
        .header("Host", &ctx.domain.0)
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);

    // A client that is not a trusted ingress cannot reach the MCP domain with forwarded headers.
    let response = http
        .post(format!(
            "http://{}:{}/mcp",
            ctx.mcp_host(),
            ctx.custom_request_port
        ))
        .header("Host", "unknown.golem.test")
        .header("X-Forwarded-Host", &ctx.domain.0)
        .header("X-Forwarded-Proto", "https")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body("{}")
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);

    Ok(())
}

/// Each port has its own MCP endpoint with its own replica memory, over one session store. The
/// session record alone carries the session from the port that issued it to the other.
#[test]
#[tag(db_postgres)]
#[tracing::instrument]
async fn mcp_session_survives_across_ports(
    #[tagged_as("postgres")] ctx: &McpTestContext,
) -> anyhow::Result<()> {
    let on_mcp_port = McpClient::initialize(
        reqwest::Client::new(),
        &ctx.mcp_url(ctx.mcp_port),
        &ctx.domain.0,
    )
    .await?;
    assert!(on_mcp_port.session_id().is_some());

    let on_custom_request_port = on_mcp_port.at(&ctx.mcp_url(ctx.custom_request_port));
    on_custom_request_port
        .send_initialized_notification()
        .await?;
    let tools = on_custom_request_port.list_tools().await?;
    assert!(
        tools
            .iter()
            .any(|tool| tool["name"] == "WeatherAgent-get_weather_report_for_city"),
        "{tools:?}"
    );
    let result = on_custom_request_port
        .call_tool(
            "WeatherAgent-get_weather_report_for_city",
            json!({ "name": "across-ports", "city": "Sydney" }),
        )
        .await?;
    assert_eq!(
        result["structuredContent"]["value"],
        json!("Agent across-ports: This is a weather report for Sydney")
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_string(
    #[dimension(db)] ctx: &McpTestContext,
) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(
            "WeatherAgent-get_weather_report_for_city",
            json!({ "name": "test-agent", "city": "Sydney" }),
        )
        .await?;

    assert_eq!(result["isError"], json!(false));
    assert_eq!(
        result["structuredContent"]["value"],
        json!("Agent test-agent: This is a weather report for Sydney")
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_multimodal(
    #[dimension(db)] ctx: &McpTestContext,
) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(
            "WeatherAgent-get_weather_report_for_city_with_images",
            json!({ "name": "test-agent", "city": "Sydney" }),
        )
        .await?;

    assert_eq!(result["isError"], json!(false));

    let multimodal_array = &result["content"].as_array().unwrap();

    assert_eq!(multimodal_array.len(), 2);

    let unstructured_text_content = &multimodal_array[0];

    assert!(
        unstructured_text_content["text"]
            .as_str()
            .unwrap()
            .contains("snow fall in Sydney")
    );

    let image_content = &multimodal_array[1];

    assert_eq!(image_content["data"].as_str().unwrap(), "AQID");
    assert_eq!(image_content["mimeType"].as_str().unwrap(), "image/png");

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_unstructured_text(
    #[dimension(db)] ctx: &McpTestContext,
) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(
            "WeatherAgent-get_weather_report_for_city_text",
            json!({ "name": "test-agent", "city": "Sydney" }),
        )
        .await?;

    assert_eq!(result["isError"], json!(false));

    let unstructured_text = &result["content"].as_array().unwrap();

    assert!(
        unstructured_text[0]["text"]
            .as_str()
            .unwrap()
            .contains("unstructured weather report for Sydney"),
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_unstructured_binary(
    #[dimension(db)] ctx: &McpTestContext,
) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(
            "WeatherAgent-get_snow_fall_image_for_city",
            json!({ "name": "test-agent", "city": "Sydney" }),
        )
        .await?;

    assert_eq!(result["isError"], json!(false));

    let contents = &result["content"];

    // Binary data is base64 encoded: vec![1, 2, 3] -> "AQID"
    assert_eq!(contents[0]["data"], "AQID");
    assert_eq!(contents[0]["mimeType"], "image/png");

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_component_model(
    #[dimension(db)] ctx: &McpTestContext,
) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(
            "WeatherAgent-get_lat_long_for_city",
            json!({ "name": "test-agent", "city": "Sydney" }),
        )
        .await?;

    assert_eq!(result["isError"], json!(false));
    let structured = &result["structuredContent"]["value"];

    assert_eq!(structured["lat"], json!(0.0));
    assert_eq!(structured["long"], json!(0.0));
    assert_eq!(structured["country"], "Unknown");
    assert_eq!(structured["population"], "0");

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_singleton_string(#[dimension(db)] ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(
            "WeatherAgentSingleton-get_weather_report_for_city",
            json!({ "city": "Darwin" }),
        )
        .await?;

    assert_eq!(result["isError"], json!(false));
    assert_eq!(
        result["structuredContent"]["value"],
        json!("This is a weather report for Darwin.")
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_singleton_component_model(
    #[dimension(db)] ctx: &McpTestContext,
) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(
            "WeatherAgentSingleton-get_lat_long_for_city",
            json!({ "city": "Darwin" }),
        )
        .await?;

    assert_eq!(result["isError"], json!(false));

    let location = &result["structuredContent"]["value"];

    assert_eq!(location["lat"], json!(0.0));
    assert_eq!(location["long"], json!(0.0));
    assert_eq!(location["country"], "Unknown");
    assert_eq!(location["population"], "0");

    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_resources(#[dimension(db)] ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;
    let resources = client.list_resources().await?;

    let resource_uris: Vec<String> = resources
        .iter()
        .filter_map(|r| r["uri"].as_str().map(String::from))
        .collect();

    assert!(
        resource_uris.contains(&"golem://StaticResource/get_static_weather_report".to_string()),
        "Expected static weather report resource in {:?}",
        resource_uris
    );
    assert!(
        resource_uris
            .contains(&"golem://StaticResource/get_static_weather_report_with_images".to_string())
    );
    assert!(
        resource_uris
            .contains(&"golem://StaticResource/get_static_weather_report_text".to_string())
    );
    assert!(
        resource_uris.contains(&"golem://StaticResource/get_static_now_fall_image".to_string())
    );
    assert!(
        !resource_uris.contains(&"golem://WeatherAgent/stream_weather_report_for_city".to_string())
    );

    assert!(
        !resource_uris
            .iter()
            .any(|u| u.starts_with("golem://DynamicResource"))
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_resource_templates(#[dimension(db)] ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;
    let templates = client.list_resource_templates().await?;

    let template_uris: Vec<String> = templates
        .iter()
        .filter_map(|t| t["uriTemplate"].as_str().map(String::from))
        .collect();

    assert!(
        template_uris.contains(&"golem://DynamicResource/get_weather_report/{name}".to_string()),
        "Expected dynamic weather report template in {:?}",
        template_uris
    );
    assert!(
        template_uris
            .contains(&"golem://DynamicResource/get_weather_report_with_images/{name}".to_string())
    );
    assert!(
        template_uris
            .contains(&"golem://DynamicResource/get_weather_report_text/{name}".to_string())
    );
    assert!(
        template_uris.contains(&"golem://DynamicResource/get_snow_fall_image/{name}".to_string())
    );
    assert!(
        !template_uris
            .contains(&"golem://WeatherAgent/stream_weather_report_for_city/{name}".to_string())
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn read_static_resource_string(#[dimension(db)] ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .read_resource("golem://StaticResource/get_static_weather_report")
        .await?;

    let contents = result["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 1);

    assert_eq!(contents[0]["mimeType"].as_str(), Some("application/json"));
    let text = contents[0]["text"].as_str().unwrap();
    let json_value: Value = serde_json::from_str(text)?;
    assert_eq!(
        json_value,
        json!("Sydney: Sunny, Darwin: Rainy, Hobart: Cloudy")
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn read_static_resource_unstructured_text(
    #[dimension(db)] ctx: &McpTestContext,
) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .read_resource("golem://StaticResource/get_static_weather_report_text")
        .await?;

    let contents = result["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 1);

    let text = contents[0]["text"].as_str().unwrap();
    assert!(
        text.contains("unstructured weather report")
            || text == "golem://StaticResource/get_static_weather_report_text",
        "Unexpected text content: {}",
        text
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn read_static_resource_binary(#[dimension(db)] ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .read_resource("golem://StaticResource/get_static_now_fall_image")
        .await?;

    let contents = result["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 1);

    assert_eq!(contents[0]["mimeType"].as_str(), Some("image/png"));

    // vec![1, 2, 3] encoded as base64 = "AQID"
    assert_eq!(contents[0]["blob"].as_str(), Some("AQID"));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn read_static_resource_multimodal(
    #[dimension(db)] ctx: &McpTestContext,
) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .read_resource("golem://StaticResource/get_static_weather_report_with_images")
        .await?;

    let contents = result["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 2);

    let text = contents[0]["text"].as_str().unwrap();
    assert!(
        text.contains("snow fall in Sydney")
            || text == "golem://StaticResource/get_static_weather_report_with_images",
        "Unexpected text: {}",
        text
    );

    assert_eq!(contents[1]["mimeType"].as_str(), Some("image/png"));
    assert_eq!(contents[1]["blob"].as_str(), Some("AQID"));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn read_dynamic_resource_string(#[dimension(db)] ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .read_resource("golem://DynamicResource/get_weather_report/test-city")
        .await?;

    let contents = result["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 1);

    assert_eq!(contents[0]["mimeType"].as_str(), Some("application/json"));
    let text = contents[0]["text"].as_str().unwrap();
    let json_value: Value = serde_json::from_str(text)?;
    assert_eq!(
        json_value,
        json!("This is a dynamic weather report for test-city.")
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn read_dynamic_resource_binary(#[dimension(db)] ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .read_resource("golem://DynamicResource/get_snow_fall_image/test-city")
        .await?;

    let contents = result["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 1);

    assert_eq!(contents[0]["mimeType"].as_str(), Some("image/png"));
    assert_eq!(contents[0]["blob"].as_str(), Some("AQID"));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_prompts(#[dimension(db)] ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;
    let prompts = client.list_prompts().await?;

    let prompt_names: Vec<String> = prompts
        .iter()
        .filter_map(|p| p["name"].as_str().map(String::from))
        .collect();

    assert!(
        prompt_names.contains(&"WeatherAgent".to_string()),
        "Expected WeatherAgent prompt in {:?}",
        prompt_names
    );

    assert!(
        prompt_names.contains(&"WeatherAgent-get_weather_report_for_city".to_string()),
        "Expected WeatherAgent-get_weather_report_for_city prompt in {:?}",
        prompt_names
    );
    assert!(
        !prompt_names.contains(&"WeatherAgent-stream_weather_report_for_city".to_string()),
        "Streaming method prompts must not be exported: {:?}",
        prompt_names
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn get_prompt_agent_level(#[dimension(db)] ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client.get_prompt("WeatherAgent").await?;

    let messages = result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"].as_str(), Some("user"));

    let text = messages[0]["content"]["text"].as_str().unwrap();
    assert_eq!(
        text,
        "You are a weather agent. Help the user get weather information for cities."
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn get_prompt_method(#[dimension(db)] ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .get_prompt("WeatherAgent-get_weather_report_for_city")
        .await?;

    let messages = result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"].as_str(), Some("user"));

    let text = messages[0]["content"]["text"].as_str().unwrap();
    let expected = "Get a weather report for a specific city\n\n\
                    Expected JSON input properties: name, city\n\n\
                    output hint: JSON";
    assert_eq!(
        text, expected,
        "Method prompt text mismatch.\nGot: {}\nExpected: {}",
        text, expected
    );

    Ok(())
}
