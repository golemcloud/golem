// Security validation for MCP server
// Input sanitization and path validation

use rmcp::prelude::McpError;
use std::path::PathBuf;

/// Validate component name to prevent path traversal
pub fn validate_component_name(name: &str) -> Result<(), McpError> {
    if name.contains("..") || name.contains("/") || name.contains("\\") {
        return Err(McpError::invalid_params(
            "Invalid component name: path traversal detected",
            None
        ));
    }

    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err(McpError::invalid_params(
            "Invalid component name: use alphanumeric, hyphen, or underscore only",
            None
        ));
    }

    Ok(())
}

/// Validate resource path to prevent access outside project
pub fn validate_resource_path(uri: &str) -> Result<PathBuf, McpError> {
    let path = uri.strip_prefix("file://")
        .ok_or_else(|| McpError::invalid_params(
            "Resource URI must use file:// scheme",
            None
        ))?;

    let path = PathBuf::from(path);
    let canonical = path.canonicalize()
        .map_err(|e| McpError::internal_error(
            format!("Invalid path: {}", e),
            None
        ))?;

    let project_root = std::env::current_dir()
        .map_err(|e| McpError::internal_error(
            format!("Cannot determine project root: {}", e),
            None
        ))?;

    if !canonical.starts_with(&project_root) {
        return Err(McpError::invalid_params(
            "Resource path outside project directory",
            None
        ));
    }

    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_component_name() {
        assert!(validate_component_name("valid-name").is_ok());
        assert!(validate_component_name("valid_name_123").is_ok());
        assert!(validate_component_name("../etc/passwd").is_err());
        assert!(validate_component_name("name;rm").is_err());
        assert!(validate_component_name("name/path").is_err());
    }
}
