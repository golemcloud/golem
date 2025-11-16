//! This module contains utility functions for querying and displaying server capabilities.

use colored::Colorize;
use rust_mcp_sdk::schema::CallToolRequestParams;
use rust_mcp_sdk::McpClient;
use rust_mcp_sdk::{ error::SdkResult, mcp_client::ClientRuntime };
use serde_json::json;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

const GREY_COLOR: (u8, u8, u8) = (90, 90, 90);
const HEADER_SIZE: usize = 31;

pub struct InquiryUtils {
    pub client: Arc<ClientRuntime>,
}

impl InquiryUtils {
    fn print_header(&self, title: &str) {
        let pad = ((HEADER_SIZE as f32) / 2.0 + (title.len() as f32) / 2.0).floor() as usize;
        println!("\n{}", "=".repeat(HEADER_SIZE).custom_color(GREY_COLOR));
        println!("{:>pad$}", title.custom_color(GREY_COLOR));
        println!("{}", "=".repeat(HEADER_SIZE).custom_color(GREY_COLOR));
    }

    fn print_list(&self, list_items: Vec<(String, String)>) {
        list_items
            .iter()
            .enumerate()
            .for_each(|(index, item)| {
                println!("{}. {}: {}", index + 1, item.0.yellow(), item.1.cyan());
            });
    }

    pub fn print_server_info(&self) {
        self.print_header("Server info");
        let server_version = self.client.server_version().unwrap();
        println!("{} {}", "Server name:".bold(), server_version.name.cyan());
        println!("{} {}", "Server version:".bold(), server_version.version.cyan());
    }

    pub fn print_server_capabilities(&self) {
        self.print_header("Capabilities");
        let capability_vec = [
            ("tools", self.client.server_has_tools()),
            ("prompts", self.client.server_has_prompts()),
            ("resources", self.client.server_has_resources()),
            ("logging", self.client.server_supports_logging()),
            ("experimental", self.client.server_has_experimental()),
        ];

        capability_vec.iter().for_each(|(tool_name, opt)| {
            println!(
                "{}: {}",
                tool_name.bold(),
                opt
                    .map(|b| if b { "Yes" } else { "No" })
                    .unwrap_or("Unknown")
                    .cyan()
            );
        });
    }

    pub async fn print_tool_list(&self) -> SdkResult<()> {
        // Return if the MCP server does not support tools
        if !self.client.server_has_tools().unwrap_or(false) {
            return Ok(());
        }

        let tools = self.client.list_tools(None).await?;
        self.print_header("Tools");
        self.print_list(
            tools.tools
                .iter()
                .map(|item| { (item.name.clone(), item.description.clone().unwrap_or_default()) })
                .collect()
        );

        Ok(())
    }

    pub async fn print_prompts_list(&self) -> SdkResult<()> {
        // Return if the MCP server does not support prompts
        if !self.client.server_has_prompts().unwrap_or(false) {
            return Ok(());
        }

        let prompts = self.client.list_prompts(None).await?;

        self.print_header("Prompts");
        self.print_list(
            prompts.prompts
                .iter()
                .map(|item| { (item.name.clone(), item.description.clone().unwrap_or_default()) })
                .collect()
        );
        Ok(())
    }

    pub async fn print_resource_list(&self) -> SdkResult<()> {
        // Return if the MCP server does not support resources
        if !self.client.server_has_resources().unwrap_or(false) {
            return Ok(());
        }

        let resources = self.client.list_resources(None).await?;

        self.print_header("Resources");

        self.print_list(
            resources.resources
                .iter()
                .map(|item| {
                    (
                        item.name.clone(),
                        format!(
                            "( uri: {} , mime: {}",
                            item.uri,
                            item.mime_type.as_ref().unwrap_or(&"?".to_string())
                        ),
                    )
                })
                .collect()
        );

        Ok(())
    }

    pub async fn print_resource_templates(&self) -> SdkResult<()> {
        // Return if the MCP server does not support resources
        if !self.client.server_has_resources().unwrap_or(false) {
            return Ok(());
        }

        let templates = self.client.list_resource_templates(None).await?;

        self.print_header("Resource Templates");

        self.print_list(
            templates.resource_templates
                .iter()
                .map(|item| { (item.name.clone(), item.description.clone().unwrap_or_default()) })
                .collect()
        );
        Ok(())
    }

    pub async fn call_add_tool(&self, a: i64, b: i64) -> SdkResult<()> {
        // Invoke the "add" tool with 100 and 25 as arguments, and display the result
        println!("{}", format!("\nCalling the \"add\" tool with {a} and {b} ...").magenta());

        // Create a `Map<String, Value>` to represent the tool parameters
        let params = json!({
            "a": a,
            "b": b
        })
            .as_object()
            .unwrap()
            .clone();

        // invoke the tool
        let result = self.client.call_tool(CallToolRequestParams {
            name: "add".to_string(),
            arguments: Some(params),
        }).await?;

        // Retrieve the result content and print it to the stdout
        let result_content = result.content.first().unwrap().as_text_content()?;
        println!("{}", result_content.text.green());

        Ok(())
    }

    pub async fn call_call_tool(
        &self,
        name: &str,
        args: Vec<String>,
        cwd: Option<String>
    ) -> SdkResult<String> {
        // Friendly log
        println!(
            "{}",
            format!(
                "\nCalling the \"call_tool\" with name=\"{}\", args={:?}, cwd={:?} ...",
                name,
                args,
                cwd
            ).magenta()
        );

        // Build the tool parameters to match CallToolTool { name, arguments: GolemRunInput { args, cwd } }
        let params =
            json!({
            "name": name,
            "arguments": {
                "args": args,     // Vec<String>
                "cwd": cwd        // Option<String>
            }
        })
                .as_object()
                .unwrap()
                .clone();

        // Invoke the server tool named "call_tool"
        let result = self.client.call_tool(CallToolRequestParams {
            name: "call_tool".to_string(),
            arguments: Some(params),
        }).await?;

        // Retrieve and print the result content
        let result_content = result.content.first().unwrap().as_text_content()?;
        println!("{}", result_content.text.green());

        Ok(result_content.text.clone())
    }



    pub async fn ping_n_times(&self, n: i32) {
        let max_pings = n;
        println!();
        for ping_index in 1..=max_pings {
            print!("Ping the server ({ping_index} out of {max_pings})...");
            std::io::stdout().flush().unwrap();
            let ping_result = self.client.ping(None).await;
            print!("\rPing the server ({} out of {}) : {}", ping_index, max_pings, if
                ping_result.is_ok()
            {
                "success".bright_green()
            } else {
                "failed".bright_red()
            });
            println!();
            sleep(Duration::from_secs(2)).await;
        }
    }
}
