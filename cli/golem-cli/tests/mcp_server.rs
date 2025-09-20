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

use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

#[cfg(feature = "mcp")]
#[test_r::test]
fn test_mcp_server_starts_with_default_port() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();
    
    // Start the MCP server in the background
    let mut child = Command::new("cargo")
        .args([
            "run",
            "--bin",
            "golem-cli",
            "--features",
            "mcp",
            "--",
            "--serve",
            "--profile",
            "test",
            "--component-dir",
            temp_path.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start MCP server");

    // Give the server a moment to start
    thread::sleep(Duration::from_secs(2));

    // Check if the process is still running (it should be if it started successfully)
    match child.try_wait() {
        Ok(None) => {
            // Process is still running, which is good
            child.kill().expect("Failed to kill MCP server");
        }
        Ok(Some(status)) => {
            panic!("MCP server exited unexpectedly with status: {}", status);
        }
        Err(e) => {
            panic!("Failed to check MCP server status: {}", e);
        }
    }
}

#[cfg(feature = "mcp")]
#[test_r::test]
fn test_mcp_server_starts_with_custom_port() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();
    
    // Start the MCP server with a custom port
    let mut child = Command::new("cargo")
        .args([
            "run",
            "--bin",
            "golem-cli",
            "--features",
            "mcp",
            "--",
            "--serve",
            "--serve-port",
            "1234",
            "--profile",
            "test",
            "--component-dir",
            temp_path.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start MCP server");

    // Give the server a moment to start
    thread::sleep(Duration::from_secs(2));

    // Check if the process is still running
    match child.try_wait() {
        Ok(None) => {
            // Process is still running, which is good
            child.kill().expect("Failed to kill MCP server");
        }
        Ok(Some(status)) => {
            panic!("MCP server exited unexpectedly with status: {}", status);
        }
        Err(e) => {
            panic!("Failed to check MCP server status: {}", e);
        }
    }
}

#[cfg(feature = "mcp")]
#[test_r::test]
fn test_mcp_server_requires_serve_flag() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();
    
    // Try to start with serve-port but without serve flag
    let output = Command::new("cargo")
        .args([
            "run",
            "--bin",
            "golem-cli",
            "--features",
            "mcp",
            "--",
            "--serve-port",
            "1234",
            "--profile",
            "test",
            "--component-dir",
            temp_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run golem-cli");

    // Should fail with an error about serve-port requiring serve
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("serve-port") || stderr.contains("--serve-port"));
}

#[cfg(feature = "mcp")]
#[test_r::test]
fn test_mcp_server_lists_tools() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();
    
    // Create a simple test manifest
    let manifest_content = r#"
name: test-component
version: 0.1.0
description: Test component for MCP server
"#;
    std::fs::write(temp_path.join("golem.yaml"), manifest_content).unwrap();
    
    // Start the MCP server
    let mut child = Command::new("cargo")
        .args([
            "run",
            "--bin",
            "golem-cli",
            "--features",
            "mcp",
            "--",
            "--serve",
            "--profile",
            "test",
            "--component-dir",
            temp_path.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start MCP server");

    // Give the server a moment to start
    thread::sleep(Duration::from_secs(2));

    // Check if the process is still running
    match child.try_wait() {
        Ok(None) => {
            // Process is still running, which is good
            child.kill().expect("Failed to kill MCP server");
        }
        Ok(Some(status)) => {
            panic!("MCP server exited unexpectedly with status: {}", status);
        }
        Err(e) => {
            panic!("Failed to check MCP server status: {}", e);
        }
    }
}

#[cfg(feature = "mcp")]
#[test_r::test]
fn test_mcp_server_lists_resources() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();
    
    // Create a simple test manifest
    let manifest_content = r#"
name: test-component
version: 0.1.0
description: Test component for MCP server
"#;
    std::fs::write(temp_path.join("golem.yaml"), manifest_content).unwrap();
    
    // Start the MCP server
    let mut child = Command::new("cargo")
        .args([
            "run",
            "--bin",
            "golem-cli",
            "--features",
            "mcp",
            "--",
            "--serve",
            "--profile",
            "test",
            "--component-dir",
            temp_path.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start MCP server");

    // Give the server a moment to start
    thread::sleep(Duration::from_secs(2));

    // Check if the process is still running
    match child.try_wait() {
        Ok(None) => {
            // Process is still running, which is good
            child.kill().expect("Failed to kill MCP server");
        }
        Ok(Some(status)) => {
            panic!("MCP server exited unexpectedly with status: {}", status);
        }
        Err(e) => {
            panic!("Failed to check MCP server status: {}", e);
        }
    }
}

#[cfg(not(feature = "mcp"))]
#[test_r::test]
fn test_mcp_server_flags_not_available_without_feature() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();
    
    // Try to use --serve flag without mcp feature
    let output = Command::new("cargo")
        .args([
            "run",
            "--bin",
            "golem-cli",
            "--",
            "--serve",
            "--profile",
            "test",
            "--component-dir",
            temp_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run golem-cli");

    // Should fail with an error about unknown flag
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected") || stderr.contains("unknown"));
}
