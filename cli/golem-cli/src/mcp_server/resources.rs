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

        // Add current directory manifest
        if let Ok(current_manifest) = self.get_current_manifest_resource().await {
            resources.push(current_manifest);
        }

        // Add ancestor directory manifests
        if let Ok(ancestor_manifests) = self.get_ancestor_manifest_resources().await {
            resources.extend(ancestor_manifests);
        }

        // Add child directory manifests
        if let Ok(child_manifests) = self.get_child_manifest_resources().await {
            resources.extend(child_manifests);
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

    async fn get_current_manifest_resource(&self) -> Result<Resource> {
        let current_dir = std::env::current_dir()
            .context("Failed to get current directory")?;
        
        let manifest_path = current_dir.join("golem.yaml");
        
        if manifest_path.exists() {
            Ok(Resource {
                uri: format!("file://{}", manifest_path.display()),
                name: Some("Current Directory Manifest".to_string()),
                description: Some("Golem manifest in the current directory".to_string()),
                mime_type: Some("text/yaml".to_string()),
            })
        } else {
            // Return a placeholder resource that indicates no manifest exists
            Ok(Resource {
                uri: "golem://current-manifest".to_string(),
                name: Some("Current Directory Manifest".to_string()),
                description: Some("No Golem manifest found in current directory".to_string()),
                mime_type: Some("text/plain".to_string()),
            })
        }
    }

    async fn get_ancestor_manifest_resources(&self) -> Result<Vec<Resource>> {
        let mut resources = Vec::new();
        let mut current_dir = std::env::current_dir()
            .context("Failed to get current directory")?;

        // Walk up the directory tree
        while let Some(parent) = current_dir.parent() {
            let manifest_path = parent.join("golem.yaml");
            
            if manifest_path.exists() {
                let relative_path = pathdiff::diff_paths(&manifest_path, &current_dir)
                    .unwrap_or_else(|| manifest_path.clone());
                
                resources.push(Resource {
                    uri: format!("file://{}", manifest_path.display()),
                    name: Some(format!("Ancestor Manifest: {}", relative_path.display())),
                    description: Some(format!("Golem manifest in ancestor directory: {}", parent.display())),
                    mime_type: Some("text/yaml".to_string()),
                });
            }
            
            current_dir = parent.to_path_buf();
        }

        Ok(resources)
    }

    async fn get_child_manifest_resources(&self) -> Result<Vec<Resource>> {
        let mut resources = Vec::new();
        let current_dir = std::env::current_dir()
            .context("Failed to get current directory")?;

        let mut entries = match fs::read_dir(&current_dir).await {
            Ok(entries) => entries,
            Err(e) => {
                error!("Failed to read current directory: {}", e);
                return Ok(resources);
            }
        };

        while let Some(entry) = entries.next_entry().await.transpose()? {
            let path = entry.path();
            
            if path.is_dir() {
                let manifest_path = path.join("golem.yaml");
                
                if manifest_path.exists() {
                    let relative_path = pathdiff::diff_paths(&manifest_path, &current_dir)
                        .unwrap_or_else(|| manifest_path.clone());
                    
                    resources.push(Resource {
                        uri: format!("file://{}", manifest_path.display()),
                        name: Some(format!("Child Manifest: {}", relative_path.display())),
                        description: Some(format!("Golem manifest in child directory: {}", path.display())),
                        mime_type: Some("text/yaml".to_string()),
                    });
                }
            }
        }

        Ok(resources)
    }

    fn extract_path_from_uri(&self, uri: &str) -> Option<std::path::PathBuf> {
        if uri.starts_with("file://") {
            Some(std::path::PathBuf::from(uri.strip_prefix("file://").unwrap()))
        } else if uri == "golem://current-manifest" {
            // Handle the special case for current directory manifest
            match std::env::current_dir() {
                Ok(dir) => Some(dir.join("golem.yaml")),
                Err(_) => None,
            }
        } else {
            None
        }
    }
}
