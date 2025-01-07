// Copyright 2024-2025 Golem Cloud
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

use std::fmt::{Display, Formatter};
use std::sync::Arc;

use golem_common::tracing::{init_tracing_with_default_debug_env_filter, TracingConfig};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
};
use strum_macros::EnumIter;
use test_r::test_dep;
use tracing::info;
use crate::cli::CliLive;

pub mod cli;

mod api_definition;
mod api_deployment;
mod api_deployment_fileserver;
mod component;
mod get;
mod profile;
mod text;
mod worker;

#[test_dep]
fn cli(deps: &EnvBasedTestDependencies) -> CliLive {
    CliLive::make("api_definition_export", Arc::new(deps.clone())).unwrap()
}

#[cfg(test)]
mod api_definition_export_test {
    use anyhow::Result;
    use golem_cli::model::{ApiDefinitionId, ApiDefinitionVersion};
    use crate::cli::{Cli, CliLive};
    use std::fs;
    use tempfile::TempDir;
    use golem_test_framework::config::EnvBasedTestDependencies;
    use crate::Tracing;
    use test_r::{inherit_test_dep, test_gen, add_test};
    use test_r::core::{DynamicTestRegistration, TestType};

    inherit_test_dep!(EnvBasedTestDependencies);
    inherit_test_dep!(Tracing);
    inherit_test_dep!(CliLive);

    pub fn test_api_definition_export_and_swagger(
        _deps: &EnvBasedTestDependencies,
        cli: &CliLive,
        _tracing: &Tracing,
    ) -> Result<()> {
        // Create a temporary directory for test files
        let temp_dir = TempDir::new()?;
        let temp_path = temp_dir.path();

        // Test parameters
        let _api_id = ApiDefinitionId("test-api".to_string());
        let _version = ApiDefinitionVersion("1.0.0".to_string());

        // First, let's create a simple API definition to work with
        let api_def = r#"{
            "id": "test-api",
            "version": "1.0.0",
            "routes": [
                {
                    "path": "/test",
                    "method": "GET",
                    "binding": {
                        "response_mapping_output": {
                            "types": {
                                "response": {
                                    "type": "record",
                                    "fields": [
                                        {
                                            "name": "message",
                                            "type": "string"
                                        }
                                    ]
                                }
                            }
                        }
                    }
                }
            ]
        }"#;

        // Write the API definition to a temporary file
        let api_def_path = temp_path.join("test-api.json");
        fs::write(&api_def_path, api_def)?;

        // Import the API definition
        cli.run::<(), _>(&["api-definition", "import", api_def_path.to_str().unwrap()])?;

        // Test the export command with JSON format
        let json_result = cli.run_string(&[
            "api-definition",
            "export",
            "--id",
            "test-api",
            "--version",
            "1.0.0",
            "--format",
            "json",
        ])?;

        // Verify JSON export contains expected OpenAPI elements
        assert!(json_result.contains("openapi"));
        assert!(json_result.contains("/test"));
        assert!(json_result.contains("GET"));

        // Test the export command with YAML format
        let yaml_result = cli.run_string(&[
            "api-definition",
            "export",
            "--id",
            "test-api",
            "--version",
            "1.0.0",
            "--format",
            "yaml",
        ])?;

        // Verify YAML export contains expected OpenAPI elements
        assert!(yaml_result.contains("openapi:"));
        assert!(yaml_result.contains("/test:"));
        assert!(yaml_result.contains("get:"));

        // Test the swagger command
        // Note: We can't actually open a browser in tests, but we can verify the URL is correct
        let swagger_result = cli.run_string(&[
            "api-definition",
            "swagger",
            "--id",
            "test-api",
            "--version",
            "1.0.0",
        ])?;

        // Verify the swagger result contains a valid URL
        assert!(swagger_result.contains("/swagger-ui/api-definitions/test-api/1.0.0"));

        Ok(())
    }

    #[test_gen]
    fn generated(r: &mut DynamicTestRegistration) {
        make(r, "_short", "CLI_short", true);
        make(r, "_long", "CLI_long", false);
    }

    fn make(r: &mut DynamicTestRegistration, suffix: &'static str, _name: &'static str, short: bool) {
        add_test!(
            r,
            format!("api_definition_export_and_swagger{suffix}"),
            TestType::IntegrationTest,
            move |deps: &EnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
                test_api_definition_export_and_swagger(deps, &cli.with_args(short), _tracing)
            }
        );
    }
}

#[derive(Debug, Copy, Clone, EnumIter)]
pub enum RefKind {
    Name,
    Url,
    Urn,
}

impl Display for RefKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RefKind::Name => write!(f, "name"),
            RefKind::Url => write!(f, "url"),
            RefKind::Urn => write!(f, "urn"),
        }
    }
}

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        init_tracing_with_default_debug_env_filter(&TracingConfig::test("cli-tests"));
        Self
    }
}

#[test_dep]
pub fn tracing() -> Tracing {
    Tracing::init()
}

#[test_dep]
async fn test_dependencies(_tracing: &Tracing) -> EnvBasedTestDependencies {
    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 3,
        keep_docker_containers: false,
        ..EnvBasedTestDependenciesConfig::new()
    })
    .await;

    let cluster = deps.worker_executor_cluster(); // forcing startup by getting it
    info!("Using cluster with {:?} worker executors", cluster.size());

    deps
}

test_r::enable!();
