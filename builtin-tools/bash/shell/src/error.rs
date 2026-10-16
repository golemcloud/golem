//! Errors returned by local shell utilities.

/// A failure in a shell builtin that belongs to no subsystem.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ShellError {
    /// The invocation was wrong: an unknown flag, a missing operand, a non-numeric where a number
    /// belongs, or a predicate this implementation does not support.
    #[error("{0}")]
    Usage(String),

    /// The named path, process, or object does not exist.
    #[error("{0}")]
    NotFound(String),

    /// A filesystem operation failed.
    #[error("{0}")]
    Io(String),
}

impl ShellError {
    /// A malformed invocation.
    #[must_use]
    pub fn usage(msg: impl Into<String>) -> Self {
        ShellError::Usage(msg.into())
    }

    /// A missing path, process, or object.
    #[must_use]
    pub fn not_found(msg: impl Into<String>) -> Self {
        ShellError::NotFound(msg.into())
    }

    /// A filesystem failure.
    #[must_use]
    pub fn io(msg: impl Into<String>) -> Self {
        ShellError::Io(msg.into())
    }

    /// The shell exit code this failure should surface: `2` for a usage error, `1` otherwise —
    /// the convention the builtins already followed by hand.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            ShellError::Usage(_) => 2,
            ShellError::NotFound(_) | ShellError::Io(_) => 1,
        }
    }
}

/// A shell-builtin result.
pub type Result<T> = std::result::Result<T, ShellError>;

#[cfg(test)]
mod tests {
    use super::ShellError;

    #[test]
    fn exit_codes_follow_the_builtin_convention() {
        assert_eq!(ShellError::usage("unknown flag").exit_code(), 2);
        assert_eq!(ShellError::not_found("no such file").exit_code(), 1);
        assert_eq!(ShellError::io("permission denied").exit_code(), 1);
    }
}
