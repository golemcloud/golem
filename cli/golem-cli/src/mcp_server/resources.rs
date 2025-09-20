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

use crate::context::Context;
use crate::app::context::{find_main_source, collect_sources_and_switch_to_app_root};
use crate::model::app::DEFAULT_CONFIG_FILE_NAME;
use crate::validation::ValidatedResult;
use anyhow::{Context, Result};
use rmcp::{ReadResourceError, ReadResourceResult, Resource, RpcError};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::fs;
use tracing::{debug, error, info};

pub struct GolemResources {
    ctx: Arc<Context>,
}

impl GolemResources {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn list_resources(&self) -> Vec<Resource> {
        let mut resources = Vec::new();

        // Use existing app::context logic to discover manifests
        if let Ok(sources_result) = self.discover_manifests() {
            resources.extend(sources_result);
        }

        resources
    }

    pub async fn read_resource(&self, uri: &str) -> Result<ReadResourceResult, ReadResourceError> {
        debug!("Reading resource: {}", uri);

        if let Some(path) = self.extract_path_from_uri(uri) {
            match fs::read_to_string(&path).await {
                Ok(content) => {
                    let resource = rmcp::ResourceContents {
                        contents: vec![content.into()],
                        mime_type: Some("text/yaml".to_string()),
                    };
                    Ok(ReadResourceResult {
                        contents: vec![resource],
                        meta: None,
                    })
                }
                Err(e) => {
                    error!("Failed to read resource {}: {}", uri, e);
                    Err(ReadResourceError::internal_error(format!(
                        "Failed to read resource {}: {}",
                        uri, e
                    )))
                }
            }
        } else {
            Err(ReadResourceError::invalid_params(format!(
                "Invalid resource URI: {}",
                uri
            )))
        }
    }

    /// Discover manifests using the existing app::context logic
    fn discover_manifests(&self) -> Result<Vec<Resource>> {
        let mut resources = Vec::new();
        
        // Use the existing find_main_source function to discover the main manifest
        if let Some(main_source) = find_main_source() {
            resources.push(Resource {
                uri: format!("file://{}", main_source.display()),
                name: Some("Main Manifest".to_string()),
                description: Some("Main Golem manifest discovered from app context".to_string()),
                mime_type: Some("text/yaml".to_string()),
            });
            
            // Try to collect additional sources using the existing logic
            if let Ok(sources_result) = collect_sources_and_switch_to_app_root(Some(&main_source)) {
                match sources_result {
                    Ok((sources, _calling_working_dir)) => {
                        for source in sources {
                            if source != main_source { // Avoid duplicating the main source
                                resources.push(Resource {
                                    uri: format!("file://{}", source.display()),
                                    name: Some(format!("Included Manifest: {}", source.display())),
                                    description: Some("Included Golem manifest".to_string()),
                                    mime_type: Some("text/yaml".to_string()),
                                });
                            }
                        }
                    }
                    Err(_) => {
                        // Log validation errors but continue with what we have
                        debug!("Validation errors while collecting manifest sources");
                    }
                }
            }
        } else {
            // No main manifest found, create a placeholder
            resources.push(Resource {
                uri: "golem://no-manifest".to_string(),
                name: Some("No Manifest Found".to_string()),
                description: Some("No Golem manifest found in current directory or ancestors".to_string()),
                mime_type: Some("text/plain".to_string()),
            });
        }
        
        Ok(resources)
    }

    fn extract_path_from_uri(&self, uri: &str) -> Option<std::path::PathBuf> {
        if uri.starts_with("file://") {
            Some(std::path::PathBuf::from(uri.strip_prefix("file://").unwrap()))
        } else if uri == "golem://no-manifest" {
            // Handle the special case for no manifest found
            None
        } else if uri == "golem://current-manifest" {
            // Handle the special case for current directory manifest (legacy)
            match std::env::current_dir() {
                Ok(dir) => Some(dir.join(DEFAULT_CONFIG_FILE_NAME)),
                Err(_) => None,
            }
        } else {
            None
        }
    }
}
