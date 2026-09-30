//! `grep`: GNU grep's command line over a line-at-a-time matcher.
//!
//! [`parse`] and [`Search`] serve both drivers: the native builtin and the cooperative WASM
//! driver feed [`Search::record`] one line at a time, so `-q` and `-m` stop an endless producer.
//! Patterns follow GNU dialects: basic (default), extended (`-E`), fixed (`-F`) and Perl (`-P`).
//! GNU grep 3.12 is the reference for output bytes, diagnostics and exit statuses.

use std::collections::{HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const USAGE: &str = "Usage: grep [OPTION]... PATTERNS [FILE]...\n";
const TRY: &str = "Try 'grep --help' for more information.\n";

const HELP: &str = "Usage: grep [OPTION]... PATTERNS [FILE]...
Search for PATTERNS in each FILE.
Example: grep -i 'hello world' menu.h main.c
PATTERNS can contain multiple patterns separated by newlines.

Pattern selection and interpretation:
  -E, --extended-regexp     PATTERNS are extended regular expressions
  -F, --fixed-strings       PATTERNS are strings
  -G, --basic-regexp        PATTERNS are basic regular expressions
  -P, --perl-regexp         PATTERNS are Perl-style regular expressions (fancy-regex)
  -e, --regexp=PATTERNS     use PATTERNS for matching
  -f, --file=FILE           take PATTERNS from FILE
  -i, --ignore-case         ignore case distinctions in patterns and data
      --no-ignore-case      do not ignore case distinctions (default)
  -w, --word-regexp         match only whole words
  -x, --line-regexp         match only whole lines
  -z, --null-data           a data line ends in 0 byte, not newline

Miscellaneous:
  -s, --no-messages         suppress error messages
  -v, --invert-match        select non-matching lines
  -V, --version             display version information and exit
      --help                display this help text and exit

Output control:
  -m, --max-count=NUM       stop after NUM selected lines
  -b, --byte-offset         print the byte offset with output lines
  -n, --line-number         print line number with output lines
      --line-buffered       accepted; output is flushed per line anyway
  -H, --with-filename       print file name with output lines
  -h, --no-filename         suppress the file name prefix on output
      --label=LABEL         use LABEL as the standard input file name prefix
  -o, --only-matching       show only nonempty parts of lines that match
  -q, --quiet, --silent     suppress all normal output
      --binary-files=TYPE   assume that binary files are TYPE;
                            TYPE is 'binary', 'text', or 'without-match'
  -a, --text                equivalent to --binary-files=text
  -I                        equivalent to --binary-files=without-match
  -d, --directories=ACTION  how to handle directories;
                            ACTION is 'read', 'recurse', or 'skip'
  -r, --recursive           like --directories=recurse
  -R, --dereference-recursive  likewise, but follow all symlinks
      --include=GLOB        search only files that match GLOB (a file pattern)
      --exclude=GLOB        skip files that match GLOB
      --exclude-dir=GLOB    skip directories that match GLOB
  -L, --files-without-match  print only names of FILEs with no selected lines
  -l, --files-with-matches  print only names of FILEs with selected lines
  -c, --count               print only a count of selected lines per FILE
  -T, --initial-tab         make tabs line up (if needed)
  -Z, --null                print 0 byte after FILE name

Context control:
  -B, --before-context=NUM  print NUM lines of leading context
  -A, --after-context=NUM   print NUM lines of trailing context
  -C, --context=NUM         print NUM lines of output context
  -NUM                      same as --context=NUM
      --group-separator=SEP  print SEP on line between matches with context
      --no-group-separator  do not print separator for matches with context
      --color[=WHEN],
      --colour[=WHEN]       use markers to highlight the matching strings;
                            WHEN is 'always', 'never', or 'auto'

When FILE is '-', read standard input.  With no FILE, read '.' if
recursive, '-' otherwise.  Exit status is 0 if any line is selected,
1 otherwise; if any error occurs and -q is not given, the exit status is 2.
";

/// A refusal before any input is read, formatted for stderr, with grep's status.
pub(crate) struct Refusal {
    pub(crate) code: i32,
    pub(crate) message: String,
}

fn usage_error(message: impl AsRef<str>) -> Refusal {
    Refusal {
        code: 2,
        message: format!("grep: {}\n{USAGE}{TRY}", message.as_ref()),
    }
}

/// GNU accepts the same dialect flag (`-E -E`, `--perl-regexp --perl-regexp`, ...) repeated,
/// but refuses two *different* ones (`-G -E`, `-E -F`, ...) with a bare one-line diagnostic —
/// no usage/`Try...` follow-up, unlike most of this parser's other refusals. Verified against
/// the oracle for every unordered pair of `-E`/`-F`/`-G`/`-P`.
fn set_dialect(explicit: &mut Option<Dialect>, new: Dialect) -> Result<Dialect, Refusal> {
    if let Some(prev) = *explicit
        && prev != new
    {
        return Err(Refusal {
            code: 2,
            message: "grep: conflicting matchers specified\n".to_owned(),
        });
    }
    *explicit = Some(new);
    Ok(new)
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Dialect {
    #[default]
    Basic,
    Extended,
    Fixed,
    Perl,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
/// `--binary-files`: how files containing NUL bytes are treated.
enum BinaryFiles {
    /// Report "binary file matches" instead of lines.
    #[default]
    Report,
    Text,
    WithoutMatch,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Directories {
    #[default]
    Read,
    Skip,
    Recurse,
}

/// What the command line asks for.
pub(crate) enum Parsed {
    Run(Box<Options>),
    Help,
    Version,
}

#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one switch per GNU grep option"
)]
pub(crate) struct Options {
    dialect: Dialect,
    patterns: Option<Vec<String>>,
    pattern_files: Vec<String>,
    ignore_case: bool,
    word: bool,
    line: bool,
    null_data: bool,
    no_messages: bool,
    invert: bool,
    max_count: Option<u64>,
    byte_offset: bool,
    line_number: bool,
    filename: Option<bool>,
    label: Option<String>,
    only_matching: bool,
    quiet: bool,
    binary: BinaryFiles,
    directories: Directories,
    follow_links: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    exclude_dir: Vec<String>,
    files_without_match: bool,
    files_with_matches: bool,
    count: bool,
    initial_tab: bool,
    null: bool,
    before: usize,
    after: usize,
    /// Whether `-A`/`-B`/`-C`/`-NUM` was given at all, distinct from `before`/`after` both
    /// being nonzero: `-A0`/`-B0`/`-C0` (or `-0`) still turn on the `--` group separator
    /// between non-adjacent matches, even though there is then no actual context to print.
    context_requested: bool,
    group_separator: Option<String>,
    /// `--color=always` (or `--colour`, or a bare `--color`/`--color=auto` when GNU would pick
    /// it because stdout is a terminal): real SGR highlighting, using GNU's own `GREP_COLORS`
    /// defaults (see `sgr` below) rather than reading the environment variable itself. `auto`
    /// and `never` both leave this false, since this tool's stdout is never a terminal.
    color: bool,
    /// Operands; empty means `-`, or `.` when recursive.
    pub(crate) files: Vec<String>,
}

/// GNU's default `GREP_COLORS` SGR sequences (`grep.c`'s own `"ms=01;31:mc=01;31:sl=:cx=:fn=35:
/// ln=32:se=36:mt=01;31"`), each wrapped exactly as GNU emits them: `\033[<sgr>m\033[K` opens
/// (the trailing `\033[K` clears to end of line, in case a color has a background), and every
/// opened span is closed the same way, with `\033[m\033[K` — verified byte-for-byte against the
/// oracle (`grep --color=always o <<<'hi' | cat -v`).
mod sgr {
    pub(super) const MATCH: &[u8] = b"\x1b[01;31m\x1b[K";
    pub(super) const FILENAME: &[u8] = b"\x1b[35m\x1b[K";
    pub(super) const LINE_NUMBER: &[u8] = b"\x1b[32m\x1b[K";
    pub(super) const SEPARATOR: &[u8] = b"\x1b[36m\x1b[K";
    pub(super) const RESET: &[u8] = b"\x1b[m\x1b[K";
}

/// Append `text` to `out`, wrapped in `start`/`sgr::RESET` when `color` is set.
fn colored(out: &mut Vec<u8>, color: bool, start: &[u8], text: &[u8]) {
    if color {
        out.extend_from_slice(start);
    }
    out.extend_from_slice(text);
    if color {
        out.extend_from_slice(sgr::RESET);
    }
}

impl Options {
    pub(crate) fn recursive(&self) -> bool {
        self.directories == Directories::Recurse
    }

    /// `-s`: file errors are not reported, though they still set the status.
    pub(crate) const fn no_messages(&self) -> bool {
        self.no_messages
    }
}

fn number(option: &str, value: &str) -> Result<u64, Refusal> {
    value.parse().map_err(|_| Refusal {
        code: 2,
        message: match option {
            "m" | "max-count" => "grep: invalid max count\n".to_owned(),
            _ => format!("grep: {value}: invalid context length argument\n"),
        },
    })
}

/// Parse GNU grep's command line (`argv[0]` is the command name). Options may follow operands.
#[allow(clippy::too_many_lines, reason = "one arm per GNU grep option")]
pub(crate) fn parse(argv: &[String]) -> Result<Parsed, Refusal> {
    let mut options = Options {
        group_separator: Some("--".into()),
        ..Options::default()
    };
    let mut context: Option<usize> = None;
    let mut before: Option<usize> = None;
    let mut after: Option<usize> = None;
    let mut explicit_dialect: Option<Dialect> = None;
    let mut operands = Vec::new();
    let mut args = argv.iter().skip(1).cloned();
    while let Some(arg) = args.next() {
        if arg == "--" {
            operands.extend(args.by_ref());
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (long, None),
            };
            let value = |args: &mut dyn Iterator<Item = String>| -> Result<String, Refusal> {
                inline
                    .clone()
                    .or_else(|| args.next())
                    .ok_or_else(|| usage_error(format!("option '--{name}' requires an argument")))
            };
            match name {
                "regexp" => options
                    .patterns
                    .get_or_insert_with(Vec::new)
                    .push(value(&mut args)?),
                "file" => options.pattern_files.push(value(&mut args)?),
                "extended-regexp" => {
                    options.dialect = set_dialect(&mut explicit_dialect, Dialect::Extended)?;
                }
                "fixed-strings" => {
                    options.dialect = set_dialect(&mut explicit_dialect, Dialect::Fixed)?;
                }
                "basic-regexp" => {
                    options.dialect = set_dialect(&mut explicit_dialect, Dialect::Basic)?;
                }
                "perl-regexp" => {
                    options.dialect = set_dialect(&mut explicit_dialect, Dialect::Perl)?;
                }
                "ignore-case" => options.ignore_case = true,
                "no-ignore-case" => options.ignore_case = false,
                "word-regexp" => options.word = true,
                "line-regexp" => options.line = true,
                "null-data" => options.null_data = true,
                "no-messages" => options.no_messages = true,
                "invert-match" => options.invert = true,
                "version" => return Ok(Parsed::Version),
                "help" => return Ok(Parsed::Help),
                "max-count" => options.max_count = Some(number("max-count", &value(&mut args)?)?),
                "byte-offset" => options.byte_offset = true,
                "line-number" => options.line_number = true,
                "line-buffered" | "unix-byte-offsets" => (),
                "with-filename" => options.filename = Some(true),
                "no-filename" => options.filename = Some(false),
                "label" => options.label = Some(value(&mut args)?),
                "only-matching" => options.only_matching = true,
                "quiet" | "silent" => options.quiet = true,
                "binary-files" => {
                    options.binary = match value(&mut args)?.as_str() {
                        "binary" => BinaryFiles::Report,
                        "text" => BinaryFiles::Text,
                        "without-match" => BinaryFiles::WithoutMatch,
                        _ => return Err(usage_error("invalid argument for --binary-files")),
                    }
                }
                "text" => options.binary = BinaryFiles::Text,
                "directories" => {
                    options.directories = match value(&mut args)?.as_str() {
                        "read" => Directories::Read,
                        "skip" => Directories::Skip,
                        "recurse" => Directories::Recurse,
                        _ => return Err(usage_error("invalid argument for --directories")),
                    }
                }
                "devices" => {
                    value(&mut args)?;
                }
                "recursive" => options.directories = Directories::Recurse,
                "dereference-recursive" => {
                    options.directories = Directories::Recurse;
                    options.follow_links = true;
                }
                "include" => options.include.push(value(&mut args)?),
                "exclude" => options.exclude.push(value(&mut args)?),
                "exclude-dir" => options.exclude_dir.push(value(&mut args)?),
                "files-without-match" => options.files_without_match = true,
                "files-with-matches" => options.files_with_matches = true,
                "count" => options.count = true,
                "initial-tab" => options.initial_tab = true,
                "null" => options.null = true,
                "before-context" => before = Some(context_number(&value(&mut args)?)?),
                "after-context" => after = Some(context_number(&value(&mut args)?)?),
                "context" => context = Some(context_number(&value(&mut args)?)?),
                "group-separator" => options.group_separator = Some(value(&mut args)?),
                "no-group-separator" => options.group_separator = None,
                "color" | "colour" => {
                    options.color = match inline.as_deref() {
                        // A bare `--color`/`--colour` means `auto`, same as GNU.
                        None | Some("auto" | "tty" | "if-tty") | Some("never" | "no" | "none") => {
                            // `auto` would light up on a real terminal; this tool's stdout
                            // never is one, so both `auto` and `never` land here as off.
                            false
                        }
                        Some("always" | "yes" | "force") => true,
                        // GNU's own quirk, verified against the oracle: an unrecognized WHEN
                        // does not raise a usage error here — it just falls through to
                        // printing the help text (status 0), the same as `--help` itself.
                        Some(_) => return Ok(Parsed::Help),
                    };
                }
                _ => return Err(usage_error(format!("unrecognized option '--{name}'"))),
            }
            continue;
        }
        let Some(cluster) = arg.strip_prefix('-').filter(|rest| !rest.is_empty()) else {
            operands.push(arg);
            continue;
        };
        let chars: Vec<char> = cluster.chars().collect();
        let mut index = 0;
        let mut digits = String::new();
        while index < chars.len() {
            let flag = chars[index];
            index += 1;
            if flag.is_ascii_digit() {
                digits.push(flag);
                continue;
            }
            // Short options with an argument take the rest of the cluster or the next argument.
            let mut take = |args: &mut dyn Iterator<Item = String>| -> Result<String, Refusal> {
                let rest: String = chars[index..].iter().collect();
                index = chars.len();
                if rest.is_empty() {
                    args.next().ok_or_else(|| {
                        usage_error(format!("option requires an argument -- '{flag}'"))
                    })
                } else {
                    Ok(rest)
                }
            };
            match flag {
                'e' => options
                    .patterns
                    .get_or_insert_with(Vec::new)
                    .push(take(&mut args)?),
                'f' => options.pattern_files.push(take(&mut args)?),
                'm' => options.max_count = Some(number("m", &take(&mut args)?)?),
                'A' => after = Some(context_number(&take(&mut args)?)?),
                'B' => before = Some(context_number(&take(&mut args)?)?),
                'C' => context = Some(context_number(&take(&mut args)?)?),
                'd' => {
                    options.directories = match take(&mut args)?.as_str() {
                        "read" => Directories::Read,
                        "skip" => Directories::Skip,
                        "recurse" => Directories::Recurse,
                        _ => return Err(usage_error("invalid argument for --directories")),
                    }
                }
                'D' => {
                    take(&mut args)?;
                }
                'E' => options.dialect = set_dialect(&mut explicit_dialect, Dialect::Extended)?,
                'F' => options.dialect = set_dialect(&mut explicit_dialect, Dialect::Fixed)?,
                'G' => options.dialect = set_dialect(&mut explicit_dialect, Dialect::Basic)?,
                'P' => options.dialect = set_dialect(&mut explicit_dialect, Dialect::Perl)?,
                'i' | 'y' => options.ignore_case = true,
                'w' => options.word = true,
                'x' => options.line = true,
                'z' => options.null_data = true,
                's' => options.no_messages = true,
                'v' => options.invert = true,
                'V' => return Ok(Parsed::Version),
                'b' => options.byte_offset = true,
                'n' => options.line_number = true,
                'H' => options.filename = Some(true),
                'h' => options.filename = Some(false),
                'o' => options.only_matching = true,
                'q' => options.quiet = true,
                'a' => options.binary = BinaryFiles::Text,
                'I' => options.binary = BinaryFiles::WithoutMatch,
                'r' => options.directories = Directories::Recurse,
                'R' => {
                    options.directories = Directories::Recurse;
                    options.follow_links = true;
                }
                'L' => options.files_without_match = true,
                'l' => options.files_with_matches = true,
                'c' => options.count = true,
                'T' => options.initial_tab = true,
                'Z' => options.null = true,
                'U' | 'u' => (),
                other => return Err(usage_error(format!("invalid option -- '{other}'"))),
            }
        }
        if !digits.is_empty() {
            context = Some(context_number(&digits)?);
        }
    }
    options.before = before.or(context).unwrap_or(0);
    options.after = after.or(context).unwrap_or(0);
    options.context_requested = before.is_some() || after.is_some() || context.is_some();
    if options.patterns.is_none() && options.pattern_files.is_empty() {
        if operands.is_empty() {
            return Err(Refusal {
                code: 2,
                message: format!("{USAGE}{TRY}"),
            });
        }
        options.patterns = Some(vec![operands.remove(0)]);
    }
    options.files = operands;
    Ok(Parsed::Run(Box::new(options)))
}

fn context_number(value: &str) -> Result<usize, Refusal> {
    value.parse().map_err(|_| Refusal {
        code: 2,
        message: format!("grep: {value}: invalid context length argument\n"),
    })
}

pub(crate) const fn help() -> &'static str {
    HELP
}

pub(crate) const fn version() -> &'static str {
    "grep (bash-tool) 3.12-compatible\n"
}

/// The result of reading the command line.
pub(crate) enum Prepared {
    Run(Box<Options>, Matcher),
    /// `--help` or `--version` output.
    Done(&'static str),
}

/// Parse and compile. `read` loads `-f` pattern files.
pub(crate) fn prepare(
    argv: &[String],
    utf8: bool,
    read: &dyn Fn(&str) -> std::io::Result<Vec<u8>>,
) -> Result<Prepared, Refusal> {
    match parse(argv)? {
        Parsed::Help => Ok(Prepared::Done(HELP)),
        Parsed::Version => Ok(Prepared::Done(version())),
        Parsed::Run(options) => {
            let matcher = compile(&options, utf8, read)?;
            Ok(Prepared::Run(options, matcher))
        }
    }
}

/// Whether the shell's locale is UTF-8: the first non-empty exported `LC_ALL`, `LC_CTYPE`,
/// `LANG`, else UTF-8 (bash-tool's default, as for sed).
pub(crate) fn utf8_locale(env: &[(String, String)]) -> bool {
    let locale = super::sed::locale(env).to_ascii_lowercase();
    !(locale == "c" || locale == "posix")
}

/// A compiled set of patterns.
pub(crate) enum Matcher {
    Plain(regex::bytes::Regex),
    /// Back-references and `-P` need a backtracking engine.
    Fancy(fancy_regex::Regex),
    /// `-f` of an empty file: no pattern, nothing matches.
    Nothing,
}

impl Matcher {
    fn is_match(&self, line: &[u8]) -> bool {
        match self {
            Self::Plain(regex) => regex.is_match(line),
            Self::Fancy(regex) => regex
                .is_match(&String::from_utf8_lossy(line))
                .unwrap_or(false),
            Self::Nothing => false,
        }
    }

    /// Nonempty match spans, left to right.
    fn spans(&self, line: &[u8]) -> Vec<(usize, usize)> {
        match self {
            Self::Plain(regex) => regex
                .find_iter(line)
                .map(|found| (found.start(), found.end()))
                .filter(|(start, end)| start < end)
                .collect(),
            Self::Fancy(regex) => {
                let text = String::from_utf8_lossy(line);
                regex
                    .find_iter(&text)
                    .filter_map(Result::ok)
                    .map(|found| (found.start(), found.end()))
                    .filter(|(start, end)| start < end)
                    .collect()
            }
            Self::Nothing => Vec::new(),
        }
    }
}

/// Translate a GNU basic regular expression into Rust regex syntax.
pub(crate) fn basic(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut chars = pattern.chars().peekable();
    // Where `*` is literal: at the start, after `\(`, `\|` or a leading `^`.
    let mut literal_star = true;
    let mut bracket = false;
    // As in `extended`: POSIX has no lazy quantifiers, so `\?` (GNU's "optional")
    // stacked straight onto a quantifier that was just emitted (`a\+\?`, `a\{2,3\}\?`,
    // a bare `*` followed by `\?`, ...) is a no-op re-quantification in GNU, not a lazy
    // modifier — which is what it would read as if passed through into Rust regex syntax
    // unchanged, since `\+`/`\?`/`\{m,n\}` all translate to their bare Rust-regex forms.
    let mut just_quantified = false;
    while let Some(c) = chars.next() {
        if bracket {
            push_bracket_char(&mut out, c, &mut bracket, &mut chars);
            just_quantified = false;
            continue;
        }
        let star_allowed_literal = literal_star;
        literal_star = false;
        let quantified = just_quantified;
        just_quantified = false;
        match c {
            '\\' => match chars.next() {
                Some('(') => {
                    out.push('(');
                    literal_star = true;
                }
                Some(')') => out.push(')'),
                Some('|') => {
                    out.push('|');
                    literal_star = true;
                }
                // GNU treats a leading repetition operator as literal, same as `extended`'s
                // `*`/`+`/`?`/`{` (its own comment explains why); an invalid interval
                // (`valid_bre_interval` — e.g. non-digit content) is also literal, same
                // reasoning as `extended`'s own `'{' if !valid_interval(...)`. A *missing*
                // lower bound (`\{,2\}`) is valid GNU BRE, meaning 0 — the `0` is inserted
                // right after `{` below, since the `regex` crate requires an explicit one.
                Some('{') if star_allowed_literal || !valid_bre_interval(chars.clone()) => {
                    out.push_str("\\{");
                }
                Some('{') => {
                    out.push('{');
                    if chars.peek() == Some(&',') {
                        out.push('0');
                    }
                }
                Some('}') => {
                    out.push('}');
                    just_quantified = true;
                }
                Some('?') if quantified => {
                    // Redundant: drop it rather than let it read as a lazy modifier.
                    just_quantified = true;
                }
                Some('?') => {
                    out.push('?');
                    just_quantified = true;
                }
                Some('+') => {
                    out.push('+');
                    just_quantified = true;
                }
                Some(d @ '1'..='9') => {
                    let _ = write!(out, "(?:\\{d})");
                }
                Some(other) => push_escape(&mut out, other),
                None => out.push_str("\\\\"),
            },
            '*' if star_allowed_literal => out.push_str("\\*"),
            '*' => {
                out.push('*');
                just_quantified = true;
            }
            // GNU: `^` anchors at the start and after `\(` or `\|`.
            '^' if star_allowed_literal => {
                out.push('^');
                literal_star = true;
            }
            '[' => {
                out.push('[');
                bracket = true;
                open_bracket(&mut out, &mut chars);
            }
            '+' | '?' | '{' | '}' | '|' | '(' | ')' => {
                out.push('\\');
                out.push(c);
            }
            '^' => out.push_str("\\^"),
            // `$` anchors at the end and before `\)` or `\|`.
            '$' if !ends_branch(chars.clone()) => out.push_str("\\$"),
            c => out.push(c),
        }
    }
    out
}

/// Translate a GNU extended regular expression into Rust regex syntax.
pub(crate) fn extended(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut chars = pattern.chars().peekable();
    let mut bracket = false;
    let mut at_start = true;
    // Whether the char just emitted was itself a quantifier (`*`, `+`, `?` or a `{m,n}`
    // interval's closing `}`). POSIX ERE has no lazy quantifiers, so a `?` stacked
    // straight onto one (`a+?`, `a{2,3}?`, ...) is not Perl-style laziness — GNU just
    // treats it as a redundant, no-op re-quantification and keeps matching greedily.
    // Emitted unchanged, that trailing `?` would instead read as Rust regex's own lazy
    // modifier syntax, silently flipping the match to non-greedy.
    let mut just_quantified = false;
    // Whether we're between a `{` that started a recognized `{m,n}` interval and its
    // closing `}` — so that `}` only counts as "just quantified" when it's really
    // closing one, not when it's a literal brace.
    let mut in_interval = false;
    while let Some(c) = chars.next() {
        if bracket {
            push_bracket_char(&mut out, c, &mut bracket, &mut chars);
            just_quantified = false;
            continue;
        }
        let starting = at_start;
        at_start = false;
        let quantified = just_quantified;
        just_quantified = false;
        match c {
            '?' if quantified => {
                // Redundant: drop it rather than let it read as a lazy modifier.
                just_quantified = true;
            }
            '\\' => match chars.next() {
                Some(d @ '1'..='9') => {
                    let _ = write!(out, "(?:\\{d})");
                }
                Some(other) => push_escape(&mut out, other),
                None => out.push_str("\\\\"),
            },
            '[' => {
                out.push('[');
                bracket = true;
                open_bracket(&mut out, &mut chars);
            }
            // GNU treats a leading repetition operator as a literal.
            '*' | '+' | '?' if starting => {
                out.push('\\');
                out.push(c);
            }
            '*' | '+' | '?' => {
                out.push(c);
                just_quantified = true;
            }
            '(' | '|' => {
                out.push(c);
                at_start = true;
            }
            // A leading `{m,n}` (nothing before it to repeat) is different from a leading
            // `*`/`+`/`?` above: verified against the oracle, GNU doesn't treat it as literal
            // text — it warns ("{...} at start of expression") and drops the whole interval,
            // as if it had never been written, leaving whatever follows as the pattern (so
            // `{1}a` matches any line containing `a`, not just one containing the literal
            // text `{1}a`). Consume it here without emitting anything.
            '{' if starting && valid_interval(chars.clone()) => {
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                }
                // Still nothing emitted yet: whatever follows is still "at the start".
                at_start = true;
            }
            // Otherwise `{` is literal unless it starts a valid interval.
            '{' if !valid_interval(chars.clone()) => out.push_str("\\{"),
            '{' => {
                out.push('{');
                // A missing lower bound (`{,2}`) is valid GNU ERE, meaning 0; the `regex`
                // crate requires an explicit one.
                if chars.peek() == Some(&',') {
                    out.push('0');
                }
                in_interval = true;
            }
            '}' if in_interval => {
                out.push('}');
                in_interval = false;
                just_quantified = true;
            }
            c => out.push(c),
        }
    }
    out
}

fn ends_branch(mut chars: std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    match chars.next() {
        None => true,
        Some('\\') => matches!(chars.next(), Some(')' | '|')),
        Some(_) => false,
    }
}

/// Whether `chars` (starting right after a BRE `\{`) is `m,n\}`/`m\}`/`,n\}` — a valid
/// interval, terminated by `\}` (not a bare `}`, which is just literal in BRE) — mirroring
/// `valid_interval` for `extended`'s bare-brace ERE syntax.
fn valid_bre_interval(mut chars: std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    let mut digits = 0;
    let mut comma = false;
    while let Some(c) = chars.next() {
        match c {
            '0'..='9' => digits += 1,
            ',' if !comma => comma = true,
            '\\' if chars.peek() == Some(&'}') => return digits > 0 || comma,
            _ => return false,
        }
    }
    false
}

fn valid_interval(mut chars: std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    let mut digits = 0;
    let mut comma = false;
    for c in chars.by_ref() {
        match c {
            '0'..='9' => digits += 1,
            ',' if !comma => comma = true,
            '}' => return digits > 0 || comma,
            _ => return false,
        }
    }
    false
}

/// Copy the start of a bracket expression: `^` and a leading `]` are part of it.
fn open_bracket(out: &mut String, chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    if chars.peek() == Some(&'^') {
        out.push('^');
        chars.next();
    }
    if chars.peek() == Some(&']') {
        out.push_str("\\]");
        chars.next();
    }
}

/// Inside a POSIX bracket expression `\` is literal and `[:class:]` passes through.
fn push_bracket_char(
    out: &mut String,
    c: char,
    bracket: &mut bool,
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) {
    match c {
        ']' => {
            out.push(']');
            *bracket = false;
        }
        '[' if matches!(chars.peek(), Some(':' | '.' | '=')) => {
            let kind = chars.next().unwrap_or(':');
            let mut name = String::new();
            for next in chars.by_ref() {
                if next == kind {
                    break;
                }
                name.push(next);
            }
            chars.next(); // the closing `]` of `[:name:]`
            if kind == ':' {
                let _ = write!(out, "[:{name}:]");
            } else {
                out.push_str(&regex::escape(&name));
            }
        }
        // Rust class syntax: nesting and set operators; POSIX treats these literally.
        '\\' | '[' | '&' | '~' => {
            out.push('\\');
            out.push(c);
        }
        c => out.push(c),
    }
}

/// GNU escapes: `\<`/`\>` word edges, `\w\W\s\S\b\B`, anything else is literal.
fn push_escape(out: &mut String, c: char) {
    match c {
        '<' => out.push_str("\\b{start}"),
        '>' => out.push_str("\\b{end}"),
        'w' | 'W' | 's' | 'S' | 'b' | 'B' => {
            out.push('\\');
            out.push(c);
        }
        '`' => out.push_str("\\A"),
        '\'' => out.push_str("\\z"),
        c => out.push_str(&regex::escape(&c.to_string())),
    }
}

/// Whether a translated pattern refers to a group, which needs the backtracking engine.
pub(crate) fn has_back_reference(translated: &str) -> bool {
    translated.contains("(?:\\") && {
        let bytes = translated.as_bytes();
        bytes
            .windows(5)
            .any(|window| window.starts_with(b"(?:\\") && window[4].is_ascii_digit())
    }
}

/// GNU's `-P` (backed by PCRE) defaults to ASCII-only `\d`/`\D`/`\s`/`\S`/`\w`/`\W` — PCRE has no
/// `(*UCP)`/`/u` modifier unless asked for one, and this tool doesn't offer one. `fancy_regex`
/// (this tool's Perl-dialect engine) instead defaults those classes to full Unicode, and — unlike
/// PCRE — has no "valid UTF-8 input, but ASCII character classes" mode to switch to (its own docs
/// list only three `utf8`/`unicode` combinations, and none of them is that one). Rewritten here
/// at the text level instead, before compiling: verified against the oracle, `-P '\d'` does not
/// match a non-ASCII (Arabic-Indic) digit. Doesn't attempt to also special-case a leading `]`
/// (POSIX's "class containing a literal `]`" spelling) inside a character class — a pattern that
/// relies on that is vanishingly rare next to `\d` itself.
fn ascii_only_classes(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        if c == '\\' {
            let class = match chars.peek() {
                Some('d') => Some(("0-9", false)),
                Some('D') => Some(("0-9", true)),
                Some('w') => Some(("0-9A-Za-z_", false)),
                Some('W') => Some(("0-9A-Za-z_", true)),
                Some('s') => Some((" \\t\\n\\r\\f\\v", false)),
                Some('S') => Some((" \\t\\n\\r\\f\\v", true)),
                _ => None,
            };
            if let Some((members, negated)) = class {
                chars.next();
                match (in_class, negated) {
                    (true, false) => out.push_str(members),
                    // `[\D]`/`[\S]`/`[\W]` (negated, already inside a class) can't splice
                    // as members — negation doesn't distribute that way — so nest instead.
                    (true, true) | (false, true) => {
                        out.push_str("[^");
                        out.push_str(members);
                        out.push(']');
                    }
                    (false, false) => {
                        out.push('[');
                        out.push_str(members);
                        out.push(']');
                    }
                }
                continue;
            }
            out.push(c);
            if let Some(next) = chars.next() {
                out.push(next);
            }
            continue;
        }
        if c == '[' {
            in_class = true;
        } else if c == ']' {
            in_class = false;
        }
        out.push(c);
    }
    out
}

/// A stand-in for one raw byte that isn't valid UTF-8, used while building a pattern's regex
/// text (which must itself be a valid Rust `str`) from bytes that might not be. Drawn from the
/// Unicode Private Use Area so it can never collide with anything the BRE/ERE/fixed-string
/// translators treat specially (they only ever special-case ASCII), and turned into the byte's
/// own `\xHH` regex escape only once translation is done — inserting `\xHH` beforehand would
/// have a dialect translator reinterpret the `\x` as its own (unrelated) escape syntax.
fn byte_marker(b: u8) -> char {
    char::from_u32(0xE000 + u32::from(b)).expect("0xE000..=0xE0FF is in the PUA")
}

/// Convert (possibly invalid-UTF-8) bytes into a `str` fit for the dialect translators: valid
/// UTF-8 passes through unchanged, and each byte that isn't part of a valid sequence becomes its
/// own [`byte_marker`]. Verified against the oracle: a `-f` pattern file containing a raw
/// (invalid-UTF-8) byte matches that literal byte in the input — not the replacement character
/// GNU substitutes when *displaying* invalid UTF-8, and not nothing.
fn bytes_to_pattern_text(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        match core::str::from_utf8(rest) {
            Ok(valid) => {
                out.push_str(valid);
                break;
            }
            Err(error) => {
                let valid_len = error.valid_up_to();
                out.push_str(core::str::from_utf8(&rest[..valid_len]).unwrap());
                out.push(byte_marker(rest[valid_len]));
                rest = &rest[valid_len + 1..];
            }
        }
    }
    out
}

/// Undo [`bytes_to_pattern_text`]'s markers in a fully-translated regex pattern, turning each
/// one into the byte's own `\xHH` hex escape — safe now because nothing downstream reinterprets
/// backslash sequences the way a BRE/ERE translator would. Wrapped in `(?-u:...)`: the `regex`
/// crate's `\xHH` means the *Unicode scalar value* U+00HH (encoded as multi-byte UTF-8 against
/// the bytes API) unless Unicode mode is off for it, in which case it means the raw byte —
/// which is what a byte straight from `bytes_to_pattern_text` actually is, valid UTF-8 or not.
fn restore_byte_markers(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    for c in pattern.chars() {
        match u32::from(c).checked_sub(0xE000) {
            Some(b @ 0..=0xFF) => {
                let _ = write!(out, "(?-u:\\x{b:02x})");
            }
            _ => out.push(c),
        }
    }
    out
}

/// Compile the patterns for `options`, reading `-f` files through `read`.
pub(crate) fn compile(
    options: &Options,
    utf8: bool,
    read: &dyn Fn(&str) -> std::io::Result<Vec<u8>>,
) -> Result<Matcher, Refusal> {
    let mut patterns: Vec<String> = Vec::new();
    for pattern in options.patterns.iter().flatten() {
        // A byte of the argument that is not UTF-8 matches that byte, as in a `-f` file.
        let text = bytes_to_pattern_text(&super::shell_bytes::encode(pattern));
        patterns.extend(text.split('\n').map(str::to_owned));
    }
    for file in &options.pattern_files {
        let bytes = read(file).map_err(|error| Refusal {
            code: 2,
            message: format!("grep: {file}: {}\n", super::io_message(&error)),
        })?;
        let text = bytes_to_pattern_text(&bytes);
        let text = text.strip_suffix('\n').unwrap_or(&text);
        if !text.is_empty() || !bytes.is_empty() {
            patterns.extend(text.split('\n').map(str::to_owned));
        }
    }
    if patterns.is_empty() {
        return Ok(Matcher::Nothing);
    }
    let translated: Vec<String> = patterns
        .iter()
        .map(|pattern| match options.dialect {
            Dialect::Basic => basic(pattern),
            Dialect::Extended => extended(pattern),
            Dialect::Fixed => regex::escape(pattern),
            Dialect::Perl => ascii_only_classes(pattern),
        })
        .collect();
    let body = if translated.len() == 1 {
        translated[0].clone()
    } else {
        translated
            .iter()
            .map(|pattern| format!("(?:{pattern})"))
            .collect::<Vec<_>>()
            .join("|")
    };
    let fancy = options.dialect == Dialect::Perl || has_back_reference(&body);
    // `-z`: a "line" is everything up to the next NUL, so an embedded real `\n` is just an
    // ordinary byte in it, not a line terminator `.` should refuse to cross — GNU's `.` matches
    // it there (verified against the oracle: `-z` makes `.` match a literal newline).
    let mut inline_flags = String::new();
    if options.ignore_case {
        inline_flags.push('i');
    }
    if options.null_data {
        inline_flags.push('s');
    }
    let flags = if inline_flags.is_empty() {
        String::new()
    } else {
        format!("(?{inline_flags})")
    };
    let body = if options.line {
        format!("^(?:{body})$")
    } else if options.word {
        if fancy {
            format!("(?<!\\w)(?:{body})(?!\\w)")
        } else {
            format!("\\b{{start-half}}(?:{body})\\b{{end-half}}")
        }
    } else {
        body
    };
    let pattern = restore_byte_markers(&format!("{flags}{body}"));
    // `[:alpha:]` is defined by the regex crate as ASCII-only (`[A-Za-z]`) regardless of the
    // `unicode` flag — that flag only widens `\w`/`\p{...}`/case folding, not POSIX bracket
    // classes — but GNU widens `isalpha`'s notion under a UTF-8 locale to match `é`, CJK, Greek,
    // Hebrew and so on. Substituted with the Unicode property that does that, verified against
    // the oracle; skipped in the C/POSIX locale, where `[:alpha:]` staying ASCII-only is
    // correct as-is. A straight substring replace is safe here: outside of exactly this POSIX
    // class token, `[:alpha:]` (with its literal colons and brackets) can't appear in a pattern
    // that compiled at all.
    let pattern = if utf8 {
        pattern.replace("[:alpha:]", "\\p{Alphabetic}")
    } else {
        pattern
    };
    let invalid = |error: String| Refusal {
        code: 2,
        message: format!("grep: {}\n", regex_error_wording(&error)),
    };
    if fancy {
        fancy_regex::Regex::new(&pattern)
            .map(Matcher::Fancy)
            .map_err(|error| invalid(error.to_string()))
    } else {
        regex::bytes::RegexBuilder::new(&pattern)
            .unicode(utf8)
            .build()
            .map(Matcher::Plain)
            .map_err(|error| invalid(first_line(&error.to_string())))
    }
}

/// Maps the `regex`/`fancy_regex` crates' own diagnostic text to GNU's, for the common cases
/// that are otherwise just the crate's internal wording leaking through — verified against the
/// oracle for each. A few more real GNU diagnostics (an unmatched `\{`, an unrecognized POSIX
/// class name, a bare trailing backslash, `[:name:]` written without its own enclosing
/// brackets) aren't produced at all yet, since this fork's own BRE/ERE bracket handling doesn't
/// detect them as errors in the first place — a real gap, not just wording, left for a
/// follow-up.
fn regex_error_wording(error: &str) -> String {
    const MAP: &[(&str, &str)] = &[
        ("unclosed group", "Unmatched ( or \\("),
        ("unclosed character class", "Invalid regular expression"),
        (
            "invalid repetition count range, the start must be <= the end",
            "Invalid content of \\{\\}",
        ),
        ("Invalid back reference to group", "Invalid back reference"),
        ("Could not parse group name", "subpattern name expected"),
    ];
    for (from, to) in MAP {
        if error.contains(from) {
            return (*to).to_owned();
        }
    }
    error.to_owned()
}

fn first_line(text: &str) -> String {
    text.lines()
        .last()
        .unwrap_or(text)
        .trim_start_matches("error: ")
        .to_owned()
}

/// What the driver should do after a record.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Step {
    Continue,
    /// Nothing more is needed from this file.
    NextFile,
    /// `-q` selected a line: the whole command is done.
    Quit,
}

/// One file's search: selection, context and output formatting.
pub(crate) struct Search<'a> {
    options: &'a Options,
    matcher: &'a Matcher,
    name: String,
    with_filename: bool,
    number: u64,
    offset: u64,
    selected: u64,
    before: VecDeque<(u64, u64, Vec<u8>)>,
    after_left: usize,
    last_printed: Option<u64>,
    printed_any: bool,
    /// Whether an earlier target (an earlier file, or stdin before this one) already printed
    /// something with context on: GNU's `--` group separator belongs between the two, since
    /// two different files' output is never "adjacent" the way two nearby lines in the same
    /// file can be. `record` treats this exactly like the within-file non-adjacency case, but
    /// only for this target's own first print (see `record`'s own comment).
    separator_owed: bool,
    binary: bool,
    /// Whether a NUL has shown up in any record read so far (selected or not) — see `record`'s
    /// own comment on why this, unlike the rest of binary detection, isn't gated on selection.
    saw_nul: bool,
    stopping: bool,
    /// Whether `[:alpha:]`-widening's own UTF-8 locale is active — reused here to decide
    /// whether a line that isn't valid UTF-8 counts as "binary", the same as GNU does under a
    /// UTF-8 locale (verified against the oracle: Latin-1, a lone `0xff`, an encoded surrogate
    /// and a truncated multi-byte sequence all make GNU report "binary file matches" instead of
    /// the line's own content, piped through stdin where there's no seekable read-ahead to peek
    /// with — see `record`'s own use of this).
    utf8: bool,
}

impl<'a> Search<'a> {
    pub(crate) fn new(
        options: &'a Options,
        matcher: &'a Matcher,
        name: String,
        with_filename: bool,
        separator_owed: bool,
        utf8: bool,
    ) -> Self {
        Self {
            options,
            matcher,
            name,
            with_filename,
            number: 0,
            offset: 0,
            selected: 0,
            before: VecDeque::new(),
            after_left: 0,
            last_printed: None,
            printed_any: false,
            separator_owed,
            binary: false,
            saw_nul: false,
            stopping: false,
            utf8,
        }
    }

    /// Whether this search printed anything at all: the next target (if any) needs to know,
    /// to carry `separator_owed` forward across a target that found no matches of its own.
    pub(crate) const fn printed(&self) -> bool {
        self.printed_any
    }

    /// The record delimiter for `options`.
    pub(crate) fn delimiter(options: &Options) -> u8 {
        if options.null_data { b'\0' } else { b'\n' }
    }

    /// Lines selected so far.
    pub(crate) const fn selected(&self) -> u64 {
        self.selected
    }

    /// Mark this target as binary before any record is seen, for a caller that already
    /// knows (e.g. from peeking a seekable file for a NUL byte before starting to read
    /// it). GNU decides "is this a binary file" from a read-ahead buffer that covers the
    /// whole file for anything that isn't huge, not incrementally as it matches each
    /// line — a NUL three lines down must suppress a match reported on line one, not just
    /// lines from that point on (verified against the oracle).
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn mark_binary(&mut self) {
        self.binary = true;
    }

    fn listing(&self) -> bool {
        self.options.count
            || self.options.files_with_matches
            || self.options.files_without_match
            || self.options.quiet
    }

    /// Process one record (including its delimiter, if present), appending output to `out` and
    /// diagnostics to `err`.
    pub(crate) fn record(&mut self, record: &[u8], out: &mut Vec<u8>, err: &mut Vec<u8>) -> Step {
        let delimiter = Self::delimiter(self.options);
        let line = record.strip_suffix(&[delimiter]).unwrap_or(record);
        self.number += 1;
        let offset = self.offset;
        self.offset += record.len() as u64;
        if self.stopping {
            // `-m` reached: only trailing context remains.
            if self.after_left == 0 {
                return Step::NextFile;
            }
            self.after_left -= 1;
            self.print_line(line, self.number, offset, b'-', out);
            return if self.after_left == 0 {
                Step::NextFile
            } else {
                Step::Continue
            };
        }
        // A NUL is checked on *every* record, selected or not, and even under `-o` — GNU's own
        // NUL check isn't about the specific line or span that ends up selected, it's "has a NUL
        // shown up anywhere in the file yet", verified against the oracle three ways: a NUL on an
        // earlier, non-matching line still makes a later match report binary (`printf
        // 'a\000b\nzz\n' | grep -n z`), a NUL past the end of an `-o` match's own span still
        // trips it (`printf 'hit\000\n' | grep -o hit`), and so does one on a *different*,
        // non-extracted part of the very line `-o` matched (`printf 'a\000b\nzz\n' | grep -o
        // '[a-z]'`, which matches "a" but still reports binary over the "b\0" that follows it in
        // the same record). This one accumulates only from records already read, not a read-ahead
        // over ones still to come, so a match reported before a NUL that only shows up further
        // ahead in the same file isn't (yet) retroactively caught the way GNU's own read-ahead
        // buffer would — not exercised by any case in this fork's own corpus.
        if !self.options.null_data && line.contains(&0) {
            self.saw_nul = true;
        }
        // `-m0` means "stop after zero selected lines" — no line is ever selected at all, not
        // even the first one found (verified against the oracle: it prints nothing and exits
        // 1, the same as if the pattern never matched).
        let selected =
            self.options.max_count != Some(0) && self.matcher.is_match(line) != self.options.invert;
        // The rest of binary detection *is* about the bytes that would actually be *written*,
        // not merely present anywhere in the file — verified against the oracle: a pattern that
        // only ever selects a clean line (`grep -n z` against a file whose *other* line has a
        // lone 0xff, not a NUL) prints normally, no "binary file matches" in sight, and `-o`
        // (each match is its own, separately-validated span, not the whole line) extracts plain
        // ASCII matches from a line with a stray invalid (non-NUL) byte elsewhere without
        // tripping it either. So the non-NUL half only checks the line when it's actually
        // selected, and skips it under `-o`, where a span that itself decodes fine shouldn't
        // inherit its untaken siblings' invalidity — unlike the NUL check just above, which knows
        // no such exemption.
        if selected
            && !self.options.null_data
            && (self.saw_nul
                || (!self.options.only_matching && self.utf8 && std::str::from_utf8(line).is_err()))
        {
            self.binary = true;
        }
        if !selected {
            if self.after_left > 0 {
                self.after_left -= 1;
                self.print_line(line, self.number, offset, b'-', out);
            } else if self.options.before > 0 {
                self.before.push_back((self.number, offset, line.to_vec()));
                if self.before.len() > self.options.before {
                    self.before.pop_front();
                }
            }
            return Step::Continue;
        }
        self.selected += 1;
        // `-I`/`--binary-files=without-match` must win over `-q`: a binary "match" is no
        // match at all under `-I`, so `-qI` on a binary file exits 1, not 0 (verified
        // against the oracle — checked first, since `-q`'s `Step::Quit` would otherwise
        // report success before this ever ran).
        if self.binary && self.options.binary == BinaryFiles::WithoutMatch {
            self.selected = 0;
            return Step::NextFile;
        }
        if self.options.quiet {
            return Step::Quit;
        }
        let limit_reached = self
            .options
            .max_count
            .is_some_and(|max| self.selected >= max);
        if self.options.files_with_matches {
            self.push_name(out, true);
            return Step::NextFile;
        }
        if self.listing() {
            return if limit_reached {
                Step::NextFile
            } else {
                Step::Continue
            };
        }
        if self.binary && self.options.binary == BinaryFiles::Report {
            let _ = writeln!(Diagnostic(err), "grep: {}: binary file matches", self.name);
            return Step::NextFile;
        }
        let context = self.options.context_requested;
        let first = self
            .before
            .front()
            .map_or(self.number, |(number, ..)| *number);
        // Non-adjacent to the last thing printed *by this target* (the usual, within-file
        // case), or this is this target's own first print and an earlier target already
        // printed something (`separator_owed` — see its own doc comment): either way, GNU
        // puts the separator here.
        let non_adjacent =
            self.printed_any && self.last_printed.is_some_and(|last| first > last + 1);
        let first_print_after_earlier_target = !self.printed_any && self.separator_owed;
        if context
            && (non_adjacent || first_print_after_earlier_target)
            && let Some(separator) = &self.options.group_separator
        {
            colored(
                out,
                self.options.color,
                sgr::SEPARATOR,
                separator.as_bytes(),
            );
            out.push(b'\n');
        }
        for (number, offset, text) in std::mem::take(&mut self.before) {
            self.print_line(&text, number, offset, b'-', out);
        }
        if self.options.only_matching {
            if !self.options.invert {
                for (start, end) in self.matcher.spans(line) {
                    self.prefix(self.number, offset + start as u64, b':', out);
                    colored(out, self.options.color, sgr::MATCH, &line[start..end]);
                    out.push(delimiter);
                }
            }
            self.last_printed = Some(self.number);
            self.printed_any = true;
        } else {
            self.print_line(line, self.number, offset, b':', out);
        }
        self.after_left = self.options.after;
        if limit_reached {
            self.stopping = true;
            if self.after_left == 0 {
                return Step::NextFile;
            }
        }
        Step::Continue
    }

    /// Output owed after the file's last record (`-c`, `-L`).
    pub(crate) fn finish(&mut self, out: &mut Vec<u8>) {
        if self.options.quiet {
            return;
        }
        if self.options.count && !self.options.files_with_matches {
            if self.with_filename {
                out.extend_from_slice(self.name.as_bytes());
                out.push(if self.options.null { b'\0' } else { b':' });
            }
            let _ = writeln!(Diagnostic(out), "{}", self.selected);
        }
        if self.options.files_without_match && self.selected == 0 {
            self.push_name(out, true);
        }
    }

    fn push_name(&self, out: &mut Vec<u8>, terminate: bool) {
        out.extend_from_slice(self.name.as_bytes());
        if terminate {
            out.push(if self.options.null { b'\0' } else { b'\n' });
        }
    }

    fn prefix(&self, number: u64, offset: u64, separator: u8, out: &mut Vec<u8>) {
        let color = self.options.color;
        if self.with_filename {
            colored(out, color, sgr::FILENAME, self.name.as_bytes());
            colored(
                out,
                color,
                sgr::SEPARATOR,
                &[if self.options.null { b'\0' } else { separator }],
            );
        }
        if self.options.line_number {
            let mut field = Vec::new();
            let _ = write!(Diagnostic(&mut field), "{number}");
            colored(out, color, sgr::LINE_NUMBER, &field);
            colored(out, color, sgr::SEPARATOR, &[separator]);
        }
        if self.options.byte_offset {
            let mut field = Vec::new();
            let _ = write!(Diagnostic(&mut field), "{offset}");
            colored(out, color, sgr::LINE_NUMBER, &field);
            colored(out, color, sgr::SEPARATOR, &[separator]);
        }
        if self.options.initial_tab
            && (self.with_filename || self.options.line_number || self.options.byte_offset)
        {
            out.push(b'\t');
        }
    }

    fn print_line(
        &mut self,
        line: &[u8],
        number: u64,
        offset: u64,
        separator: u8,
        out: &mut Vec<u8>,
    ) {
        self.prefix(number, offset, separator, out);
        if self.options.color {
            // Every match on this line, `:` or `-`, gets highlighted (context lines that
            // happen to match too, not just the selected one) — matches GNU's own behavior.
            let mut pos = 0;
            for (start, end) in self.matcher.spans(line) {
                out.extend_from_slice(&line[pos..start]);
                colored(out, true, sgr::MATCH, &line[start..end]);
                pos = end;
            }
            out.extend_from_slice(&line[pos..]);
        } else {
            out.extend_from_slice(line);
        }
        out.push(Self::delimiter(self.options));
        self.last_printed = Some(number);
        self.printed_any = true;
    }
}

/// `write!` into a byte buffer.
struct Diagnostic<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for Diagnostic<'_> {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0.extend_from_slice(text.as_bytes());
        Ok(())
    }
}

/// One operand to search, as grep names it in output.
#[derive(Debug)]
pub(crate) enum Target {
    /// Standard input, named by its operand when that is a path such as `/dev/stdin`.
    Stdin(Option<String>),
    /// A file: its displayed name and the path to open.
    File(String, PathBuf),
    /// A diagnostic already decided (missing file, directory without `-r`).
    Error(String),
}

/// Expand operands into files, walking directories for `-r`. `resolve` maps an operand to an
/// absolute path against the shell's working directory.
/// Returns the expanded targets and whether a directory was actually walked (`-r`/`-R` finding
/// a directory operand and descending into it) — distinct from `options.recursive()`, which is
/// just whether the flag was given: `grep -r pat file` (a single, explicit, *non-directory*
/// operand) never walks anything, and GNU's filename-prefix decision (`with_filename`) cares
/// about the former, not the latter (verified against the oracle: `-r` on one plain file omits
/// the prefix, `-r` on one directory that happens to contain exactly one file still shows it).
pub(crate) fn targets(options: &Options, resolve: &dyn Fn(&str) -> PathBuf) -> (Vec<Target>, bool) {
    let operands: Vec<String> = if options.files.is_empty() {
        vec![if options.recursive() { "." } else { "-" }.into()]
    } else {
        options.files.clone()
    };
    let implicit_dot = options.files.is_empty() && options.recursive();
    let mut targets = Vec::new();
    let mut recursed = false;
    for operand in operands {
        if operand == "-" {
            targets.push(Target::Stdin(None));
            continue;
        }
        let path = resolve(&operand);
        if super::devices::classify(&path) == Some(super::devices::Device::Stream(0)) {
            targets.push(Target::Stdin(Some(operand)));
            continue;
        }
        if path == Path::new("/dev/null") {
            targets.push(Target::File(operand, path));
            continue;
        }
        match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_dir() => match options.directories {
                Directories::Recurse => {
                    recursed = true;
                    if glob_matches(&options.exclude_dir, &operand_basename(&operand)) {
                        continue;
                    }
                    let shown = if implicit_dot {
                        String::new()
                    } else {
                        operand.clone()
                    };
                    let mut visited = HashSet::new();
                    if options.follow_links
                        && let Ok(real) = std::fs::canonicalize(&path)
                    {
                        visited.insert(real);
                    }
                    walk(options, &path, &shown, &mut targets, &mut visited);
                }
                Directories::Skip => (),
                Directories::Read => {
                    targets.push(Target::Error(format!("grep: {operand}: Is a directory\n")));
                }
            },
            // GNU applies `--include`/`--exclude` to every file target, including an operand
            // named directly on the command line — not just files discovered by recursing —
            // and regardless of whether `-r`/`-R` was even given (verified against the oracle:
            // a bare `grep --include=*.log pat file.txt` silently skips `file.txt`).
            Ok(_)
                if (!options.include.is_empty() || !options.exclude.is_empty())
                    && (!glob_matches_or_empty(&options.include, &operand_basename(&operand))
                        || glob_matches(&options.exclude, &operand_basename(&operand))) => {}
            Ok(_) => targets.push(Target::File(operand, path)),
            Err(error) => targets.push(Target::Error(format!(
                "grep: {operand}: {}\n",
                super::io_message(&error)
            ))),
        }
    }
    (targets, recursed)
}

/// The final path component, for matching against `--include`/`--exclude`/`--exclude-dir` globs
/// (which match a bare file/directory name, never a path).
fn operand_basename(operand: &str) -> String {
    Path::new(operand)
        .file_name()
        .map_or_else(|| operand.to_owned(), |n| n.to_string_lossy().into_owned())
}

fn glob_matches_or_empty(globs: &[String], name: &str) -> bool {
    globs.is_empty() || glob_matches(globs, name)
}

fn glob_matches(globs: &[String], name: &str) -> bool {
    globs
        .iter()
        .any(|glob| super::find::fnmatch(glob, name, false))
}

/// Recursive directory walk for `-r`/`-R`. `visited` holds the canonicalized real path of
/// every directory on the path from the root down to the one currently being read — the
/// current *ancestor chain*, not every directory seen so far, so two symlinks that happen
/// to reach the same real directory from unrelated branches aren't mistaken for a cycle.
/// With `-R` (`options.follow_links`), a symlink can lead back to an ancestor and recurse
/// forever, so each directory is checked against this chain before `walk` descends into
/// it, the same way GNU grep detects a "recursive directory loop". Without `-R`,
/// symlinked directories are never followed (`options.follow_links` is false, so
/// `symlink_metadata` reports them as non-directories) and a loop through ordinary
/// directories alone isn't possible, so `visited` never grows.
fn walk(
    options: &Options,
    directory: &Path,
    shown: &str,
    targets: &mut Vec<Target>,
    visited: &mut HashSet<PathBuf>,
) {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            targets.push(Target::Error(format!(
                "grep: {shown}: {}\n",
                super::io_message(&error)
            )));
            return;
        }
    };
    let mut children: Vec<_> = entries.filter_map(Result::ok).collect();
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let name = child.file_name().to_string_lossy().into_owned();
        let display = if shown.is_empty() {
            name.clone()
        } else if shown.ends_with('/') {
            format!("{shown}{name}")
        } else {
            format!("{shown}/{name}")
        };
        let path = child.path();
        let metadata = if options.follow_links {
            std::fs::metadata(&path)
        } else {
            std::fs::symlink_metadata(&path)
        };
        let Ok(metadata) = metadata else {
            continue;
        };
        if metadata.is_dir() {
            if glob_matches(&options.exclude_dir, &name) {
                continue;
            }
            if options.follow_links {
                let real = match std::fs::canonicalize(&path) {
                    Ok(real) => real,
                    // Can't resolve it (e.g. a dangling symlink slipped past the
                    // `metadata` check above in a race): nothing to loop through.
                    Err(_) => continue,
                };
                if !visited.insert(real.clone()) {
                    targets.push(Target::Error(format!(
                        "grep: {display}: recursive directory loop\n"
                    )));
                    continue;
                }
                walk(options, &path, &display, targets, visited);
                visited.remove(&real);
            } else {
                walk(options, &path, &display, targets, visited);
            }
            continue;
        } else if metadata.is_file()
            && (options.include.is_empty() || glob_matches(&options.include, &name))
            && !glob_matches(&options.exclude, &name)
        {
            targets.push(Target::File(display, path));
        }
    }
}

/// The name shown for standard input.
pub(crate) fn stdin_name(options: &Options) -> String {
    options
        .label
        .clone()
        .unwrap_or_else(|| "(standard input)".to_owned())
}

/// How much of a seekable target [`file_starts_with_nul`] reads looking for a NUL byte —
/// GNU's own binary check reads a comparable size (its default `--binary-files` read-ahead
/// buffer), so a genuinely huge file could still see a match reported before a NUL far past
/// this point, same as GNU.
#[cfg(target_arch = "wasm32")]
const BINARY_PEEK_LEN: usize = 64 * 1024;

/// Whether the first [`BINARY_PEEK_LEN`] bytes of the (already open, seekable) file `source`
/// contain a NUL byte, restoring its position to the start either way. Checked once before
/// any record is read, rather than line by line as `Search::record` also does: GNU decides
/// "binary" from a read-ahead over the file, not incrementally, so a NUL found only partway
/// through must still suppress output from lines read *before* it (see
/// [`Search::mark_binary`]). Only meaningful for a real file — a pipe/stream target can't be
/// rewound, so it keeps the line-by-line fallback alone.
#[cfg(target_arch = "wasm32")]
pub(crate) fn file_starts_with_nul(source: &mut std::fs::File) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let mut buf = [0u8; BINARY_PEEK_LEN];
    let mut total = 0;
    let found = loop {
        match source.read(&mut buf[total..]) {
            Ok(0) => break false,
            Ok(n) => {
                if buf[total..total + n].contains(&0) {
                    break true;
                }
                total += n;
                if total == buf.len() {
                    break false;
                }
            }
            Err(_) => break false,
        }
    };
    let _ = source.seek(SeekFrom::Start(0));
    found
}

/// Whether output lines carry file names.
pub(crate) fn with_filename(options: &Options, targets: &[Target], recursed: bool) -> bool {
    options.filename.unwrap_or(targets.len() > 1 || recursed)
}

/// grep's exit status from what happened.
pub(crate) fn status(options: &Options, selected: bool, failed: bool) -> i32 {
    if selected && options.quiet {
        0
    } else if failed {
        2
    } else {
        i32::from(!selected)
    }
}

/// The native driver: every target is read synchronously, one record at a time.
pub(crate) fn run_sync(
    options: &Options,
    matcher: &Matcher,
    utf8: bool,
    stdin: &mut dyn std::io::Read,
    out: &mut dyn std::io::Write,
    err: &mut dyn std::io::Write,
    resolve: &dyn Fn(&str) -> PathBuf,
) -> std::io::Result<i32> {
    use std::io::BufRead;
    let (targets, recursed) = targets(options, resolve);
    let with_filename = with_filename(options, &targets, recursed);
    let delimiter = Search::delimiter(options);
    let mut failed = false;
    let mut selected = false;
    let mut stdin = Some(stdin);
    // Whether an earlier target already printed something with context on — see
    // `Search::separator_owed`'s own doc comment.
    let mut separator_owed = false;
    for target in targets {
        let (name, reader): (String, Box<dyn std::io::Read + '_>) = match target {
            Target::Stdin(name) => {
                let name = name.unwrap_or_else(|| stdin_name(options));
                match stdin.take() {
                    Some(stdin) => (name, Box::new(stdin)),
                    None => (name, Box::new(std::io::empty())),
                }
            }
            Target::File(name, path) => match super::open_file(&path) {
                Ok(file) => (name, file),
                Err(error) => {
                    failed = true;
                    if !options.no_messages() {
                        writeln!(err, "grep: {name}: {}", super::io_message(&error))?;
                    }
                    continue;
                }
            },
            Target::Error(message) => {
                failed = true;
                if !options.no_messages {
                    err.write_all(message.as_bytes())?;
                }
                continue;
            }
        };
        let mut search = Search::new(options, matcher, name, with_filename, separator_owed, utf8);
        let mut reader = std::io::BufReader::new(reader);
        let mut record = Vec::new();
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        loop {
            record.clear();
            if reader.read_until(delimiter, &mut record)? == 0 {
                break;
            }
            output.clear();
            diagnostics.clear();
            let step = search.record(&record, &mut output, &mut diagnostics);
            out.write_all(&output)?;
            err.write_all(&diagnostics)?;
            match step {
                Step::Continue => (),
                Step::NextFile => break,
                Step::Quit => return Ok(0),
            }
        }
        output.clear();
        search.finish(&mut output);
        out.write_all(&output)?;
        selected |= search.selected() > 0;
        separator_owed |= search.printed();
    }
    Ok(status(options, selected, failed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grep(args: &[&str], input: &str) -> (i32, String, String) {
        let (code, out, err) = grep_bytes(args, input.as_bytes());
        (code, String::from_utf8_lossy(&out).into_owned(), err)
    }

    fn grep_bytes(args: &[&str], mut input: &[u8]) -> (i32, Vec<u8>, String) {
        let argv: Vec<String> = std::iter::once("grep")
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect();
        let options = match parse(&argv) {
            Ok(Parsed::Run(options)) => options,
            Ok(_) => return (0, Vec::new(), String::new()),
            Err(refusal) => return (refusal.code, Vec::new(), refusal.message),
        };
        let matcher = match compile(&options, true, &|path| std::fs::read(path)) {
            Ok(matcher) => matcher,
            Err(refusal) => return (refusal.code, Vec::new(), refusal.message),
        };
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_sync(
            &options,
            &matcher,
            true,
            &mut input,
            &mut out,
            &mut err,
            &|path| PathBuf::from(path),
        )
        .unwrap();
        (code, out, String::from_utf8(err).unwrap())
    }

    #[test]
    // The regex crates' own diagnostic text, mapped to GNU's wording for the common cases —
    // verified against the oracle for each.
    fn regex_error_messages_match_gnu_wording() {
        let r = grep(&["-E", "("], "a\n");
        assert_eq!(
            r,
            (2, String::new(), "grep: Unmatched ( or \\(\n".to_owned())
        );
        assert_eq!(grep(&["["], "a\n").2, "grep: Invalid regular expression\n");
        assert_eq!(
            grep(&["a\\{2,1\\}"], "a\n").2,
            "grep: Invalid content of \\{\\}\n"
        );
        assert_eq!(
            grep(&["-E", "(a)\\2"], "a\n").2,
            "grep: Invalid back reference\n"
        );
        assert_eq!(
            grep(&["-P", "(?P<>a)"], "a\n").2,
            "grep: subpattern name expected\n"
        );
    }

    #[test]
    fn dialects_follow_gnu() {
        assert_eq!(grep(&["a\\|b"], "a\nb\nc\n").1, "a\nb\n");
        assert_eq!(grep(&["a+b"], "a+b\naab\n").1, "a+b\n");
        assert_eq!(grep(&["a\\+b"], "a+b\naab\n").1, "aab\n");
        assert_eq!(grep(&["-E", "a+b"], "a+b\naab\n").1, "aab\n");
        assert_eq!(grep(&["\\(ab\\)\\1"], "abab\nab\n").1, "abab\n");
        assert_eq!(grep(&["-E", "(ab)\\1"], "abab\nab\n").1, "abab\n");
        assert_eq!(grep(&["*abc"], "*abc\nabc\n").1, "*abc\n");
        assert_eq!(grep(&["-E", "x{1"], "x{1}\n").1, "x{1}\n");
        assert_eq!(grep(&["-F", "."], "a.b\nab\n").1, "a.b\n");
        assert_eq!(grep(&["-oP", "(?<=\\$)\\d+"], "price: $42\n").1, "42\n");
        assert_eq!(grep(&["[[:digit:]]\\{2\\}"], "a1\nb22\n").1, "b22\n");
        assert_eq!(grep(&["\\<foo\\>"], "foo bar\nfoobar\n").1, "foo bar\n");
    }

    #[test]
    // two *different* dialect flags conflict (GNU: "conflicting matchers specified",
    // status 2, no usage/Try follow-up); the same one repeated is fine.
    fn conflicting_dialect_flags_are_refused() {
        let (code, out, err) = grep(&["-E", "-F", "a+b"], "a+b\n");
        assert_eq!((code, out.as_str()), (2, ""));
        assert_eq!(err, "grep: conflicting matchers specified\n");
        let (code, out, err) = grep(&["-G", "-E", "a"], "a\n");
        assert_eq!((code, out.as_str()), (2, ""));
        assert_eq!(err, "grep: conflicting matchers specified\n");
        // Repeating the same one, in either spelling, is fine.
        assert_eq!(grep(&["-E", "-E", "a"], "a\n").0, 0);
        assert_eq!(grep(&["--extended-regexp", "-E", "a"], "a\n").0, 0);
    }

    #[test]
    #[cfg(unix)]
    // a `-f` pattern file containing a raw, invalid-UTF-8 byte must match that literal
    // byte in the input, not silently fail to match because the byte got lossily replaced with
    // U+FFFD while the pattern file was being read (`printf '\xff\n' > p; printf '\xff\n' |
    // grep -c -f p` gives GNU's 1, not 0). Verified against the oracle.
    fn pattern_file_with_invalid_utf8_matches_the_raw_byte() {
        // `tempfile::NamedTempFile` is native-only (see its dependency's own wasm32 exclusion);
        // `test_scratch` is this crate's own path helper for a unit test's scratch file, which
        // works under both the native test runner and the WASM one.
        let file = crate::tools::test_scratch("grep-pattern-file");
        std::fs::write(&file, b"\xff\n").unwrap();
        let path = file.to_str().unwrap();
        let (code, out, _) = grep(&["-c", "-f", path], "\u{ff}\n");
        // A lossily-decoded pattern file would compile `\xff` as U+00FF and so match this
        // line (its UTF-8 encoding, 2 bytes: 0xC3 0xBF) instead of the single raw byte —
        // proving the fix actually changed *which* byte sequence is matched, not just that
        // something now matches.
        assert_eq!((code, out.as_str()), (1, "0\n"));
        let (code, out, _) = grep(&["-c", "-f", path], "\u{100}\n");
        assert_eq!((code, out.as_str()), (1, "0\n"));

        // The real repro: the *input* line is the single raw byte too. `grep()`'s test helper
        // takes `&str`, so build one whose bytes are deliberately invalid UTF-8 at runtime
        // (never as a `b"..."` literal, which rustc's own lint refuses to accept here) — safe
        // because the helper only ever calls `.as_bytes()` on it, never anything that assumes
        // validity.
        let raw_bytes: Vec<u8> = [0xffu8, b'\n'].to_vec();
        let raw_line = unsafe { core::str::from_utf8_unchecked(&raw_bytes) };
        let (code, out, _) = grep(&["-c", "-f", path], raw_line);
        assert_eq!((code, out.as_str()), (0, "1\n"));
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    // GNU's `-P` (PCRE) defaults `\d`/`\w`/`\s` (and their negations) to ASCII only,
    // unlike fancy_regex's own Unicode-aware default.
    fn perl_classes_are_ascii_only() {
        // "١٢٣" is Arabic-Indic digits (U+0661 U+0662 U+0663), not ASCII.
        assert_eq!(grep(&["-P", "\\d"], "١٢٣\n").1, "");
        assert_eq!(grep(&["-oP", "\\d+"], "a1٢3\n").1, "1\n3\n");
        assert_eq!(grep(&["-oP", "[\\d]+"], "a1٢3\n").1, "1\n3\n");
        assert_eq!(grep(&["-oP", "\\D+"], "1٢a2\n").1, "٢a\n");
        // still matches plain ASCII digits/word chars/space normally.
        assert_eq!(grep(&["-oP", "\\w+"], "foo_1 bar\n").1, "foo_1\nbar\n");
        assert_eq!(grep(&["-oP", "a\\sb"], "a b\n").1, "a b\n");
    }

    #[test]
    // `[:alpha:]` widens to Unicode letters under a UTF-8 locale (verified against the
    // oracle) — the regex crate's own POSIX classes are always ASCII-only, unlike GNU's.
    fn alpha_class_matches_unicode_letters_under_utf8() {
        assert_eq!(
            grep(&["-o", "[[:alpha:]]*"], "héllo wörld\n").1,
            "héllo\nwörld\n"
        );
        assert_eq!(
            grep(&["-o", "[[:alpha:]]*"], "日本語 テキスト\n").1,
            "日本語\nテキスト\n"
        );
        // A negated class still works — the substitution has to land inside the brackets.
        assert_eq!(grep(&["-o", "[^[:alpha:]]"], "a é\n").1, " \n");
    }

    #[test]
    // POSIX has no lazy quantifiers, so a `?` stacked onto one (`a+?`, `a*?`,
    // `a??`, `a{2,3}?`) is a redundant no-op in GNU, not Perl-style laziness.
    fn stacked_quantifiers_are_not_lazy() {
        assert_eq!(grep(&["-oE", "a+?"], "aaa\n").1, "aaa\n");
        assert_eq!(grep(&["-oE", "a*?"], "aaa\n").1, "aaa\n");
        assert_eq!(grep(&["-oE", "a??"], "a\n").1, "a\n");
        assert_eq!(grep(&["-oE", "a{2,3}?"], "aaa\n").1, "aaa\n");
        assert_eq!(grep(&["-o", "a\\+\\?"], "aaa\n").1, "aaa\n");
        assert_eq!(grep(&["-o", "a\\{2,3\\}\\?"], "aaa\n").1, "aaa\n");
    }

    #[test]
    // interval edge cases, verified against the oracle.
    fn interval_edge_cases_follow_gnu() {
        // A missing lower bound means 0, in both dialects (every line matches: unanchored at
        // the front, `a{0,2}$` finds a 1-or-2-`a` match ending each line, `aaa` included).
        assert_eq!(grep(&["-E", "a{,2}$"], "a\naa\naaa\n").1, "a\naa\naaa\n");
        assert_eq!(grep(&["a\\{,2\\}$"], "a\naa\naaa\n").1, "a\naa\naaa\n");
        // BRE: a leading `\{m,n\}` (nothing before it to repeat) is literal text.
        assert_eq!(grep(&["\\{1\\}a"], "{1}a\nba\n").1, "{1}a\n");
        // ERE: a leading `{m,n}` is different — GNU drops the whole interval (as if it had
        // never been written) rather than treating it as literal text, so what's left is
        // whatever follows: `{1}a` ends up matching any line containing `a`, `ba` included.
        assert_eq!(grep(&["-E", "{1}a"], "{1}a\nba\nc\n").1, "{1}a\nba\n");
    }

    #[test]
    fn selection_and_output_modes() {
        assert_eq!(grep(&["-ow", "foo"], "foo-bar foobar\n").1, "foo\n");
        assert_eq!(
            grep(&["-x", "-e", "l1", "-e", "foo"], "l1\nfoo 1\n").1,
            "l1\n"
        );
        assert_eq!(grep(&["-c", ""], "a\nb\n").1, "2\n");
        assert_eq!(grep(&["-vc", "a"], "a\nb\nc\n").1, "2\n");
        assert_eq!(grep(&["-n", "b"], "a\nb\n").1, "2:b\n");
        assert_eq!(grep(&["-ob", "o"], "foo\n").1, "1:o\n2:o\n");
        assert_eq!(grep(&["-io", "école"], "ÉCOLE école\n").1, "ÉCOLE\nécole\n");
        assert_eq!(
            grep(&["-q", "b"], "a\nb\n"),
            (0, String::new(), String::new())
        );
        assert_eq!(grep(&["zzz"], "a\n").0, 1);
    }

    #[test]
    fn context_and_limits() {
        let input = "l1\nfoo 1\nl3\nl4\nfoo 2\nl6\nl7\nl8\nfoo 3\nl10\n";
        assert_eq!(
            grep(&["-A1", "-n", "foo"], input).1,
            "2:foo 1\n3-l3\n--\n5:foo 2\n6-l6\n--\n9:foo 3\n10-l10\n"
        );
        assert_eq!(
            grep(&["-C1", "foo"], input).1,
            "l1\nfoo 1\nl3\nl4\nfoo 2\nl6\n--\nl8\nfoo 3\nl10\n"
        );
        assert_eq!(grep(&["-m1", "-A1", "foo"], input).1, "foo 1\nl3\n");
        assert_eq!(grep(&["-2", "foo 2"], input).1, "l3\nl4\nfoo 2\nl6\nl7\n");
        assert_eq!(
            grep(&["--no-group-separator", "-A1", "foo"], input).1,
            "foo 1\nl3\nfoo 2\nl6\nfoo 3\nl10\n"
        );
        // `-m0` selects zero lines — not even the first match — same as no match at
        // all, rather than printing one line before honoring the limit.
        let (code, out, _) = grep(&["-m0", "foo"], input);
        assert_eq!((code, out.as_str()), (1, ""));
        // `-A0`/`-B0`/`-C0` still turn on the `--` group separator between
        // non-adjacent matches, even though the explicit context amount is zero.
        assert_eq!(
            grep(&["-A0", "-e", "foo 1", "-e", "foo 3"], input).1,
            "foo 1\n--\nfoo 3\n"
        );
    }

    #[test]
    #[cfg(unix)]
    // The `--` group separator belongs between two *file operands* too, not just between two
    // non-adjacent matches within one file: two different files' output is never "adjacent".
    // `Search` is constructed fresh per target, so this needs carrying `printed()` forward as
    // each target's `separator_owed` — verified against the oracle, which puts `--` here
    // exactly where a single file with the same content, searched twice, would put it between
    // its own two non-adjacent matches.
    fn group_separator_appears_between_file_operands() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), "a\nmatch\nb\n").unwrap();
        std::fs::write(root.path().join("n"), "other\n").unwrap();
        let resolve = |path: &str| root.path().join(path);
        let block = "a-a\na:match\na-b\n";

        let argv = [
            "grep".to_owned(),
            "-C1".to_owned(),
            "match".to_owned(),
            "a".to_owned(),
            "a".to_owned(),
        ];
        let Ok(Parsed::Run(options)) = parse(&argv) else {
            panic!("expected to parse");
        };
        let matcher = match compile(&options, true, &|path| std::fs::read(path)) {
            Ok(matcher) => matcher,
            Err(refusal) => panic!("expected to compile: {}", refusal.message),
        };
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_sync(
            &options,
            &matcher,
            true,
            &mut std::io::empty(),
            &mut out,
            &mut err,
            &resolve,
        )
        .unwrap();
        assert_eq!(
            (code, String::from_utf8(out).unwrap()),
            (0, format!("{block}--\n{block}"))
        );

        // A second target ("n", no match of its own) doesn't reset the flag: a third target
        // that does match still gets the separator against the *first* target's output.
        let argv = [
            "grep".to_owned(),
            "-C1".to_owned(),
            "match".to_owned(),
            "a".to_owned(),
            "n".to_owned(),
            "a".to_owned(),
        ];
        let Ok(Parsed::Run(options)) = parse(&argv) else {
            panic!("expected to parse");
        };
        let matcher = match compile(&options, true, &|path| std::fs::read(path)) {
            Ok(matcher) => matcher,
            Err(refusal) => panic!("expected to compile: {}", refusal.message),
        };
        let mut out = Vec::new();
        let code = run_sync(
            &options,
            &matcher,
            true,
            &mut std::io::empty(),
            &mut out,
            &mut err,
            &resolve,
        )
        .unwrap();
        assert_eq!(
            (code, String::from_utf8(out).unwrap()),
            (0, format!("{block}--\n{block}"))
        );
    }

    #[test]
    // with `-z`, a "line" is delimited by NUL, so an embedded real `\n` is just an
    // ordinary character — `.` must match it too, not stop at it.
    fn null_data_lets_dot_match_an_embedded_newline() {
        let (code, out, _) = grep(&["-z", "-o", "one.two"], "one\ntwo\0three\0");
        assert_eq!((code, out.as_str()), (0, "one\ntwo\0"));
    }

    #[test]
    // `--color=always` used to be a silent no-op. Byte sequences verified
    // against the real oracle (`grep --color=always o <<<hi | cat -v`, etc.) — see
    // `sgr`/`colored` and the `text-tools: grep --color=...` matrix cases.
    fn color_always_highlights_matches() {
        let (code, out, _) = grep(&["--color=always", "o"], "hello world\n");
        assert_eq!(
            (code, out.as_str()),
            (
                0,
                "hell\x1b[01;31m\x1b[Ko\x1b[m\x1b[K w\x1b[01;31m\x1b[Ko\x1b[m\x1b[Krld\n"
            )
        );
        // `-o`: only the matched text is written, still colored.
        let (code, out, _) = grep(&["--color=always", "-o", "o"], "hello world\n");
        assert_eq!(
            (code, out.as_str()),
            (
                0,
                "\x1b[01;31m\x1b[Ko\x1b[m\x1b[K\n\x1b[01;31m\x1b[Ko\x1b[m\x1b[K\n"
            )
        );
        // `-v`: no match spans exist to highlight, so no color at all.
        let (code, out, _) = grep(&["--color=always", "-v", "xyz"], "hello world\n");
        assert_eq!((code, out.as_str()), (0, "hello world\n"));
    }

    #[test]
    // `auto` and a bare `--color` both mean "color if stdout is a terminal" — this tool's
    // stdout never is, so both stay plain, same as the pre-existing (and still correct)
    // default of no color at all.
    fn color_auto_and_bare_produce_no_color() {
        assert_eq!(
            grep(&["--color=auto", "o"], "hello world\n").1,
            "hello world\n"
        );
        assert_eq!(grep(&["--color", "o"], "hello world\n").1, "hello world\n");
        assert_eq!(
            grep(&["--color=never", "o"], "hello world\n").1,
            "hello world\n"
        );
    }

    #[test]
    // GNU's own quirk (verified against the oracle): an unrecognized `--color` value does not
    // raise a usage error — it shows the help text and exits 0, same as `--help`.
    fn color_invalid_value_shows_help() {
        let argv: Vec<String> = ["grep", "--color=bogus", "o"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert!(matches!(parse(&argv), Ok(Parsed::Help)));
    }

    #[test]
    fn usage_errors() {
        let (code, _, err) = grep(&["-k", "x"], "");
        assert_eq!(code, 2);
        assert_eq!(
            err,
            "grep: invalid option -- 'k'\nUsage: grep [OPTION]... PATTERNS [FILE]...\nTry 'grep --help' for more information.\n"
        );
        assert_eq!(grep(&[], "").0, 2);
        let (code, out, err) = grep(&["b", "/nonexistent/file", "-"], "b\n");
        assert_eq!((code, out.as_str()), (2, "(standard input):b\n"));
        assert_eq!(err, "grep: /nonexistent/file: No such file or directory\n");
    }

    #[test]
    fn binary_files() {
        let (code, out, err) = grep(&["hello"], "bin\0hello\n");
        assert_eq!((code, out.as_str()), (0, ""));
        assert_eq!(err, "grep: (standard input): binary file matches\n");
        assert_eq!(grep(&["-c", "hello"], "bin\0hello\n").1, "1\n");
        assert_eq!(grep(&["-a", "hello"], "bin\0hello\n").1, "bin\0hello\n");
    }

    #[test]
    // Under a UTF-8 locale, GNU also treats a line that isn't valid UTF-8 as binary — not just
    // one with a NUL byte — verified against the oracle for Latin-1 text, a lone 0xff, an
    // encoded surrogate and a truncated multi-byte sequence, all piped through stdin (no
    // seekable file to read ahead in, so this has to be the same incremental check a NUL gets).
    fn invalid_utf8_is_binary_under_a_utf8_locale() {
        for bytes in [
            &b"caf\xe9 ol\xe9\n"[..], // Latin-1, not UTF-8
            &b"a\xffb\n"[..],         // a lone 0xff
            &b"a\xed\xa0\x80b\n"[..], // an encoded surrogate (invalid in UTF-8)
            &b"ab\xc3\n"[..],         // a truncated 2-byte sequence
        ] {
            let (code, out, err) = grep_bytes(&["a"], bytes);
            assert_eq!((code, out.as_slice()), (0, &b""[..]), "{bytes:?}");
            assert_eq!(
                err, "grep: (standard input): binary file matches\n",
                "{bytes:?}"
            );
        }
    }

    #[test]
    // Binary detection is about the bytes actually written, not merely present anywhere in the
    // file — verified against the oracle: a pattern that only selects a clean line still prints
    // normally even though another line has a lone 0xff, and `-o` (each match is its own
    // separately-validated span) extracts plain-ASCII matches from a line with a stray invalid
    // byte elsewhere without tripping binary detection either.
    fn invalid_utf8_elsewhere_in_the_file_does_not_poison_a_clean_selected_line() {
        let input = b"a\xffb\nzz\n";
        let (code, out, err) = grep_bytes(&["-n", "z"], input);
        assert_eq!((code, err.as_str()), (0, ""));
        assert_eq!(out, b"2:zz\n");

        let (code, out, err) = grep_bytes(&["-o", "[a-z]"], input);
        assert_eq!((code, err.as_str()), (0, ""));
        assert_eq!(out, b"a\nb\nz\nz\n");

        // The line that *is* selected still trips it when that's the invalid one.
        let (code, out, err) = grep_bytes(&["a"], input);
        assert_eq!((code, out.as_slice()), (0, &b""[..]));
        assert_eq!(err, "grep: (standard input): binary file matches\n");
    }

    #[test]
    // Unlike a plain invalid byte, a NUL isn't exempted by an earlier, non-matching line or by
    // `-o` — verified against the oracle three ways.
    fn a_nul_trips_binary_detection_even_off_the_matching_line_or_span() {
        // A NUL on an earlier, non-matching line still makes a later match report binary.
        let (code, out, err) = grep_bytes(&["-n", "z"], b"a\0b\nzz\n");
        assert_eq!((code, out.as_slice()), (0, &b""[..]));
        assert_eq!(err, "grep: (standard input): binary file matches\n");

        // `-o`'s per-span exemption is for a plain invalid byte, not a NUL: one past the end of
        // the matched span still trips it.
        let (code, out, err) = grep_bytes(&["-o", "hit"], b"hit\0\n");
        assert_eq!((code, out.as_slice()), (0, &b""[..]));
        assert_eq!(err, "grep: (standard input): binary file matches\n");

        // ...and so does one elsewhere on the very line `-o` matched, not just past its own span.
        let (code, out, err) = grep_bytes(&["-o", "[a-z]"], b"a\0b\nzz\n");
        assert_eq!((code, out.as_slice()), (0, &b""[..]));
        assert_eq!(err, "grep: (standard input): binary file matches\n");
    }

    #[test]
    #[cfg(unix)]
    // `-r` against a single, explicit, non-directory file operand never actually walks
    // anything — the filename prefix decision follows that (whether recursion *happened*), not
    // whether `-r` was merely given. Verified against the oracle.
    fn recursive_flag_alone_does_not_force_a_filename_prefix() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("nums"), "one\n").unwrap();
        let resolve = |path: &str| root.path().join(path);

        let argv = [
            "grep".to_owned(),
            "-r".to_owned(),
            "one".to_owned(),
            "nums".to_owned(),
        ];
        let Ok(Parsed::Run(options)) = parse(&argv) else {
            panic!("expected to parse");
        };
        let (found, recursed) = targets(&options, &resolve);
        assert!(!recursed, "a plain file operand must not set `recursed`");
        assert!(!with_filename(&options, &found, recursed));

        // The same flag against a directory operand *does* walk, and still prefixes even
        // though exactly one file is found inside it.
        std::fs::create_dir(root.path().join("d")).unwrap();
        std::fs::write(root.path().join("d/nums"), "one\n").unwrap();
        let argv = [
            "grep".to_owned(),
            "-r".to_owned(),
            "one".to_owned(),
            "d".to_owned(),
        ];
        let Ok(Parsed::Run(options)) = parse(&argv) else {
            panic!("expected to parse");
        };
        let (found, recursed) = targets(&options, &resolve);
        assert!(recursed);
        assert!(with_filename(&options, &found, recursed));
    }

    #[test]
    #[cfg(unix)]
    // `--include`/`--exclude` filter every file target, including one named directly
    // on the command line — not just files a recursive walk discovers — and regardless of
    // whether `-r`/`-R` was given at all. Verified against the oracle.
    fn include_exclude_filter_explicit_file_operands() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("nums.txt"), "one\n").unwrap();
        let resolve = |path: &str| root.path().join(path);

        let argv = [
            "grep".to_owned(),
            "--include=*.log".to_owned(),
            "one".to_owned(),
            "nums.txt".to_owned(),
        ];
        let Ok(Parsed::Run(options)) = parse(&argv) else {
            panic!("expected to parse");
        };
        let (found, _) = targets(&options, &resolve);
        assert!(
            !found.iter().any(|t| matches!(t, Target::File(..))),
            "--include=*.log must filter out nums.txt: {found:?}"
        );

        let argv = [
            "grep".to_owned(),
            "--exclude=*.txt".to_owned(),
            "one".to_owned(),
            "nums.txt".to_owned(),
        ];
        let Ok(Parsed::Run(options)) = parse(&argv) else {
            panic!("expected to parse");
        };
        let (found, _) = targets(&options, &resolve);
        assert!(
            !found.iter().any(|t| matches!(t, Target::File(..))),
            "--exclude=*.txt must filter out nums.txt: {found:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    // `--exclude-dir` also filters a directory named directly as an operand, not just
    // subdirectories discovered while recursing. Verified against the oracle.
    fn exclude_dir_filters_a_directory_operand() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("sub")).unwrap();
        std::fs::write(root.path().join("sub/f"), "one\n").unwrap();
        let resolve = |path: &str| root.path().join(path);

        let argv = [
            "grep".to_owned(),
            "-r".to_owned(),
            "--exclude-dir=sub".to_owned(),
            "one".to_owned(),
            "sub".to_owned(),
        ];
        let Ok(Parsed::Run(options)) = parse(&argv) else {
            panic!("expected to parse");
        };
        let (found, _) = targets(&options, &resolve);
        assert!(
            !found.iter().any(|t| matches!(t, Target::File(..))),
            "--exclude-dir=sub must filter out the sub/ operand itself: {found:?}"
        );
    }

    #[test]
    // `-I` (treat a binary match as no match) must win over `-q` (exit as soon as a
    // match is found) — `-qI` on a binary file exits 1, not 0, same as plain `-I` would print
    // nothing and exit 1 for the same input.
    fn quiet_defers_to_binary_without_match() {
        let (code, out, err) = grep(&["-qI", "foo"], "foo\0bar\n");
        assert_eq!((code, out.as_str(), err.as_str()), (1, "", ""));
    }

    #[test]
    #[cfg(unix)]
    // `-R` follows symlinks, so a symlink back to an ancestor directory must not
    // make the walk match the same file repeatedly or never finish.
    fn recursive_dereference_stops_at_a_symlink_loop() {
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("a");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("file"), "needle\n").unwrap();
        std::os::unix::fs::symlink("..", sub.join("up1")).unwrap();

        let argv = ["grep".to_owned(), "-R".to_owned(), "needle".to_owned()];
        let Ok(Parsed::Run(options)) = parse(&argv) else {
            panic!("expected -R needle to parse as Run");
        };
        let resolve = |path: &str| {
            if path == "." {
                root.path().to_path_buf()
            } else {
                PathBuf::from(path)
            }
        };
        let (found, _recursed) = targets(&options, &resolve);
        let files: Vec<_> = found
            .iter()
            .filter_map(|t| match t {
                Target::File(name, _) => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(files, vec!["a/file".to_owned()], "found: {files:?}");
        let errors: Vec<_> = found
            .iter()
            .filter_map(|t| match t {
                Target::Error(message) => Some(message.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
        assert!(errors[0].contains("recursive directory loop"), "{errors:?}");
    }
}
