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

use rust_mcp_schema::{
    ReadResourceContent, ReadResourceResult, Resource, RpcError, TextResourceContents,
};
use std::path::{Path, PathBuf};
use tracing::debug;
use walkdir::WalkDir;

const MANIFEST_FILENAME: &str = "golem.yaml";

/// Discover all golem.yaml manifests in current, ancestor, and child directories.
pub fn discover_manifests(working_dir: &Path) -> Vec<Resource> {
    let mut manifests = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // 1. Check current directory
    check_and_add(working_dir, &mut manifests, &mut seen);

    // 2. Walk ancestor directories
    let mut ancestor = working_dir.parent();
    while let Some(dir) = ancestor {
        check_and_add(dir, &mut manifests, &mut seen);
        ancestor = dir.parent();
    }

    // 3. Walk child directories (max depth 5 to avoid traversing the world)
    for entry in WalkDir::new(working_dir)
        .max_depth(5)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() && entry.file_name() == MANIFEST_FILENAME {
            let dir = entry.path().parent().unwrap_or(working_dir);
            check_and_add(dir, &mut manifests, &mut seen);
        }
    }

    debug!("Discovered {} golem.yaml manifest(s)", manifests.len());

    manifests
}

fn check_and_add(
    dir: &Path,
    manifests: &mut Vec<Resource>,
    seen: &mut std::collections::HashSet<PathBuf>,
) {
    let manifest_path = dir.join(MANIFEST_FILENAME);
    if manifest_path.exists() {
        let canonical = manifest_path
            .canonicalize()
            .unwrap_or_else(|_| manifest_path.clone());
        if seen.insert(canonical.clone()) {
            let uri = format!("file://{}", canonical.display());
            let name = format!(
                "golem.yaml ({})",
                dir.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| dir.display().to_string())
            );
            manifests.push(Resource {
                uri,
                name,
                description: Some(format!(
                    "Golem application manifest at {}",
                    canonical.display()
                )),
                mime_type: Some("text/yaml".to_string()),
                annotations: None,
                icons: vec![],
                meta: None,
                size: None,
                title: None,
            });
        }
    }
}

/// Read the content of a resource by its URI.
pub fn read_resource(uri: &str) -> Result<ReadResourceResult, RpcError> {
    let path = uri.strip_prefix("file://").ok_or_else(|| {
        RpcError::invalid_request().with_message(format!("Invalid resource URI: {uri}"))
    })?;

    let path = Path::new(path);
    if !path.exists() {
        return Err(RpcError::invalid_request().with_message(format!("Resource not found: {uri}")));
    }

    let content = std::fs::read_to_string(path).map_err(|e| {
        RpcError::internal_error().with_message(format!("Failed to read resource: {e}"))
    })?;

    Ok(ReadResourceResult {
        contents: vec![ReadResourceContent::TextResourceContents(
            TextResourceContents {
                uri: uri.to_string(),
                mime_type: Some("text/yaml".to_string()),
                text: content,
                meta: None,
            },
        )],
        meta: None,
    })
}
