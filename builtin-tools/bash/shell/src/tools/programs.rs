//! Which commands stand for programs a Linux system keeps as files in `/bin` and `/usr/bin`.
//!
//! Every command of this crate (and every bound tool) is a program there: `/bin/cat` runs `cat`,
//! and `type`, `command -v`, `hash` and `which` report `/bin/cat` as they report a program found
//! on `PATH`. Of bash's own builtins, only those that also ship as programs and cannot change the
//! calling shell are (`/bin/echo`, `/usr/bin/test`): `type echo` still says it is a shell builtin.
//! Every other builtin (`cd`, `read`, `export`) has no file, so `/bin/read` is not found, as on
//! Linux, where a program cannot change the shell that runs it.

use brush_core::builtins::ProgramKind;

/// Bash 5's builtins (`compgen -b`): what bash reports as a shell builtin.
const BASH_BUILTINS: &[&str] = &[
    ".",
    ":",
    "[",
    "alias",
    "bg",
    "bind",
    "break",
    "builtin",
    "caller",
    "cd",
    "command",
    "compgen",
    "complete",
    "compopt",
    "continue",
    "declare",
    "dirs",
    "disown",
    "echo",
    "enable",
    "eval",
    "exec",
    "exit",
    "export",
    "false",
    "fc",
    "fg",
    "getopts",
    "hash",
    "help",
    "history",
    "jobs",
    "kill",
    "let",
    "local",
    "logout",
    "mapfile",
    "popd",
    "printf",
    "pushd",
    "pwd",
    "read",
    "readarray",
    "readonly",
    "return",
    "set",
    "shift",
    "shopt",
    "source",
    "suspend",
    "test",
    "times",
    "trap",
    "true",
    "type",
    "typeset",
    "ulimit",
    "umask",
    "unalias",
    "unset",
    "wait",
];

/// Bash builtins that also ship as programs (GNU coreutils' and procps' files in `/bin` and
/// `/usr/bin`) and change nothing in the shell that runs them.
const BUILTIN_PROGRAMS: &[&str] = &[
    "[", "echo", "false", "kill", "printf", "pwd", "test", "true",
];

/// Whether `name` is one of bash's own builtins.
pub(crate) fn is_bash_builtin(name: &str) -> bool {
    BASH_BUILTINS.contains(&name)
}

/// How the command `name` stands for a program, if it does.
pub(crate) fn kind(name: &str) -> Option<ProgramKind> {
    if !BASH_BUILTINS.contains(&name) {
        Some(ProgramKind::File)
    } else if BUILTIN_PROGRAMS.contains(&name) {
        Some(ProgramKind::Builtin)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::kind;
    use brush_core::builtins::ProgramKind;

    #[test]
    fn only_programs_that_cannot_change_the_shell_have_files() {
        assert_eq!(kind("cat"), Some(ProgramKind::File));
        assert_eq!(kind("sh"), Some(ProgramKind::File));
        assert_eq!(kind("echo"), Some(ProgramKind::Builtin));
        assert_eq!(kind("["), Some(ProgramKind::Builtin));
        for name in [
            "read",
            "cd",
            "export",
            "declare",
            "readarray",
            "compgen",
            "hash",
            "type",
        ] {
            assert_eq!(kind(name), None, "{name}");
        }
    }
}
