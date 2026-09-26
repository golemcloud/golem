//! File metadata and formatting.
use std::io::Write;
use std::time::SystemTime;

use brush_core::builtins::{ContentOptions, ContentType, Registration, SimpleCommand};
use brush_core::commands::ExecutionContext;
use brush_core::extensions::ShellExtensions;
use brush_core::{Error, ExecutionResult};

use crate::manifest::Manifest;

pub(crate) struct Stat;

/// What GNU adds after a usage error.
const TRY_HELP: &str = "Try 'stat --help' for more information.\n";

impl Stat {
    const NAME: &'static str = "stat";
    const SYNOPSIS: &'static str = "display file status";
}

/// What we can honestly know about one path.
#[derive(Debug)]
struct StatInfo {
    path: String,
    size: u64,
    /// Human-readable file type, GNU-style (`regular file`, `directory`, `symbolic link`) plus
    kind: &'static str,
    modified: Option<SystemTime>,
    accessed: Option<SystemTime>,
    created: Option<SystemTime>,
    /// The symlink's own target, unresolved, for `%N`'s `'link' -> 'target'` form.
    symlink_target: Option<String>,
    /// This target has no `chmod` and no varying umask (see the `unsupported` refusal for
    /// `chmod`), so every file's mode is exactly its type's GNU default and never anything
    /// else: `40755` for a directory, `100644` for a regular file, `120777` for a symlink
    /// (Linux ignores a symlink's own permission bits and always reports `rwxrwxrwx`).
    mode: u32,
}

/// Resolve metadata for one file operand.
/// `follow` selects `metadata` (follow symlinks, `-L`) over the default `symlink_metadata`.
/// `stdin` is standard input's file when it is one (`< file`), which `-` and a followed
/// `/dev/stdin` then describe, as on Linux.
fn resolve(
    path: &str,
    follow: bool,
    stdin: Option<&std::fs::Metadata>,
) -> crate::error::Result<StatInfo> {
    let not_found = || {
        crate::ShellError::not_found(format!("cannot statx '{path}': No such file or directory"))
    };

    let names_stdin = matches!(
        super::devices::operand(path),
        Some(super::devices::Device::Stream(0))
    ) && (path == "-" || follow);
    if let (true, Some(md)) = (names_stdin, stdin) {
        return Ok(info_from(path, md, None));
    }

    // The emulated devices (see `devices`): `-` is standard input, which is a pipe.
    let device = match super::devices::operand(path) {
        Some(super::devices::Device::Null) => Some("character special file"),
        Some(super::devices::Device::Stream(_)) if path == "-" || follow => Some("fifo"),
        _ => None,
    };
    if let Some(kind) = device {
        return Ok(StatInfo {
            path: path.to_string(),
            size: 0,
            kind,
            modified: None,
            accessed: None,
            created: None,
            symlink_target: None,
            mode: 0o020_000 | 0o666, // character special
        });
    }
    let md = if follow {
        std::fs::metadata(path)
    } else {
        std::fs::symlink_metadata(path)
    }
    .map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => not_found(),
        // Anything else (permissions, a broken mount) is an I/O failure, not an absent path — the
        // distinction the flat String could not carry. Worded as strerror words it.
        _ => crate::ShellError::io(format!("cannot statx '{path}': {}", strerror(&e))),
    })?;
    let symlink_target = md
        .file_type()
        .is_symlink()
        .then(|| std::fs::read_link(path).ok())
        .flatten()
        .map(|p| p.to_string_lossy().into_owned());
    Ok(info_from(path, &md, symlink_target))
}

/// An I/O error's text without Rust's `(os error N)` suffix, as GNU prints `strerror`.
fn strerror(error: &std::io::Error) -> String {
    let text = error.to_string();
    match text.find(" (os error ") {
        Some(end) => text[..end].to_owned(),
        None => text,
    }
}

/// What stat reports for `path` from its metadata.
fn info_from(path: &str, md: &std::fs::Metadata, symlink_target: Option<String>) -> StatInfo {
    let ft = md.file_type();
    let (kind, mode) = if ft.is_dir() {
        ("directory", 0o040_000 | 0o755)
    } else if ft.is_symlink() {
        ("symbolic link", 0o120_000 | 0o777)
    } else if md.len() == 0 {
        ("regular empty file", 0o100_000 | 0o644)
    } else {
        ("regular file", 0o100_000 | 0o644)
    };
    StatInfo {
        path: path.to_string(),
        size: md.len(),
        kind,
        modified: md.modified().ok(),
        accessed: md.accessed().ok(),
        created: md.created().ok(),
        symlink_target,
        mode,
    }
}

/// A name the way `%N` prints it: single-quoted, `\` and `'` escaped as GNU's own quoting does.
fn quote_name(name: &str) -> String {
    format!("'{}'", name.replace('\\', "\\\\").replace('\'', "'\\''"))
}

/// `%a` (octal permission bits only, no file-type bits).
fn octal_mode(mode: u32) -> String {
    format!("{:o}", mode & 0o7777)
}

/// `%A`: `ls -l`'s ten-character mode string (`-rw-r--r--`, `drwxr-xr-x`, `lrwxrwxrwx`, ...).
fn symbolic_mode(mode: u32) -> String {
    let file_type = match mode & 0o170_000 {
        0o040_000 => 'd',
        0o120_000 => 'l',
        0o020_000 => 'c',
        0o060_000 => 'b',
        0o010_000 => 'p',
        0o140_000 => 's',
        _ => '-',
    };
    let bit = |flag: u32, c: char| if mode & flag != 0 { c } else { '-' };
    format!(
        "{file_type}{}{}{}{}{}{}{}{}{}",
        bit(0o400, 'r'),
        bit(0o200, 'w'),
        bit(0o100, 'x'),
        bit(0o040, 'r'),
        bit(0o020, 'w'),
        bit(0o010, 'x'),
        bit(0o004, 'r'),
        bit(0o002, 'w'),
        bit(0o001, 'x'),
    )
}

/// `2026-07-10 09:12:33.123456789 +0000` (GNU stat's human timestamp; the agent runs in UTC), or
/// `-` when the platform can't supply the time.
fn human_time(t: Option<SystemTime>) -> String {
    match t {
        Some(t) => chrono::DateTime::<chrono::Utc>::from(t)
            .format("%Y-%m-%d %H:%M:%S%.9f +0000")
            .to_string(),
        None => "-".to_string(),
    }
}

/// The default (no `-c`) block, Linux-shaped with honest `-` for sandbox-unknowable fields.
fn render_default(info: &StatInfo) -> String {
    format!(
        "  File: {path}\n  \
         Size: {size}\tBlocks: -\tIO Block: -\t{kind}\n\
         Device: -\tInode: -\tLinks: -\n\
         Access: (-)\tUid: (-)\tGid: (-)\n\
         Access: {atime}\n\
         Modify: {mtime}\n\
         Change: -\n \
         Birth: {btime}\n",
        path = info.path,
        size = info.size,
        kind = info.kind,
        atime = human_time(info.accessed),
        mtime = human_time(info.modified),
        btime = human_time(info.created),
    )
}

/// Seconds since the epoch with `precision` digits of the fraction (GNU's `%.3Y`), or `0`.
fn epoch_with_fraction(t: Option<SystemTime>, precision: Option<usize>) -> String {
    let Some(duration) = t.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()) else {
        return "0".to_string();
    };
    match precision {
        Some(0) | None => duration.as_secs().to_string(),
        Some(digits) => {
            let mut fraction = format!("{:09}", duration.subsec_nanos());
            fraction.truncate(digits.min(9));
            let padding = digits.saturating_sub(9);
            format!("{}.{fraction}{}", duration.as_secs(), "0".repeat(padding))
        }
    }
}

/// Apply a FORMAT string: `%`-directives as GNU stat reads them (`%[-#0+ '][WIDTH][.PREC]`,
/// then an optional `H` or `L` before `d` or `r`, then the directive). `--printf` also takes
/// `\n`-style escapes and adds no newline; `-c`/`--format` take the text as written and end
/// with a newline (`escapes` false, `auto_newline` true). Unknown directives render as `?`.
fn render_format(info: &StatInfo, format: &str, escapes: bool, auto_newline: bool) -> String {
    let mut out = String::new();
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        if c == '\\' && escapes {
            match chars.get(i) {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') | None => out.push('\\'),
                Some(&other) => out.push(other),
            }
            i += 1;
            continue;
        }
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut flags = String::new();
        while let Some(&flag) = chars.get(i).filter(|c| "-#0+ '".contains(**c)) {
            flags.push(flag);
            i += 1;
        }
        let mut width = String::new();
        while let Some(&digit) = chars.get(i).filter(|c| c.is_ascii_digit()) {
            width.push(digit);
            i += 1;
        }
        let mut precision = None;
        if chars.get(i) == Some(&'.') {
            i += 1;
            let mut digits = String::new();
            while let Some(&digit) = chars.get(i).filter(|c| c.is_ascii_digit()) {
                digits.push(digit);
                i += 1;
            }
            precision = Some(digits.parse::<usize>().unwrap_or(0));
        }
        // `%Hd`/`%Ld` (and `r`): the major and minor device numbers, both 0 here.
        if matches!(chars.get(i), Some('H' | 'L')) && matches!(chars.get(i + 1), Some('d' | 'r')) {
            i += 1;
        }
        let Some(&directive) = chars.get(i) else {
            out.push('%');
            out.push_str(&flags);
            out.push_str(&width);
            break;
        };
        i += 1;
        // The value, and whether it is a number (for `0` padding and integer precision).
        let (value, numeric) = match directive {
            'n' => (info.path.clone(), false),
            'N' => {
                let mut name = quote_name(&info.path);
                if let Some(target) = &info.symlink_target {
                    name.push_str(" -> ");
                    name.push_str(&quote_name(target));
                }
                (name, false)
            }
            's' => (info.size.to_string(), true),
            'F' => (info.kind.to_string(), false),
            'a' => (octal_mode(info.mode), true),
            'A' => (symbolic_mode(info.mode), false),
            // No real ownership in the sandbox (see `%U`/`%G`): this crate's one fixed
            // identity, matching `ls -l`/`-n`'s own fallback (`which.rs`, `ls`'s
            // `display.rs`) — a numeric uid/gid with no matching passwd/group entry.
            'u' | 'g' => ("1000".to_string(), true),
            'U' | 'G' => ("UNKNOWN".to_string(), false),
            // No real hard-link tracking in the sandbox; every path this crate can create
            // has exactly one name.
            'h' => ("1".to_string(), true),
            // No real device number in the sandbox.
            'd' | 'D' | 'r' | 'R' => ("0".to_string(), true),
            'y' => (human_time(info.modified), false),
            'x' => (human_time(info.accessed), false),
            'w' => (human_time(info.created), false),
            // Epoch seconds take a precision as digits of the fraction.
            'Y' | 'X' | 'W' => {
                let time = match directive {
                    'Y' => info.modified,
                    'X' => info.accessed,
                    _ => info.created,
                };
                let text = epoch_with_fraction(time, precision);
                precision = None;
                (text, false)
            }
            '%' => {
                out.push('%');
                continue;
            }
            _ => ("?".to_string(), false),
        };
        let mut value = value;
        if let Some(precision) = precision {
            if numeric {
                // An integer's precision is its least number of digits.
                if value.len() < precision {
                    value = format!("{}{value}", "0".repeat(precision - value.len()));
                }
            } else {
                value = value.chars().take(precision).collect();
            }
        }
        let width = width.parse::<usize>().unwrap_or(0);
        let length = value.chars().count();
        if length < width {
            let padding = width - length;
            if flags.contains('-') {
                value.push_str(&" ".repeat(padding));
            } else if flags.contains('0') && numeric && precision.is_none() {
                value = format!("{}{value}", "0".repeat(padding));
            } else {
                value = format!("{}{value}", " ".repeat(padding));
            }
        }
        out.push_str(&value);
    }
    if auto_newline {
        out.push('\n');
    }
    out
}

impl SimpleCommand for Stat {
    fn get_content(
        name: &str,
        content_type: ContentType,
        _options: &ContentOptions,
    ) -> Result<String, Error> {
        match content_type {
            ContentType::ShortDescription => Ok(format!("{name} - {}\n", Stat::SYNOPSIS)),
            ContentType::ShortUsage => Ok(format!("{name}: {name} [-L] [-c FORMAT] FILE...\n")),
            ContentType::DetailedHelp => Ok(format!(
                "{name} - {}\n\nHonest wasm subset: size/type/timestamps are real; mode bits are \
                 this sandbox's fixed defaults (no chmod exists to change them: 644 for a \
                 regular file, 755 for a directory, 777 for a symlink); uid/gid are this \
                 sandbox's fixed identity (1000, UNKNOWN by name); inode, device, and block \
                 counts are not available and print as '-' or 0. -f/-t are refused: neither has \
                 real data to report.\n",
                Stat::SYNOPSIS
            )),
            ContentType::ManPage => brush_core::error::unimp("man page not yet implemented"),
        }
    }

    #[allow(clippy::similar_names)] // argv/arg are conventional
    fn execute<SE, I, S>(
        context: ExecutionContext<'_, SE>,
        args: I,
    ) -> Result<ExecutionResult, Error>
    where
        SE: ShellExtensions,
        I: Iterator<Item = S>,
        S: AsRef<str>,
    {
        let _cwd = super::coreutils::ShellCwd::enter(&context);
        let argv: Vec<String> = args.skip(1).map(|s| s.as_ref().to_string()).collect();
        let mut follow = false;
        let mut format: Option<String> = None;
        // `--printf` (unlike `-c`/`--format`) does not append a trailing newline of its own.
        let mut auto_newline = true;
        let mut operands: Vec<String> = Vec::new();
        let mut iter = argv.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--" => {
                    operands.extend(iter.by_ref());
                }
                "-L" | "--dereference" => follow = true,
                "-c" | "--format" => {
                    let Some(f) = iter.next() else {
                        let message = if arg == "-c" {
                            "option requires an argument -- 'c'"
                        } else {
                            "option '--format' requires an argument"
                        };
                        let _ = write!(context.stderr(), "stat: {message}\n{TRY_HELP}");
                        return Ok(ExecutionResult::new(1));
                    };
                    format = Some(f);
                    auto_newline = true;
                }
                "--printf" => {
                    let Some(f) = iter.next() else {
                        let _ = write!(
                            context.stderr(),
                            "stat: option '--printf' requires an argument\n{TRY_HELP}"
                        );
                        return Ok(ExecutionResult::new(1));
                    };
                    format = Some(f);
                    auto_newline = false;
                }
                // GNU options this sandbox cannot answer faithfully: there is no real
                // filesystem (block/inode counts, filesystem type) or terse device/inode
                // data to report, so these are refused rather than shown as plausible but
                // fabricated numbers.
                "-f" | "--file-system" => {
                    let _ = writeln!(
                        context.stderr(),
                        "stat: -f is unsupported in bash-tool: no real filesystem statistics in the sandbox"
                    );
                    return Ok(ExecutionResult::new(2));
                }
                "-t" | "--terse" => {
                    let _ = writeln!(
                        context.stderr(),
                        "stat: -t is unsupported in bash-tool: no real device/inode data in the sandbox"
                    );
                    return Ok(ExecutionResult::new(2));
                }
                f if f.starts_with("--format=") => {
                    format = Some(f["--format=".len()..].to_string());
                    auto_newline = true;
                }
                f if f.starts_with("--printf=") => {
                    format = Some(f["--printf=".len()..].to_string());
                    auto_newline = false;
                }
                f if f.starts_with("--") => {
                    let message = format!(
                        "stat: unrecognized option '{f}'\nTry 'stat --help' for more information.\n"
                    );
                    let _ = context.stderr().write_all(message.as_bytes());
                    return Ok(ExecutionResult::new(1));
                }
                f if f.starts_with('-') && f.len() > 1 => {
                    // A short option or a getopt-style cluster (`-Lc%n`): walk it left to
                    // right, applying what's recognized and reporting the first character
                    // that isn't, exactly where GNU's own getopt would stop.
                    let mut chars = f[1..].chars();
                    let mut rejected = false;
                    while let Some(c) = chars.next() {
                        match c {
                            'L' => follow = true,
                            'c' => {
                                let rest: String = chars.by_ref().collect();
                                let value = if rest.is_empty() {
                                    match iter.next() {
                                        Some(v) => v,
                                        None => {
                                            let _ = write!(
                                                context.stderr(),
                                                "stat: option requires an argument -- 'c'\n{TRY_HELP}"
                                            );
                                            return Ok(ExecutionResult::new(1));
                                        }
                                    }
                                } else {
                                    rest
                                };
                                format = Some(value);
                                auto_newline = true;
                                break;
                            }
                            other => {
                                let message = format!(
                                    "stat: invalid option -- '{other}'\nTry 'stat --help' for more information.\n"
                                );
                                let _ = context.stderr().write_all(message.as_bytes());
                                rejected = true;
                                break;
                            }
                        }
                    }
                    if rejected {
                        return Ok(ExecutionResult::new(1));
                    }
                }
                op => operands.push(op.to_string()),
            }
        }

        if operands.is_empty() {
            let _ = write!(context.stderr(), "stat: missing operand\n{TRY_HELP}");
            return Ok(ExecutionResult::new(1));
        }

        // Standard input's file, when it is one, for `-` and `-L /dev/stdin`.
        let stdin = match context.stdin() {
            brush_core::openfiles::OpenFile::File(file) => file.metadata().ok(),
            _ => None,
        };
        let mut out = context.stdout();
        let mut failed = false;
        for op in &operands {
            match resolve(op, follow, stdin.as_ref()) {
                Ok(info) => {
                    // `--printf` takes backslash escapes and ends as written; `-c` neither.
                    let rendered = match &format {
                        Some(f) => render_format(&info, f, !auto_newline, auto_newline),
                        None => render_default(&info),
                    };
                    // A format's own text stands for bytes, as every argument does.
                    let _ = out.write_all(&super::shell_bytes::encode(&rendered));
                }
                Err(msg) => {
                    let _ = writeln!(context.stderr(), "stat: {msg}");
                    failed = true;
                }
            }
        }
        Ok(ExecutionResult::new(u8::from(failed)))
    }
}

pub(crate) fn builtins<SE: ShellExtensions>() -> Vec<(String, Registration<SE>)> {
    use crate::helpshim::simple_utility_with_help;
    vec![("stat".into(), simple_utility_with_help::<Stat, SE>())]
}

pub(crate) fn manifests() -> Vec<Manifest> {
    vec![Manifest::builtin(Stat::NAME, Stat::SYNOPSIS).with_help(
        "stat [-L] [-c FORMAT | --printf=FORMAT] FILE... — display file status. Size, type, and \
         timestamps are real; mode bits and uid/gid are this sandbox's fixed defaults (644/755/\
         777 by type, uid/gid 1000 with no matching name); inode, device, and block counts are \
         not available and print as '-' or 0. -f/-t are refused. FORMAT directives: %n/%N name, \
         %s size, %F type, %a/%A mode, %u/%g/%U/%G owner, %h links, %d/%D device, %y/%Y mtime, \
         %x/%X atime, %w/%W birth, %% literal.",
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpfile(content: &[u8]) -> std::path::PathBuf {
        let path = crate::tools::test_scratch("bash-stat-test");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn resolve_real_file_reports_size_and_type() {
        let path = tmpfile(b"hello!");
        let info = resolve(path.to_str().unwrap(), false, None).unwrap();
        assert_eq!(info.size, 6);
        assert_eq!(info.kind, "regular file");
        assert!(info.modified.is_some());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn resolve_missing_file_is_an_error() {
        let err = resolve("/definitely/not/here", false, None).unwrap_err();
        assert!(
            err.to_string().contains("No such file or directory"),
            "got: {err}"
        );
    }

    #[test]
    fn default_format_has_honest_dashes() {
        let path = tmpfile(b"x");
        let info = resolve(path.to_str().unwrap(), false, None).unwrap();
        let rendered = render_default(&info);
        assert!(rendered.contains("Size: 1"));
        assert!(rendered.contains("regular file"));
        assert!(rendered.contains("Inode: -"));
        assert!(rendered.contains("Uid: (-)"));
        assert!(rendered.contains("Change: -"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn format_directives_render() {
        let path = tmpfile(b"abc");
        let info = resolve(path.to_str().unwrap(), false, None).unwrap();
        assert_eq!(
            render_format(&info, "%s %F", false, true),
            "3 regular file\n"
        );
        assert!(render_format(&info, "%n", false, true).contains("bash-stat-test"));
        // Unknown directive → '?', escaped percent; `-c` keeps a backslash as written.
        assert_eq!(render_format(&info, "%q%%\\t", false, true), "?%\\t\n");
        // `--printf` takes escapes and does not add its own trailing newline.
        assert_eq!(render_format(&info, "%s\\t", true, false), "3\t");
        // Width, justification and precision, as GNU's printf-style directives.
        assert_eq!(
            render_format(&info, "[%-5s][%5s][%05s]", false, false),
            "[3    ][    3][00003]"
        );
        assert_eq!(render_format(&info, "%.2F|%Hd%Lr", false, false), "re|00");
        std::fs::remove_file(path).unwrap();
    }
}
