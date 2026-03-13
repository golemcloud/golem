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
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::sync::atomic::{AtomicU64, Ordering};
use test_r::test_dep;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

const MCP_PORT: u16 = 9007;

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

struct McpClient {
    http: reqwest::Client,
    url: String,
    session_id: Option<String>,
}

fn parse_sse_json(body: &str) -> anyhow::Result<Value> {
    for line in body.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if !data.is_empty() {
                return Ok(serde_json::from_str(data)?);
            }
        }
    }
    anyhow::bail!("No data line found in SSE response: {}", body)
}

impl McpClient {
    fn next_id() -> u64 {
        REQUEST_ID.fetch_add(1, Ordering::SeqCst)
    }

    async fn new(url: String, host: &str) -> anyhow::Result<Self> {
        let http = reqwest::Client::new();

        // Send initialize request
        let init_req = json!({
            "jsonrpc": "2.0",
            "id": Self::next_id(),
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {
                    "name": "golem-mcp-integration-test",
                    "version": "0.0.1"
                }
            }
        });

        let resp = http
            .post(&url)
            .header("Host", host)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&init_req)
            .send()
            .await?;

        let session_id = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        // Response is SSE, parse the JSON from it
        let body = resp.text().await?;
        let _init_result = parse_sse_json(&body)?;

        // Send initialized notification
        let notif = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });

        let mut notif_req = http
            .post(&url)
            .header("Host", host)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        if let Some(sid) = &session_id {
            notif_req = notif_req.header("mcp-session-id", sid.as_str());
        }

        notif_req.json(&notif).send().await?;

        Ok(McpClient {
            http,
            url,
            session_id,
        })
    }

    async fn request(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let req_body = json!({
            "jsonrpc": "2.0",
            "id": Self::next_id(),
            "method": method,
            "params": params
        });

        let mut builder = self
            .http
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        if let Some(sid) = &self.session_id {
            builder = builder.header("mcp-session-id", sid.as_str());
        }

        let resp = builder.json(&req_body).send().await?;
        let body = resp.text().await?;
        let json_body = parse_sse_json(&body)?;

        if let Some(error) = json_body.get("error") {
            anyhow::bail!("MCP error: {}", error);
        }

        Ok(json_body["result"].clone())
    }

    async fn list_tools(&self) -> anyhow::Result<Vec<Value>> {
        let result = self.request("tools/list", json!({})).await?;
        Ok(result["tools"].as_array().cloned().unwrap_or_default())
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> anyhow::Result<Value> {
        self.request(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments
            }),
        )
        .await
    }

    async fn list_resources(&self) -> anyhow::Result<Vec<Value>> {
        let result = self.request("resources/list", json!({})).await?;
        Ok(result["resources"].as_array().cloned().unwrap_or_default())
    }

    async fn list_resource_templates(&self) -> anyhow::Result<Vec<Value>> {
        let result = self.request("resources/templates/list", json!({})).await?;
        Ok(result["resourceTemplates"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    async fn read_resource(&self, uri: &str) -> anyhow::Result<Value> {
        self.request("resources/read", json!({ "uri": uri })).await
    }

    async fn list_prompts(&self) -> anyhow::Result<Vec<Value>> {
        let result = self.request("prompts/list", json!({})).await?;
        Ok(result["prompts"].as_array().cloned().unwrap_or_default())
    }

    async fn get_prompt(&self, name: &str) -> anyhow::Result<Value> {
        self.request("prompts/get", json!({ "name": name })).await
    }
}

pub struct McpTestContext {
    pub domain: Domain,
}

impl Debug for McpTestContext {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "McpTestContext")
    }
}

impl McpTestContext {
    async fn connect_mcp_client(&self) -> anyhow::Result<McpClient> {
        let url = format!("http://127.0.0.1:{}/mcp", MCP_PORT);
        McpClient::new(url, &self.domain.0).await
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

    McpTestContext { domain }
}

#[test]
#[tracing::instrument]
async fn list_tools(ctx: &McpTestContext) -> anyhow::Result<()> {
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

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_string(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(
            "WeatherAgent-get_weather_report_for_city",
            json!({ "name": "test-agent", "city": "Sydney" }),
        )
        .await?;

    assert_eq!(result["isError"], json!(false));
    assert_eq!(
        result["structuredContent"],
        json!("Agent test-agent: This is a weather report for Sydney")
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_multimodal(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(
            "WeatherAgent-get_weather_report_for_city_with_images",
            json!({ "name": "test-agent", "city": "Sydney" }),
        )
        .await?;

    assert_eq!(result["isError"], json!(false));
    let structured = &result["structuredContent"];

    let parts = structured["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2);

    assert!(parts[0]["value"]["data"]
        .as_str()
        .unwrap()
        .contains("snow fall in Sydney"));

    assert_eq!(parts[1]["value"]["mimeType"], "image/png");
    assert!(parts[1]["value"]["data"].as_str().is_some());

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_unstructured_text(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(
            "WeatherAgent-get_weather_report_for_city_text",
            json!({ "name": "test-agent", "city": "Sydney" }),
        )
        .await?;

    assert_eq!(result["isError"], json!(false));
    let structured = &result["structuredContent"];
    assert!(
        structured["data"]
            .as_str()
            .unwrap()
            .contains("unstructured weather report for Sydney"),
        "Expected unstructured text, got: {:?}",
        structured
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_unstructured_binary(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(
            "WeatherAgent-get_snow_fall_image_for_city",
            json!({ "name": "test-agent", "city": "Sydney" }),
        )
        .await?;

    assert_eq!(result["isError"], json!(false));
    let structured = &result["structuredContent"];

    // Binary data is base64 encoded: vec![1, 2, 3] -> "AQID"
    assert_eq!(structured["data"], "AQID");
    assert_eq!(structured["mimeType"], "image/png");

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_weather_agent_component_model(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(
            "WeatherAgent-get_lat_long_for_city",
            json!({ "name": "test-agent", "city": "Sydney" }),
        )
        .await?;

    assert_eq!(result["isError"], json!(false));
    let structured = &result["structuredContent"];

    assert_eq!(structured["lat"], json!(0.0));
    assert_eq!(structured["long"], json!(0.0));
    assert_eq!(structured["country"], "Unknown");
    assert_eq!(structured["population"], 0);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_singleton_string(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(
            "WeatherAgentSingleton-get_weather_report_for_city",
            json!({ "city": "Darwin" }),
        )
        .await?;

    assert_eq!(result["isError"], json!(false));
    assert_eq!(
        result["structuredContent"],
        json!("This is a weather report for Darwin.")
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn call_tool_singleton_component_model(ctx: &McpTestContext) -> anyhow::Result<()> {
    let client = ctx.connect_mcp_client().await?;

    let result = client
        .call_tool(
            "WeatherAgentSingleton-get_lat_long_for_city",
            json!({ "city": "Darwin" }),
        )
        .await?;

    assert_eq!(result["isError"], json!(false));
    let structured = &result["structuredContent"];
    assert_eq!(structured["lat"], json!(0.0));
    assert_eq!(structured["long"], json!(0.0));
    assert_eq!(structured["country"], "Unknown");
    assert_eq!(structured["population"], 0);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_resources(ctx: &McpTestContext) -> anyhow::Result<()> {
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
    assert!(resource_uris
        .contains(&"golem://StaticResource/get_static_weather_report_with_images".to_string()));
    assert!(resource_uris
        .contains(&"golem://StaticResource/get_static_weather_report_text".to_string()));
    assert!(resource_uris.contains(&"golem://StaticResource/get_static_now_fall_image".to_string()));

    assert!(!resource_uris
        .iter()
        .any(|u| u.starts_with("golem://DynamicResource")));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn list_resource_templates(ctx: &McpTestContext) -> anyhow::Result<()> {
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
    assert!(template_uris
        .contains(&"golem://DynamicResource/get_weather_report_with_images/{name}".to_string()));
    assert!(template_uris
        .contains(&"golem://DynamicResource/get_weather_report_text/{name}".to_string()));
    assert!(
        template_uris.contains(&"golem://DynamicResource/get_snow_fall_image/{name}".to_string())
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn read_static_resource_string(ctx: &McpTestContext) -> anyhow::Result<()> {
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
async fn read_static_resource_unstructured_text(ctx: &McpTestContext) -> anyhow::Result<()> {
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
async fn read_static_resource_binary(ctx: &McpTestContext) -> anyhow::Result<()> {
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
async fn read_static_resource_multimodal(ctx: &McpTestContext) -> anyhow::Result<()> {
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
async fn read_dynamic_resource_string(ctx: &McpTestContext) -> anyhow::Result<()> {
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
async fn read_dynamic_resource_binary(ctx: &McpTestContext) -> anyhow::Result<()> {
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
async fn list_prompts(ctx: &McpTestContext) -> anyhow::Result<()> {
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

    Ok(())
}

#[test]
#[tracing::instrument]
async fn get_prompt_agent_level(ctx: &McpTestContext) -> anyhow::Result<()> {
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
async fn get_prompt_method(ctx: &McpTestContext) -> anyhow::Result<()> {
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
                    Output: result: Str";
    assert_eq!(
        text, expected,
        "Method prompt text mismatch.\nGot: {}\nExpected: {}",
        text, expected
    );

    Ok(())
}
