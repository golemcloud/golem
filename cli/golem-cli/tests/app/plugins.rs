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

use crate::app::{cmd, flag, TestContext};
use crate::Tracing;
use assert2::{assert, check};
use axum::extract::{DefaultBodyLimit, Multipart};
use axum::routing::post;
use axum::Router;
use base64::Engine;
use bytes::Bytes;
use golem_cli::fs;
use indoc::{formatdoc, indoc};
use serde_json::json;
use std::path::Path;
use test_r::{inherit_test_dep, test};
use tokio::spawn;
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

inherit_test_dep!(Tracing);

#[test]
async fn plugin_installation_test1(_tracing: &Tracing) {
    let mut ctx = TestContext::new();
    let app_name = "test-app-name";

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]);
    assert!(outputs.success());

    ctx.cd(app_name);

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "rust", "test:rust1"]);
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::COMPONENT, cmd::NEW, "rust", "test:rust2"]);
    assert!(outputs.success());

    fs::write_str(
        ctx.cwd_path_join(
            Path::new("components-rust")
                .join("test-rust1")
                .join("golem.yaml"),
        ),
        indoc! {"
            components:
              test:rust1:
                template: rust
                profiles:
                  debug:
                    plugins: []
        "},
    )
    .unwrap();

    ctx.start_server();
    let plugin_transformer = TestPlugin::new().await;

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY]);
    assert!(outputs.success());

    let plugin_manifest_path = "plugin.yaml";
    fs::write_str(
        ctx.cwd_path_join(Path::new("icon.svg")),
        indoc! {r#"<?xml version="1.0" encoding="UTF-8"?><svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#},
    ).unwrap();
    fs::write_str(
        ctx.cwd_path_join(Path::new(plugin_manifest_path)),
        formatdoc!(
            "
            name: component-transformer-1
            version: v1
            description: Test plugin
            type: transform
            icon: icon.svg
            homepage: none
            specs:
                type: ComponentTransformer
                validateUrl: http://localhost:{}/validate
                transformUrl: http://localhost:{}/transform
            ",
            plugin_transformer.port,
            plugin_transformer.port
        ),
    )
    .unwrap();

    let outputs = ctx.cli([cmd::PLUGIN, cmd::REGISTER, plugin_manifest_path]);
    assert!(outputs.success());

    let plugin_manifest_path2 = "plugin2.yaml";
    fs::write_str(
        ctx.cwd_path_join(Path::new(plugin_manifest_path2)),
        formatdoc!(
            "
            name: component-transformer-2
            version: 0.0.1
            description: Test plugin
            type: transform
            icon: icon.svg
            homepage: none
            specs:
                type: ComponentTransformer
                validateUrl: http://localhost:{}/validate
                transformUrl: http://localhost:{}/transform
            ",
            plugin_transformer.port,
            plugin_transformer.port
        ),
    )
    .unwrap();

    let outputs = ctx.cli([cmd::PLUGIN, cmd::REGISTER, plugin_manifest_path2]);
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::COMPONENT, cmd::PLUGIN, cmd::GET]);
    assert!(outputs.success());
    check!(outputs.stdout.len() == 5);

    fs::write_str(
        ctx.cwd_path_join(
            Path::new("components-rust")
                .join("test-rust1")
                .join("golem.yaml"),
        ),
        indoc! {"
            components:
              test:rust1:
                template: rust
                profiles:
                  debug:
                    plugins:
                        - name: component-transformer-1
                          version: v1
                          parameters:
                            x: 1
                            y: 2
        "},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY, flag::YES]);
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::COMPONENT, cmd::PLUGIN, cmd::GET]);
    assert!(outputs.success());
    check!(outputs.stdout.len() == 7);
    check!(outputs.stdout_contains("component-transformer-1"));
    check!(outputs.stdout_contains("v1"));
    check!(outputs.stdout_contains("x: 1"));
    check!(outputs.stdout_contains("y: 2"));

    fs::write_str(
        ctx.cwd_path_join(
            Path::new("components-rust")
                .join("test-rust1")
                .join("golem.yaml"),
        ),
        indoc! {"
            components:
              test:rust1:
                template: rust
                profiles:
                  debug:
                    plugins:
                        - name: component-transformer-1
                          version: v1
                          parameters:
                            z: 3
                        - name: component-transformer-2
                          version: 0.0.1
                          parameters: {}
                        - name: component-transformer-1
                          version: v1
                          parameters:
                            x: 1
                            y: 2
        "},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY, flag::YES]);
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::COMPONENT, cmd::PLUGIN, cmd::GET]);
    assert!(outputs.success());
    check!(outputs.stdout_contains_row_with_cells(&["component-transformer-1", "v1", "0", "z: 3"]));
    check!(outputs.stdout_contains_row_with_cells(&["component-transformer-2", "0.0.1", "1"]));
    check!(
        outputs.stdout_contains_row_with_cells(&[
            "component-transformer-1",
            "v1",
            "2",
            "x: 1, y: 2"
        ]) || outputs.stdout_contains_row_with_cells(&[
            "component-transformer-1",
            "v1",
            "2",
            "y: 2, x: 1"
        ])
    );

    fs::write_str(
        ctx.cwd_path_join(
            Path::new("components-rust")
                .join("test-rust1")
                .join("golem.yaml"),
        ),
        indoc! {"
            components:
              test:rust1:
                template: rust
                profiles:
                  debug:
                    plugins:
                        - name: component-transformer-2
                          version: 0.0.1
                          parameters: {}
        "},
    )
    .unwrap();

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY, flag::YES]);
    assert!(outputs.success());

    let outputs = ctx.cli([cmd::COMPONENT, cmd::PLUGIN, cmd::GET]);
    assert!(outputs.success());
    check!(outputs.stdout.len() == 7);
    check!(outputs.stdout_contains("component-transformer-2"));
    check!(outputs.stdout_contains("0.0.1"));
}

struct TestPlugin {
    pub port: u16,
    handle: JoinHandle<()>,
}

impl Drop for TestPlugin {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl TestPlugin {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = spawn(async move {
            let route = Router::new()
                .route("/transform", post(Self::transform))
                .layer(DefaultBodyLimit::max(500 * 1024 * 1024));
            axum::serve(listener, route).await.unwrap();
        });
        Self { port, handle }
    }

    async fn transform(mut multipart: Multipart) -> axum::Json<serde_json::Value> {
        let mut component = None;

        while let Some(field) = multipart.next_field().await.unwrap() {
            let name = field.name().unwrap().to_string();
            let data = field.bytes().await;

            if let Ok(data) = &data {
                debug!("Length of `{}` is {} bytes", name, data.len());
            } else {
                error!("Failed to read field `{}`: {:?}", name, data)
            }

            match name.as_str() {
                "component" => {
                    let data = data.unwrap();
                    component = Some(data);
                }
                "metadata" => {
                    let data = data.unwrap();
                    let json =
                        std::str::from_utf8(&data).expect("Failed to parse metadata as UTF-8");
                    info!("Metadata: {}", json);
                }
                _ => {
                    let data = data.unwrap();
                    let value = std::str::from_utf8(&data).expect("Failed to parse field as UTF-8");
                    info!("Configuration field: {} = {}", name, value);
                }
            }
        }

        let transformed_bytes =
            Self::transform_component(component.expect("did not receive a component part"))
                .expect("Failed to transform component");

        let data_base64 = base64::engine::general_purpose::STANDARD.encode(&transformed_bytes);

        let response = json!({
            "data": data_base64
        });

        axum::Json(response)
    }

    fn transform_component(component: Bytes) -> anyhow::Result<Vec<u8>> {
        Ok(component.to_vec())
    }
}
