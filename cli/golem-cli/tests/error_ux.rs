use crate::Tracing;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);

#[test]
fn test_error_formatting(_tracing: &Tracing) {
    let error = anyhow::anyhow!("Connection refused")
        .context("Failed to connect to worker")
        .context("Failed to execute command");

    println!("Raw error: {:#}", error);

    let formatted = golem_cli::error_display::format_error(&error);
    println!("Formatted: {}", formatted);

    assert!(formatted.contains("Error: Could not connect to the Golem Server."));
    assert!(formatted.contains("Check if the server is running."));
}
