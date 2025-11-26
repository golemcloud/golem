use anyhow::{Context, Result};
use rmcp::model::{RawResource, Resource, ResourceContents};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tokio::fs;
use walkdir::WalkDir;

const MANIFEST_FILES: &[&str] = &["golem.yaml", "golem.yml"];

pub struct ResourceScanner {
    /// canonicalized root directory that bounds all access
    root_dir: PathBuf,
}

impl ResourceScanner {
    pub fn new(working_dir: PathBuf) -> Result<Self> {
        let root_dir = working_dir.canonicalize().context(format!(
            "Failed to canonicalize working dir: {:?}",
            working_dir
        ))?;
        Ok(Self { root_dir })
    }

    pub async fn discover_manifests(&self) -> Result<Vec<Resource>> {
        let mut resources = Vec::new();
        let mut visited_paths = HashSet::new();

        self.search_ancestors(&mut resources, &mut visited_paths)
            .await?;
        self.search_children(&mut resources, &mut visited_paths)
            .await?;

        Ok(resources)
    }

    /// Search for manifests in parent directories, but never escape
    /// the canonical root_dir boundary.
    async fn search_ancestors(
        &self,
        resources: &mut Vec<Resource>,
        visited: &mut HashSet<PathBuf>,
    ) -> Result<()> {
        let mut current = self.root_dir.as_path();

        while let Some(parent) = current.parent() {
            let parent_canonical = parent
                .canonicalize()
                .with_context(|| format!("Failed to canonicalize parent: {:?}", parent))?;

            // Security: do NOT go above root_dir
            if !parent_canonical.starts_with(&self.root_dir) {
                break; // stop immediately
            }

            // Check any manifest files inside this parent directory
            for entry in std::fs::read_dir(&parent_canonical)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_file() {
                    if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                        if MANIFEST_FILES.contains(&filename) {
                            let canonical = path
                                .canonicalize()
                                .with_context(|| format!("Failed to canonicalize: {:?}", path))?;

                            if visited.insert(canonical.clone()) {
                                resources.push(self.create_resource(&canonical)?);
                            }
                        }
                    }
                }
            }

            current = parent;
        }

        Ok(())
    }

    /// Search child directories recursively (bounded to root_dir).
    async fn search_children(
        &self,
        resources: &mut Vec<Resource>,
        visited: &mut HashSet<PathBuf>,
    ) -> Result<()> {
        for entry in WalkDir::new(&self.root_dir)
            .follow_links(false)
            .max_depth(5)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            if !path.is_file() {
                continue;
            }
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                if MANIFEST_FILES.contains(&filename) {
                    let candidate_canonical = path
                        .canonicalize()
                        .with_context(|| format!("Failed to canonicalize path: {:?}", path))?;

                    if visited.insert(candidate_canonical.clone()) {
                        resources.push(self.create_resource(&candidate_canonical)?);
                    }
                }
            }
        }

        Ok(())
    }

    /// Create MCP Resource from a canonical file path (must be inside root
    fn create_resource(&self, canonical_path: &Path) -> Result<Resource> {
        if !canonical_path.starts_with(&self.root_dir) {
            anyhow::bail!(
                "create_resource: path is outside the allowed root: {:?}",
                canonical_path
            );
        }

        let uri = format!("file://{}", canonical_path.display());
        let name = canonical_path
            .strip_prefix(&self.root_dir)
            .unwrap_or(canonical_path)
            .to_string_lossy()
            .to_string();

        let description = self.get_resource_description(canonical_path);
        let mime_type = self.get_mime_type(canonical_path);
        let size = canonical_path
            .metadata()
            .ok()
            .and_then(|m| u32::try_from(m.len()).ok());

        Ok(Resource {
            raw: RawResource {
                uri,
                name,
                description: Some(description),
                mime_type: Some(mime_type),
                size,
                icons: None,
                title: None,
            },
            annotations: None,
        })
    }

    /// Get human-readable description for resource
    fn get_resource_description(&self, path: &Path) -> String {
        match path.file_name().and_then(|n| n.to_str()) {
            Some("golem.yaml") => {
                "Golem application manifest defining components, APIs, and deployment configuration"
            }
            _ => "Configuration file",
        }
        .to_string()
    }

    /// Get MIME type for file
    fn get_mime_type(&self, path: &Path) -> String {
        match path.extension().and_then(|e| e.to_str()) {
            Some("yaml") | Some("yml") => "application/yaml",
            _ => "text/plain",
        }
        .to_string()
    }

    /// Read resource file contents from a file:// URI — only allows files under root_dir
    pub async fn read_resources(&self, uri: &str) -> Result<Vec<ResourceContents>> {
        // Extract path from URI
        let path_str = uri
            .strip_prefix("file://")
            .ok_or_else(|| anyhow::anyhow!("Invalid URI: must start with file://"))?;

        let path = PathBuf::from(path_str);

        // Canonicalize candidate path
        let canonical = path.canonicalize().context("Failed to canonicalize path")?;

        // Security: ensure canonical is inside root_dir
        if !canonical.starts_with(&self.root_dir) {
            anyhow::bail!("Access denied: file is outside allowed root");
        }

        let filename = canonical
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid filename"))?;

        if !MANIFEST_FILES.contains(&filename) {
            anyhow::bail!("Access denied: only golem.yaml or golem.yml can be accessed");
        }

        // Read file contents
        let content = fs::read_to_string(&canonical)
            .await
            .context(format!("Failed to read file: {:?}", canonical))?;

        Ok(vec![ResourceContents::TextResourceContents {
            uri: uri.to_string(),
            mime_type: Some("text/plain".to_string()),
            text: content,
            meta: None,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_discover_manifests_file() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let working_dir = temp_dir.path().to_path_buf();
        std::env::set_current_dir(working_dir.as_path()).unwrap();

        // Create test files inside working_dir and a child subdir
        tokio::fs::write(working_dir.join("golem.yaml"), "root: content").await?;

        let component_dir = working_dir.join("component1");
        tokio::fs::create_dir(&component_dir).await?;
        tokio::fs::write(component_dir.join("golem.yaml"), "component: test").await?;

        let scanner = ResourceScanner::new(working_dir.clone())?;
        let resources = scanner.discover_manifests().await?;

        // Must find both manifests inside the working_dir subtree
        assert!(resources.iter().any(|r| r.uri.contains("golem.yaml")));
        assert!(resources
            .iter()
            .any(|r| r.uri.ends_with("/component1/golem.yaml")));
        Ok(())
    }

    #[tokio::test]
    async fn test_read_resource_allowed() -> Result<()> {
        let temp_dir = TempDir::new()?;

        let working_dir = temp_dir.path().to_path_buf();
        std::env::set_current_dir(working_dir.as_path()).unwrap();

        let test_content = "test: yaml content";
        let manifest_path = working_dir.join("golem.yaml");
        tokio::fs::write(&manifest_path, test_content).await?;

        let scanner = ResourceScanner::new(working_dir.clone())?;
        let resources = scanner.discover_manifests().await?;
        assert_eq!(resources.len(), 1);

        // Use MCP-generated URI
        let uri = &resources[0].raw.uri;

        // Now it's 100% correct MCP-style
        let contents = scanner.read_resources(uri).await?;

        assert_eq!(contents.len(), 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_read_resource_denied_outside_root() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let working_dir = temp_dir.path().to_path_buf();
        std::env::set_current_dir(working_dir.as_path()).unwrap();

        // Create a file outside the working dir (simulate attacker file)
        let outside_dir = TempDir::new()?; // different temp dir
        let outside_file = outside_dir.path().join("golem.yaml");
        tokio::fs::write(&outside_file, "bad: content").await?;

        let scanner = ResourceScanner::new(working_dir.clone())?;
        let uri = format!("file://{}", outside_file.canonicalize()?.display());

        // Should be denied because file is not under the scanner's root_dir
        let res = scanner.read_resources(&uri).await;
        assert!(res.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_path_traversal_blocked() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let working_dir = temp_dir.path().to_path_buf();

        tokio::fs::write(working_dir.join("golem.yaml"), "safe: content").await?;

        let scanner = ResourceScanner::new(working_dir.clone())?;

        // Try to read with path traversal
        let uri = format!("file://{}/../../etc/passwd", working_dir.display());
        let result = scanner.read_resources(&uri).await;

        assert!(result.is_err(), "Path traversal should be blocked");
        Ok(())
    }

    #[tokio::test]
    async fn test_non_manifest_file_blocked() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let working_dir = temp_dir.path().to_path_buf();

        // Create a non-manifest file
        let secret_file = working_dir.join("secrets.txt");
        tokio::fs::write(&secret_file, "password123").await?;

        let scanner = ResourceScanner::new(working_dir.clone())?;
        let uri = format!("file://{}", secret_file.canonicalize()?.display());

        let result = scanner.read_resources(&uri).await;
        assert!(result.is_err(), "Non-manifest files should be blocked");
        Ok(())
    }
}
