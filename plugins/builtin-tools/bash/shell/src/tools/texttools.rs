//! Text and data utilities with cooperative WASM stream adapters.
#![allow(clippy::similar_names)] // argv/args/arg-style locals are inherent to arg parsing here

use brush_core::builtins::{ContentOptions, ContentType, Registration, SimpleCommand};
use brush_core::commands::ExecutionContext;
use brush_core::extensions::ShellExtensions;
use brush_core::{Error, ExecutionResult};
use std::io::{Read, Write};
use std::path::Path;

type ToolResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// The `$synopsis` feeds both `get_content` and the command [`Manifest`](crate::manifest::Manifest)
/// in [`manifests`], defined once so they can't drift.
macro_rules! text_builtin {
    ($ty:ident, $name:literal, $synopsis:literal, $run:path) => {
        pub(crate) struct $ty;

        impl $ty {
            const NAME: &'static str = $name;
            const SYNOPSIS: &'static str = $synopsis;
        }

        impl SimpleCommand for $ty {
            fn get_content(
                name: &str,
                content_type: ContentType,
                _options: &ContentOptions,
            ) -> Result<String, Error> {
                match content_type {
                    ContentType::ShortDescription => Ok(format!("{name} - {}\n", $ty::SYNOPSIS)),
                    ContentType::ShortUsage => Ok(format!("{name}: {name} [args...]\n")),
                    ContentType::DetailedHelp => Ok(format!(
                        "{name} - {}\n\n(the shell text/data builtin)\n",
                        $ty::SYNOPSIS
                    )),
                    ContentType::ManPage => {
                        brush_core::error::unimp("man page not yet implemented")
                    }
                }
            }

            #[allow(clippy::cast_sign_loss)] // code is clamped to 0..=255 before the u8 cast
            fn execute<SE, I, S>(
                context: ExecutionContext<'_, SE>,
                args: I,
            ) -> Result<ExecutionResult, Error>
            where
                SE: ShellExtensions,
                I: Iterator<Item = S>,
                S: AsRef<str>,
            {
                let argv: Vec<String> = args.map(|s| s.as_ref().to_string()).collect();
                // Write through Brush's stdout/stderr sinks (captured on wasm), not io::stdout().
                // `stdin` is Brush's assigned input `OpenFile` — the upstream pipe stage's output when
                // this command is on the right-hand side of a `|` — so tools can read piped input.
                let code =
                    crate::tools::coreutils::run_tool(&context, move |stdin, out, err| match $run(
                        &argv, stdin, out, err,
                    ) {
                        Ok(code) => code,
                        Err(e) => {
                            let _ = writeln!(err, "{}: {e}", $name);
                            1
                        }
                    });
                Ok(ExecutionResult::new(code.clamp(0, 255) as u8))
            }
        }
    };
}

pub(crate) struct Jq;

impl Jq {
    const NAME: &'static str = "jq";
    const SYNOPSIS: &'static str = "filter and transform JSON (jq 1.8 command line, jaq engine)";
}

impl SimpleCommand for Jq {
    fn get_content(
        name: &str,
        content_type: ContentType,
        _options: &ContentOptions,
    ) -> Result<String, Error> {
        match content_type {
            ContentType::ShortDescription => Ok(format!("{name} - {}\n", Jq::SYNOPSIS)),
            ContentType::ShortUsage => Ok(format!("{name}: {name} [OPTIONS] FILTER [FILES...]\n")),
            ContentType::DetailedHelp => Ok(super::jq::help().to_owned()),
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
        // jq reads its arguments as text: a byte that is not UTF-8 becomes U+FFFD, as in jq.
        let argv: Vec<String> = args
            .map(|s| crate::tools::shell_bytes::to_utf8_lossy(s.as_ref()))
            .collect();
        let env = crate::tools::coreutils::exported_env_utf8(&context);
        let shell = &context.shell;
        let resolve = |path: &str| shell.absolute_path(Path::new(path));
        let code = crate::tools::coreutils::run_tool(&context, |stdin, out, err| {
            super::jq::run_buffered(&argv, env, &resolve, stdin, out, err).unwrap_or_else(|error| {
                let _ = writeln!(err, "jq: {}", super::io_message(&error));
                2
            })
        });
        Ok(ExecutionResult::new(
            u8::try_from(code.clamp(0, 255)).unwrap_or(1),
        ))
    }
}
pub(crate) struct Grep;

impl Grep {
    const NAME: &'static str = "grep";
    const SYNOPSIS: &'static str = "print lines that match patterns (GNU grep command line)";
}

impl SimpleCommand for Grep {
    fn get_content(
        name: &str,
        content_type: ContentType,
        _options: &ContentOptions,
    ) -> Result<String, Error> {
        match content_type {
            ContentType::ShortDescription => Ok(format!("{name} - {}\n", Grep::SYNOPSIS)),
            ContentType::ShortUsage => {
                Ok(format!("{name}: {name} [OPTION]... PATTERNS [FILE]...\n"))
            }
            ContentType::DetailedHelp => Ok(super::grep::help().to_owned()),
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
        let argv: Vec<String> = args.map(|s| s.as_ref().to_string()).collect();
        let shell = &context.shell;
        let resolve = |path: &str| shell.absolute_path(Path::new(path));
        let utf8 = super::grep::utf8_locale(&crate::tools::coreutils::exported_env(&context));
        let code = crate::tools::coreutils::run_tool(&context, |stdin, out, err| {
            let outcome =
                super::grep::prepare(&argv, utf8, &|path| super::read_file(&resolve(path)));
            let result = match outcome {
                Ok(super::grep::Prepared::Run(options, matcher)) => {
                    super::grep::run_sync(&options, &matcher, utf8, stdin, out, err, &resolve)
                }
                Ok(super::grep::Prepared::Done(text)) => out.write_all(text.as_bytes()).map(|()| 0),
                Err(refusal) => err
                    .write_all(refusal.message.as_bytes())
                    .map(|()| refusal.code),
            };
            result.unwrap_or_else(|error| {
                let _ = writeln!(err, "grep: {}", super::io_message(&error));
                2
            })
        });
        Ok(ExecutionResult::new(
            u8::try_from(code.clamp(0, 255)).unwrap_or(1),
        ))
    }
}
pub(crate) struct Sed;

impl Sed {
    const NAME: &'static str = "sed";
    const SYNOPSIS: &'static str = "stream editor (uutils sed: POSIX and GNU scripts)";
}

impl SimpleCommand for Sed {
    fn get_content(
        name: &str,
        content_type: ContentType,
        _options: &ContentOptions,
    ) -> Result<String, Error> {
        match content_type {
            ContentType::ShortDescription => Ok(format!("{name} - {}\n", Sed::SYNOPSIS)),
            ContentType::ShortUsage => {
                Ok(format!("{name}: {name} [OPTION]... [SCRIPT] [FILE]...\n"))
            }
            ContentType::DetailedHelp => Ok(format!("{name} - {}\n", Sed::SYNOPSIS)),
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
        let argv: Vec<String> = args.map(|s| s.as_ref().to_string()).collect();
        let locale = super::sed::locale(&crate::tools::coreutils::exported_env(&context));
        // `prepare` compiles the script, which for `w`, `r`, `R` and `-f` opens a file by
        // whatever relative path the script gives — through code (the sed fork) that
        // resolves it against the process's own directory, not the shell's `cd`. Point the
        // process there for just the compile, the same way `run_uu` does for `-i`/`-s`.
        let prepared = {
            let _cwd = crate::tools::coreutils::ShellCwd::enter(&context);
            super::sed::prepare(&argv, locale)
        };
        let code = match prepared {
            // `-i` and `-s` depend on whole, separate files: uutils' own entry point.
            Ok((engine, _)) if engine.needs_files() => {
                // Same raw-byte conversion as `sed::prepare` itself: `-i`/`-s` scripts can
                // carry a shell-substituted raw byte too.
                let args = argv
                    .iter()
                    .map(|arg| crate::tools::shell_bytes::to_os_string(arg));
                crate::tools::coreutils::run_uu(&context, "sed", move || uu_sed::sed::uumain(args))
            }
            Ok((mut engine, files)) => {
                let shell = &context.shell;
                let open = |path: &str| std::fs::File::open(shell.absolute_path(Path::new(path)));
                crate::tools::coreutils::run_tool(&context, |stdin, out, err| {
                    super::sed::run_sync(&mut engine, &files, stdin, out, err, &open)
                        .unwrap_or_else(|error| {
                            let _ = writeln!(err, "sed: {}", super::io_message(&error));
                            4
                        })
                })
            }
            Err(refusal) => {
                // `--help`/`--version` (code 0 — see `sed::prepare`'s own note) belong
                // on stdout; every other refusal belongs on stderr as usual.
                let write_result = if refusal.code == 0 {
                    context.stdout().write_all(refusal.message.as_bytes())
                } else {
                    context.stderr().write_all(refusal.message.as_bytes())
                };
                let _ = write_result;
                refusal.code
            }
        };
        Ok(ExecutionResult::new(
            u8::try_from(code.clamp(0, 255)).unwrap_or(1),
        ))
    }
}
text_builtin!(File, "file", "identify file type", run_file);
// Small streaming utilities of our own (no uu_* crate backs these). The pure
// transforms (`yes_line`, `SeqPlan`, `rev_line`, `tac_bytes`) are shared with the wasm
// cooperative drivers in `streaming.rs`, so native and wasm can never format differently.
text_builtin!(
    Yes,
    "yes",
    "repeatedly output a line until stopped",
    run_yes
);
text_builtin!(Seq, "seq", "print a sequence of numbers", run_seq);
text_builtin!(Rev, "rev", "reverse the characters of each line", run_rev);
text_builtin!(
    Tac,
    "tac",
    "concatenate and print files in reverse line order",
    run_tac
);

pub(crate) struct Diff;

impl Diff {
    const NAME: &'static str = "diff";
    const SYNOPSIS: &'static str =
        "compare files line by line (unified/context/normal/ed/side-by-side, -r)";
}

impl SimpleCommand for Diff {
    fn get_content(
        name: &str,
        content_type: ContentType,
        _options: &ContentOptions,
    ) -> Result<String, Error> {
        match content_type {
            ContentType::ShortDescription => Ok(format!("{name} - {}\n", Diff::SYNOPSIS)),
            ContentType::ShortUsage => Ok(format!("{name}: {name} [OPTION]... FILES\n")),
            ContentType::DetailedHelp => Ok(format!("{name} - {}\n", Diff::SYNOPSIS)),
            ContentType::ManPage => brush_core::error::unimp("man page not yet implemented"),
        }
    }

    #[allow(clippy::cast_sign_loss)] // code is clamped to 0..=255 before the u8 cast
    fn execute<SE, I, S>(
        context: ExecutionContext<'_, SE>,
        args: I,
    ) -> Result<ExecutionResult, Error>
    where
        SE: ShellExtensions,
        I: Iterator<Item = S>,
        S: AsRef<str>,
    {
        let argv: Vec<String> = args.map(|s| s.as_ref().to_string()).collect();
        let shell = &context.shell;
        let resolve = |path: &str| shell.absolute_path(Path::new(path));
        let code = crate::tools::coreutils::run_tool(&context, |stdin, out, err| {
            super::diff::run_diff(&argv, stdin, &resolve, out, err).unwrap_or_else(|error| {
                let _ = writeln!(err, "diff: {}", super::io_message(&error));
                2
            })
        });
        Ok(ExecutionResult::new(code.clamp(0, 255) as u8))
    }
}

pub(crate) struct Cmp;

impl Cmp {
    const NAME: &'static str = "cmp";
    const SYNOPSIS: &'static str = "compare two files byte by byte";
}

impl SimpleCommand for Cmp {
    fn get_content(
        name: &str,
        content_type: ContentType,
        _options: &ContentOptions,
    ) -> Result<String, Error> {
        match content_type {
            ContentType::ShortDescription => Ok(format!("{name} - {}\n", Cmp::SYNOPSIS)),
            ContentType::ShortUsage => Ok(format!("{name}: {name} [OPTION]... FILE1 FILE2\n")),
            ContentType::DetailedHelp => Ok(format!("{name} - {}\n", Cmp::SYNOPSIS)),
            ContentType::ManPage => brush_core::error::unimp("man page not yet implemented"),
        }
    }

    #[allow(clippy::cast_sign_loss)] // code is clamped to 0..=255 before the u8 cast
    fn execute<SE, I, S>(
        context: ExecutionContext<'_, SE>,
        args: I,
    ) -> Result<ExecutionResult, Error>
    where
        SE: ShellExtensions,
        I: Iterator<Item = S>,
        S: AsRef<str>,
    {
        let argv: Vec<String> = args.map(|s| s.as_ref().to_string()).collect();
        let shell = &context.shell;
        let resolve = |path: &str| shell.absolute_path(Path::new(path));
        let code = crate::tools::coreutils::run_tool(&context, |stdin, out, err| {
            super::diff::run_cmp(&argv, stdin, &resolve, out, err).unwrap_or_else(|error| {
                let _ = writeln!(err, "cmp: {}", super::io_message(&error));
                2
            })
        });
        Ok(ExecutionResult::new(code.clamp(0, 255) as u8))
    }
}

pub(crate) struct Patch;

impl Patch {
    const NAME: &'static str = "patch";
    const SYNOPSIS: &'static str = "apply a diff to one or more files";
}

impl SimpleCommand for Patch {
    fn get_content(
        name: &str,
        content_type: ContentType,
        _options: &ContentOptions,
    ) -> Result<String, Error> {
        match content_type {
            ContentType::ShortDescription => Ok(format!("{name} - {}\n", Patch::SYNOPSIS)),
            ContentType::ShortUsage => {
                Ok(format!("{name}: {name} [OPTION]... [FILE [PATCHFILE]]\n"))
            }
            ContentType::DetailedHelp => Ok(format!("{name} - {}\n", Patch::SYNOPSIS)),
            ContentType::ManPage => brush_core::error::unimp("man page not yet implemented"),
        }
    }

    #[allow(clippy::cast_sign_loss)] // code is clamped to 0..=255 before the u8 cast
    fn execute<SE, I, S>(
        context: ExecutionContext<'_, SE>,
        args: I,
    ) -> Result<ExecutionResult, Error>
    where
        SE: ShellExtensions,
        I: Iterator<Item = S>,
        S: AsRef<str>,
    {
        let argv: Vec<String> = args.map(|s| s.as_ref().to_string()).collect();
        let shell = &context.shell;
        let resolve = |path: &str| shell.absolute_path(Path::new(path));
        let code = crate::tools::coreutils::run_tool(&context, |stdin, out, err| {
            super::patch::run_patch(&argv, stdin, &resolve, out, err).unwrap_or_else(|error| {
                let _ = writeln!(err, "patch: {}", super::io_message(&error));
                2
            })
        });
        Ok(ExecutionResult::new(code.clamp(0, 255) as u8))
    }
}

pub(crate) fn builtins<SE: ShellExtensions>() -> Vec<(String, Registration<SE>)> {
    use brush_core::builtins::simple_builtin;
    #[allow(unused_mut)]
    let mut registrations: Vec<(String, Registration<SE>)> = vec![
        ("jq".into(), simple_builtin::<Jq, SE>()),
        ("grep".into(), simple_builtin::<Grep, SE>()),
        ("sed".into(), simple_builtin::<Sed, SE>()),
        ("diff".into(), simple_builtin::<Diff, SE>()),
        ("cmp".into(), simple_builtin::<Cmp, SE>()),
        ("patch".into(), simple_builtin::<Patch, SE>()),
        ("file".into(), simple_builtin::<File, SE>()),
        ("yes".into(), simple_builtin::<Yes, SE>()),
        ("seq".into(), simple_builtin::<Seq, SE>()),
        ("rev".into(), simple_builtin::<Rev, SE>()),
        ("tac".into(), simple_builtin::<Tac, SE>()),
    ];
    #[cfg(target_arch = "wasm32")]
    for (name, registration) in &mut registrations {
        match name.as_str() {
            "grep" => registration.execute_func = cooperative::grep,
            "sed" => registration.execute_func = cooperative::sed,
            "jq" => registration.execute_func = cooperative::jq,
            "diff" => *registration = super::streaming::finite_stdin_utility_builtin::<Diff, SE>(),
            "cmp" => *registration = super::streaming::finite_stdin_utility_builtin::<Cmp, SE>(),
            "patch" => {
                *registration = super::streaming::finite_stdin_utility_builtin::<Patch, SE>();
            }
            "file" => *registration = super::streaming::finite_dash_utility_builtin::<File, SE>(),
            // yes/seq/rev are unbounded producers/consumers — they get their own
            // record-at-a-time async drivers (see streaming.rs) instead of the
            // capture-then-forward `finite_utility_builtin` used above.
            "yes" => registration.execute_func = super::streaming::yes,
            "seq" => registration.execute_func = super::streaming::seq,
            "rev" => registration.execute_func = super::streaming::rev,
            // tac must see the whole input before it can emit anything, so it's finite; it also
            // reads stdin when given no FILE operand, so it needs the stdin-aware variant.
            "tac" => *registration = super::streaming::finite_stdin_utility_builtin::<Tac, SE>(),
            _ => (),
        }
        registration.execution_boundary = brush_core::builtins::ExecutionBoundary::Command;
    }
    registrations
}

#[cfg(target_arch = "wasm32")]
#[path = "streaming_text.rs"]
mod cooperative;

/// The [`Manifest`](crate::manifest::Manifest) for each text/data builtin, from the same
/// `NAME`/`SYNOPSIS` the commands expose. Names must match [`builtins`] (registry drift-guard test).
pub(crate) fn manifests() -> Vec<crate::manifest::Manifest> {
    use crate::manifest::Manifest;
    vec![
        Manifest::builtin(Jq::NAME, Jq::SYNOPSIS),
        Manifest::builtin(Grep::NAME, Grep::SYNOPSIS),
        Manifest::builtin(Sed::NAME, Sed::SYNOPSIS).with_help(
            "sed [OPTION]... {SCRIPT | -e SCRIPT | -f FILE}... [FILE]... — uutils sed: POSIX \
             commands and addresses plus GNU extensions (-E/-r, -i[SUFFIX], -s, -z, -n, 0,/re/, \
             addr,+N, first~step, I and M flags). The e command and s///e are refused: \
             bash-tool has no shell to run them.",
        ),
        Manifest::builtin(Diff::NAME, Diff::SYNOPSIS).with_help(
            "diff [-u[N]|-c[N]|-e|-y] [-q] [-s] [-N] [-r] [-i] [-w|-b] [--label TEXT]... OLD NEW \
             — GNU-style file comparison (unified/context/normal/ed/side-by-side; recursive \
             directory diff; `-` reads stdin; bundled short options like `-ruN` are accepted). \
             -i/-w/-b (ignore case/all whitespace/whitespace-amount changes) work with the \
             default, unified and context formats; combined with -e/-y they have no effect. -B \
             (ignore blank lines) is refused: not supported. -y/--side-by-side is an \
             approximate port: a changed line GNU shows as one `|`-joined row may come out here \
             as a separate deleted/inserted row pair.",
        ),
        Manifest::builtin(Cmp::NAME, Cmp::SYNOPSIS).with_help(
            "cmp [-l] [-s] FILE1 FILE2 — byte-for-byte comparison; -l lists every differing \
             byte, -s suppresses all output (exit status only).",
        ),
        Manifest::builtin(Patch::NAME, Patch::SYNOPSIS).with_help(
            "patch [-p N] [-R] [--dry-run] [-N] [-o OUT] [-s] [-E] [-i PATCHFILE] [FILE \
             [PATCHFILE]] — applies unified diffs (including multi-file `diff --git` patches) \
             from a file or stdin, with GNU's hunk offset search and .rej files on failure.",
        ),
        Manifest::builtin(File::NAME, File::SYNOPSIS),
        Manifest::builtin(Yes::NAME, Yes::SYNOPSIS),
        Manifest::builtin(Seq::NAME, Seq::SYNOPSIS).with_help(
            "seq [-s SEP] [-w] [-f FORMAT] [FIRST [INCR]] LAST — print a sequence of numbers, \
             one per line (or separated by SEP). Streams, so `seq 1 1000000000 | head -1` \
             does not materialize the whole range.",
        ),
        Manifest::builtin(Rev::NAME, Rev::SYNOPSIS),
        Manifest::builtin(Tac::NAME, Tac::SYNOPSIS).with_help(
            "tac [-b] [-s SEPARATOR] [FILE]... — print records (lines, or ended by -s's \
             SEPARATOR, NUL for an empty one; -b puts it before each) in reverse order; each \
             FILE (or stdin, with none or `-`) is reversed independently and printed in operand \
             order, like GNU tac. -r (a regex separator) is refused (exit 2).",
        ),
    ]
}

/// GNU's own usage text (`file --help`'s first block), reproduced verbatim since several error
/// cases print it.
const FILE_USAGE: &str = "Usage: file [-bcCdEhikLlNnprsSvzZ0] [--apple] [--extension] [--mime-encoding]\n            [--mime-type] [-e <testname>] [-F <separator>]  [-f <namefile>]\n            [-m <magicfiles>] [-P <parameter=value>] [--exclude-quiet]\n            <file> ...\n       file -C [-m <magicfiles>]\n       file [--help]\n";

/// `file [-bikLNsz] [--mime-type|--mime|--mime-encoding] [-F sep] [-f namefile] FILE...`: GNU
/// `file`'s wording for the common cases — text and its line terminators, scripts, empty files,
/// directories, symbolic links and a number of binary formats identifiable from a signature alone
/// — with names aligned like GNU. Anything else is `data`. There is no magic database, so binary
/// descriptions stop at what a signature (rather than a full parse) can say.
fn run_file(
    argv: &[String],
    stdin: &mut dyn std::io::Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> ToolResult<i32> {
    let mut brief = false;
    let mut mime = FileMime::None;
    let mut opts = FileOpts::default();
    let mut separator = ":".to_string();
    let mut pad = true;
    let mut names_from: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut options = true;
    let mut args = argv[1..].iter();
    let missing_arg = |err: &mut dyn Write, letter: char| -> ToolResult<i32> {
        writeln!(err, "file: option requires an argument -- '{letter}'")?;
        write!(err, "{FILE_USAGE}")?;
        Ok(1)
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" if options => options = false,
            "-b" | "--brief" if options => brief = true,
            "--mime-type" if options => mime = FileMime::Type,
            "-i" | "--mime" if options => mime = FileMime::Full,
            "--mime-encoding" if options => mime = FileMime::Encoding,
            "-L" | "--dereference" if options => opts.dereference = true,
            "-N" | "--no-pad" if options => pad = false,
            "-k" | "--keep-going" if options => opts.keep_going = true,
            "-s" | "--special-files" if options => opts.special_files = true,
            "-z" | "--uncompress" if options => opts.uncompress = true,
            "-F" if options => {
                let Some(value) = args.next() else {
                    return missing_arg(err, 'F');
                };
                separator.clone_from(value);
            }
            "-f" if options => {
                let Some(value) = args.next() else {
                    return missing_arg(err, 'f');
                };
                names_from.push(value.clone());
            }
            long if options && long.starts_with("--") && long.len() > 2 => {
                writeln!(err, "file: unrecognized option: {}", &long[2..])?;
                write!(err, "{FILE_USAGE}")?;
                return Ok(1);
            }
            flag if options && flag.starts_with('-') && flag.len() > 1 => {
                writeln!(err, "file: unrecognized option: {}", &flag[1..])?;
                write!(err, "{FILE_USAGE}")?;
                return Ok(1);
            }
            _ => names.push(arg.clone()),
        }
    }
    for source in &names_from {
        let text = if source == "-" {
            let mut bytes = Vec::new();
            stdin.read_to_end(&mut bytes)?;
            String::from_utf8_lossy(&bytes).into_owned()
        } else {
            std::fs::read_to_string(source)?
        };
        names.extend(text.lines().map(str::to_owned));
    }
    if names.is_empty() {
        write!(err, "{FILE_USAGE}")?;
        return Ok(1);
    }
    // GNU pads each name to the widest one's *display* width in terminal columns (wide
    // characters count for more than one byte or char would), not a raw length -- unless `-N`
    // disables padding, in which case nothing is measured at all.
    let columns = |name: &str| unicode_width::UnicodeWidthStr::width(name);
    let label_width = |displayed: &str| columns(displayed) + separator.len();
    let width = if pad {
        names
            .iter()
            .map(|name| {
                if name.is_empty() {
                    0
                } else {
                    label_width(name)
                }
            })
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    for name in &names {
        let description = describe_file(name, mime, &opts, stdin);
        // GNU shows a bare `-` operand's own path (`/dev/stdin`), and an empty name's
        // description alone, with no label at all (there is nothing to put before the colon).
        if brief || name.is_empty() {
            writeln!(out, "{description}")?;
        } else {
            let displayed = if name == "-" { "/dev/stdin" } else { name };
            let padding = " ".repeat(width.saturating_sub(label_width(displayed)));
            writeln!(out, "{displayed}{separator}{padding} {description}")?;
        }
    }
    // As in GNU, a file that cannot be opened is described, not treated as a failure.
    Ok(0)
}

#[derive(Clone, Copy, PartialEq)]
enum FileMime {
    None,
    Type,
    Full,
    Encoding,
}

/// Flags beyond `-b`/mime selection that change how a name is described.
#[derive(Default)]
struct FileOpts {
    /// `-L`: describe a symlink's target instead of the link itself.
    dereference: bool,
    /// `-k`: accepted; this never has more than one classification to report, so it never
    /// changes what gets printed.
    keep_going: bool,
    /// `-s`: read a block/character special file's own content (e.g. `/dev/null` is then
    /// `empty`) instead of just naming it as a device.
    special_files: bool,
    /// `-z`: accepted; only changes the answer for an archive this can't actually decompress.
    uncompress: bool,
}

fn describe_file(
    name: &str,
    mime: FileMime,
    opts: &FileOpts,
    stdin: &mut dyn std::io::Read,
) -> String {
    let path = Path::new(name);
    let pick = |description: String, mime_type: &str, charset: &str| match mime {
        FileMime::None => description,
        FileMime::Type => mime_type.to_string(),
        FileMime::Full => format!("{mime_type}; charset={charset}"),
        FileMime::Encoding => charset.to_string(),
    };
    let device = super::devices::operand(name);
    if name == "-" {
        let mut bytes = Vec::new();
        if let Err(error) = stdin.read_to_end(&mut bytes) {
            return format!("cannot read `-' ({})", super::io_message(&error));
        }
        return describe_bytes(&bytes, opts, pick);
    }
    if device == Some(super::devices::Device::Null) && !opts.special_files {
        return pick(
            "character special (1/3)".into(),
            "inode/chardevice",
            "binary",
        );
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return format!("cannot open `{name}' ({})", super::io_message(&error));
        }
    };
    if metadata.file_type().is_symlink() && !opts.dereference {
        let target =
            std::fs::read_link(path).map_or_else(|_| "?".into(), |t| t.display().to_string());
        // A dangling target is only known by trying to resolve it: GNU says so instead of
        // "symbolic link to" for exactly that case.
        let broken = std::fs::metadata(path).is_err();
        return pick(
            if broken {
                format!("broken symbolic link to {target}")
            } else {
                format!("symbolic link to {target}")
            },
            "inode/symlink",
            "binary",
        );
    }
    if !opts.dereference && metadata.is_dir() {
        return pick("directory".into(), "inode/directory", "binary");
    }
    if opts.dereference && std::fs::metadata(path).is_ok_and(|m| m.is_dir()) {
        return pick("directory".into(), "inode/directory", "binary");
    }
    match super::read_file(path) {
        Ok(bytes) => describe_bytes(&bytes, opts, pick),
        Err(error) => format!("cannot open `{name}' ({})", super::io_message(&error)),
    }
}

/// What `file` says about content, given how to pick between a description and a MIME type.
fn describe_bytes(
    bytes: &[u8],
    opts: &FileOpts,
    pick: impl Fn(String, &str, &str) -> String,
) -> String {
    if bytes.is_empty() {
        return pick("empty".into(), "inode/x-empty", "binary");
    }

    if let Some(rest) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf])
        && std::str::from_utf8(rest).is_ok()
    {
        let mut description = "Unicode text, UTF-8 (with BOM) text".to_string();
        describe_line_terminators(rest, &mut description);
        return pick(description, "text/plain", "utf-8");
    }
    if let Some(endian) = if bytes.starts_with(&[0xff, 0xfe]) {
        Some("little-endian")
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        Some("big-endian")
    } else {
        None
    } {
        let mut description = format!("Unicode text, UTF-16, {endian} text");
        if !bytes[2..].contains(&b'\n') {
            description.push_str(", with no line terminators");
        }
        let charset = if endian == "little-endian" {
            "utf-16le"
        } else {
            "utf-16be"
        };
        return pick(description, "text/plain", charset);
    }

    // ELF: the identification bytes alone (all a truncated sample has) already name the class
    // and endianness, even with nothing else to say about it.
    if let [0x7f, b'E', b'L', b'F', class, data, ..] = bytes {
        let class = match class {
            1 => Some("32-bit"),
            2 => Some("64-bit"),
            _ => None,
        };
        let data = match data {
            1 => Some("LSB"),
            2 => Some("MSB"),
            _ => None,
        };
        if let (Some(class), Some(data)) = (class, data) {
            return pick(
                format!("ELF {class} {data}"),
                "application/x-executable",
                "binary",
            );
        }
    }

    // gzip: GNU reads the trailing 4-byte ISIZE field as the *last* 4 bytes of the whole file,
    // even when that isn't really the end of a full stream (confirmed against the oracle for a
    // bare, data-less header: GNU still reports a size from it, read the same way).
    if bytes.len() >= 10 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        let os = if bytes[9] == 3 { ", from Unix" } else { "" };
        let tail = &bytes[bytes.len() - 4..];
        let size = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
        return pick(
            format!("gzip compressed data{os}, original size modulo 2^32 {size}"),
            "application/gzip",
            "binary",
        );
    }

    // PDF: the version is the digits right after "%PDF-".
    if let Some(rest) = bytes.strip_prefix(b"%PDF-") {
        let version: String = rest
            .iter()
            .take_while(|b| b.is_ascii_digit() || **b == b'.')
            .map(|&b| b as char)
            .collect();
        let description = if version.is_empty() {
            "PDF document".to_string()
        } else {
            format!("PDF document, version {version}")
        };
        return pick(description, "application/pdf", "us-ascii");
    }

    // Zip: `-z` tries to gunzip the archive and, since it isn't actually gzip, fails -- GNU's
    // own wording for exactly that takes priority over describing it as a zip at all.
    if opts.uncompress && bytes.starts_with(b"PK\x03\x04") {
        return pick(
            "ERROR:[gzip: ] (data)".into(),
            "application/octet-stream",
            "binary",
        );
    }

    // PNG: enough of an IHDR chunk to read width, height, bit depth, color type and
    // interlacing -- GNU's own description names all five.
    if let Some(header) = bytes
        .strip_prefix(b"\x89PNG\r\n\x1a\n")
        .filter(|rest| rest.len() >= 21 && &rest[4..8] == b"IHDR")
    {
        let width = u32::from_be_bytes(header[8..12].try_into().unwrap_or_default());
        let height = u32::from_be_bytes(header[12..16].try_into().unwrap_or_default());
        let bit_depth = header[16];
        let color_type = match header[17] {
            0 => "grayscale",
            2 => "RGB",
            3 => "colormap",
            4 => "gray+alpha",
            6 => "RGBA",
            _ => "unknown",
        };
        let interlaced = if header.get(20) == Some(&1) {
            "interlaced"
        } else {
            "non-interlaced"
        };
        return pick(
            format!(
                "PNG image data, {width} x {height}, {bit_depth}-bit/color {color_type}, {interlaced}"
            ),
            "image/png",
            "binary",
        );
    }

    // PNG and Zip: GNU's real magic wants more than just the bare signature (confirmed against
    // the oracle: a signature with nothing else after it does *not* match, and falls through to
    // the text/binary classification below instead) -- so a bare-signature match from `infer`
    // (a check just as shallow) is skipped the same way, rather than trusted.
    // A real zip local file header is 30 fixed bytes before its filename even starts; GNU's
    // real magic doesn't trust anything shorter as zip (confirmed against the oracle: an 8-byte
    // sample with the right signature plus a plausible "version needed"/flags still isn't
    // enough), so neither does a bare-signature match from `infer` (a check just as shallow).
    let bare_signature_only = (bytes.len() <= 8 && bytes.starts_with(b"\x89PNG\r\n\x1a\n"))
        || (bytes.len() < 30 && bytes.starts_with(b"PK\x03\x04"));

    // Everything else `infer` (a signature-only check, like the ones above) can name. Text
    // formats (`infer` knows shell scripts, for one) are described by the text path below
    // instead.
    if !bare_signature_only
        && let Some(kind) = infer::get(bytes).filter(|kind| !kind.mime_type().starts_with("text/"))
    {
        let description = binary_description(kind.mime_type()).unwrap_or("data");
        return pick(description.into(), kind.mime_type(), "binary");
    }
    // A NUL byte alone always reads as binary to GNU, valid UTF-8 or not (confirmed against the
    // oracle: a NUL among otherwise-invalid-UTF-8 bytes still gives `data`, not a text
    // classification) -- checked only here, after every signature above has already had its
    // chance (several legitimately contain NUL bytes as part of the format itself: gzip's mtime
    // field, PNG's IHDR, a UTF-16 code unit with a high byte of 0).
    if bytes.contains(&0) {
        return pick("data".into(), "application/octet-stream", "binary");
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        // Any *other* control byte reads as binary to GNU too, but only within text that is
        // otherwise valid ASCII/UTF-8 (confirmed against the oracle: plain control characters
        // like DEL still give `data`, not text) -- but this is checked only here, not for
        // content that isn't valid UTF-8 to begin with: a signature-like byte string that
        // merely happens to contain one (like PNG's own, `\x1a`) still gets a text/binary
        // classification from the rules below instead, not this one (also confirmed against
        // the oracle). libmagic's text holds no control character but BEL, BS, HT, LF, VT, FF,
        // CR and ESC -- everything else in the control range disqualifies it.
        let control = |byte: &u8| matches!(byte, 0..=6 | 0x0e..=0x1a | 0x1c..=0x1f | 0x7f);
        if bytes.iter().any(control) {
            return pick("data".into(), "application/octet-stream", "binary");
        }
        if is_json_document(text) {
            let charset = if text.is_ascii() { "us-ascii" } else { "utf-8" };
            return pick("JSON text data".into(), "application/json", charset);
        }
        let (kind, charset) = if text.is_ascii() {
            ("ASCII text", "us-ascii")
        } else {
            ("Unicode text, UTF-8 text", "utf-8")
        };
        // A script's own `#!` line names its interpreter; failing that, libmagic's own
        // Python heuristic still recognises common Python syntax with no shebang at all.
        let script =
            script_interpreter(text).or_else(|| is_python(text).then(|| "Python script".into()));
        let mut description = if let Some(named) = sniff_named_format(text) {
            format!("{named}, {kind}")
        } else if let Some(script) = &script {
            format!("{script}, {kind} executable")
        } else {
            kind.to_string()
        };
        if let Some(longest) = text.lines().map(str::chars).map(Iterator::count).max()
            && longest > 300
        {
            description.push_str(&format!(", with very long lines ({longest})"));
        }
        describe_line_terminators(text.as_bytes(), &mut description);
        let mime_type = if script.is_some_and(|s| s.contains("shell")) {
            "text/x-shellscript"
        } else {
            "text/plain"
        };
        return pick(description, mime_type, charset);
    }

    // Invalid UTF-8: GNU still calls this text when every byte is ASCII-printable, a permitted
    // control character, or in Latin-1's own printable upper half (0xA0-0xFF) -- "ISO-8859
    // text". A byte in 0x80-0x9F (Latin-1's C1 control range, never printable) instead gets
    // "Non-ISO extended-ASCII text" (confirmed against the oracle for both).
    let iso_8859 = bytes.iter().all(|&b| {
        (0x20..=0x7e).contains(&b)
            || matches!(b, b'\t' | b'\n' | b'\r')
            || (0xa0..=0xff).contains(&b)
    });
    if iso_8859 {
        let mut description = "ISO-8859 text".to_string();
        describe_line_terminators(bytes, &mut description);
        return pick(description, "text/plain", "iso-8859-1");
    }
    let mut description = "Non-ISO extended-ASCII text".to_string();
    describe_line_terminators(bytes, &mut description);
    pick(description, "text/plain", "unknown-8bit")
}

/// GNU's line-terminator clause: silent for plain `\n`, otherwise naming what else was found.
/// Takes raw bytes, not `&str`: GNU still reports this for text that isn't valid UTF-8 (Latin-1
/// or extended-ASCII), and a CR/LF byte means the same thing regardless of what surrounds it.
fn describe_line_terminators(bytes: &[u8], description: &mut String) {
    let crlf = bytes.windows(2).filter(|w| *w == b"\r\n").count();
    let lf = bytes.iter().filter(|&&b| b == b'\n').count();
    if lf == 0 {
        description.push_str(", with no line terminators");
    } else if crlf == lf {
        description.push_str(", with CRLF line terminators");
    } else if crlf > 0 {
        description.push_str(", with CRLF, LF line terminators");
    }
}

/// A named document format sniffed from its opening content, where recognising it takes more
/// than a script's `#!` line (see [`script_interpreter`]).
fn sniff_named_format(text: &str) -> Option<String> {
    let trimmed = text.trim_start();
    let starts_with_ci = |needle: &str| -> bool {
        trimmed
            .get(..needle.len().min(trimmed.len()))
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(needle))
    };
    if starts_with_ci("<?xml") {
        let version = trimmed
            .split_once("version=\"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(version, _)| version);
        return Some(match version {
            Some(version) => format!("XML {version} document"),
            None => "XML document".to_string(),
        });
    }
    if starts_with_ci("<!doctype html") || starts_with_ci("<html") {
        return Some("HTML document".to_string());
    }
    if trimmed.starts_with("#include") {
        return Some("C source".to_string());
    }
    None
}

/// Whether `text` is, as a whole, a JSON object or array document -- matching libmagic's own
/// JSON test (what real `file` uses), which requires a top-level `{` or `[`: a bare scalar
/// like `42`, `"x"`, `true` or `null` is valid JSON on its own, but real `file` still reports
/// that as plain text, not `JSON text data`. `serde_json::from_str` both validates the syntax
/// and requires the entire (trimmed) input to be consumed as that one value, so trailing
/// garbage after an otherwise-valid document (`{"a":1} extra`) is rejected the same way real
/// `file` rejects it.
fn is_json_document(text: &str) -> bool {
    match text.trim_start().as_bytes().first() {
        Some(b'{' | b'[') => {}
        _ => return false,
    }
    serde_json::from_str::<serde_json::Value>(text).is_ok()
}

/// Whether text with no `#!` line reads as Python by libmagic's own tests (its `python` magic):
/// in its first 8 KiB, a line that begins `import MODULE`, `from MODULE import`, `class NAME:`
/// or `def NAME(...):`, or a `def __init__` with `self` after it.
fn is_python(text: &str) -> bool {
    static PATTERNS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        #[allow(clippy::expect_used, reason = "a fixed pattern")]
        regex::Regex::new(
            r"(?m)^(?:import[ \t\f\r]+[A-Za-z0-9_.]+|from[ \t\f\r]+[A-Za-z0-9_.]+[ \t\f\r]+import\b|class[ \t\f\r]+[A-Za-z_]+(?:\(.*\))?[ \t\f\r]*:|def [A-Za-z_][A-Za-z_0-9]*\(.*\):)",
        )
        .expect("pattern")
    });
    let head = match text.char_indices().nth(8192) {
        Some((at, _)) => &text[..at],
        None => text,
    };
    PATTERNS.is_match(head)
        || head.find("def __init__").is_some_and(|at| {
            head[at..]
                .chars()
                .take(64 + 12)
                .collect::<String>()
                .contains("self")
        })
}

/// GNU's name for a script's interpreter, from its `#!` line.
fn script_interpreter(text: &str) -> Option<String> {
    let line = text.strip_prefix("#!")?.lines().next()?.trim();
    let mut words = line.split_whitespace();
    let program = words.next()?;
    let program = if program.ends_with("/env") {
        words.next()?
    } else {
        program
    };
    Some(match program.rsplit('/').next().unwrap_or(program) {
        "sh" => "POSIX shell script".into(),
        "bash" => "Bourne-Again shell script".into(),
        name if name.starts_with("python") => "Python script".into(),
        _ => format!("a {line} script"),
    })
}

/// GNU's description of a binary format recognised by its signature, where it is simple enough
/// to reproduce without a magic database.
fn binary_description(mime_type: &str) -> Option<&'static str> {
    Some(match mime_type {
        "application/gzip" => "gzip compressed data",
        "application/x-bzip2" => "bzip2 compressed data",
        "application/x-xz" => "XZ compressed data",
        "application/zstd" => "Zstandard compressed data",
        "application/zip" => "Zip archive data",
        "application/x-tar" => "POSIX tar archive",
        "application/pdf" => "PDF document",
        "application/wasm" => "WebAssembly (wasm) binary module",
        "application/x-executable" => "ELF executable",
        "image/png" => "PNG image data",
        "image/jpeg" => "JPEG image data",
        "image/gif" => "GIF image data",
        _ => return None,
    })
}

/// Process-wide: ignore SIGPIPE so a downstream `head`/`exit` closing its end of a native pipe
/// returns `EPIPE` from `write`, instead of killing this process with the default disposition.
/// `run_uu` (coreutils.rs) does the same for every uu_* builtin; unbounded producers written by
/// hand (`yes`, `seq`) need the same guard since they never go through `run_uu`.
#[cfg(not(target_arch = "wasm32"))]
fn ignore_sigpipe() {
    // SAFETY: a plain `libc::signal` FFI call with valid constants; returns the previous handler
    // (or SIG_ERR), never exhibiting UB.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
}

/// What `yes` does with its arguments, parsed as GNU's `yes` parses them: `--help` and
/// `--version` (or any abbreviation) are its only options, `--` ends them and is dropped, and any
/// other option is an error.
pub(crate) enum YesPlan {
    /// Repeat this line forever.
    Repeat(Vec<u8>),
    /// Print this and exit with this status; the text goes to stdout for status 0, else stderr.
    Exit(String, u8),
}

const YES_HELP: &str = "Usage: yes [STRING]...\n  or:  yes OPTION\nRepeatedly output a line \
with all specified STRING(s), or 'y'.\n\n      --help        display this help and exit\n      \
--version     output version information and exit\n";

pub(crate) fn yes_plan(args: &[String]) -> YesPlan {
    let usage = |message: String| {
        YesPlan::Exit(
            format!("yes: {message}\nTry 'yes --help' for more information.\n"),
            1,
        )
    };
    let mut operands = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            operands.extend(args.by_ref().cloned());
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, value) = long
                .split_once('=')
                .map_or((long, None), |(n, v)| (n, Some(v)));
            let option = ["help", "version"]
                .into_iter()
                .find(|option| option.starts_with(name));
            return match (option, value) {
                (None, _) => usage(format!("unrecognized option '{arg}'")),
                (Some(option), Some(_)) => {
                    usage(format!("option '--{option}' doesn't allow an argument"))
                }
                (Some("help"), None) => YesPlan::Exit(YES_HELP.into(), 0),
                (Some(_), None) => {
                    YesPlan::Exit("yes (bash-tool, GNU coreutils compatible)\n".into(), 0)
                }
            };
        }
        if let Some(letter) = arg.strip_prefix('-').and_then(|rest| rest.chars().next()) {
            return usage(format!("invalid option -- '{letter}'"));
        }
        operands.push(arg.clone());
    }
    YesPlan::Repeat(yes_line(&operands))
}

/// The line `yes` repeats: the arguments joined by spaces, or `y` with none. Shared between the
/// native driver here and the wasm streaming driver in `streaming.rs`.
pub(crate) fn yes_line(args: &[String]) -> Vec<u8> {
    let message = if args.is_empty() {
        "y".to_string()
    } else {
        args.join(" ")
    };
    let mut line = super::shell_bytes::encode(&message).into_owned();
    line.push(b'\n');
    line
}

fn run_yes(
    argv: &[String],
    _stdin: &mut dyn std::io::Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> ToolResult<i32> {
    let line = match yes_plan(&argv[1..]) {
        YesPlan::Repeat(line) => line,
        YesPlan::Exit(text, 0) => {
            out.write_all(text.as_bytes())?;
            return Ok(0);
        }
        YesPlan::Exit(text, code) => {
            err.write_all(text.as_bytes())?;
            return Ok(i32::from(code));
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    ignore_sigpipe();
    loop {
        if let Err(error) = out.write_all(&line) {
            if error.kind() == std::io::ErrorKind::BrokenPipe {
                // Matches GNU with SIGPIPE ignored: `yes` reports its own broken-pipe write
                // failure rather than silently succeeding.
                writeln!(err, "yes: standard output: Broken pipe")?;
                return Ok(1);
            }
            return Err(error.into());
        }
    }
}

/// How many digits follow the decimal point in a numeric operand's literal text — GNU `seq`
/// derives its default display precision from the arguments' own text, not from the parsed
/// `f64` (so `seq 0.50 1` prints two decimals, not one).
fn frac_digits(text: &str) -> usize {
    text.split_once('.').map_or(0, |(_, frac)| frac.len())
}

/// `-5`, `-0.3`, `.5` are numeric operands, not options; every other `-...` token is an option.
fn is_numeric_operand(text: &str) -> bool {
    let rest = text.strip_prefix('-').unwrap_or(text);
    rest.starts_with(|c: char| c.is_ascii_digit() || c == '.')
}

fn pad_numeric(plain: &str, width: usize) -> String {
    let (sign, rest) = plain
        .strip_prefix('-')
        .map_or(("", plain), |rest| ("-", rest));
    let target = width.saturating_sub(sign.len());
    if rest.len() >= target {
        format!("{sign}{rest}")
    } else {
        format!("{sign}{}{rest}", "0".repeat(target - rest.len()))
    }
}

struct FormatSpec {
    zero: bool,
    left: bool,
    plus: bool,
    width: usize,
    precision: Option<usize>,
}

fn parse_format_spec(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> FormatSpec {
    let mut zero = false;
    let mut left = false;
    let mut plus = false;
    loop {
        match chars.peek() {
            Some('0') => {
                zero = true;
                chars.next();
            }
            Some('-') => {
                left = true;
                chars.next();
            }
            Some('+') => {
                plus = true;
                chars.next();
            }
            Some(' ' | '#') => {
                chars.next();
            }
            _ => break,
        }
    }
    let mut width_text = String::new();
    while chars.peek().is_some_and(char::is_ascii_digit) {
        width_text.push(chars.next().unwrap_or_default());
    }
    let width = width_text.parse().unwrap_or(0);
    let mut precision = None;
    if chars.peek() == Some(&'.') {
        chars.next();
        let mut precision_text = String::new();
        while chars.peek().is_some_and(char::is_ascii_digit) {
            precision_text.push(chars.next().unwrap_or_default());
        }
        precision = Some(precision_text.parse().unwrap_or(0));
    }
    FormatSpec {
        zero,
        left,
        plus,
        width,
        precision,
    }
}

/// C's `%e`/`%E`: one leading digit, `precision` fractional digits, a signed 2+-digit exponent.
fn format_exp(value: f64, precision: usize, upper: bool) -> String {
    let marker = if upper { 'E' } else { 'e' };
    if value == 0.0 {
        return format!("{:.*}{marker}+00", precision, 0.0);
    }
    let mut exponent = value.abs().log10().floor() as i32;
    let mut mantissa = value / 10f64.powi(exponent);
    // Rounding the mantissa to `precision` digits can carry it to 10.0 (e.g. 9.9999996 at low
    // precision); renormalize so the printed mantissa always has exactly one leading digit. This
    // MUST be a numeric comparison — comparing the formatted strings lexicographically (as an
    // earlier version of this function did) is wrong: `"2.000000" >= "10"` is true because '2' >
    // '1', which turned `seq -f %e 1 2` into `0.200000e+01` instead of `2.000000e+00`.
    let scale = 10f64.powi(i32::try_from(precision).unwrap_or(i32::MAX));
    if (mantissa.abs() * scale).round() / scale >= 10.0 {
        mantissa /= 10.0;
        exponent += 1;
    }
    let sign = if exponent < 0 { '-' } else { '+' };
    format!(
        "{:.*}{marker}{sign}{:02}",
        precision,
        mantissa,
        exponent.abs()
    )
}

/// Strips insignificant trailing fractional zeros (and a bare trailing `.`) the way `%g` does,
/// without touching an `e`/`E` exponent suffix if there is one.
fn strip_trailing_zeros(text: &str) -> String {
    let (mantissa, suffix) = match text.find(['e', 'E']) {
        Some(index) => (&text[..index], &text[index..]),
        None => (text, ""),
    };
    if !mantissa.contains('.') {
        return format!("{mantissa}{suffix}");
    }
    let trimmed = mantissa.trim_end_matches('0').trim_end_matches('.');
    format!("{trimmed}{suffix}")
}

/// C's `%g`/`%G`: precision `P` (default 6; 0 means 1) counts *significant* digits, not
/// fractional ones. Round to `P` significant digits to find the decimal exponent `X` that will
/// actually be printed, then use `%f` with precision `P-1-X` when `-4 <= X < P`, else `%e` with
/// precision `P-1` — and strip trailing zeros either way. Reusing `format_exp` to find `X` keeps
/// the exponent/carry logic in exactly one place.
fn format_general(value: f64, precision: Option<usize>, upper: bool) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    let significant = precision.map_or(6, |value| value.max(1));
    let exp_form = format_exp(value, significant - 1, false);
    let exponent: i32 = exp_form
        .rsplit_once('e')
        .and_then(|(_, exp)| exp.parse().ok())
        .unwrap_or(0);
    let body = if exponent >= -4 && exponent < i32::try_from(significant).unwrap_or(i32::MAX) {
        let fixed_precision =
            usize::try_from(i32::try_from(significant).unwrap_or(0) - 1 - exponent).unwrap_or(0);
        format!("{value:.fixed_precision$}")
    } else {
        format_exp(value, significant - 1, upper)
    };
    strip_trailing_zeros(&body)
}

fn render_format_conversion(spec: &FormatSpec, conversion: char, value: f64) -> String {
    let precision = spec.precision.unwrap_or(6);
    let mut body = match conversion {
        'f' | 'F' => format!("{value:.precision$}"),
        'e' | 'E' => format_exp(value, precision, conversion == 'E'),
        _ => format_general(value, spec.precision, conversion == 'G'),
    };
    if spec.plus && value >= 0.0 && !body.starts_with('+') {
        body = format!("+{body}");
    }
    if body.len() < spec.width {
        let pad = spec.width - body.len();
        if spec.left {
            body.push_str(&" ".repeat(pad));
        } else if spec.zero {
            let (sign, rest) = body
                .strip_prefix(['-', '+'])
                .map_or(("", body.as_str()), |rest| (&body[..1], rest));
            body = format!("{sign}{}{rest}", "0".repeat(pad));
        } else {
            body = format!("{}{body}", " ".repeat(pad));
        }
    }
    body
}

/// Whether `format` contains the one `%...(f|F|e|E|g|G)` conversion `seq -f` requires. GNU `seq`
/// refuses a format with none (`seq -f x 1 2` -> `seq: format 'x' has no % directive`).
fn seq_format_has_directive(format: &str) -> bool {
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            continue;
        }
        let _ = parse_format_spec(&mut chars);
        if matches!(chars.next(), Some('f' | 'F' | 'e' | 'E' | 'g' | 'G')) {
            return true;
        }
    }
    false
}

/// `seq -f FORMAT`: one `%[flags][width][.precision](f|F|e|E|g|G)` conversion embedded in literal
/// text, plus `%%`. A second conversion (which real `seq` also rejects) is left as a literal `%`.
fn apply_seq_format(format: &str, value: f64) -> String {
    let mut out = String::new();
    let mut chars = format.chars().peekable();
    let mut applied = false;
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            out.push('%');
            continue;
        }
        if applied {
            out.push('%');
            continue;
        }
        let spec = parse_format_spec(&mut chars);
        match chars.next() {
            Some(conversion @ ('f' | 'F' | 'e' | 'E' | 'g' | 'G')) => {
                out.push_str(&render_format_conversion(&spec, conversion, value));
                applied = true;
            }
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Formats a `seq` diagnostic exactly like GNU: `seq: <reason>\nTry 'seq --help' for more
/// information.\n`. `reason` already carries GNU's curly quotes (`‘…’`) around any offending
/// argument text, e.g. `invalid floating point argument: ‘x’`.
fn seq_usage_error(reason: String) -> String {
    format!("seq: {reason}\nTry 'seq --help' for more information.\n")
}

/// The number of terms `seq FIRST [INCR] LAST` produces, matching GNU exactly including at the
/// edges float arithmetic gets wrong: `seq 1 inf` (an unbounded ascending run, stopped only by
/// whatever is reading it — see `streaming::seq_impl`), `seq 1 1e20` (huge but finite), and
/// `seq 1 10000000000 30000000000` (large integers, where `(last - first) / incr` in f64 loses
/// enough precision to overshoot the true end by one term).
///
/// When every operand's *original text* parses as a plain base-10 integer (no `.`, exponent, or
/// `inf`/`nan`), the count is computed with exact `i128` arithmetic instead, sidestepping float
/// precision entirely — this is the common case and the one the overshoot bug above hits.
fn seq_term_count(
    first_text: &str,
    incr_text: Option<&str>,
    last_text: &str,
    first: f64,
    incr: f64,
    last: f64,
) -> u64 {
    let as_int = |text: &str| text.trim().parse::<i128>().ok();
    if let (Some(first_i), Some(incr_i), Some(last_i)) = (
        as_int(first_text),
        incr_text.map_or(Some(1), as_int),
        as_int(last_text),
    ) {
        let diff = last_i - first_i;
        // Integer division truncates toward zero, which is `floor` whenever the quotient
        // is non-negative -- and a negative quotient means the range is empty (count 0).
        return if diff.signum() != 0 && diff.signum() != incr_i.signum() {
            0
        } else {
            let steps = diff / incr_i;
            u64::try_from(steps).map_or(u64::MAX, |steps| steps.saturating_add(1))
        };
    }
    let raw_steps = (last - first) / incr;
    if raw_steps.is_nan() {
        return 0;
    }
    if raw_steps.is_infinite() {
        // An unbounded run in the ascending or descending direction requested; `seq_impl`
        // stops as soon as whatever is reading stdout stops reading, exactly like `yes`.
        return if raw_steps > 0.0 { u64::MAX } else { 0 };
    }
    if raw_steps < 0.0 {
        return 0;
    }
    // A relative epsilon: an absolute `1e-9` fudge (the naive fix) still overshoots once
    // the magnitudes involved exceed roughly 1e9, which is exactly what GNU's own large
    // integer tests exercise.
    let epsilon = (raw_steps.abs().max(1.0)) * f64::EPSILON * 8.0;
    let terms = (raw_steps + epsilon).floor();
    // `terms as u64` alone saturates correctly (Rust's float-to-int casts always saturate),
    // but adding the final `+ 1` as plain `u64` arithmetic does not: in a release build,
    // integer overflow checks are off, so `u64::MAX + 1` silently wraps to 0 -- turning a
    // huge-but-finite `seq 1 1e20` into a range that prints nothing at all instead of
    // streaming until whatever reads it stops (see `seq_impl`).
    if terms >= u64::MAX as f64 {
        u64::MAX
    } else {
        terms as u64 + 1
    }
}

/// The `FIRST [INCR] LAST` arithmetic progression a `seq` invocation describes, plus enough of
/// the original argument text (`-w`/`-f`/`-s` and each operand's own digit count) to reproduce
/// GNU `seq`'s formatting. Shared, pure, and side-effect free, so both the native driver here and
/// the wasm streaming driver (`streaming::seq`) render byte-identical output.
pub(crate) struct SeqPlan {
    first: f64,
    incr: f64,
    /// Number of terms; the loop bound both drivers iterate `0..count` over.
    pub(crate) count: u64,
    precision: usize,
    equal_width: bool,
    pad_width: usize,
    format: Option<String>,
    separator: Vec<u8>,
}

impl SeqPlan {
    pub(crate) fn parse(args: &[String]) -> Result<Self, String> {
        let mut separator = vec![b'\n'];
        let mut equal_width = false;
        let mut format = None;
        let mut operands = Vec::new();
        let mut no_more_flags = false;
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            if no_more_flags || !arg.starts_with('-') || arg == "-" || is_numeric_operand(arg) {
                operands.push(arg.clone());
                continue;
            }
            match arg.as_str() {
                "--" => no_more_flags = true,
                "-w" | "--equal-width" => equal_width = true,
                "-s" | "--separator" => {
                    let text = it
                        .next()
                        .ok_or_else(|| "seq: option '-s' requires an argument\n".to_string())?;
                    separator = super::shell_bytes::encode(text).into_owned();
                }
                "-f" | "--format" => {
                    format = Some(
                        it.next()
                            .ok_or_else(|| "seq: option '-f' requires an argument\n".to_string())?
                            .clone(),
                    );
                }
                _ if arg.starts_with("--separator=") => {
                    separator =
                        super::shell_bytes::encode(&arg["--separator=".len()..]).into_owned();
                }
                _ if arg.starts_with("--format=") => {
                    format = Some(arg["--format=".len()..].to_string());
                }
                _ if arg.starts_with("-s") => {
                    separator = super::shell_bytes::encode(&arg[2..]).into_owned();
                }
                _ if arg.starts_with("-f") => format = Some(arg[2..].to_string()),
                other => return Err(format!("seq: {other}: unsupported option in bash-tool\n")),
            }
        }
        if operands.is_empty() {
            return Err(seq_usage_error("missing operand".to_string()));
        }
        if operands.len() > 3 {
            return Err(seq_usage_error(format!(
                "extra operand \u{2018}{}\u{2019}",
                operands[3]
            )));
        }
        let parse_num = |text: &str| {
            let trimmed = text.trim();
            let lower = trimmed.trim_start_matches(['+', '-']).to_ascii_lowercase();
            if lower == "nan" {
                return Err(seq_usage_error(format!(
                    "invalid \u{2018}not-a-number\u{2019} argument: \u{2018}{text}\u{2019}"
                )));
            }
            trimmed.parse::<f64>().map_err(|_| {
                seq_usage_error(format!(
                    "invalid floating point argument: \u{2018}{text}\u{2019}"
                ))
            })
        };
        let (first_text, incr_text, last_text) = match operands.len() {
            1 => ("1".to_string(), None, operands[0].clone()),
            2 => (operands[0].clone(), None, operands[1].clone()),
            _ => (
                operands[0].clone(),
                Some(operands[1].clone()),
                operands[2].clone(),
            ),
        };
        let first = parse_num(&first_text)?;
        let incr = match &incr_text {
            Some(text) => parse_num(text)?,
            None => 1.0,
        };
        let last = parse_num(&last_text)?;
        if incr == 0.0 {
            return Err(seq_usage_error(format!(
                "invalid Zero increment value: \u{2018}{}\u{2019}",
                incr_text.as_deref().unwrap_or("0")
            )));
        }
        if let Some(format) = &format
            && !seq_format_has_directive(format)
        {
            return Err(format!(
                "seq: format \u{2018}{format}\u{2019} has no % directive\n"
            ));
        }
        let precision = if format.is_some() {
            0
        } else {
            [
                Some(first_text.as_str()),
                incr_text.as_deref(),
                Some(last_text.as_str()),
            ]
            .into_iter()
            .flatten()
            .map(frac_digits)
            .max()
            .unwrap_or(0)
        };
        let count = seq_term_count(
            &first_text,
            incr_text.as_deref(),
            &last_text,
            first,
            incr,
            last,
        );
        let mut plan = Self {
            first,
            incr,
            count,
            precision,
            equal_width,
            pad_width: 0,
            format,
            separator,
        };
        if plan.equal_width && plan.format.is_none() && plan.count > 0 {
            let first_width = plan.plain(0).len();
            let last_width = plan.plain(plan.count - 1).len();
            plan.pad_width = first_width.max(last_width);
        }
        Ok(plan)
    }

    fn value(&self, index: u64) -> f64 {
        let value = self.first + self.incr * index as f64;
        // Normalize -0.0 so it prints as "0", matching GNU seq.
        if value == 0.0 { 0.0 } else { value }
    }

    fn plain(&self, index: u64) -> String {
        format!("{:.*}", self.precision, self.value(index))
    }

    fn formatted(&self, index: u64) -> String {
        if let Some(format) = &self.format {
            return apply_seq_format(format, self.value(index));
        }
        let plain = self.plain(index);
        if self.equal_width && self.pad_width > 0 {
            pad_numeric(&plain, self.pad_width)
        } else {
            plain
        }
    }

    /// One rendered term, including its trailing separator (or a final `\n` for the last term).
    pub(crate) fn render(&self, index: u64) -> Vec<u8> {
        // A format's own text stands for bytes, as every argument does.
        let mut line = super::shell_bytes::encode(&self.formatted(index)).into_owned();
        if index + 1 < self.count {
            line.extend_from_slice(&self.separator);
        } else {
            line.push(b'\n');
        }
        line
    }
}

fn run_seq(
    argv: &[String],
    _stdin: &mut dyn std::io::Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> ToolResult<i32> {
    let plan = match SeqPlan::parse(&argv[1..]) {
        Ok(plan) => plan,
        Err(message) => {
            // `message` is already a complete, GNU-formatted diagnostic (see `seq_usage_error`),
            // trailing newline(s) included — do not re-wrap it in another "seq: " prefix.
            write!(err, "{message}")?;
            return Ok(1);
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    ignore_sigpipe();
    // `out` is Brush's raw `OpenFile` writer (unbuffered: each `write_all` is its own syscall or
    // IPC round trip through the cooperative shell). A term at a time through that would make
    // `seq N > file` do N of them; buffer locally and flush once at the end instead.
    let mut out = std::io::BufWriter::new(out);
    let broken_pipe = |error: &std::io::Error| error.kind() == std::io::ErrorKind::BrokenPipe;
    for index in 0..plan.count {
        if let Err(error) = out.write_all(&plan.render(index)) {
            if broken_pipe(&error) {
                // Matches GNU with SIGPIPE ignored.
                writeln!(err, "seq: write error: Broken pipe")?;
                return Ok(1);
            }
            return Err(error.into());
        }
    }
    if let Err(error) = out.flush() {
        if broken_pipe(&error) {
            writeln!(err, "seq: write error: Broken pipe")?;
            return Ok(1);
        }
        return Err(error.into());
    }
    Ok(0)
}

/// Splits `input` into records that each keep their trailing `\n` — except a final, unterminated
/// record, which keeps none. Shared by `tac` (whole-buffer reversal) and `rev` (per-record
/// reversal).
fn split_records(input: &[u8]) -> Vec<&[u8]> {
    let mut records = Vec::new();
    let mut start = 0;
    for (index, &byte) in input.iter().enumerate() {
        if byte == b'\n' {
            records.push(&input[start..=index]);
            start = index + 1;
        }
    }
    if start < input.len() {
        records.push(&input[start..]);
    }
    records
}

/// Reverses the characters of one record (a line, with or without its trailing `\n`). A record
/// that is not UTF-8 is written unchanged, as util-linux `rev` writes a line it cannot decode.
/// Shared with the wasm streaming driver in `streaming.rs`.
pub(crate) fn rev_line(record: &[u8]) -> Vec<u8> {
    let (content, newline) = match record.split_last() {
        Some((b'\n', rest)) => (rest, true),
        _ => (record, false),
    };
    let mut reversed = match std::str::from_utf8(content) {
        Ok(text) => {
            let mut bytes = Vec::with_capacity(content.len());
            let mut buffer = [0_u8; 4];
            for character in text.chars().rev() {
                bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            }
            bytes
        }
        Err(_) => content.to_vec(),
    };
    if newline {
        reversed.push(b'\n');
    }
    reversed
}

/// The oracle's own `rev` is BusyBox's, not GNU's (it isn't one of the tools the oracle image
/// overrides with a GNU build) — its usage banner is a fixed string baked into that specific
/// build, reproduced verbatim, not something to compute; verified against the oracle for
/// `--help`, an unrecognized long option and an unrecognized short one (BusyBox `rev` doesn't
/// implement any real option, `--help` included — `--help` itself only prints this banner and
/// exits 0, everything else, `--version`/`-h` included, is "unrecognized option").
const REV_BUSYBOX_BANNER: &str = "BusyBox v1.37.0 (2026-01-10 15:38:28 UTC) multi-call binary.\n\nUsage: rev [FILE]...\n\nReverse lines of FILE\n";

fn run_rev(
    argv: &[String],
    stdin: &mut dyn std::io::Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> ToolResult<i32> {
    let mut files: Vec<&str> = Vec::new();
    let mut end_of_opts = false;
    for arg in &argv[1..] {
        if end_of_opts || arg == "-" || !arg.starts_with('-') {
            files.push(arg);
        } else if arg == "--" {
            end_of_opts = true;
        } else if arg == "--help" {
            err.write_all(REV_BUSYBOX_BANNER.as_bytes())?;
            return Ok(0);
        } else if let Some(long) = arg.strip_prefix("--") {
            writeln!(err, "rev: unrecognized option: {long}")?;
            err.write_all(REV_BUSYBOX_BANNER.as_bytes())?;
            return Ok(1);
        } else {
            // A short-option bundle: BusyBox names the first character only.
            let c = arg[1..].chars().next().unwrap_or('-');
            writeln!(err, "rev: unrecognized option: {c}")?;
            err.write_all(REV_BUSYBOX_BANNER.as_bytes())?;
            return Ok(1);
        }
    }
    let mut failed = false;
    let process = |reader: &mut dyn std::io::Read, out: &mut dyn Write| -> std::io::Result<()> {
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;
        for record in split_records(&buffer) {
            out.write_all(&rev_line(record))?;
        }
        Ok(())
    };
    if files.is_empty() {
        process(stdin, out)?;
    } else {
        for path in files {
            if super::devices::operand(path) == Some(super::devices::Device::Stream(0)) {
                process(stdin, out)?;
                continue;
            }
            match std::fs::File::open(path) {
                Ok(mut file) => process(&mut file, out)?,
                Err(error) => {
                    failed = true;
                    writeln!(err, "rev: {path}: {}", super::io_message(&error))?;
                }
            }
        }
    }
    Ok(i32::from(failed))
}

/// Reverses whole-buffer record order (the default `tac` mode: last line first). Shared with the
/// wasm streaming driver via the same native `Tac` command (`tac` is finite either way — it must
/// see EOF before it can emit its first output line).
pub(crate) fn tac_bytes(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    for record in split_records(input).into_iter().rev() {
        out.extend_from_slice(record);
    }
    out
}

/// `-s SEP`/`--separator=SEP` (a fixed string, not `-r`'s regex, which this fork doesn't
/// implement) and `-b`/`--before` (the separator attaches to the record it precedes, not the
/// one it follows). Splits `input` into `n+1` pieces around `n` occurrences of `sep`, reverses
/// the *order* of the pieces, and reattaches each occurrence to whichever side GNU says it
/// belongs on — verified against the oracle for both modes, including their interaction with a
/// missing trailing separator. An empty `sep` is the NUL byte, as in GNU tac (verified against
/// the oracle): input with no NUL comes back unchanged.
pub(crate) fn tac_bytes_with_separator(input: &[u8], sep: &[u8], before: bool) -> Vec<u8> {
    let sep: &[u8] = if sep.is_empty() { b"\0" } else { sep };
    let mut pieces: Vec<&[u8]> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + sep.len() <= input.len() {
        if &input[i..i + sep.len()] == sep {
            pieces.push(&input[start..i]);
            i += sep.len();
            start = i;
        } else {
            i += 1;
        }
    }
    pieces.push(&input[start..]);

    let mut out = Vec::with_capacity(input.len());
    let last = pieces.len() - 1;
    for (index, piece) in pieces.into_iter().enumerate().rev() {
        if before {
            // Every piece but the first (now last, after reversing) carries the separator that
            // preceded it in the original order.
            if index > 0 {
                out.extend_from_slice(sep);
            }
            out.extend_from_slice(piece);
        } else {
            out.extend_from_slice(piece);
            // Every piece but the last (now first, after reversing) carries the separator that
            // followed it in the original order.
            if index < last {
                out.extend_from_slice(sep);
            }
        }
    }
    out
}

fn run_tac(
    argv: &[String],
    stdin: &mut dyn std::io::Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> ToolResult<i32> {
    let args = &argv[1..];
    let mut separator: Option<Vec<u8>> = None;
    let mut before = false;
    let mut positionals: Vec<&String> = Vec::new();
    let mut end_of_opts = false;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if end_of_opts || a == "-" || !a.starts_with('-') {
            positionals.push(a);
        } else if a == "--" {
            end_of_opts = true;
        } else if a == "--help" {
            out.write_all(
                b"Usage: tac [OPTION]... [FILE]...\n\
                  Write each FILE to standard output, last line first.\n",
            )?;
            return Ok(0);
        } else if a == "--before" {
            before = true;
        } else if let Some(value) = a.strip_prefix("--separator=") {
            separator = Some(super::shell_bytes::encode(value).into_owned());
        } else if a == "--separator" {
            i += 1;
            let Some(value) = args.get(i) else {
                writeln!(err, "tac: option '--separator' requires an argument")?;
                writeln!(err, "Try 'tac --help' for more information.")?;
                return Ok(1);
            };
            separator = Some(super::shell_bytes::encode(value).into_owned());
        } else if a == "--regex" || a == "-r" {
            writeln!(
                err,
                "tac: -r/--regex is unsupported in bash-tool (only a fixed-string separator is implemented)"
            )?;
            return Ok(2);
        } else if a.starts_with("--") {
            // Any other `--xxx` is a long option this fork (like GNU) doesn't have at all — not
            // a short-option bundle, which the generic `-` branch below would otherwise misread
            // its second `-` as the start of.
            writeln!(err, "tac: unrecognized option '{a}'")?;
            writeln!(err, "Try 'tac --help' for more information.")?;
            return Ok(1);
        } else if let Some(rest) = a.strip_prefix('-') {
            // Bundled short options: `-b`/`-s SEP` (or `-sSEP` attached), getopt-style.
            let bytes = rest.as_bytes();
            let mut ci = 0;
            let mut consumed_rest = false;
            while ci < bytes.len() {
                match bytes[ci] as char {
                    'b' => before = true,
                    's' => {
                        let value = if ci + 1 < bytes.len() {
                            // The letters before it are ASCII, so this is a character boundary.
                            super::shell_bytes::encode(&rest[ci + 1..]).into_owned()
                        } else {
                            i += 1;
                            let Some(value) = args.get(i) else {
                                writeln!(err, "tac: option requires an argument -- 's'")?;
                                writeln!(err, "Try 'tac --help' for more information.")?;
                                return Ok(1);
                            };
                            super::shell_bytes::encode(value).into_owned()
                        };
                        separator = Some(value);
                        consumed_rest = true;
                    }
                    c => {
                        writeln!(err, "tac: invalid option -- '{c}'")?;
                        writeln!(err, "Try 'tac --help' for more information.")?;
                        return Ok(1);
                    }
                }
                if consumed_rest {
                    break;
                }
                ci += 1;
            }
        }
        i += 1;
    }
    let args = &positionals;
    // GNU `tac FILE...` reverses each file independently and prints them in operand order — it
    // does NOT concatenate the files first and reverse the whole thing. No operand (or a bare
    // `-`) means stdin.
    let paths: Vec<&String> = if args.is_empty() {
        vec![&argv[0]]
    } else {
        args.to_vec()
    };
    // `-s`/`-b` need the whole buffer (the separator can be any string, so the block-at-a-time
    // backward reader `tac_file` does for the plain `\n` case doesn't generalize) — bypassing
    // it changes nothing observable for a file small enough for any of these cases to exist.
    let custom = separator.is_some() || before;
    let sep = separator.unwrap_or_else(|| b"\n".to_vec());
    let mut failed = false;
    for path in paths {
        // No operand and `-` read standard input. A file there (`tac < f`) is read backwards like
        // any other; a pipe's input is already in memory. On WASM a name for it such as
        // `/dev/stdin` opens like any other path (see `devices`): a file opens again from its
        // start, as on Linux.
        if args.is_empty()
            || path == "-"
            || (cfg!(not(target_arch = "wasm32"))
                && super::devices::operand(path) == Some(super::devices::Device::Stream(0)))
        {
            match seekable_stdin() {
                Some(mut file) if !custom => match tac_file(&mut file, out) {
                    Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => {
                        // Matches the named-file branch below, but with GNU's fixed name for
                        // stdin (`'standard input'`, straight-quoted, not the curly quoting a
                        // real path gets) -- verified against the oracle: a directory
                        // redirected onto stdin previously leaked a raw, unwrapped
                        // `(os error N)` here instead of this wrapped message.
                        failed = true;
                        writeln!(
                            err,
                            "tac: 'standard input': read error: {}",
                            super::io_message(&error)
                        )?;
                    }
                    result => result?,
                },
                Some(mut file) => {
                    use std::io::Read as _;
                    let mut buffer = Vec::new();
                    match file.read_to_end(&mut buffer) {
                        Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => {
                            failed = true;
                            writeln!(
                                err,
                                "tac: 'standard input': read error: {}",
                                super::io_message(&error)
                            )?;
                        }
                        result => {
                            result?;
                            out.write_all(&tac_bytes_with_separator(&buffer, &sep, before))?;
                        }
                    }
                }
                None => {
                    let mut buffer = Vec::new();
                    stdin.read_to_end(&mut buffer)?;
                    if custom {
                        out.write_all(&tac_bytes_with_separator(&buffer, &sep, before))?;
                    } else {
                        out.write_all(&tac_bytes(&buffer))?;
                    }
                }
            }
            continue;
        }
        match std::fs::File::open(path) {
            Ok(mut file) if custom => {
                use std::io::Read as _;
                let mut buffer = Vec::new();
                match file.read_to_end(&mut buffer) {
                    Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => {
                        failed = true;
                        writeln!(
                            err,
                            "tac: {path}: read error: {}",
                            super::io_message(&error)
                        )?;
                    }
                    result => {
                        result?;
                        out.write_all(&tac_bytes_with_separator(&buffer, &sep, before))?;
                    }
                }
            }
            // `tac_opened` (not the plain `tac_file`): a named operand can be a non-seekable
            // pipe too (process substitution, `/dev/fd/N`), and only `tac_opened`'s own ESPIPE
            // check falls back to reading it forward instead of trying to seek it.
            Ok(mut file) => match tac_opened(&mut file, out) {
                Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => {
                    failed = true;
                    writeln!(
                        err,
                        "tac: {path}: read error: {}",
                        super::io_message(&error)
                    )?;
                }
                result => result?,
            },
            Err(error) => {
                failed = true;
                writeln!(
                    err,
                    "tac: failed to open '{path}' for reading: {}",
                    super::io_message(&error)
                )?;
            }
        }
    }
    Ok(i32::from(failed))
}

/// [`tac_file`] for a file that seeks. One that does not, such as a pipe opened as `/dev/stdin`
/// or `/dev/fd/N`, is read to its end first.
fn tac_opened(file: &mut std::fs::File, out: &mut dyn Write) -> std::io::Result<()> {
    match std::io::Seek::stream_position(file) {
        Err(error) if error.raw_os_error() == Some(libc::ESPIPE) => {
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer)?;
            out.write_all(&tac_bytes(&buffer))
        }
        _ => tac_file(file, out),
    }
}

/// Standard input as a file tac can read backwards, when it is one: on WASM a served command's
/// descriptor 0 seeks only when a regular file is behind it (see `devices`).
#[cfg(target_arch = "wasm32")]
#[allow(unsafe_code, reason = "borrows descriptor 0 without ever closing it")]
fn seekable_stdin() -> Option<std::mem::ManuallyDrop<std::fs::File>> {
    use std::io::Seek;
    use std::os::fd::FromRawFd;
    // SAFETY: descriptor 0 stays open for the call; `ManuallyDrop` never closes it.
    let mut file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(0) });
    file.stream_position().ok().map(|_| file)
}

#[cfg(not(target_arch = "wasm32"))]
fn seekable_stdin() -> Option<std::mem::ManuallyDrop<std::fs::File>> {
    None
}

/// Writes the records of `file` last first, reading it backwards in blocks as GNU tac does, so
/// memory holds a block and the longest line rather than the file. Always reads the *whole*
/// file from byte 0, regardless of where the descriptor's own position already was — verified
/// against the oracle: `{ read -r first; tac -; } < file` (the shell's own `read` has already
/// advanced the position past the first line) still tac's every line, the first included, not
/// just the ones after wherever `read` left off.
fn tac_file(file: &mut std::fs::File, out: &mut dyn Write) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom};
    let start = 0u64;
    let mut position = file.seek(SeekFrom::End(0))?;
    let mut block_size: u64 = 64 * 1024;
    // The bytes after `position` not yet written: a record whose start is still unread.
    let mut carry: Vec<u8> = Vec::new();
    while position > start {
        let count = block_size.min(position - start);
        position -= count;
        file.seek(SeekFrom::Start(position))?;
        let mut block = vec![0; usize::try_from(count).unwrap_or(usize::MAX)];
        file.read_exact(&mut block)?;
        block.extend_from_slice(&carry);
        // Each record ends with its newline; one starts after the newline before it.
        let mut end = block.len();
        while let Some(newline) = block[..end.saturating_sub(1)]
            .iter()
            .rposition(|&byte| byte == b'\n')
        {
            out.write_all(&block[newline + 1..end])?;
            end = newline + 1;
        }
        if end == block.len() {
            // A record longer than the block: read further back in larger steps.
            block_size = block_size.saturating_mul(2);
        }
        block.truncate(end);
        carry = block;
    }
    out.write_all(&carry)
}

#[cfg(test)]
mod m7_tests {
    use super::*;

    #[test]
    fn yes_parses_options_as_gnu_does() {
        let plan =
            |args: &[&str]| yes_plan(&args.iter().map(ToString::to_string).collect::<Vec<_>>());
        let line = |args: &[&str]| match plan(args) {
            YesPlan::Repeat(line) => String::from_utf8(line).unwrap(),
            YesPlan::Exit(text, code) => format!("exit {code}: {text}"),
        };
        assert_eq!(line(&["--", "-a"]), "-a\n");
        assert_eq!(line(&["a", "--", "b"]), "a b\n");
        assert_eq!(line(&["-"]), "-\n");
        assert_eq!(line(&["--"]), "y\n");
        assert!(line(&["--ver"]).starts_with("exit 0: yes (bash-tool"));
        assert!(line(&["--help"]).starts_with("exit 0: Usage: yes [STRING]..."));
        assert_eq!(
            line(&["-a"]),
            "exit 1: yes: invalid option -- 'a'\nTry 'yes --help' for more information.\n"
        );
        assert_eq!(
            line(&["x", "--bogus"]),
            "exit 1: yes: unrecognized option '--bogus'\nTry 'yes --help' for more information.\n"
        );
        assert_eq!(
            line(&["--help=1"]),
            "exit 1: yes: option '--help' doesn't allow an argument\nTry 'yes --help' for more \
             information.\n"
        );
    }

    #[test]
    fn yes_repeats_args_joined_by_spaces_or_y_alone() {
        assert_eq!(yes_line(&[]), b"y\n");
        assert_eq!(yes_line(&["a".into(), "b".into(), "c".into()]), b"a b c\n");
    }

    #[test]
    fn seq_default_integer_range() {
        let plan = SeqPlan::parse(&["1".into(), "5".into()]).unwrap();
        let out: Vec<u8> = (0..plan.count).flat_map(|i| plan.render(i)).collect();
        assert_eq!(out, b"1\n2\n3\n4\n5\n");
    }

    #[test]
    fn seq_single_arg_is_the_last_value() {
        let plan = SeqPlan::parse(&["3".into()]).unwrap();
        let out: Vec<u8> = (0..plan.count).flat_map(|i| plan.render(i)).collect();
        assert_eq!(out, b"1\n2\n3\n");
    }

    #[test]
    fn seq_decimal_increment_pads_to_the_widest_fraction() {
        let plan = SeqPlan::parse(&["1".into(), "0.25".into(), "2".into()]).unwrap();
        let out: Vec<u8> = (0..plan.count).flat_map(|i| plan.render(i)).collect();
        assert_eq!(out, b"1.00\n1.25\n1.50\n1.75\n2.00\n");
    }

    #[test]
    fn seq_descending_range() {
        let plan = SeqPlan::parse(&["5".into(), "-1".into(), "1".into()]).unwrap();
        let out: Vec<u8> = (0..plan.count).flat_map(|i| plan.render(i)).collect();
        assert_eq!(out, b"5\n4\n3\n2\n1\n");
    }

    #[test]
    fn seq_unreachable_range_prints_nothing() {
        let plan = SeqPlan::parse(&["5".into(), "1".into()]).unwrap();
        assert_eq!(plan.count, 0);
    }

    #[test]
    fn seq_separator_only_between_terms() {
        let plan = SeqPlan::parse(&["-s".into(), ",".into(), "1".into(), "3".into()]).unwrap();
        let out: Vec<u8> = (0..plan.count).flat_map(|i| plan.render(i)).collect();
        assert_eq!(out, b"1,2,3\n");
    }

    #[test]
    fn seq_equal_width_pads_only_up_to_the_widest_endpoint() {
        let plan = SeqPlan::parse(&["-w".into(), "-5".into(), "5".into()]).unwrap();
        let out: String = (0..plan.count)
            .map(|i| String::from_utf8(plan.render(i)).unwrap())
            .collect();
        assert_eq!(out, "-5\n-4\n-3\n-2\n-1\n00\n01\n02\n03\n04\n05\n");
    }

    #[test]
    fn seq_format_overrides_default_rendering() {
        let plan = SeqPlan::parse(&["-f".into(), "%.2f".into(), "1".into(), "3".into()]).unwrap();
        let out: Vec<u8> = (0..plan.count).flat_map(|i| plan.render(i)).collect();
        assert_eq!(out, b"1.00\n2.00\n3.00\n");
    }

    #[test]
    fn seq_billion_range_never_materializes_up_front() {
        // The guarantee this asserts on is O(1) construction, not the loop itself — a matrix
        // case (`seq 1 1000000000 | head -1`) exercises the actual streaming/backpressure path.
        let plan = SeqPlan::parse(&["1".into(), "1000000000".into()]).unwrap();
        assert_eq!(plan.count, 1_000_000_000);
    }

    #[test]
    fn rev_reverses_utf8_characters_not_bytes() {
        assert_eq!(rev_line(b"hello\n"), b"olleh\n");
        assert_eq!(rev_line("héllo\n".as_bytes()), "olléh\n".as_bytes());
        // No trailing newline (last, unterminated line) — none is added back.
        assert_eq!(rev_line(b"tail"), b"liat");
        // A line that is not UTF-8 comes out unchanged, as util-linux writes it.
        assert_eq!(rev_line(b"ab\xffcd\n"), b"ab\xffcd\n");
    }

    #[test]
    fn tac_reverses_line_order_and_keeps_a_missing_final_newline_first() {
        assert_eq!(tac_bytes(b"a\nb\nc\n"), b"c\nb\na\n");
        // "c" has no trailing newline; reversed, it fuses onto the front of the next record.
        assert_eq!(tac_bytes(b"a\nb\nc"), b"cb\na\n");
    }

    // -- broken-pipe status, seq -f, seq diagnostics, rev/tac files ----

    #[test]
    fn seq_diagnostics_match_gnu_exactly_curly_quotes_included() {
        assert_eq!(
            SeqPlan::parse(&["x".into()]).err().unwrap(),
            "seq: invalid floating point argument: \u{2018}x\u{2019}\nTry 'seq --help' for more information.\n"
        );
        assert_eq!(
            SeqPlan::parse(&["1".into(), "0".into(), "3".into()])
                .err()
                .unwrap(),
            "seq: invalid Zero increment value: \u{2018}0\u{2019}\nTry 'seq --help' for more information.\n"
        );
        assert_eq!(
            SeqPlan::parse(&[]).err().unwrap(),
            "seq: missing operand\nTry 'seq --help' for more information.\n"
        );
        assert_eq!(
            SeqPlan::parse(&["1".into(), "2".into(), "3".into(), "4".into()])
                .err()
                .unwrap(),
            "seq: extra operand \u{2018}4\u{2019}\nTry 'seq --help' for more information.\n"
        );
    }

    #[test]
    fn seq_format_e_matches_c_printf_not_lexical_carry() {
        // Regression: comparing the formatted mantissa string ("2.000000" >= "10") instead of a
        // numeric value used to misfire on every mantissa starting '2'..='9', turning this into
        // "0.200000e+01".
        let plan = SeqPlan::parse(&["-f".into(), "%e".into(), "1".into(), "2".into()]).unwrap();
        let out: Vec<u8> = (0..plan.count).flat_map(|i| plan.render(i)).collect();
        assert_eq!(out, b"1.000000e+00\n2.000000e+00\n");
    }

    #[test]
    fn seq_format_g_strips_trailing_zeros_and_rounds_away_float_noise() {
        // 1.0 + 0.1*2 is 1.2000000000000002 as an f64; a real %g (round to 6 significant
        // digits, then strip trailing zeros) prints "1.2", not the raw Display of that float.
        let plan = SeqPlan::parse(&[
            "-f".into(),
            "%g".into(),
            "1".into(),
            "0.1".into(),
            "1.3".into(),
        ])
        .unwrap();
        let out: Vec<u8> = (0..plan.count).flat_map(|i| plan.render(i)).collect();
        assert_eq!(out, b"1\n1.1\n1.2\n1.3\n");
    }

    #[test]
    fn seq_format_g_switches_to_exponent_form_past_the_precision_window() {
        // 3 significant digits: 99001 rounds to "9.90e+04", trailing zero stripped to "9.9e+04".
        assert_eq!(apply_seq_format("%.3g", 99001.0), "9.9e+04");
        // Small values stay in %f form while the exponent is within [-4, P).
        assert_eq!(apply_seq_format("%.3g", 0.0001234), "0.000123");
    }

    #[test]
    fn yes_and_seq_report_broken_pipe_as_a_failure_not_silently() {
        struct BrokenPipe;
        impl Write for BrokenPipe {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut out = BrokenPipe;
        let mut err = Vec::new();
        let code = run_yes(&["yes".into()], &mut std::io::empty(), &mut out, &mut err).unwrap();
        assert_eq!(code, 1);
        assert_eq!(err, b"yes: standard output: Broken pipe\n");

        let mut err = Vec::new();
        let code = run_seq(
            &["seq".into(), "1".into(), "5".into()],
            &mut std::io::empty(),
            &mut out,
            &mut err,
        )
        .unwrap();
        assert_eq!(code, 1);
        assert_eq!(err, b"seq: write error: Broken pipe\n");
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn rev_continues_past_a_missing_file_without_the_os_error_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good");
        std::fs::write(&good, "ab\n").unwrap();
        let missing = dir.path().join("nonexist");
        let argv = vec![
            "rev".to_string(),
            good.to_string_lossy().into_owned(),
            missing.to_string_lossy().into_owned(),
            good.to_string_lossy().into_owned(),
        ];
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_rev(&argv, &mut std::io::empty(), &mut out, &mut err).unwrap();
        assert_eq!(code, 1);
        assert_eq!(out, b"ba\nba\n");
        let err = String::from_utf8(err).unwrap();
        assert!(
            err.contains(&format!(
                "rev: {}: No such file or directory\n",
                missing.display()
            )),
            "unexpected stderr: {err:?}"
        );
        assert!(
            !err.contains("os error"),
            "raw os-error suffix leaked through: {err:?}"
        );
    }

    #[test]
    // The oracle's own `rev` is BusyBox's — it implements no real options at all (`--help`
    // included, which only prints the usage banner) — verified against the oracle. Before this
    // fix, an option-looking argument was silently misread as a file operand instead.
    fn rev_rejects_every_option_like_busybox() {
        for (argv, expect_status, expect_word) in [
            (vec!["rev", "--bogus-option"], 1, "bogus-option"),
            (vec!["rev", "-@"], 1, "@"),
            (vec!["rev", "--version"], 1, "version"),
            (vec!["rev", "-h"], 1, "h"),
        ] {
            let argv: Vec<String> = argv.into_iter().map(str::to_owned).collect();
            let mut out = Vec::new();
            let mut err = Vec::new();
            let code = run_rev(&argv, &mut std::io::empty(), &mut out, &mut err).unwrap();
            assert_eq!(code, expect_status, "{argv:?}");
            let err = String::from_utf8(err).unwrap();
            assert_eq!(
                err,
                format!("rev: unrecognized option: {expect_word}\n{REV_BUSYBOX_BANNER}"),
                "{argv:?}"
            );
        }

        let argv: Vec<String> = ["rev", "--help"].into_iter().map(str::to_owned).collect();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_rev(&argv, &mut std::io::empty(), &mut out, &mut err).unwrap();
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(err).unwrap(), REV_BUSYBOX_BANNER);

        // `--` still ends option parsing, and a bare `-` still means stdin, not an option.
        let argv: Vec<String> = ["rev", "--", "-x"].into_iter().map(str::to_owned).collect();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_rev(&argv, &mut std::io::empty(), &mut out, &mut err).unwrap();
        assert_eq!(code, 1);
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "rev: -x: No such file or directory\n"
        );
    }

    #[test]
    fn tac_reads_files_backwards_across_blocks() {
        let path = crate::tools::test_scratch("tac-blocks");
        // Records of every length around the block size, and one far longer, unterminated.
        let mut text = Vec::new();
        for length in [0, 1, 65_535, 65_536, 65_537, 3, 200_000] {
            text.extend(std::iter::repeat_n(b'x', length));
            text.push(b'\n');
        }
        text.extend(std::iter::repeat_n(b'y', 300_000));
        std::fs::write(&path, &text).unwrap();
        let mut out = Vec::new();
        tac_file(&mut std::fs::File::open(&path).unwrap(), &mut out).unwrap();
        assert_eq!(out, tac_bytes(&text));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn tac_reverses_several_files_independently_in_operand_order() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("t1");
        let second = dir.path().join("t2");
        std::fs::write(&first, "a\nb\n").unwrap();
        std::fs::write(&second, "c\nd\n").unwrap();
        let argv = vec![
            "tac".to_string(),
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ];
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_tac(&argv, &mut std::io::empty(), &mut out, &mut err).unwrap();
        assert_eq!(code, 0);
        assert_eq!(out, b"b\na\nd\nc\n");
        assert!(err.is_empty());
    }

    #[test]
    // -s (a custom, fixed-string separator) and -b (attach it before the record instead of
    // after) — verified against the oracle, including their combination and a missing final
    // separator.
    fn tac_custom_separator_and_before_mode_follow_gnu() {
        assert_eq!(tac_bytes_with_separator(b"a,b,c,", b",", false), b"c,b,a,");
        assert_eq!(tac_bytes_with_separator(b"a\nb\n", b"\n", true), b"\n\nba");
        // A missing trailing separator: the piece that never had one (originally last/first,
        // now first/last after reversing, in "after"/"before" mode respectively) stays bare.
        assert_eq!(tac_bytes_with_separator(b"a,b,c", b",", false), b"cb,a,");
        assert_eq!(tac_bytes_with_separator(b"a,b,c", b",", true), b",c,ba");
        // An empty separator never matches — the whole input comes back unchanged.
        assert_eq!(tac_bytes_with_separator(b"abc", b"", false), b"abc");
        // An empty separator is NUL, as in GNU tac.
        assert_eq!(
            tac_bytes_with_separator(b"b\0a\0c\0", b"", false),
            b"c\0a\0b\0"
        );
        assert_eq!(tac_bytes_with_separator(b"b\0a\0c", b"", false), b"ca\0b\0");
    }

    #[test]
    fn run_tac_accepts_separator_and_before_options() {
        let argv = ["tac", "-s", ","]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_tac(&argv, &mut &b"a,b,c,"[..], &mut out, &mut err).unwrap();
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
        assert_eq!(out, b"c,b,a,");

        let argv = ["tac", "--separator", ",", "--before"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_tac(&argv, &mut &b"a,b,c,"[..], &mut out, &mut err).unwrap();
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
        assert_eq!(out, b",,c,ba");
    }

    #[test]
    // GNU's own bad-option wording (`unrecognized option`/`invalid option --`, status 1) — this
    // fork's own catch-all "-X is unsupported in bash-tool" refusal (status 2) was wrong here:
    // these aren't options tac chose not to implement, they don't exist at all. Verified against
    // the oracle.
    fn tac_bad_options_use_gnus_own_wording() {
        let argv = ["tac", "--bogus-option"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_tac(&argv, &mut std::io::empty(), &mut out, &mut err).unwrap();
        assert_eq!(code, 1);
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "tac: unrecognized option '--bogus-option'\nTry 'tac --help' for more information.\n"
        );

        let argv = ["tac", "-@"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_tac(&argv, &mut std::io::empty(), &mut out, &mut err).unwrap();
        assert_eq!(code, 1);
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "tac: invalid option -- '@'\nTry 'tac --help' for more information.\n"
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn tac_reports_a_missing_file_and_keeps_going() {
        let dir = tempfile::tempdir().unwrap();
        let present = dir.path().join("present");
        std::fs::write(&present, "a\nb\n").unwrap();
        let missing = dir.path().join("nonexist");
        let argv = vec![
            "tac".to_string(),
            missing.to_string_lossy().into_owned(),
            present.to_string_lossy().into_owned(),
        ];
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_tac(&argv, &mut std::io::empty(), &mut out, &mut err).unwrap();
        assert_eq!(code, 1);
        assert_eq!(out, b"b\na\n");
        let err = String::from_utf8(err).unwrap();
        assert!(
            err.contains(&format!(
                "tac: failed to open '{}' for reading: No such file or directory\n",
                missing.display()
            )),
            "unexpected stderr: {err:?}"
        );
    }
}
