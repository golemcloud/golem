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

use crate::Tracing;
use crate::app::{TestContext, builtin_artifact_sources};
use axum::Router;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use test_r::{inherit_test_dep, test, timeout};
use tokio::sync::mpsc;

inherit_test_dep!(Tracing);

const HELD_PLUGIN: BuiltinArtifact = BuiltinArtifact {
    id: "otlp_exporter",
    path: "plugins/otlp-exporter.wasm",
};
const HELD_TOOL: BuiltinArtifact = BuiltinArtifact {
    id: "web_fetch",
    path: "builtin-tools/web-fetch.wasm",
};

struct BuiltinArtifact {
    id: &'static str,
    path: &'static str,
}

impl BuiltinArtifact {
    fn source(&self) -> (PathBuf, String) {
        builtin_artifact_sources()
            .iter()
            .find(|(source, _)| source.ends_with(self.path))
            .cloned()
            .unwrap_or_else(|| panic!("no built-in artifact source for '{}'", self.id))
    }
}

/// An HTTP server that never answers. Each request that reaches it is reported on `requests`.
struct HoldingArtifactServer {
    address: SocketAddr,
    requests: mpsc::UnboundedReceiver<String>,
    task: tokio::task::JoinHandle<()>,
}

impl HoldingArtifactServer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, requests) = mpsc::unbounded_channel();
        let routes = Router::new().fallback(move |uri: axum::http::Uri| {
            let sender = sender.clone();
            async move {
                let _ = sender.send(uri.path().to_string());
                std::future::pending::<()>().await;
            }
        });
        let task = tokio::spawn(async move {
            axum::serve(listener, routes).await.unwrap();
        });
        Self {
            address,
            requests,
            task,
        }
    }
}

impl Drop for HoldingArtifactServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Fills a cache directory with every built-in artifact except the held one, and points the
/// registry service at that directory and at the server for the held artifact.
fn configure_held_artifact(
    ctx: &mut TestContext,
    held: &BuiltinArtifact,
    server: &HoldingArtifactServer,
) -> PathBuf {
    let cache_dir = ctx.working_dir.join("held-artifact-cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let (held_source, held_sha256) = held.source();
    for (source, sha256) in builtin_artifact_sources() {
        if source != &held_source {
            std::fs::copy(source, cache_dir.join(format!("{sha256}.wasm"))).unwrap();
        }
    }

    let config_file = ctx.working_dir.join("golem-server-held-artifact.toml");
    std::fs::write(
        &config_file,
        format!(
            r#"
                [registry_service.builtin_artifacts]
                cache_dir = "{}"

                [registry_service.builtin_artifacts.source_overrides.{}]
                url = "http://{}/{}.wasm"
                sha256 = "{held_sha256}"
            "#,
            cache_dir.display(),
            held.id,
            server.address,
            held.id,
        ),
    )
    .unwrap();
    ctx.add_server_args(["--config", config_file.to_str().unwrap()]);
    cache_dir
}

fn make_available(held: &BuiltinArtifact, cache_dir: &Path) {
    let (source, sha256) = held.source();
    std::fs::copy(source, cache_dir.join(format!("{sha256}.wasm"))).unwrap();
}

async fn registry_db_count(registry_db: &Path, query: &str) -> i64 {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(registry_db)
                .read_only(true),
        )
        .await
        .unwrap();
    let count = sqlx::query_scalar::<_, i64>(query)
        .fetch_one(&pool)
        .await
        .unwrap();
    pool.close().await;
    count
}

impl TestContext {
    async fn wait_for_request(&mut self, server: &mut HoldingArtifactServer) -> String {
        let start = Instant::now();
        loop {
            let server_process = self
                .server_process
                .as_mut()
                .expect("server process should be running while waiting for the request");
            if let Some(exit_status) = server_process
                .try_wait()
                .expect("failed to check golem server process status")
            {
                panic!("Golem server exited before it asked for the held artifact: {exit_status}");
            }
            assert!(
                start.elapsed() < Duration::from_secs(180),
                "timed out waiting for the request for the held artifact"
            );
            if let Ok(Some(path)) =
                tokio::time::timeout(Duration::from_millis(100), server.requests.recv()).await
            {
                return path;
            }
        }
    }

    async fn interrupt_server(&mut self) {
        assert!(
            !self.ports_file.exists(),
            "the server finished its startup while it waited for the held artifact"
        );

        let server_process = self
            .server_process
            .as_mut()
            .expect("server process should be running");
        let pid = server_process
            .id()
            .expect("golem server process has no process id");
        let status = tokio::process::Command::new("kill")
            .args(["-INT", &pid.to_string()])
            .status()
            .await
            .expect("failed to run kill");
        assert!(status.success(), "failed to send SIGINT to the server");

        let exit_status = tokio::time::timeout(Duration::from_secs(60), server_process.wait())
            .await
            .expect("timed out waiting for the interrupted server to exit")
            .expect("failed to wait for the interrupted server");
        self.server_process = None;
        println!("> interrupted server exited: {exit_status}");
        assert!(
            !self.ports_file.exists(),
            "the server finished its startup while it was interrupted"
        );
    }
}

#[test]
#[timeout("10m")]
async fn server_restarts_in_data_dir_after_start_interrupted_before_any_built_in_is_stored(
    _tracing: &Tracing,
) {
    let mut ctx = TestContext::new();
    let mut server = HoldingArtifactServer::start().await;
    let cache_dir = configure_held_artifact(&mut ctx, &HELD_PLUGIN, &server);

    ctx.spawn_server();
    let requested = ctx.wait_for_request(&mut server).await;
    assert_eq!(requested, format!("/{}.wasm", HELD_PLUGIN.id));
    ctx.interrupt_server().await;

    make_available(&HELD_PLUGIN, &cache_dir);
    ctx.start_server().await;

    let registry_db = ctx.data_dir.path().join("registry.db");
    assert_eq!(
        registry_db_count(
            &registry_db,
            "SELECT COUNT(*) FROM plugins WHERE name = 'golem-otlp-exporter' AND deleted_at IS NULL"
        )
        .await,
        1
    );
}

#[test]
#[timeout("10m")]
async fn server_restarts_in_data_dir_after_start_interrupted_while_built_ins_are_stored(
    _tracing: &Tracing,
) {
    let mut ctx = TestContext::new();
    let mut server = HoldingArtifactServer::start().await;
    let cache_dir = configure_held_artifact(&mut ctx, &HELD_TOOL, &server);
    let registry_db = ctx.data_dir.path().join("registry.db");

    ctx.spawn_server();
    let requested = ctx.wait_for_request(&mut server).await;
    assert_eq!(requested, format!("/{}.wasm", HELD_TOOL.id));
    assert_eq!(
        registry_db_count(
            &registry_db,
            "SELECT COUNT(*) FROM plugins WHERE name = 'golem-otlp-exporter' AND deleted_at IS NULL"
        )
        .await,
        1,
        "the built-in plugin is stored when the tool artifact is requested"
    );
    assert_eq!(
        registry_db_count(
            &registry_db,
            "SELECT COUNT(*) FROM tool_releases WHERE component_id IS NOT NULL"
        )
        .await,
        0,
        "no component-backed tool is stored when the tool artifact is requested"
    );
    ctx.interrupt_server().await;

    make_available(&HELD_TOOL, &cache_dir);
    ctx.start_server().await;

    assert_eq!(
        registry_db_count(
            &registry_db,
            "SELECT COUNT(*) FROM plugins WHERE name = 'golem-otlp-exporter' AND deleted_at IS NULL"
        )
        .await,
        1
    );
    assert_eq!(
        registry_db_count(
            &registry_db,
            "SELECT COUNT(DISTINCT tool_name) FROM tool_releases WHERE component_id IS NOT NULL AND tool_name IN ('bash', 'read-file', 'git', 'web-fetch', 'tsc')"
        )
        .await,
        5
    );
}
