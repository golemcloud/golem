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

use rmcp::model::{AnnotateAble, RawResource, Resource, ResourceContents};
use std::path::{Path, PathBuf};

const MANIFEST_FILENAME: &str = "golem.yaml";

/// Discover all golem.yaml manifest files in the current directory,
/// ancestor directories, and immediate child directories.
pub fn discover_manifest_resources() -> Vec<Resource> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut manifests = Vec::new();

    // Current directory
    check_and_add_manifest(&cwd, &mut manifests);

    // Ancestor directories
    let mut parent = cwd.parent();
    while let Some(dir) = parent {
        check_and_add_manifest(dir, &mut manifests);
        parent = dir.parent();
    }

    // Immediate child directories
    if let Ok(entries) = std::fs::read_dir(&cwd) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                check_and_add_manifest(&path, &mut manifests);
            }
        }
    }

    manifests
}

fn check_and_add_manifest(dir: &Path, manifests: &mut Vec<Resource>) {
    let manifest_path = dir.join(MANIFEST_FILENAME);
    if manifest_path.exists() && manifest_path.is_file() {
        let uri = path_to_file_uri(&manifest_path);
        let name = format!(
            "golem.yaml ({})",
            dir.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| dir.to_string_lossy().to_string())
        );
        manifests.push(RawResource::new(uri, name).no_annotation());
    }
}

fn path_to_file_uri(path: &Path) -> String {
    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path_str = absolute.to_string_lossy();

    // On Windows, convert backslashes to forward slashes and handle the drive prefix
    #[cfg(target_os = "windows")]
    {
        let path_str = path_str.replace('\\', "/");
        if path_str.starts_with("\\\\?\\") || path_str.starts_with("//?/") {
            format!("file:///{}", &path_str[4..])
        } else {
            format!("file:///{}", path_str)
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        format!("file://{}", path_str)
    }
}

/// Read the contents of a manifest file given its file:// URI.
pub fn read_manifest_resource(uri: &str) -> Result<Vec<ResourceContents>, String> {
    let file_path = file_uri_to_path(uri)?;

    let content =
        std::fs::read_to_string(&file_path).map_err(|e| format!("Failed to read {}: {}", uri, e))?;

    Ok(vec![ResourceContents::text(content, uri.to_string())])
}

fn file_uri_to_path(uri: &str) -> Result<PathBuf, String> {
    let path_str = uri
        .strip_prefix("file:///")
        .or_else(|| uri.strip_prefix("file://"))
        .ok_or_else(|| format!("Invalid file URI: {}", uri))?;

    Ok(PathBuf::from(path_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_to_file_uri() {
        let path = Path::new("/tmp/test/golem.yaml");
        let uri = path_to_file_uri(path);
        assert!(
            uri.starts_with("file://"),
            "URI should start with file://, got: {}",
            uri
        );
    }

    #[test]
    fn test_file_uri_to_path() {
        let result = file_uri_to_path("file:///tmp/test/golem.yaml");
        assert!(result.is_ok());
    }
}
