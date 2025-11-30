/// Commands that should *never* be exposed through MCP.
const SENSITIVE_COMMAND_PREFIXES: &[&str] = &[
    "profile",
    "cloud account",
    "cloud token",
    "cloud project grant",
    "cloud project policy",
    "cloud secret",
    "auth",
    "login",
    "logout",
    "session",
];

/// Dangerous patterns for SHELL EXECUTION (not for argument values)
/// These are only dangerous if someone tries to execute shell commands
const SHELL_INJECTION_PATTERNS: &[&str] = &["&&", "||", ";", "|", "`", "$("];

/// Check if a command string (the command itself) is sensitive
pub fn is_sensitive_command(command: &str) -> bool {
    let normalized = command.trim().to_lowercase();

    // Block sensitive prefixes
    if SENSITIVE_COMMAND_PREFIXES
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
    {
        return true;
    }

    if SHELL_INJECTION_PATTERNS
        .iter()
        .any(|pattern| normalized.contains(pattern))
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_injection_blocked() {
        // These should be blocked
        assert!(is_sensitive_command("component && rm -rf /"));
        assert!(is_sensitive_command("agent; cat /etc/passwd"));
        assert!(is_sensitive_command("build`whoami`"));
        assert!(is_sensitive_command("deploy$(cat /etc/passwd)"));
    }
    #[test]
    fn test_safe_commands_with_operators_allowed() {
        // These are safe - operators in arguments, not in command name
        assert!(!is_sensitive_command(
            "agent invoke my-worker compare 5 < 10"
        ));
        assert!(!is_sensitive_command("component build --filter name>2"));
        assert!(!is_sensitive_command("app deploy"));
    }

    #[test]
    fn test_sensitive_commands() {
        assert!(is_sensitive_command("profile list"));
        assert!(is_sensitive_command("cloud token get"));
        assert!(is_sensitive_command("cloud account grant"));
    }
}
