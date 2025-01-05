// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use assert2::assert;
use test_r::{test_gen, add_test, inherit_test_dep, test_dep};
use test_r::core::{DynamicTestRegistration, TestType};
use golem_test_framework::config::EnvBasedTestDependencies;
use crate::cli::{Cli, CliLive};
use crate::Tracing;
use std::time::Duration;
use reqwest::blocking::Client;
use std::thread;
use std::process::Command;

inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[test_dep]
fn cli(deps: &EnvBasedTestDependencies) -> CliLive {
    CliLive::make("api_definition_export_ui", Arc::new(deps.clone())).unwrap()
}

#[test_gen]
fn generated(r: &mut DynamicTestRegistration) {
    add_test!(
        r,
        "api_definition_export_yaml",
        TestType::UnitTest,
        move |deps: &EnvBasedTestDependencies, _tracing: &Tracing| {
            test_export_yaml((deps, &cli(deps)))
        }
    );

    add_test!(
        r,
        "api_definition_export_json",
        TestType::UnitTest,
        move |deps: &EnvBasedTestDependencies, _tracing: &Tracing| {
            test_export_json((deps, &cli(deps)))
        }
    );

    add_test!(
        r,
        "api_definition_ui",
        TestType::UnitTest,
        move |deps: &EnvBasedTestDependencies, _tracing: &Tracing| {
            test_ui((deps, &cli(deps)))
        }
    );
}

fn test_export_yaml((deps, cli): (&EnvBasedTestDependencies, &CliLive)) -> anyhow::Result<()> {
    // Create a test component and API definition
    let component_name = "test_export_yaml";
    let component = crate::api_definition::make_shopping_cart_component(deps, component_name, cli)?;
    let component_id = component.component_urn.id.0.to_string();
    
    // Export the API definition to YAML
    cli.run_unit(&[
        "api-definition",
        "export",
        "--id",
        &component_id,
        "--version",
        "0.1.0",
        "--format",
        "yaml"
    ])?;
    
    // Verify the exported file
    let path = PathBuf::from(format!("api_definition_{}_{}.yaml", component_id, "0.1.0"));
    assert!(path.exists());
    let content = fs::read_to_string(&path)?;
    assert!(content.contains("openapi:"));
    
    // Clean up
    fs::remove_file(path)?;
    
    Ok(())
}

fn test_export_json((deps, cli): (&EnvBasedTestDependencies, &CliLive)) -> anyhow::Result<()> {
    // Create a test component and API definition
    let component_name = "test_export_json";
    let component = crate::api_definition::make_shopping_cart_component(deps, component_name, cli)?;
    let component_id = component.component_urn.id.0.to_string();
    
    // Export the API definition to JSON
    cli.run_unit(&[
        "api-definition",
        "export",
        "--id",
        &component_id,
        "--version",
        "0.1.0",
        "--format",
        "json"
    ])?;
    
    // Verify the exported file
    let path = PathBuf::from(format!("api_definition_{}_{}.json", component_id, "0.1.0"));
    assert!(path.exists());
    let content = fs::read_to_string(&path)?;
    assert!(content.contains("\"openapi\""));
    
    // Clean up
    fs::remove_file(path)?;
    
    Ok(())
}

fn test_ui((deps, cli): (&EnvBasedTestDependencies, &CliLive)) -> anyhow::Result<()> {
    // Create a test component and API definition
    let component_name = "test_ui";
    let component = crate::api_definition::make_shopping_cart_component(deps, component_name, cli)?;
    let component_id = component.component_urn.id.0.to_string();
    
    // Start the UI server (in background)
    cli.run_unit(&[
        "api-definition",
        "ui",
        "--id",
        &component_id,
        "--version",
        "0.1.0",
        "--port",
        "9000"
    ])?;
    
    // Give the server a moment to start
    thread::sleep(Duration::from_secs(2));
    
    // Create an HTTP client with a timeout
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    
    // Try to access the Swagger UI
    let response = client.get("http://localhost:9000")
        .send()?;
        
    // Verify we got a successful response
    assert!(response.status().is_success(), "Failed to access Swagger UI");
    
    // Verify the response contains expected Swagger UI content
    let body = response.text()?;
    assert!(body.contains("swagger-ui"), "Response doesn't contain Swagger UI");
    
    // Cleanup: Find and kill the server process
    if cfg!(windows) {
        Command::new("taskkill")
            .args(["/F", "/IM", "golem-cli.exe"])
            .output()?;
    } else {
        Command::new("pkill")
            .arg("golem-cli")
            .output()?;
    }
    
    Ok(())
} 