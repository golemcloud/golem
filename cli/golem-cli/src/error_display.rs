use anyhow::Error;
use std::fmt::Write;

pub trait HumanReadableError {
    fn to_human_readable(&self) -> String;
}

impl HumanReadableError for Error {
    fn to_human_readable(&self) -> String {
        let chain: Vec<String> = self.chain().map(|e| e.to_string()).collect();
        let root_cause = chain.last().unwrap_or(&self.to_string()).clone();

        let mut message = String::new();

        // Detect common error patterns
        if root_cause.contains("Connection refused") {
            let _ = write!(message, "Error: Could not connect to the Golem Server.\n\n");
            let _ = write!(message, "  • Check if the server is running.\n");
            let _ = write!(message, "  • Verify the URL in your profile or GOLEM_BASE_URL environment variable.\n");
            let _ = write!(message, "  • Ensure your network connection is stable.\n");
        } else if root_cause.contains("401 Unauthorized") {
            let _ = write!(message, "Error: You are not authorized to perform this action.\n\n");
            let _ = write!(message, "  • Your session token may have expired.\n");
            let _ = write!(message, "  • Please run `golem-cli login` to authenticate again.\n");
        } else if root_cause.contains("403 Forbidden") {
             let _ = write!(message, "Error: Permission denied.\n\n");
             let _ = write!(message, "  • You do not have the required permissions for this resource.\n");
             let _ = write!(message, "  • Contact your project administrator.\n");
        } else if root_cause.contains("404 Not Found") {
             let _ = write!(message, "Error: Resource not found.\n\n");
             let _ = write!(message, "  • The requested component, worker, or resource does not exist.\n");
             let _ = write!(message, "  • Check for typos in IDs or names.\n");
        } else {
             // Default human readable format: Top level error + Root cause (if different)
             let _ = write!(message, "Error: {}\n", self);
             if chain.len() > 1 {
                 let _ = write!(message, "\nCaused by:\n");
                 // Indent causes
                 for (i, cause) in chain.iter().skip(1).enumerate() {
                     let _ = write!(message, "  {}: {}\n", i + 1, cause);
                 }
             }
        }

        message
    }
}

pub fn format_error(error: &Error) -> String {
    error.to_human_readable()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use test_r::test;

    #[test]
    fn test_connection_refused() {
        let err = anyhow::anyhow!("Connection refused").context("Failed to connect");
        let msg = format_error(&err);
        assert!(msg.contains("Could not connect to the Golem Server"));
        assert!(msg.contains("Check if the server is running"));
    }

    #[test]
    fn test_unauthorized() {
        let err = anyhow::anyhow!("Request failed with 401 Unauthorized").context("API Call failed");
        let msg = format_error(&err);
        assert!(msg.contains("You are not authorized"));
        assert!(msg.contains("golem-cli login"));
    }

    #[test]
    fn test_generic_chain() {
        let err = anyhow::anyhow!("Low level io error")
            .context("Config parsing failed")
            .context("Application start failed");

        let msg = format_error(&err);
        assert!(msg.contains("Error: Application start failed"));
        assert!(msg.contains("Caused by:"));
        assert!(msg.contains("1: Config parsing failed"));
        assert!(msg.contains("2: Low level io error"));
    }
}
