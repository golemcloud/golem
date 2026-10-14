//! Resolve command names using registered builtins and the shell search path.
use std::io::Write;

use brush_core::builtins::{ContentOptions, ContentType, Registration, SimpleCommand};
use brush_core::commands::ExecutionContext;
use brush_core::extensions::ShellExtensions;
use brush_core::{Error, ExecutionResult};

use crate::manifest::Manifest;

pub(crate) struct Which;

impl Which {
    pub(crate) const NAME: &'static str = "which";
    pub(crate) const SYNOPSIS: &'static str = "locate a file-backed command on $PATH";
}

impl SimpleCommand for Which {
    fn get_content(
        name: &str,
        content_type: ContentType,
        _options: &ContentOptions,
    ) -> Result<String, Error> {
        match content_type {
            ContentType::ShortDescription => Ok(format!("{name} - {}\n", Which::SYNOPSIS)),
            ContentType::ShortUsage => Ok(format!("{name}: {name} <name>...\n")),
            ContentType::DetailedHelp => Ok(format!(
                "{name} - {}\n\n(the shell builtin)\n",
                Which::SYNOPSIS
            )),
            ContentType::ManPage => brush_core::error::unimp("man page not yet implemented"),
        }
    }

    fn execute<SE, I, S>(
        context: ExecutionContext<'_, SE>,
        args: I,
    ) -> Result<ExecutionResult, Error>
    where
        SE: ShellExtensions,
        I: Iterator<Item = S>,
        S: AsRef<str>,
    {
        // Skip argv[0] (the command name Brush passes); the rest are names to resolve.
        let names: Vec<String> = args
            .skip(1)
            .map(|s| s.as_ref().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        // Read $PATH from the shell env and split it. Reading before borrowing stdout keeps the
        // immutable borrow of `context.shell` self-contained.
        let path_dirs: Vec<std::path::PathBuf> = context
            .shell
            .env()
            .get_str("PATH", context.shell)
            .map(|p| brush_core::sys::fs::split_paths(p.as_ref()).collect())
            .unwrap_or_default();

        if names.is_empty() {
            let _ = writeln!(context.stderr(), "which: missing operand");
            return Ok(ExecutionResult::new(1));
        }

        // Every in-process command this shell answers to, whether one of this crate's own
        // (`ls`, `sed`, `grep`, ...) or a Brush builtin that stands in for a real GNU/POSIX
        // program (`echo`, `printf`, `test`, ...): on a real system every one of these is a
        // real file `which` finds on `PATH`, and there is no way to tell a script apart from
        // that once it is running, so this reports the conventional `/bin/<name>` a real
        // system would have for it -- the path the shell runs the command by (`/bin/cat f`).
        // Deliberately excludes the builtins only a shell can be (`cd`, `read`, `export`, ...),
        // which even real `which` cannot find -- see `tools::programs`, the same distinction
        // `xargs`/`find -exec` need. The shell's program table (`Shell::program_path`,
        // `Shell::program_builtin`) is the one place that decides.
        let mut out = context.stdout();
        // Exit status follows POSIX `which`: 0 if every name resolved, 1 if any did not.
        let mut all_found = true;
        for name in &names {
            match resolve_file_backed(&context, &path_dirs, name) {
                Some(path) => {
                    let _ = writeln!(out, "{}", path.display());
                }
                None if name.contains('/') && context.shell.program_builtin(name).is_some() => {
                    let _ = writeln!(out, "{name}");
                }
                None if let Some(path) = context
                    .shell
                    .program_path(name)
                    .filter(|_| !name.contains('/')) =>
                {
                    let _ = writeln!(out, "{}", path.display());
                }
                None => all_found = false,
            }
        }

        let code = u8::from(!all_found);
        Ok(ExecutionResult::new(code))
    }
}

/// The first `<dir>/<name>` across `path_dirs` that is a real, existing file (not a directory). A
/// `name` containing a `/` is treated as a literal path and checked directly, like `which`,
/// resolved against the shell's own (carried, not the WASI process's) working directory.
///
/// Uses `Path::exists()` (excluding directories), NOT `is_file()`: on wasip2/Golem the two diverge —
/// `exists()` correctly reports a missing path (verified: `test -e` works on the agent), while
/// `is_file()` returned true for phantom paths, making `which` report files that aren't there.
fn resolve_file_backed<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    path_dirs: &[std::path::PathBuf],
    name: &str,
) -> Option<std::path::PathBuf> {
    let is_real_file = |p: &std::path::Path| p.exists() && !p.is_dir();
    // An explicit path (contains a `/`) isn't searched on $PATH — check it as-is.
    if name.contains('/') {
        let p = context.shell.absolute_path(std::path::Path::new(name));
        return is_real_file(&p).then_some(std::path::PathBuf::from(name));
    }
    path_dirs.iter().find_map(|dir| {
        let candidate = context.shell.absolute_path(dir.join(name));
        is_real_file(&candidate).then_some(dir.join(name))
    })
}

/// The `which` builtin registration, for `build_shell`.
pub(crate) fn builtins<SE: ShellExtensions>() -> Vec<(String, Registration<SE>)> {
    use crate::helpshim::simple_builtin_with_help;
    vec![(Which::NAME.into(), simple_builtin_with_help::<Which, SE>())]
}

/// The `which` manifest. `shell-internal` scope (README classifies `which` with `type`), `Allow`.
pub(crate) fn manifests() -> Vec<Manifest> {
    vec![Manifest::builtin(Which::NAME, Which::SYNOPSIS)]
}
