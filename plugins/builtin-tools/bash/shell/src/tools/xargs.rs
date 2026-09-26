//! GNU-compatible `xargs`: split standard input (or `-a FILE`) into arguments and run command
//! lines through the shell as isolated child commands. On WASM the input is consumed as it
//! streams, so endless producers stop when the consumer closes.
//!
//! Also hosts the child-command helpers `find -exec` shares.

use std::path::Path;

use brush_core::builtins::{
    BoxFuture, ContentOptions, ContentType, ExecutionBoundary, Registration,
};
use brush_core::commands::ExecutionContext;
use brush_core::extensions::ShellExtensions;
use brush_core::openfiles::{OpenFile, OpenFiles};
use brush_core::{CommandArg, Error, ExecutionParameters, ExecutionResult, Shell};
use futures::io::{AsyncReadExt, AsyncWriteExt};

use crate::manifest::Manifest;

/// GNU's default command-line budget, counting a NUL after every word.
const DEFAULT_MAX_CHARS: usize = 128 * 1024;
const TRY_HELP: &str = "Try 'xargs --help' for more information.";

// ------------------------------------------------------------------------------------------------
// Child commands (shared with find -exec)
// ------------------------------------------------------------------------------------------------

/// Quote one word for safe re-entry through the shell parser: plain words pass through, anything
/// else is single-quoted with embedded quotes escaped (`can't` → `'can'\''t'`).
pub(crate) fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/=:,+%@".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

const RESERVED: [&str; 16] = [
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "select", "while", "until", "do",
    "done", "in", "function", "coproc",
];

/// A command line for the shell parser. The command word is always taken literally, so an `=` or
/// a reserved word there cannot turn into an assignment or a keyword.
pub(crate) fn command_line(words: &[String]) -> String {
    words
        .iter()
        .enumerate()
        .map(|(index, word)| {
            if index == 0 && (word.contains('=') || RESERVED.contains(&word.as_str())) {
                format!("'{}'", word.replace('\'', r"'\''"))
            } else {
                shell_quote(word)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// How exec(3) would fare with a command name.
pub(crate) enum Lookup {
    Found,
    Missing,
    Denied,
}

/// Resolve `name` as a child command, as exec(3) would: a command that stands for a program
/// (`ls`, `sed`, `echo`, ... -- see `tools::programs`), a path relative to `cwd`, or a `PATH`
/// search. Never a shell function, and never a builtin only a shell can be (`cd`, `read`,
/// `export`, ...): those aren't real executables, so real `xargs`/`find -exec` fail to exec them
/// exactly like any other missing command (`xargs: cd: No such file or directory`, status 127)
/// even though the *shell* itself understands `cd`.
pub(crate) fn lookup<SE: ShellExtensions>(shell: &Shell<SE>, name: &str, cwd: &Path) -> Lookup {
    if !name.contains('/') && shell.program_path(name).is_some() {
        return Lookup::Found;
    }
    if name.contains('/') {
        // A program a builtin stands for (`/bin/cat`) runs the builtin.
        if shell
            .program_builtin(&cwd.join(name).to_string_lossy())
            .is_some()
        {
            return Lookup::Found;
        }
        return match std::fs::metadata(cwd.join(name)) {
            Ok(meta) if meta.is_dir() => Lookup::Denied,
            Ok(_) => Lookup::Found,
            Err(_) => Lookup::Missing,
        };
    }
    if !name.is_empty() && shell.find_first_executable_in_path(name).is_some() {
        Lookup::Found
    } else {
        Lookup::Missing
    }
}

/// Run `line` as an isolated child command: in a copy of the shell (so `cd` and assignments stay
/// local), optionally in `cwd`. On WASM it is its own logical process, so a child killed by
/// SIGPIPE reports the signal to the caller instead of ending it.
///
/// The child stands for a new process: it sees the caller's exported variables and functions and
/// working directory, but none of its other variables, aliases, options or directory stack (see
/// [`crate::session::fresh_child`]), starts without the caller's EXIT, ERR, DEBUG and RETURN
/// traps (as `exec` resets them), and runs its own EXIT trap when it ends.
///
/// `prologue` (options, positional parameters) runs first as its own evaluation, so the line
/// numbers the command sees are its own. `name`, when given, is the child's `$0`. `script` marks
/// the line as a `bash -c` string, whose last command may run in place of the child as bash's
/// does (see [`Shell::exec_last_command`]); a command line a utility runs is a program already.
pub(crate) async fn run_child<SE: ShellExtensions>(
    shell: &mut Shell<SE>,
    params: &ExecutionParameters,
    prologue: Option<String>,
    line: String,
    cwd: Option<&Path>,
    name: Option<&str>,
    script: bool,
) -> Result<ExecutionResult, Error> {
    run_child_as(shell, params, prologue, line, cwd, name, script, None).await
}

/// [`run_child`], as process `pid` when given (one the caller allocated from the shell's process
/// table, to signal the child while it runs), else as a new number.
pub(crate) async fn run_child_as<SE: ShellExtensions>(
    shell: &mut Shell<SE>,
    params: &ExecutionParameters,
    prologue: Option<String>,
    line: String,
    cwd: Option<&Path>,
    name: Option<&str>,
    script: bool,
    pid: Option<brush_core::process_table::Pid>,
) -> Result<ExecutionResult, Error> {
    run_child_with(shell, params, prologue, line, cwd, name, script, pid, false).await
}

/// [`run_child_as`], as a login shell (`bash -l`: `shopt login_shell` is on) when `login`.
pub(crate) async fn run_child_with<SE: ShellExtensions>(
    shell: &mut Shell<SE>,
    params: &ExecutionParameters,
    prologue: Option<String>,
    line: String,
    cwd: Option<&Path>,
    name: Option<&str>,
    script: bool,
    pid: Option<brush_core::process_table::Pid>,
    login: bool,
) -> Result<ExecutionResult, Error> {
    use brush_core::traps::TrapSignal;
    #[cfg(not(target_arch = "wasm32"))]
    let _ = pid;
    let mut child = crate::session::fresh_child(shell);
    child.options_mut().login_shell = login;
    if let Some(name) = name {
        child.set_shell_name(name);
    }
    // A new process: its line numbers and call stack start afresh, in a command string.
    child.reset_call_stack();
    child.start_command_string_mode();
    let source_info = brush_core::SourceInfo::from("bash");
    if let Some(cwd) = cwd {
        child.set_working_dir(cwd)?;
    }
    for signal in [
        TrapSignal::Exit,
        TrapSignal::Err,
        TrapSignal::Debug,
        TrapSignal::Return,
    ] {
        child.traps_mut().remove_handlers(signal);
    }
    #[cfg(target_arch = "wasm32")]
    child.traps_mut().reset_pipe_for_subshell();
    // A new process with a number of its own: `$$` and `$BASHPID`, so `kill $$` ends only it.
    #[cfg(target_arch = "wasm32")]
    let process = {
        use brush_core::execution::process;
        let table = shell.processes().clone();
        let pid = pid.unwrap_or_else(|| table.allocate(shell.own_pid(), String::new()));
        child.set_shell_pid(pid);
        child.set_own_pid(pid);
        let dispositions = process::inherited_dispositions(process::pipe_disposition().for_exec());
        process::NumberedProcess::register(&table, pid, dispositions)
    };
    #[cfg(target_arch = "wasm32")]
    let completed = std::cell::Cell::new(false);
    let run = async {
        if let Some(prologue) = prologue {
            child.run_string(prologue, &source_info, params).await?;
        }
        if script {
            child.exec_last_command();
        }
        let result = child.run_string(line, &source_info, params).await;
        let result = child.exit_with_trap_in(result, params).await;
        #[cfg(target_arch = "wasm32")]
        completed.set(true);
        result
    };
    #[cfg(target_arch = "wasm32")]
    {
        let result = process.run(run).await;
        // A signal ended the child before it finished: its EXIT trap still runs, as in bash.
        if !completed.get() {
            child.exit_trap_after_signal(&result, params).await;
        }
        result
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        run.await
    }
}

/// Quote a word as GNU `xargs -t` prints it (gnulib shell-escape style): only when needed,
/// double quotes for a lone apostrophe, and `$'\ooo'` for control characters and bytes that are
/// not UTF-8.
fn verbose_quote(word: &str) -> String {
    let control = |c: char| c.is_control() || super::shell_bytes::raw_byte(c).is_some();
    if word.is_empty() {
        return "''".to_string();
    }
    let special = |(index, c): (usize, char)| {
        matches!(
            c,
            ' ' | '!'
                | '"'
                | '$'
                | '&'
                | '('
                | ')'
                | '*'
                | ';'
                | '<'
                | '='
                | '>'
                | '?'
                | '['
                | '\\'
                | '^'
                | '`'
                | '|'
                | '\''
        ) || control(c)
            || (index == 0 && matches!(c, '#' | '~'))
    };
    if !word.chars().enumerate().any(special) && word != "{" && word != "}" {
        return word.to_string();
    }
    let has_control = word.chars().any(control);
    if !has_control && word.contains('\'') && !word.contains(['$', '`', '"', '\\', '!']) {
        return format!("\"{word}\"");
    }
    let mut out = String::new();
    let mut open = false;
    for c in word.chars() {
        if control(c) {
            if open {
                out.push('\'');
                open = false;
            } else if out.is_empty() {
                out.push_str("''");
            }
            let escape = match c {
                '\u{7}' => "\\a".to_string(),
                '\u{8}' => "\\b".to_string(),
                '\t' => "\\t".to_string(),
                '\n' => "\\n".to_string(),
                '\u{b}' => "\\v".to_string(),
                '\u{c}' => "\\f".to_string(),
                '\r' => "\\r".to_string(),
                other => format!(
                    "\\{:03o}",
                    super::shell_bytes::raw_byte(other).map_or(u32::from(other), u32::from)
                ),
            };
            out.push_str(&format!("$'{escape}'"));
        } else {
            if !open {
                out.push('\'');
                open = true;
            }
            if c == '\'' {
                out.push_str("'\\''");
            } else {
                out.push(c);
            }
        }
    }
    if open {
        out.push('\'');
    }
    out
}

// ------------------------------------------------------------------------------------------------
// Options
// ------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delimiter {
    /// Blank-separated with quotes and backslashes (the default).
    Quoted,
    Byte(u8),
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "one switch per GNU xargs flag"
)]
struct Options {
    command: Vec<String>,
    delimiter: Delimiter,
    eof: Option<String>,
    replace: Option<String>,
    max_args: Option<usize>,
    max_lines: Option<usize>,
    max_chars: usize,
    /// The `-s` value as given, which GNU checks against the limit the environment leaves.
    size_given: Option<i64>,
    exit_on_size: bool,
    no_run_if_empty: bool,
    verbose: bool,
    arg_file: Option<String>,
}

enum Parsed {
    Run(Options),
    Help,
    Version,
}

/// A usage error: the complete stderr text and exit status.
#[derive(Debug)]
struct Failure {
    text: String,
    code: u8,
}

fn usage_failure(message: &str) -> Failure {
    Failure {
        text: format!("xargs: {message}\n{TRY_HELP}\n"),
        code: 1,
    }
}

fn refusal(option: &str) -> Failure {
    Failure {
        text: format!("xargs: {option} is unsupported in bash-tool\n"),
        code: 2,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Arity {
    Flag,
    Required,
    Optional,
}

fn short_arity(option: char) -> Option<Arity> {
    match option {
        '0' | 'o' | 'p' | 'r' | 't' | 'x' => Some(Arity::Flag),
        'a' | 'E' | 'I' | 'L' | 'n' | 'P' | 's' | 'd' => Some(Arity::Required),
        'e' | 'i' | 'l' => Some(Arity::Optional),
        _ => None,
    }
}

/// Long options and the short option each behaves as (`S`: --show-limits, `V`:
/// --process-slot-var, `h`/`v`: --help/--version).
const LONG: [(&str, char, Arity); 18] = [
    ("null", '0', Arity::Flag),
    ("arg-file", 'a', Arity::Required),
    ("delimiter", 'd', Arity::Required),
    ("eof", 'e', Arity::Optional),
    ("replace", 'i', Arity::Optional),
    ("max-lines", 'l', Arity::Optional),
    ("max-args", 'n', Arity::Required),
    ("open-tty", 'o', Arity::Flag),
    ("interactive", 'p', Arity::Flag),
    ("max-procs", 'P', Arity::Required),
    ("no-run-if-empty", 'r', Arity::Flag),
    ("max-chars", 's', Arity::Required),
    ("verbose", 't', Arity::Flag),
    ("exit", 'x', Arity::Flag),
    ("show-limits", 'S', Arity::Flag),
    ("process-slot-var", 'V', Arity::Required),
    ("help", 'h', Arity::Flag),
    ("version", 'v', Arity::Flag),
];

/// Parse a number like GNU `parse_num`: a decimal integer no smaller than `min`.
fn number(value: &str, option: char, min: i64) -> Result<usize, Failure> {
    let parsed: i64 = value
        .trim_start()
        .parse()
        .map_err(|_| usage_failure(&format!("invalid number \"{value}\" for -{option} option")))?;
    if parsed < min {
        return Err(usage_failure(&format!(
            "value {parsed} for -{option} option should be >= {min}"
        )));
    }
    usize::try_from(parsed)
        .map_err(|_| usage_failure(&format!("invalid number \"{value}\" for -{option} option")))
}

/// What a word costs of the command-line budget: its bytes and the NUL after it.
fn word_size(word: &str) -> usize {
    super::shell_bytes::encode(word).len() + 1
}

/// Parse a `-d` delimiter: one byte, or a C escape (`\n`, `\t`, `\0`, `\x41`, `\101`, ...).
fn delimiter(spec: &str) -> Result<u8, Failure> {
    let fatal = |text: String| Failure {
        text: format!("xargs: {text}\n"),
        code: 1,
    };
    let invalid = || {
        fatal(format!(
            "Invalid input delimiter specification {spec}: the delimiter must be either a single \
             character or an escape sequence starting with \\."
        ))
    };
    // The bytes the specification stands for: `-d $'\xff'` is the one byte 0xFF.
    let bytes = &*super::shell_bytes::encode(spec);
    match bytes {
        [single] => Ok(*single),
        [b'\\', escape, rest @ ..] => {
            let simple = match escape {
                b'a' => Some(7),
                b'b' => Some(8),
                b'f' => Some(12),
                b'n' => Some(b'\n'),
                b'r' => Some(b'\r'),
                b't' => Some(b'\t'),
                b'v' => Some(11),
                b'\\' => Some(b'\\'),
                _ => None,
            };
            if let Some(byte) = simple {
                return if rest.is_empty() {
                    Ok(byte)
                } else {
                    Err(invalid())
                };
            }
            let (digits, radix) = match escape {
                b'x' => (rest, 16),
                b'0'..=b'7' => (&bytes[1..], 8),
                _ => {
                    return Err(fatal(format!(
                        "Invalid escape sequence {spec} in input delimiter specification."
                    )));
                }
            };
            let digits = std::str::from_utf8(digits).map_err(|_| invalid())?;
            if digits.is_empty() {
                return Ok(0);
            }
            u32::from_str_radix(digits, radix)
                .ok()
                .and_then(|value| u8::try_from(value).ok())
                .ok_or_else(invalid)
        }
        _ => Err(invalid()),
    }
}

struct OptionParser {
    options: Options,
    warnings: String,
    help: bool,
    version: bool,
}

impl OptionParser {
    fn warn_exclusive(&mut self, offending: &str, option: &str) {
        self.warnings.push_str(&format!(
            "xargs: warning: options {offending} and {option} are mutually exclusive, ignoring \
             previous {offending} value\n"
        ));
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one arm per GNU option keeps the table readable"
    )]
    fn apply(&mut self, option: char, spelled: &str, value: Option<String>) -> Result<(), Failure> {
        let value_or_empty = value.clone().unwrap_or_default();
        match option {
            '0' => self.options.delimiter = Delimiter::Byte(0),
            'a' => self.options.arg_file = value,
            'd' => self.options.delimiter = Delimiter::Byte(delimiter(&value_or_empty)?),
            'E' | 'e' => self.options.eof = value.filter(|eof| !eof.is_empty()),
            'I' | 'i' => {
                let pattern = if option == 'I' {
                    value_or_empty
                } else {
                    value.unwrap_or_else(|| "{}".to_string())
                };
                if self.options.max_args.take().is_some() {
                    self.warn_exclusive("--max-args", "--replace/-I/-i");
                }
                if self.options.max_lines.take().is_some() {
                    self.warn_exclusive("--max-lines", "--replace/-I/-i");
                }
                self.options.replace = Some(pattern);
                self.options.exit_on_size = true;
            }
            'L' | 'l' => {
                let lines = match value {
                    Some(value) => number(&value, option, 1)?,
                    None => 1,
                };
                let name = if option == 'L' {
                    "-L"
                } else {
                    "--max-lines/-l"
                };
                if self.options.max_args.take().is_some() {
                    self.warn_exclusive("--max-args", name);
                }
                if self.options.replace.take().is_some() {
                    self.warn_exclusive("--replace", name);
                }
                self.options.max_lines = Some(lines);
                self.options.exit_on_size |= option == 'L';
            }
            'n' => {
                let args = number(&value_or_empty, 'n', 1)?;
                if self.options.max_lines.take().is_some() {
                    self.warn_exclusive("--max-lines", "--max-args/-n");
                }
                self.options.max_args = Some(args);
            }
            // Commands run one at a time; any valid -P is accepted.
            'P' => {
                number(&value_or_empty, 'P', 0)?;
            }
            's' => {
                let parsed: i64 = value_or_empty.trim_start().parse().map_err(|_| {
                    usage_failure(&format!(
                        "invalid number \"{value_or_empty}\" for -s option"
                    ))
                })?;
                if parsed < 1 {
                    self.warnings.push_str(&format!(
                        "xargs: value {parsed} for -s option should be >= 1\n"
                    ));
                }
                self.options.max_chars = usize::try_from(parsed.max(1)).unwrap_or(usize::MAX);
                self.options.size_given = Some(parsed);
            }
            'r' => self.options.no_run_if_empty = true,
            't' => self.options.verbose = true,
            'x' => self.options.exit_on_size = true,
            'h' => self.help = true,
            'v' => self.version = true,
            // The agent has no terminal to prompt on or reopen, and there is no process table.
            _ => return Err(refusal(spelled)),
        }
        Ok(())
    }
}

fn parse(args: &[String]) -> Result<(Parsed, String), Failure> {
    let mut parser = OptionParser {
        options: Options {
            command: Vec::new(),
            delimiter: Delimiter::Quoted,
            eof: None,
            replace: None,
            max_args: None,
            max_lines: None,
            max_chars: DEFAULT_MAX_CHARS,
            size_given: None,
            exit_on_size: false,
            no_run_if_empty: false,
            verbose: false,
            arg_file: None,
        },
        warnings: String::new(),
        help: false,
        version: false,
    };
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if arg == "--" {
            index += 1;
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            let candidates: Vec<&(&str, char, Arity)> =
                match LONG.iter().find(|(full, ..)| *full == name) {
                    Some(exact) => vec![exact],
                    None => LONG
                        .iter()
                        .filter(|(full, ..)| full.starts_with(name))
                        .collect(),
                };
            let (full, option, arity) = match candidates.as_slice() {
                [only] => **only,
                [] => return Err(usage_failure(&format!("unrecognized option '{arg}'"))),
                many => {
                    let names: Vec<String> = many
                        .iter()
                        .map(|(full, ..)| format!("'--{full}'"))
                        .collect();
                    return Err(usage_failure(&format!(
                        "option '--{name}' is ambiguous; possibilities: {}",
                        names.join(" ")
                    )));
                }
            };
            index += 1;
            let value = match arity {
                Arity::Flag if inline.is_some() => {
                    return Err(usage_failure(&format!(
                        "option '--{full}' doesn't allow an argument"
                    )));
                }
                Arity::Flag | Arity::Optional => inline,
                Arity::Required => match inline {
                    Some(value) => Some(value),
                    None => {
                        let value = args.get(index).cloned().ok_or_else(|| {
                            usage_failure(&format!("option '--{full}' requires an argument"))
                        })?;
                        index += 1;
                        Some(value)
                    }
                },
            };
            parser.apply(option, &format!("--{full}"), value)?;
            continue;
        }
        if arg.len() > 1 && arg.starts_with('-') {
            let chars: Vec<char> = arg[1..].chars().collect();
            index += 1;
            let mut position = 0;
            while position < chars.len() {
                let option = chars[position];
                position += 1;
                let rest: String = chars[position..].iter().collect();
                match short_arity(option) {
                    None => {
                        return Err(usage_failure(&format!("invalid option -- '{option}'")));
                    }
                    Some(Arity::Flag) => parser.apply(option, &format!("-{option}"), None)?,
                    Some(Arity::Optional) => {
                        parser.apply(
                            option,
                            &format!("-{option}"),
                            (!rest.is_empty()).then_some(rest),
                        )?;
                        break;
                    }
                    Some(Arity::Required) => {
                        let value = if rest.is_empty() {
                            let value = args.get(index).cloned().ok_or_else(|| {
                                usage_failure(&format!("option requires an argument -- '{option}'"))
                            })?;
                            index += 1;
                            value
                        } else {
                            rest
                        };
                        parser.apply(option, &format!("-{option}"), Some(value))?;
                        break;
                    }
                }
            }
            continue;
        }
        break;
    }
    if parser.help {
        return Ok((Parsed::Help, parser.warnings));
    }
    if parser.version {
        return Ok((Parsed::Version, parser.warnings));
    }
    parser.options.command = args[index..].to_vec();
    if parser.options.command.is_empty() {
        parser.options.command.push("echo".to_string());
    }
    Ok((Parsed::Run(parser.options), parser.warnings))
}

// ------------------------------------------------------------------------------------------------
// Input splitting
// ------------------------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum Item {
    Arg(Vec<u8>),
    /// An input line ended (the unit `-L` counts).
    Line,
    /// The logical end-of-file string was read.
    Stop,
    /// A quote was still open at a line's end or the input's.
    Unmatched {
        quote: u8,
    },
    /// The first NUL in input split on blanks: an argument ends at it, as a C string does.
    Nul,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Space,
    Normal,
    Quote(u8),
    Escape,
}

/// C `isspace` in the C locale: what GNU skips between arguments.
const fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 11 | 12 | b'\r')
}

/// What ends an argument in GNU's input: a blank, space or tab. A carriage return, form feed or
/// vertical tab after an argument's first byte belongs to it.
const fn is_blank(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

/// Push-based GNU input splitter (`read_line`/`read_string` in xargs.c).
struct Splitter {
    delimiter: Delimiter,
    eof: Option<Vec<u8>>,
    replace: bool,
    state: State,
    token: Vec<u8>,
    done: bool,
    /// Whether a NUL has been read (and reported).
    saw_nul: bool,
}

impl Splitter {
    fn new(options: &Options) -> Self {
        Self {
            delimiter: options.delimiter,
            eof: options
                .eof
                .as_ref()
                .map(|eof| super::shell_bytes::encode(eof).into_owned()),
            replace: options.replace.is_some(),
            state: State::Space,
            token: Vec::new(),
            done: false,
            saw_nul: false,
        }
    }

    fn emit(&mut self, items: &mut Vec<Item>) {
        let token = std::mem::take(&mut self.token);
        if self.eof.as_deref() == Some(token.as_slice()) {
            items.push(Item::Stop);
            self.done = true;
        } else {
            items.push(Item::Arg(token));
        }
    }

    fn feed(&mut self, byte: u8, items: &mut Vec<Item>) {
        if self.done {
            return;
        }
        if byte == 0 && self.delimiter == Delimiter::Quoted && !self.saw_nul {
            self.saw_nul = true;
            items.push(Item::Nul);
        }
        if let Delimiter::Byte(delimiter) = self.delimiter {
            if byte == delimiter {
                items.push(Item::Arg(std::mem::take(&mut self.token)));
                items.push(Item::Line);
            } else {
                self.token.push(byte);
            }
            return;
        }
        if self.state == State::Space {
            // Blanks, including newlines after a trailing blank, separate but never end a line.
            if is_space(byte) {
                return;
            }
            self.state = State::Normal;
        }
        match self.state {
            State::Space | State::Normal => {
                if byte == b'\n' {
                    self.state = State::Space;
                    if !self.token.is_empty() {
                        self.emit(items);
                        if !self.done {
                            items.push(Item::Line);
                        }
                    }
                } else if !self.replace && is_blank(byte) {
                    self.state = State::Space;
                    self.emit(items);
                } else {
                    match byte {
                        b'\\' => self.state = State::Escape,
                        b'\'' | b'"' => self.state = State::Quote(byte),
                        _ => self.token.push(byte),
                    }
                }
            }
            State::Quote(quote) => {
                if byte == b'\n' {
                    items.push(Item::Unmatched { quote });
                    self.done = true;
                } else if byte == quote {
                    self.state = State::Normal;
                } else {
                    self.token.push(byte);
                }
            }
            State::Escape => {
                self.state = State::Normal;
                self.token.push(byte);
            }
        }
    }

    fn finish(&mut self, items: &mut Vec<Item>) {
        if self.done {
            return;
        }
        if let State::Quote(quote) = self.state
            && self.delimiter == Delimiter::Quoted
        {
            items.push(Item::Unmatched { quote });
        } else if !self.token.is_empty() {
            if self.delimiter == Delimiter::Quoted {
                self.emit(items);
            } else {
                items.push(Item::Arg(std::mem::take(&mut self.token)));
            }
        }
        self.done = true;
    }
}

// ------------------------------------------------------------------------------------------------
// Running
// ------------------------------------------------------------------------------------------------

enum Flow {
    Continue,
    Stop,
    Exit(u8),
}

enum Source {
    #[cfg_attr(
        not(target_arch = "wasm32"),
        allow(dead_code, reason = "native stdin is read up front")
    )]
    Stream(OpenFile),
    Bytes(Vec<u8>),
}

impl Source {
    async fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Stream(file) => file.async_io().read(buffer).await,
            Self::Bytes(bytes) => {
                let count = bytes.len().min(buffer.len());
                buffer[..count].copy_from_slice(&bytes[..count]);
                bytes.drain(..count);
                Ok(count)
            }
        }
    }
}

struct Xargs<'a, SE: ShellExtensions> {
    context: ExecutionContext<'a, SE>,
    options: Options,
    stderr: OpenFile,
    child_params: ExecutionParameters,
    initial_bytes: usize,
    pending: Vec<String>,
    pending_bytes: usize,
    lines: usize,
    ran: bool,
    failed: bool,
}

impl<SE: ShellExtensions> Xargs<'_, SE> {
    async fn say(&mut self, message: &str) -> Result<(), Error> {
        self.stderr
            .async_io()
            .write_all(format!("xargs: {message}\n").as_bytes())
            .await?;
        Ok(())
    }

    async fn item(&mut self, item: Item) -> Result<Flow, Error> {
        match item {
            Item::Arg(mut bytes) => {
                // Every byte of the item reaches the command, UTF-8 or not, up to a NUL, where
                // a C string ends. Under -I the NUL ends the command's word the item goes into.
                if self.options.replace.is_none()
                    && let Some(end) = bytes.iter().position(|&byte| byte == 0)
                {
                    bytes.truncate(end);
                }
                let arg = super::shell_bytes::decode_vec(bytes);
                if self.options.replace.is_some() {
                    self.replace(&arg).await
                } else {
                    self.push(arg).await
                }
            }
            Item::Nul => {
                self.say(
                    "WARNING: a NUL character occurred in the input.  It cannot be passed \
                     through in the argument list.  Did you mean to use the --null option?",
                )
                .await?;
                Ok(Flow::Continue)
            }
            Item::Line => {
                if let Some(limit) = self.options.max_lines
                    && self.options.replace.is_none()
                {
                    self.lines += 1;
                    if self.lines >= limit {
                        self.lines = 0;
                        if !self.pending.is_empty() {
                            return self.flush().await;
                        }
                    }
                }
                Ok(Flow::Continue)
            }
            Item::Stop => Ok(Flow::Stop),
            // As GNU's does, xargs runs the command on the arguments before the quote, then
            // reports it: at a line's end or the input's.
            Item::Unmatched { quote } => {
                if !self.pending.is_empty()
                    && let Flow::Exit(code) = self.flush().await?
                {
                    return Ok(Flow::Exit(code));
                }
                let kind = if quote == b'"' { "double" } else { "single" };
                self.say(&format!(
                    "unmatched {kind} quote; by default quotes are special to xargs unless you \
                     use the -0 option"
                ))
                .await?;
                Ok(Flow::Exit(1))
            }
        }
    }

    async fn push(&mut self, arg: String) -> Result<Flow, Error> {
        let size = word_size(&arg);
        if self.pending_bytes + size > self.options.max_chars {
            let limited = self.options.max_args.is_some() || self.options.max_lines.is_some();
            if self.pending.is_empty() {
                self.say("argument line too long").await?;
                return Ok(Flow::Exit(1));
            }
            if self.options.exit_on_size && limited {
                self.say("argument list too long").await?;
                return Ok(Flow::Exit(1));
            }
            if let Flow::Exit(code) = self.flush().await? {
                return Ok(Flow::Exit(code));
            }
        }
        self.pending_bytes += size;
        self.pending.push(arg);
        if self
            .options
            .max_args
            .is_some_and(|limit| self.pending.len() >= limit)
        {
            return self.flush().await;
        }
        Ok(Flow::Continue)
    }

    async fn replace(&mut self, item: &str) -> Result<Flow, Error> {
        let pattern = self.options.replace.clone().unwrap_or_default();
        let words: Vec<String> = self
            .options
            .command
            .iter()
            .map(|word| {
                let mut word = if pattern.is_empty() {
                    word.clone()
                } else {
                    word.replace(&pattern, item)
                };
                if let Some(end) = word.find('\0') {
                    word.truncate(end);
                }
                word
            })
            .collect();
        if words.iter().map(|word| word_size(word)).sum::<usize>() > self.options.max_chars {
            self.say("argument list too long").await?;
            return Ok(Flow::Exit(1));
        }
        self.run(words).await
    }

    async fn flush(&mut self) -> Result<Flow, Error> {
        let mut words = self.options.command.clone();
        words.append(&mut self.pending);
        self.pending_bytes = self.initial_bytes;
        self.run(words).await
    }

    async fn run(&mut self, words: Vec<String>) -> Result<Flow, Error> {
        self.ran = true;
        if self.options.verbose {
            let line: Vec<String> = words.iter().map(|word| verbose_quote(word)).collect();
            let line = format!("{}\n", line.join(" "));
            self.stderr
                .async_io()
                .write_all(&super::shell_bytes::encode(&line))
                .await?;
        }
        let cwd = self.context.shell.working_dir().to_path_buf();
        match lookup(self.context.shell, &words[0], &cwd) {
            Lookup::Found => {}
            Lookup::Missing => {
                self.say(&format!("{}: No such file or directory", words[0]))
                    .await?;
                return Ok(Flow::Exit(127));
            }
            Lookup::Denied => {
                self.say(&format!("{}: Permission denied", words[0]))
                    .await?;
                return Ok(Flow::Exit(126));
            }
        }
        let result = run_child(
            &mut *self.context.shell,
            &self.child_params,
            None,
            command_line(&words),
            None,
            None,
            false,
        )
        .await?;
        if let Some(signal) = result.terminating_signal {
            self.say(&format!("{}: terminated by signal {signal}", words[0]))
                .await?;
            return Ok(Flow::Exit(125));
        }
        match u8::from(result.exit_code) {
            0 => {}
            255 => {
                self.say(&format!("{}: exited with status 255; aborting", words[0]))
                    .await?;
                return Ok(Flow::Exit(124));
            }
            _ => self.failed = true,
        }
        Ok(Flow::Continue)
    }

    async fn consume(&mut self, mut source: Source) -> Result<u8, Error> {
        let mut splitter = Splitter::new(&self.options);
        let mut buffer = vec![0; 16 * 1024];
        let mut items = Vec::new();
        'input: loop {
            let count = source.read(&mut buffer).await?;
            if count == 0 {
                splitter.finish(&mut items);
            } else {
                for &byte in &buffer[..count] {
                    splitter.feed(byte, &mut items);
                }
            }
            for item in std::mem::take(&mut items) {
                match self.item(item).await? {
                    Flow::Continue => {}
                    Flow::Stop => break 'input,
                    Flow::Exit(code) => return Ok(code),
                }
            }
            if count == 0 {
                break;
            }
        }
        // GNU runs the command once even for empty input unless -r (never with -I).
        let run_once = !self.ran && !self.options.no_run_if_empty && self.options.replace.is_none();
        if (!self.pending.is_empty() || run_once)
            && let Flow::Exit(code) = self.flush().await?
        {
            return Ok(code);
        }
        Ok(if self.failed { 123 } else { 0 })
    }
}

/// Standard input without ever touching the real WASI stdin resource.
fn stdin_source<SE: ShellExtensions>(context: &ExecutionContext<'_, SE>) -> Source {
    #[cfg(target_arch = "wasm32")]
    {
        Source::Stream(super::streaming::input(context))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut bytes = Vec::new();
        let _ = std::io::Read::read_to_end(&mut super::coreutils::tool_stdin(context), &mut bytes);
        Source::Bytes(bytes)
    }
}

const HELP: &str = "\
Usage: xargs [OPTION]... COMMAND [INITIAL-ARGS]...
Run COMMAND with arguments INITIAL-ARGS and more arguments read from input.

  -0, --null                   items are separated by a null, not whitespace;
                                 disables quote and backslash processing and
                                 logical EOF processing
  -a, --arg-file=FILE          read arguments from FILE, not standard input
  -d, --delimiter=CHARACTER    items in input stream are separated by CHARACTER,
                                 not by whitespace; disables quote and backslash
                                 processing and logical EOF processing
  -E END                       set logical EOF string; if END occurs as a line
                                 of input, the rest of the input is ignored
  -e, --eof[=END]              equivalent to -E END if END is specified;
                                 otherwise, there is no end-of-file string
  -I R                         same as --replace=R
  -i, --replace[=R]            replace R in INITIAL-ARGS with names read
                                 from standard input, split at newlines;
                                 if R is unspecified, assume {}
  -L, --max-lines=MAX-LINES    use at most MAX-LINES non-blank input lines per
                                 command line
  -l[MAX-LINES]                similar to -L but defaults to at most one non-
                                 blank input line if MAX-LINES is not specified
  -n, --max-args=MAX-ARGS      use at most MAX-ARGS arguments per command line
  -P, --max-procs=MAX-PROCS    accepted; commands always run one at a time
  -r, --no-run-if-empty        if there are no arguments, then do not run COMMAND;
                                 if this option is not given, COMMAND will be
                                 run at least once
  -s, --max-chars=MAX-CHARS    limit length of command line to MAX-CHARS
  -t, --verbose                print commands before executing them
  -x, --exit                   exit if the size (see -s) is exceeded
      --help                   display this help and exit
      --version                output version information and exit

COMMAND runs through the shell as a child command (builtins and functions included).
Not supported: -p/--interactive, -o/--open-tty, --show-limits, --process-slot-var.
";

fn content(name: &str, content_type: ContentType, _: &ContentOptions) -> Result<String, Error> {
    match content_type {
        ContentType::ShortDescription => Ok(format!(
            "{name} - build and run command lines from standard input\n"
        )),
        ContentType::ShortUsage => Ok(format!(
            "{name}: {name} [OPTION]... COMMAND [INITIAL-ARGS]...\n"
        )),
        ContentType::DetailedHelp => Ok(HELP.to_string()),
        ContentType::ManPage => brush_core::error::unimp("man page not yet implemented"),
    }
}

fn execute<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let args: Vec<String> = args
            .into_iter()
            .skip(1)
            .map(|arg| arg.to_string())
            .collect();
        let mut stderr = context.stderr();
        let (parsed, warnings) = match parse(&args) {
            Ok(parsed) => parsed,
            Err(failure) => {
                stderr
                    .async_io()
                    .write_all(&super::shell_bytes::encode(&failure.text))
                    .await?;
                return Ok(ExecutionResult::new(failure.code));
            }
        };
        stderr.async_io().write_all(warnings.as_bytes()).await?;
        let mut options = match parsed {
            Parsed::Run(options) => options,
            Parsed::Help => {
                context
                    .stdout()
                    .async_io()
                    .write_all(HELP.as_bytes())
                    .await?;
                return Ok(ExecutionResult::success());
            }
            Parsed::Version => {
                context
                    .stdout()
                    .async_io()
                    .write_all(b"xargs (bash-tool builtin, GNU findutils compatible)\n")
                    .await?;
                return Ok(ExecutionResult::success());
            }
        };
        // GNU's ceiling for -s: 128 KiB, less 2 KiB of headroom and what the environment takes.
        let environment: usize = super::coreutils::exported_env(&context)
            .iter()
            .map(|(name, value)| name.len() + value.len() + 2)
            .sum();
        let ceiling = (128 * 1024 - 2048_usize).saturating_sub(environment);
        if let Some(given) = options.size_given
            && i64::try_from(ceiling).is_ok_and(|ceiling| given > ceiling)
        {
            let warning = format!("xargs: value {given} for -s option should be <= {ceiling}\n");
            stderr.async_io().write_all(warning.as_bytes()).await?;
            options.max_chars = ceiling;
        }
        let initial_bytes: usize = options.command.iter().map(|word| word_size(word)).sum();
        if options.replace.is_none() && initial_bytes > options.max_chars {
            stderr
                .async_io()
                .write_all(b"xargs: cannot fit single argument within argument list size limit\n")
                .await?;
            return Ok(ExecutionResult::new(1));
        }
        // Children read /dev/null unless the items come from -a, as in GNU xargs.
        let mut child_params = context.params.clone();
        let source = if let Some(file) = &options.arg_file {
            match arg_file_source(&context, file) {
                Ok(source) => source,
                Err(error) => {
                    let message = format!(
                        "xargs: Cannot open input file \u{2018}{file}\u{2019}: {}\n",
                        super::io_message(&error)
                    );
                    stderr.async_io().write_all(message.as_bytes()).await?;
                    return Ok(ExecutionResult::new(1));
                }
            }
        } else {
            child_params.set_fd(
                OpenFiles::STDIN_FD,
                brush_core::openfiles::from_bytes(Vec::new()),
            );
            stdin_source(&context)
        };
        let mut xargs = Xargs {
            context,
            options,
            stderr,
            child_params,
            initial_bytes,
            pending: Vec::new(),
            pending_bytes: initial_bytes,
            lines: 0,
            ran: false,
            failed: false,
        };
        let code = xargs.consume(source).await?;
        Ok(ExecutionResult::new(code))
    })
}

/// `-a FILE`'s items: the file's bytes, or, when FILE names one of this command's streams
/// (`/dev/stdin`), that stream, as on Linux.
fn arg_file_source<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    file: &str,
) -> std::io::Result<Source> {
    #[cfg(target_arch = "wasm32")]
    if let Some(stream) = super::streaming::standard_stream(context, file) {
        return Ok(Source::Stream(super::streaming::opened_again(stream)));
    }
    super::read_file(&context.shell.absolute_path(Path::new(file))).map(Source::Bytes)
}

#[cfg(target_arch = "wasm32")]
fn execute_utility<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    super::streaming::utility(context, args, execute::<SE>)
}

pub(crate) fn builtins<SE: ShellExtensions>() -> Vec<(String, Registration<SE>)> {
    let registration = Registration {
        execute_func: execute::<SE>,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
        execution_boundary: ExecutionBoundary::Command,
    };
    #[cfg(target_arch = "wasm32")]
    let registration = Registration {
        execute_func: execute_utility::<SE>,
        ..registration
    };
    vec![("xargs".into(), registration)]
}

pub(crate) fn manifests() -> Vec<Manifest> {
    vec![Manifest::builtin("xargs", "build and run command lines from standard input").with_help(
        "xargs [-0] [-d DELIM] [-a FILE] [-n MAX-ARGS] [-L MAX-LINES] [-I REPLACE] [-E EOF] [-s \
         MAX-CHARS] [-r] [-t] [-x] [-P N] [COMMAND [ARG...]] — GNU xargs: split input on blanks \
         (honouring quotes and backslashes), NULs (-0) or DELIM, and run COMMAND (default echo) \
         with them appended. Without -r the command runs once even for empty input. Exit 123 if \
         any command failed, 124 on status 255, 125 if killed by a signal, 127 if not found. \
         Commands run one at a time as child shell commands (builtins and functions included); \
         -P is accepted and -p is refused.",
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(options: &Options, input: &[u8]) -> Vec<Item> {
        let mut splitter = Splitter::new(options);
        let mut items = Vec::new();
        for &byte in input {
            splitter.feed(byte, &mut items);
        }
        splitter.finish(&mut items);
        items
    }

    fn options(args: &[&str]) -> Options {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        match parse(&args) {
            Ok((Parsed::Run(options), _)) => options,
            _ => panic!("expected options for {args:?}"),
        }
    }

    fn failure(args: &[&str]) -> String {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        parse(&args)
            .err()
            .map(|failure| failure.text)
            .unwrap_or_default()
    }

    fn args(items: &[Item]) -> Vec<String> {
        items
            .iter()
            .filter_map(|item| match item {
                Item::Arg(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn quoting_passes_plain_words_and_wraps_the_rest() {
        assert_eq!(shell_quote("plain-word.txt"), "plain-word.txt");
        assert_eq!(shell_quote("has space"), "'has space'");
        assert_eq!(shell_quote("can't"), r"'can'\''t'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(
            command_line(&["a=b".to_string(), "x=y".to_string()]),
            "'a=b' x=y"
        );
        assert_eq!(command_line(&["if".to_string()]), "'if'");
    }

    #[test]
    fn verbose_quoting_matches_gnu() {
        assert_eq!(verbose_quote("a b"), "'a b'");
        assert_eq!(verbose_quote("it's"), "\"it's\"");
        assert_eq!(verbose_quote("it's $x"), r"'it'\''s $x'");
        assert_eq!(verbose_quote("a\tb"), r"'a'$'\t''b'");
        assert_eq!(verbose_quote("\u{1}"), r"''$'\001'");
        assert_eq!(verbose_quote(""), "''");
        assert_eq!(verbose_quote("x=y"), "'x=y'");
        assert_eq!(verbose_quote("é"), "é");
        assert_eq!(verbose_quote("#x"), "'#x'");
        assert_eq!(verbose_quote("x#"), "x#");
        assert_eq!(verbose_quote("{}"), "{}");
        assert_eq!(verbose_quote("{"), "'{'");
        assert_eq!(verbose_quote("a]b"), "a]b");
    }

    #[test]
    fn default_splitting_honours_quotes_and_lines() {
        let default = options(&[]);
        assert_eq!(
            args(&split(&default, b"'a b' \"c d\" e\\ f\n")),
            vec!["a b", "c d", "e f"]
        );
        assert_eq!(args(&split(&default, b"a\n\n b")), vec!["a", "b"]);
        assert_eq!(
            split(&default, b"'a b\n"),
            vec![Item::Unmatched { quote: b'\'' }]
        );
        let lines = split(&default, b"a b\nc d \ne\n");
        let count = lines.iter().filter(|item| **item == Item::Line).count();
        assert_eq!(count, 2, "a trailing blank continues the line");
        let replace = options(&["-I", "{}"]);
        assert_eq!(
            args(&split(&replace, b"  a b  \n\n c \n")),
            vec!["a b  ", "c "]
        );
        let eof = options(&["-E", "STOP"]);
        assert_eq!(args(&split(&eof, b"a STOP b")), vec!["a"]);
    }

    #[test]
    fn delimited_splitting_keeps_empty_items() {
        let null = options(&["-0"]);
        assert_eq!(args(&split(&null, b"a\0\0b c\0")), vec!["a", "", "b c"]);
        let colon = options(&["-d:"]);
        assert_eq!(args(&split(&colon, b"a::b\n")), vec!["a", "", "b\n"]);
        assert_eq!(delimiter("\\n").ok(), Some(b'\n'));
        assert_eq!(delimiter("\\x41").ok(), Some(b'A'));
        assert_eq!(delimiter("\\040").ok(), Some(b' '));
        assert_eq!(delimiter("\\x").ok(), Some(0));
        assert!(delimiter("ab").is_err());
        assert!(delimiter("").is_err());
    }

    #[test]
    fn option_parsing_follows_getopt() {
        let parsed = options(&["-0rt", "-n1", "echo", "-n"]);
        assert_eq!(parsed.delimiter, Delimiter::Byte(0));
        assert!(parsed.no_run_if_empty && parsed.verbose);
        assert_eq!(parsed.max_args, Some(1));
        assert_eq!(parsed.command, vec!["echo", "-n"]);
        assert_eq!(options(&["-i"]).replace.as_deref(), Some("{}"));
        assert_eq!(options(&["--replace=X"]).replace.as_deref(), Some("X"));
        assert_eq!(options(&["-l"]).max_lines, Some(1));
        assert_eq!(options(&["-eb"]).eof.as_deref(), Some("b"));
        assert_eq!(options(&["--max-a=3"]).max_args, Some(3));
        assert_eq!(options(&["--", "-x"]).command, vec!["-x"]);
        assert_eq!(
            failure(&["-Z"]),
            format!("xargs: invalid option -- 'Z'\n{TRY_HELP}\n")
        );
        assert_eq!(
            failure(&["--bogus"]),
            format!("xargs: unrecognized option '--bogus'\n{TRY_HELP}\n")
        );
        assert_eq!(
            failure(&["-n"]),
            format!("xargs: option requires an argument -- 'n'\n{TRY_HELP}\n")
        );
        assert_eq!(
            failure(&["-n", "0"]),
            format!("xargs: value 0 for -n option should be >= 1\n{TRY_HELP}\n")
        );
        assert_eq!(
            failure(&["-n", "1x"]),
            format!("xargs: invalid number \"1x\" for -n option\n{TRY_HELP}\n")
        );
        assert_eq!(failure(&["-p"]), "xargs: -p is unsupported in bash-tool\n");
        let args: Vec<String> = ["-L1", "-n1"]
            .iter()
            .map(|arg| (*arg).to_string())
            .collect();
        let (_, warnings) = parse(&args).unwrap();
        assert_eq!(
            warnings,
            "xargs: warning: options --max-lines and --max-args/-n are mutually exclusive, \
             ignoring previous --max-lines value\n"
        );
    }
}
