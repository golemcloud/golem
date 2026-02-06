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

use crate::Tracing;
use anyhow::Context as AnyhowContext;
use assert2::{assert, check};
use golem_cli::mcp::GolemMcpServer;
use rmcp::model::{ListResourcesResult, ReadResourceRequestParams, ResourceContents};
use rmcp::transport::{
    StreamableHttpClientTransport, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::ServiceExt;
use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;
use test_r::{inherit_test_dep, test};
use tokio::sync::oneshot;
use url::Url;

inherit_test_dep!(Tracing);

struct TestServerHandle {
    addr: SocketAddr,
    shutdown_tx: oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl TestServerHandle {
    async fn shutdown(self) -> anyhow::Result<()> {
        let _ = self.shutdown_tx.send(());
        self.handle.await??;
        Ok(())
    }
}

async fn start_mcp_server() -> anyhow::Result<TestServerHandle> {
    let server = GolemMcpServer::from_env()?;
    let service: StreamableHttpService<GolemMcpServer> = StreamableHttpService::new(
        move || Ok(server.clone()),
        Default::default(),
        StreamableHttpServerConfig::default(),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("Failed to bind test server")?;
    let addr = listener.local_addr()?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .context("MCP test server failed")?;
        Ok(())
    });

    Ok(TestServerHandle {
        addr,
        shutdown_tx,
        handle,
    })
}

fn write_manifest(dir: &Path, name: &str, contents: &str) -> anyhow::Result<PathBuf> {
    let path = dir.join(name);
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, contents)?;
    Ok(path)
}

fn file_uri(path: &Path) -> String {
    Url::from_file_path(path)
        .expect("valid file path")
        .to_string()
}

fn assert_resource_contains(resources: &ListResourcesResult, uri: &str) {
    let found = resources
        .resources
        .iter()
        .any(|resource| resource.uri == uri);
    assert!(found, "resource list should include {uri}");
}

#[test]
async fn mcp_manifest_resources(_tracing: &Tracing) -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let workspace_dir = temp_dir.path().join("workspace");
    let child_dir = workspace_dir.join("child");

    let root_manifest = write_manifest(&workspace_dir, "golem.yaml", "root: true")?;
    let ancestor_manifest = write_manifest(temp_dir.path(), "golem.yaml", "ancestor: true")?;
    let child_manifest = write_manifest(&child_dir, "golem.yaml", "child: true")?;

    let original_dir = env::current_dir()?;
    env::set_current_dir(&workspace_dir)?;

    env::set_var("GOLEM_API_KEY", "test-token");
    env::set_var("GOLEM_API_URL", "https://api.golem.cloud");

    let server_handle = start_mcp_server().await?;
    let transport = StreamableHttpClientTransport::from_uri(format!(
        "http://{}/mcp",
        server_handle.addr
    ));
    let mut client = ().serve(transport).await?;

    let tools = client.list_tools(Default::default()).await?;
    assert!(!tools.tools.is_empty());

    let resources = client.list_resources(Default::default()).await?;
    assert!(!resources.resources.is_empty());

    assert_resource_contains(&resources, &file_uri(&root_manifest));
    assert_resource_contains(&resources, &file_uri(&ancestor_manifest));
    assert_resource_contains(&resources, &file_uri(&child_manifest));

    let read_result = client
        .read_resource(ReadResourceRequestParams {
            meta: None,
            uri: file_uri(&root_manifest),
        })
        .await?;
    check!(read_result.contents.len() == 1);
    let text = match &read_result.contents[0] {
        ResourceContents::TextResourceContents { text, .. } => text,
        _ => "",
    };
    check!(text == "root: true");

    // Close client to ensure the server can shut down cleanly.
    let _ = client.close_with_timeout(Duration::from_secs(5)).await?;
    drop(client);

    // Avoid hanging indefinitely if the server fails to terminate.
    tokio::time::timeout(Duration::from_secs(5), server_handle.shutdown())
        .await
        .context("timed out waiting for MCP test server shutdown")??;
    env::set_current_dir(original_dir)?;
    Ok(())
}
