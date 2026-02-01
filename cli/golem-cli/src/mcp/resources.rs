// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://github.com/golemcloud/golem/blob/main/LICENSE

use std::path::{Path, PathBuf};

use rust_mcp_sdk::schema::{ReadResourceResult, Resource, ResourceContents, RpcError};
use tokio::fs;
use walkdir::WalkDir;

const MANIFEST_FILENAME: &str = "golem.yaml";

pub struct GolemResources {
    base_path: PathBuf,
}

impl Default for GolemResources {
    fn default() -> Self {
        Self {
            base_path: std::env::current_dir().unwrap_or_default(),
        }
    }
}

impl GolemResources {
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    pub fn list_manifests(&self) -> Vec<Resource> {
        let mut resources = Vec::new();

        if let Some(manifest) = self.find_manifest_in_ancestors() {
            resources.push(self.create_resource(&manifest, "Application manifest (root)"));
        }

        for manifest in self.find_manifests_in_children() {
            let name = manifest
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(|n| format!("Component manifest ({})", n))
                .unwrap_or_else(|| "Component manifest".to_string());

            resources.push(self.create_resource(&manifest, &name));
        }

        resources
    }

    pub async fn read_resource(&self, uri: &str) -> Result<ReadResourceResult, RpcError> {
        let path = uri
            .strip_prefix("file://")
            .ok_or_else(|| RpcError::invalid_params("Invalid resource URI format"))?;

        let content = fs::read_to_string(path)
            .await
            .map_err(|e| RpcError::internal_error(format!("Failed to read manifest: {}", e)))?;

        Ok(ReadResourceResult {
            contents: vec![ResourceContents::text(uri.to_string(), content)],
            meta: None,
        })
    }

    fn find_manifest_in_ancestors(&self) -> Option<PathBuf> {
        let mut current = self.base_path.as_path();

        loop {
            let manifest_path = current.join(MANIFEST_FILENAME);
            if manifest_path.exists() {
                return Some(manifest_path);
            }

            match current.parent() {
                Some(parent) => current = parent,
                None => return None,
            }
        }
    }

    fn find_manifests_in_children(&self) -> Vec<PathBuf> {
        WalkDir::new(&self.base_path)
            .max_depth(3)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name() == MANIFEST_FILENAME)
            .filter(|entry| entry.path() != self.base_path.join(MANIFEST_FILENAME))
            .map(|entry| entry.path().to_path_buf())
            .collect()
    }

    fn create_resource(&self, path: &Path, description: &str) -> Resource {
        let uri = format!("file://{}", path.display());

        Resource {
            uri,
            name: path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(MANIFEST_FILENAME)
                .to_string(),
            description: Some(description.to_string()),
            mime_type: Some("application/yaml".to_string()),
            size: None,
            annotations: None,
        }
    }
}
