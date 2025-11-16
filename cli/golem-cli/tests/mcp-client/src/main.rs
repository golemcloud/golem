mod handler;
mod inquiry_utils;

use handler::MyClientHandler;

use rust_mcp_sdk::error::SdkResult;
use rust_mcp_sdk::mcp_client::client_runtime;
use rust_mcp_sdk::mcp_client::client_runtime_core;

use rust_mcp_sdk::schema::{
    ClientCapabilities,
    Implementation,
    InitializeRequestParams,
    LoggingLevel,
    LATEST_PROTOCOL_VERSION,
};
use rust_mcp_sdk::{ McpClient, RequestOptions, ClientSseTransport, ClientSseTransportOptions };
use std::sync::Arc;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use regex::Regex;
use serde_json::Value;

struct Cmd {
    tool: &'static str,
    args: Vec<String>,
    cwd: Option<String>,
}

use crate::inquiry_utils::InquiryUtils;

const MCP_SERVER_URL: &str = "http://127.0.0.1:3001/mcp";

#[tokio::main]
async fn main() -> SdkResult<()> {
    tracing_subscriber
        ::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Step1 : Define client details and capabilities
    let client_details: InitializeRequestParams = InitializeRequestParams {
        capabilities: ClientCapabilities::default(),
        client_info: Implementation {
            name: "golem-mcp-client-sse".to_string(),
            version: "0.1.0".to_string(),
            title: Some("Golem MCP Client (SSE)".to_string()),
        },
        protocol_version: LATEST_PROTOCOL_VERSION.into(),
    };

    // Step2 : Create a transport, with options to launch/connect to a MCP Server
    // Assuming @modelcontextprotocol/server-everything is launched with sse argument and listening on port 3001
    let transport = ClientSseTransport::new(MCP_SERVER_URL, ClientSseTransportOptions::default())?;

    // STEP 3: instantiate our custom handler that is responsible for handling MCP messages
    let handler = MyClientHandler {};

    // STEP 4: create the client
    let client = client_runtime::create_client(client_details, transport, handler);

    // STEP 5: start the MCP client
    client.clone().start().await?;

    // You can utilize the client and its methods to interact with the MCP Server.
    // The following demonstrates how to use client methods to retrieve server information,
    // and print them in the terminal, set the log level, invoke a tool, and more.

    // Create a struct with utility functions for demonstration purpose, to utilize different client methods and display the information.
    let utils = InquiryUtils {
        client: Arc::clone(&client),
    };

    // Display server information (name and version)
    utils.print_server_info();

    // Display server capabilities
    utils.print_server_capabilities();

    // Display the list of tools available on the server
    utils.print_tool_list().await?;

    // Display the list of prompts available on the server
    //utils.print_prompts_list().await?;

    // Display the list of resources available on the server
    //utils.print_resource_list().await?;

    // Display the list of resource templates available on the server
    //utils.print_resource_templates().await?;

    let mut installation_id = String::new();

    // Call add tool, and print the result
    let mut commands = vec![
        Cmd {
            tool: "server",
            args: vec!["run".into()],
            cwd: None,
        },
        Cmd {
            tool: "app",
            args: vec!["new".into(), "mytestapp".into(), "typescript".into()],
            cwd: None,
        },
        Cmd {
            tool: "component",
            args: vec!["new".into(), "typescript".into(), "pack:mycomponent".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "app",
            args: vec!["build".into(), "pack:mycomponent".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "app",
            args: vec!["deploy".into(), "pack:mycomponent".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "app",
            args: vec!["update-agents".into(), "pack:mycomponent".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "app",
            args: vec!["redeploy-agents".into(), "pack:mycomponent".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "app",
            args: vec!["list-agent-types".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "app",
            args: vec!["diagnose".into(), "pack:mycomponent".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "component",
            args: vec!["templates".into()],
            cwd: None,
        },
        Cmd {
            tool: "component",
            args: vec!["build".into(), "pack:mycomponent".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "component",
            args: vec!["deploy".into(), "pack:mycomponent".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "component",
            args: vec!["add-dependency".into(), "--component-name".into(), "pack:mycomponent".into(), "--dependency-type".into(), "wasm-rpc".into(), "--target-component-name".into(), "pack:mycomponent".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "component",
            args: vec!["list".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "component",
            args: vec!["get".into(), "pack:mycomponent".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "component",
            args: vec!["update-agents".into(), "pack:mycomponent".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "component",
            args: vec!["redeploy-agents".into(), "pack:mycomponent".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "plugin",
            args: vec!["register".into(), "cli/golem-cli/test-data/mcp-client/example-plugin/manifest.yaml".into()],
            cwd: None,
        },
        Cmd {
            tool: "component",
            args: vec!["plugin".into(), "install".into(), "pack:mycomponent".into(), "--plugin-name".into(), "my-awesome-plugin-app".into(), "--plugin-version".into(), "1.0.0".into(), "--priority".into(), "1".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "component",
            args: vec!["plugin".into(), "get".into(), "pack:mycomponent".into()],
            cwd: Some("mytestapp".into()),
        }
    ];

    // call commands
    let mut i = 0;
    while i < commands.len() {
        // Borrow the current command, then clone only the fields we need to own
        let (tool, args, cwd) = {
            let c = &commands[i];
            (c.tool, c.args.clone(), c.cwd.clone())
        };

        let output = utils.call_call_tool(tool, args.clone(), cwd.clone()).await?;

        // Trying to get installation id from previous command get, so we can execute the update & uninstall commands
        if tool == "component" && args.as_slice() == ["plugin", "get", "pack:mycomponent"] {
            let ids = extract_installation_ids(&output);
            if let Some(first_id) = ids.first() {
                // installation_id = first_id.clone();

                // Enqueue the update now that we have the installation id
                commands.push(Cmd {
                    tool: "component",
                    args: vec![
                        "plugin".into(),
                        "update".into(),
                        "--installation-id".into(),
                        first_id.clone(),
                        "--priority".into(),
                        "1".into(),
                        "pack:mycomponent".into()
                    ],
                    cwd: Some("mytestapp".into()),
                });

                // Enqueue the uninstall now that we have the installation id
                commands.push(Cmd {
                    tool: "component",
                    args: vec![
                        "plugin".into(),
                        "uninstall".into(),
                        "--installation-id".into(),
                        first_id.clone(),
                        "pack:mycomponent".into()
                    ],
                    cwd: Some("mytestapp".into()),
                });
            }
        }

        i += 1;
    }

    commands = vec![
        Cmd {
            tool: "component",
            args: vec!["diagnose".into(), "pack:mycomponent".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "agent",
            args: vec!["new".into(), r#"mycomponent/counter-agent("clean-agent")"#.into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "agent",
            args: vec!["invoke".into(), r#"counter-agent("clean-agent")"#.into(), r#"mycomponent/counter-agent.{increment}"#.into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "agent",
            args: vec!["get".into(), r#"counter-agent("clean-agent")"#.into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "agent",
            args: vec!["list".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "agent",
            args: vec!["stream".into(), r#"counter-agent("clean-agent")"#.into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "agent",
            args: vec!["update".into(), r#"counter-agent("clean-agent")"#.into(), "manual".into(), "3".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "agent",
            args: vec!["interrupt".into(), r#"counter-agent("clean-agent")"#.into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "agent",
            args: vec!["resume".into(), r#"counter-agent("clean-agent")"#.into()],
            cwd: Some("mytestapp".into()),
        },
         Cmd {
            tool: "agent",
            args: vec!["simulate-crash".into(), r#"counter-agent("clean-agent")"#.into()],
            cwd: Some("mytestapp".into()),
        },
         Cmd {
            tool: "agent",
            args: vec!["oplog".into(), r#"counter-agent("clean-agent")"#.into()],
            cwd: Some("mytestapp".into()),
        },
         Cmd {
            tool: "agent",
            args: vec!["revert".into(), r#"counter-agent("clean-agent")"#.into(),"--number-of-invocations".into(), "1".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "agent",
            args: vec!["invoke".into(),"-t".into(), "-i".into(), "123".into(), r#"counter-agent("clean-agent")"#.into(), r#"mycomponent/counter-agent.{increment}"#.into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "agent",
            args: vec!["cancel-invocation".into(), r#"counter-agent("clean-agent")"#.into(), "123".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "api",
            args: vec!["deploy".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "api",
            args: vec!["definition".into(), "deploy".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "api",
            args: vec!["definition".into(), "list".into()],
            cwd: Some("mytestapp".into()),
        }
    ];

    let mut i = 0;
    while i < commands.len() {
        // Borrow the current command, then clone only the fields we need to own
        let (tool, args, cwd) = {
            let c = &commands[i];
            (c.tool, c.args.clone(), c.cwd.clone())
        };

        let output = utils.call_call_tool(tool, args.clone(), cwd.clone()).await?;
        if tool == "api" && args.as_slice() == ["definition", "list"] {
            if let Some((api_id, version)) = extract_api_id_and_version(&output) {
                commands.push(Cmd {
                    tool: "api",
                    args: vec![
                        "definition".into(),
                        "get".into(),
                        "--id".into(),
                        api_id.clone(),
                        "--version".into(),
                        version.clone()
                    ],
                    cwd: Some("mytestapp".into()),
                });
                commands.push(Cmd {
                    tool: "api",
                    args: vec![
                        "definition".into(),
                        "export".into(),
                        "--id".into(),
                        api_id.clone(),
                        "--version".into(),
                        version.clone()
                    ],
                    cwd: Some("mytestapp".into()),
                });
                commands.push(Cmd {
                    tool: "api",
                    args: vec![
                        "definition".into(),
                        "swagger".into(),
                        "--id".into(),
                        api_id.clone(),
                        "--version".into(),
                        version.clone()
                    ],
                    cwd: Some("mytestapp".into()),
                });
            }
        }

        i += 1;
    }

    /* In some API calls, you may need to create new token in console.golem.cloud, then add Static auth in ./golem/config-v2.json, also create "the test project" or change it to your existing project */
    commands = vec![
        Cmd {
            tool: "api",
            args: vec!["deployment".into(), "deploy".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "api",
            args: vec!["deployment".into(), "list".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "api",
            args: vec!["deployment".into(), "get".into(), "localhost:9006".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "api",
            args: vec!["security-scheme".into(), "create".into(), "--provider-type".into(), "google".into(), "--client-id".into(), "REPLACE_WITH_GOOGLE_CLIENT_ID".into(), "--client-secret".into(), "REPLACE_WITH_GOOGLE_CLIENT_SECRET".into(), "--redirect-url".into(), "http://localhost:9006/auth/callback".into(), "--scope".into(), "openid,email,profile".into(), "my-security".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "api",
            args: vec!["security-scheme".into(), "get".into(), "my-security".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "api",
            args: vec!["cloud".into(), "domain".into(), "new".into(), "the test project".into(), "mytestdomain.com".into(), "--cloud".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "api",
            args: vec!["cloud".into(), "domain".into(), "get".into(), "the test project".into(), "--cloud".into()],
            cwd: Some("mytestapp".into()),
        },
        Cmd {
            tool: "plugin",
            args: vec!["list".into()],
            cwd: None,
        },   
        Cmd {
            tool: "plugin",
            args: vec!["get".into(), "my-awesome-plugin-app/1.0.0".into()],
            cwd: None,
        },
        Cmd {
            tool: "plugin",
            args: vec!["unregister".into(), "my-awesome-plugin-app/1.0.0".into()],
            cwd: None,
        },
        Cmd {
            tool: "profile",
            args: vec!["new".into(), "local2".into()],
            cwd: None,
        },        
        Cmd {
            tool: "profile",
            args: vec!["list".into()],
            cwd: None,
        },
        Cmd {
            tool: "profile",
            args: vec!["switch".into(), "local2".into()],
            cwd: None,
        },
        Cmd {
            tool: "profile",
            args: vec!["config".into(), "local2".into(), "set-format".into(), "json".into()],
            cwd: None,
        },
        Cmd {
            tool: "profile",
            args: vec!["get".into(), "local2".into()],
            cwd: None,
        },
        Cmd {
            tool: "profile",
            args: vec!["switch".into(), "local".into()],
            cwd: None,
        },
        Cmd {
            tool: "profile",
            args: vec!["delete".into(), "local2".into()],
            cwd: None,
        },
        Cmd {
            tool: "cloud",
            args: vec!["account".into(), "new".into(), "Steve".into(), "steve@local".into()],
            cwd: None,
        }
    ];

    let mut i = 0;
    while i < commands.len() {
        // Borrow the current command, then clone only the fields we need to own
        let (tool, args, cwd) = {
            let c = &commands[i];
            (c.tool, c.args.clone(), c.cwd.clone())
        };

        let output = utils.call_call_tool(tool, args.clone(), cwd.clone()).await?;
        // Detect the `cloud account new ...` command that just ran
        if tool == "cloud" && args.as_slice() == ["account", "new", "Steve", "steve@local"] {
            if let Some(account_id) = extract_account_id(&output) {
                // Push the follow-up command using the extracted account ID
                commands.push(Cmd {
                    tool: "cloud",
                    args: vec!["account".into(), "get".into(), "--account-id".into(), account_id.clone()],
                    cwd: None,
                });
                // Push the follow-up command using the extracted account ID
                commands.push(Cmd {
                    tool: "cloud",
                    args: vec!["account".into(), "update".into(), "--account-id".into(), account_id.clone(), "Samuel".into(), "samuel@local".into()],
                    cwd: None,
                });
                // Push the follow-up command using the extracted account ID
                commands.push(Cmd {
                    tool: "cloud",
                    args: vec!["account".into(), "grant".into(), "get".into(), "--account-id".into(), account_id.clone()],
                    cwd: None,
                });
                // Push the follow-up command using the extracted account ID
                commands.push(Cmd {
                    tool: "cloud",
                    args: vec!["account".into(), "grant".into(), "new".into(), "--account-id".into(), account_id.clone(), "Admin".into()],
                    cwd: None,
                });
                // Push the follow-up command using the extracted account ID
                commands.push(Cmd {
                    tool: "cloud",
                    args: vec!["account".into(), "grant".into(), "delete".into(), "--account-id".into(), account_id.clone(), "Admin".into()],
                    cwd: None,
                });
            } else {
                eprintln!("Failed to extract Account ID from output:\n{output}");
            }
        }

        i += 1;
    }

    commands = vec![
        Cmd {
            tool: "cloud",
            args: vec!["project".into(), "new".into(), "the test project".into()],
            cwd: None,
        },
        Cmd {
            tool: "cloud",
            args: vec!["project".into(), "list".into()],
            cwd: None,
        },
        Cmd {
            tool: "cloud",
            args: vec!["project".into(), "get-default".into()],
            cwd: None,
        },
        Cmd {
            tool: "cloud",
            args: vec!["project".into(), "grant".into(), "the test project".into(), "samuel@local".into(), "--action".into(), "ViewComponent".into()],
            cwd: None,
        },
        Cmd {
            tool: "cloud",
            args: vec!["project".into(), "policy".into(), "new".into(), "main".into(), "--cloud".into()],
            cwd: None,
        },
    ];

    let mut i = 0;
    while i < commands.len() {
        // Borrow the current command, then clone only the fields we need to own
        let (tool, args, cwd) = {
            let c = &commands[i];
            (c.tool, c.args.clone(), c.cwd.clone())
        };

        let output = utils.call_call_tool(tool, args.clone(), cwd.clone()).await?;
        // Detect the `cloud account new ...` command that just ran
        if tool == "cloud" && args.as_slice() == ["project", "policy", "new", "main", "--cloud"] {
            if let Some(policy_id) = extract_policy_id(&output) {
                // Push the follow-up command using the extracted policy ID
                commands.push(Cmd {
                    tool: "cloud",
                    args: vec!["project".into(), "policy".into(), "get".into(), policy_id, "--cloud".into()],
                    cwd: None,
                });
            } else {
                eprintln!("Failed to extract Policy ID from output:\n{output}");
            }
        }

        i += 1;
    }


    commands = vec![
        Cmd {
            tool: "cloud",
            args: vec!["token".into(), "list".into()],
            cwd: None,
        },
        Cmd {
            tool: "cloud",
            args: vec!["token".into(), "list".into()],
            cwd: None,
        },
        Cmd {
            tool: "cloud",
            args: vec!["token".into(), "list".into()],
            cwd: None,
        },
        Cmd {
            tool: "cloud",
            args: vec!["token".into(), "new".into()],
            cwd: None,
        }
    ];


    let mut i = 0;
    while i < commands.len() {
        // Borrow the current command, then clone only the fields we need to own
        let (tool, args, cwd) = {
            let c = &commands[i];
            (c.tool, c.args.clone(), c.cwd.clone())
        };

        let output = utils.call_call_tool(tool, args.clone(), cwd.clone()).await?;
        // Detect the `cloud account new ...` command that just ran
        if tool == "cloud" && args.as_slice() == ["token", "new"] {
            if let Some(token_id) = extract_token_id(&output) {
                // Push the follow-up command using the extracted token ID
                commands.push(Cmd {
                    tool: "cloud",
                    args: vec!["token".into(), "delete".into(), token_id],
                    cwd: None,
                });
            } else {
                eprintln!("Failed to extract Token ID from output:\n{output}");
            }
        }

        i += 1;
    }

    client.shut_down().await?;

    Ok(())
}

fn strip_ansi(s: &str) -> String {
    // strips \x1b[...m sequences
    let ansi = Regex::new(r"\x1B\[[0-?]*[ -/]*[@-~]").unwrap();
    ansi.replace_all(s, "").to_string()
}

fn extract_installation_ids(output_json: &str) -> Vec<String> {
    let v: Value = match serde_json::from_str(output_json) {
        Ok(v) => v,
        Err(_) => {
            return vec![];
        }
    };

    let uuid_re = Regex::new(
        r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
    ).unwrap();

    let mut ids = Vec::new();
    if let Some(logs) = v.get("logs").and_then(|x| x.as_array()) {
        for log in logs {
            if let Some(line) = log.get("line").and_then(|x| x.as_str()) {
                let clean = strip_ansi(line);
                for m in uuid_re.find_iter(&clean) {
                    ids.push(m.as_str().to_string());
                }
            }
        }
    }
    ids
}
fn extract_api_id_and_version(output_json: &str) -> Option<(String, String)> {
    let v: Value = serde_json::from_str(output_json).ok()?;
    let logs = v.get("logs")?.as_array()?;

    // capture two "columns": ID and Version
    let re = Regex::new(r"^\s*\|\s*([^\|]+?)\s*\|\s*([^\|]+?)\s*\|").unwrap();

    for log in logs {
        let line = log
            .get("line")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        let clean = strip_ansi(line);
        let trimmed = clean.trim();

        if trimmed.contains("ID") && trimmed.contains("Version") {
            continue; // header
        }
        if trimmed.contains("---") {
            continue; // separator
        }

        if let Some(caps) = re.captures(trimmed) {
            let api_id = caps[1].trim().to_string();
            let version = caps[2].trim().to_string();
            if !api_id.is_empty() && !version.is_empty() {
                return Some((api_id, version));
            }
        }
    }

    None
}

fn extract_account_id(output: &str) -> Option<String> {
    // Example line:
    // "║ Account ID: 2ecb743f-0e70-4536-b80b-c0ec1a257175"
    // This matches the UUID after `Account ID:`
    let re = Regex::new(r"Account ID:\s*([0-9a-fA-F-]{36})").ok()?;
    let caps = re.captures(output)?;
    Some(caps[1].to_string())
}

fn extract_policy_id(output: &str) -> Option<String> {
    // Example line:
    // "║ Policy ID: 2ecb743f-0e70-4536-b80b-c0ec1a257175"
    // This matches the UUID after `Policy ID:`
    let re = Regex::new(r"Policy ID:\s*([0-9a-fA-F-]{36})").ok()?;
    let caps = re.captures(output)?;
    Some(caps[1].to_string())
}

fn extract_token_id(output: &str) -> Option<String> {
    // Example line:
    // "║ Token ID: 2ecb743f-0e70-4536-b80b-c0ec1a257175"
    // This matches the UUID after `Token ID:`
    let re = Regex::new(r"Token ID:\s*([0-9a-fA-F-]{36})").ok()?;
    let caps = re.captures(output)?;
    Some(caps[1].to_string())
}
