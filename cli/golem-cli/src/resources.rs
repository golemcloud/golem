// resources.rs
use rust_mcp_sdk::schema::{
    ListResourcesResult, Resource,Annotations, 
    ReadResourceResult, ReadResourceResultContentsItem, TextResourceContents,
};

use std::path::{Path, PathBuf};


use serde_json::Map;
use serde_json::Value;

/// Build the MCP `resources/list` result from the manifest discovery.
pub fn list_resources_from_manifests(cwd: Option<&str>) -> ListResourcesResult {
    let tree = crate::tools::discover_manifest_tree(cwd);

    let root = cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let mut out: Vec<Resource> = Vec::new();
    flatten_tree_into_resources(&tree, &root, &mut Vec::new(), &mut out);

    ListResourcesResult { resources: out, next_cursor: None, meta: None }
}

/// Resolve and read a single resource by its file:// URI (YAML/YML).
pub fn read_manifest_resource(uri: &str) -> Option<ReadResourceResult> {
    let path = uri.strip_prefix("file://")?;
    let text = std::fs::read_to_string(path).ok()?; // load the YAML/YML content

    let item = ReadResourceResultContentsItem::TextResourceContents(
        TextResourceContents {
            uri: uri.to_string(),
            mime_type: Some("application/yaml".to_string()), // or detect yml/yaml
            text,                                            // << required
            meta: None,
        },
    );

    Some(ReadResourceResult {
        contents: vec![item],
        meta: None,
    })
}

// ---------- helpers ----------

fn flatten_tree_into_resources(
    node: &serde_json::Value,
    root: &Path,
    segments: &mut Vec<String>,
    out: &mut Vec<Resource>,
) {
    match node {
        serde_json::Value::String(filename) => {
            let mut full = PathBuf::from(root);
            for seg in segments.iter() { full.push(seg); }
            full.push(filename);

            let abs = full.canonicalize().unwrap_or(full.clone());
            push_manifest_resource(out, &abs, segments);
        }
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                segments.push(k.clone());
                flatten_tree_into_resources(v, root, segments, out);
                segments.pop();
            }
        }
        _ => {}
    }
}


fn mime_for_path(p: &std::path::Path) -> String {
    match p.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()) {
        Some(ext) if ext == "yaml" || ext == "yml" => "application/yaml".to_string(),
        _ => "text/plain".to_string(),
    }
}


fn push_manifest_resource(
    out: &mut Vec<Resource>,
    abs_path: &std::path::Path,
    logical_dirs: &[String],
) {
    let file_name = abs_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("manifest.yaml")
        .to_string();

    let uri = format!("file://{}", abs_path.display());

    let description = if logical_dirs.is_empty() {
        format!("Manifest file {}", file_name)
    } else {
        format!("Manifest for {}", logical_dirs.join("/"))
    };

    let size = std::fs::metadata(abs_path).ok().and_then(|m| m.len().try_into().ok());

    out.push(Resource {
        uri,
        name: file_name.clone(),
        title: Some(file_name.clone()),
        description: Some(description),
        mime_type: Some(mime_for_path(abs_path)),       // "application/yaml" for yml/yaml
        // extra fields your SDK expects:
        annotations: None,                              // or Some(Annotations { audience: vec![], last_modified: None, priority: None })
        meta: None::<Map<String, Value>>,
        size,                                           // Option<i64>
        // if your struct has any additional required field, add it here similarly.
    });
}