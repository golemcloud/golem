// MCP Resources - Manifest file discovery and exposure
// Discovers golem.yaml manifest files and exposes them as MCP resources

use rmcp::model::*;
use std::path::{Path, PathBuf};
use tokio::fs;

const MANIFEST_FILENAME: &str = "golem.yaml";

/// Discover all accessible golem.yaml manifest files
/// Searches: current directory, parent directories, and immediate children
pub async fn discover_manifests(base_dir: &Path) -> anyhow::Result<Vec<Resource>> {
    let mut resources = Vec::new();

    // Search current directory
    if let Some(resource) = check_manifest_in_dir(base_dir).await? {
        resources.push(resource);
    }

    // Search parent directories (up to project root)
    if let Some(resource) = search_parent_manifests(base_dir).await? {
        resources.push(resource);
    }

    // Search immediate child directories
    let child_resources = search_child_manifests(base_dir).await?;
    resources.extend(child_resources);

    Ok(resources)
}

/// Check if manifest exists in a specific directory
async fn check_manifest_in_dir(dir: &Path) -> anyhow::Result<Option<Resource>> {
    let manifest_path = dir.join(MANIFEST_FILENAME);

    if manifest_path.exists() && manifest_path.is_file() {
        Ok(Some(create_resource_from_path(&manifest_path)?))
    } else {
        Ok(None)
    }
}

/// Search parent directories for manifests
async fn search_parent_manifests(start_dir: &Path) -> anyhow::Result<Option<Resource>> {
    let mut current = start_dir.to_path_buf();

    // Walk up parent directories (limit to 5 levels to avoid infinite loops)
    for _ in 0..5 {
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
            if let Some(resource) = check_manifest_in_dir(&current).await? {
                return Ok(Some(resource));
            }
        } else {
            break;
        }
    }

    Ok(None)
}

/// Search immediate child directories for manifests
async fn search_child_manifests(parent_dir: &Path) -> anyhow::Result<Vec<Resource>> {
    let mut resources = Vec::new();

    let mut entries = fs::read_dir(parent_dir).await?;

    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_dir() {
            let child_dir = entry.path();
            if let Some(resource) = check_manifest_in_dir(&child_dir).await? {
                resources.push(resource);
            }
        }
    }

    Ok(resources)
}

/// Create MCP Resource from manifest file path
fn create_resource_from_path(path: &Path) -> anyhow::Result<Resource> {
    let uri = format!("file://{}", path.canonicalize()?.display());
    let name = path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("golem.yaml")
        .to_string();

    let raw_resource = RawResource {
        uri,
        name,
        title: Some("Golem Manifest".to_string()),
        description: Some("Golem application manifest file".to_string()),
        mime_type: Some("application/x-yaml".to_string()),
        size: None,
        icons: None,
    };

    Ok(raw_resource.optional_annotate(None))
}

/// Read manifest file contents
pub async fn read_manifest(uri: &str) -> anyhow::Result<String> {
    // Validate URI format
    let path_str = uri.strip_prefix("file://")
        .ok_or_else(|| anyhow::anyhow!("Invalid URI: must use file:// scheme"))?;

    let path = PathBuf::from(path_str);

    // Security: canonicalize to prevent path traversal
    let canonical_path = path.canonicalize()
        .map_err(|e| anyhow::anyhow!("Invalid path: {}", e))?;

    // Security: verify it's a golem.yaml file
    if canonical_path.file_name().and_then(|n| n.to_str()) != Some(MANIFEST_FILENAME) {
        anyhow::bail!("Can only read golem.yaml manifest files");
    }

    // Read file contents
    let contents = fs::read_to_string(&canonical_path).await?;

    Ok(contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_uri_format() {
        let path = PathBuf::from("/tmp/test-project/golem.yaml");
        // This test will fail until we have a test manifest
        // but it demonstrates expected behavior
    }
}
