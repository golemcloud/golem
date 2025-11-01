// Security validation for MCP server
// Input sanitization and path validation

use rmcp::ErrorData as McpError;
use std::path::PathBuf;

/// List of command prefixes that should not be exposed via MCP
const SENSITIVE_COMMAND_PREFIXES: &[&str] = &[
    "profile",           // Profile management (credentials)
    "cloud account grant", // Grant management (security)
    "cloud token",       // Token management (credentials)
    "cloud account",     // Account management (sensitive)
    "cloud project grant", // Project grants (security)
    "cloud project policy", // Policy management (security)
];

/// Check if a command should be filtered from MCP exposure
pub fn is_sensitive_command(command: &str) -> bool {
    SENSITIVE_COMMAND_PREFIXES
        .iter()
        .any(|prefix| command.starts_with(prefix))
}

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
    fn test_is_sensitive_command_filters_profile() {
        assert!(is_sensitive_command("profile"));
        assert!(is_sensitive_command("profile add"));
        assert!(is_sensitive_command("profile delete"));
    }

    #[test]
    fn test_is_sensitive_command_filters_cloud_account() {
        assert!(is_sensitive_command("cloud account"));
        assert!(is_sensitive_command("cloud account get"));
        assert!(is_sensitive_command("cloud account grant"));
        assert!(is_sensitive_command("cloud account grant list"));
    }

    #[test]
    fn test_is_sensitive_command_filters_cloud_token() {
        assert!(is_sensitive_command("cloud token"));
        assert!(is_sensitive_command("cloud token get"));
    }

    #[test]
    fn test_is_sensitive_command_filters_cloud_grants() {
        assert!(is_sensitive_command("cloud project grant"));
        assert!(is_sensitive_command("cloud project grant list"));
    }

    #[test]
    fn test_is_sensitive_command_filters_cloud_policy() {
        assert!(is_sensitive_command("cloud project policy"));
        assert!(is_sensitive_command("cloud project policy get"));
    }

    #[test]
    fn test_is_sensitive_command_allows_safe_commands() {
        // Component commands should be allowed
        assert!(!is_sensitive_command("component list"));
        assert!(!is_sensitive_command("component add"));
        assert!(!is_sensitive_command("component get"));

        // Agent commands should be allowed
        assert!(!is_sensitive_command("agent list"));
        assert!(!is_sensitive_command("agent invoke"));

        // Safe cloud commands should be allowed
        assert!(!is_sensitive_command("cloud project list"));
        assert!(!is_sensitive_command("cloud project get"));
    }

    #[test]
    fn test_is_sensitive_command_prefix_matching() {
        // Should match by prefix, not exact match
        assert!(is_sensitive_command("profile some subcommand"));

        // Should not match if prefix is in middle
        assert!(!is_sensitive_command("get profile data"));
    }

    #[test]
    fn test_validate_component_name() {
        assert!(validate_component_name("valid-name").is_ok());
        assert!(validate_component_name("valid_name_123").is_ok());
        assert!(validate_component_name("../etc/passwd").is_err());
        assert!(validate_component_name("name;rm").is_err());
        assert!(validate_component_name("name/path").is_err());
    }
}
