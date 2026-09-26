//! GNU-compatible `find`. The expression is parsed with POSIX precedence into a short-circuit
//! program, each start point is walked iteratively in sorted order, and the program runs once per
//! entry. `-exec` runs through the shell as an isolated child command, so it works in the
//! cooperative WASM shell as well as natively.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use brush_core::builtins::{
    BoxFuture, ContentOptions, ContentType, ExecutionBoundary, Registration,
};
use brush_core::commands::ExecutionContext;
use brush_core::extensions::ShellExtensions;
use brush_core::openfiles::OpenFile;
use brush_core::{CommandArg, Error, ExecutionResult};
use futures::io::AsyncWriteExt;

use crate::manifest::Manifest;

const NAME: &str = "find";
const SYNOPSIS: &str = "search for files in a directory hierarchy";
/// GNU flushes `-exec ... +` batches well below the kernel's argument limit.
const BATCH_BYTES: usize = 128 * 1024;
const NANOS: i128 = 1_000_000_000;

/// Quote a name the way GNU find does in a UTF-8 locale.
fn quote(text: &str) -> String {
    format!("\u{2018}{text}\u{2019}")
}

// ------------------------------------------------------------------------------------------------
// Glob and regex matching
// ------------------------------------------------------------------------------------------------

enum Glob {
    Any,
    Star,
    Char(char),
    Class {
        negated: bool,
        items: Vec<ClassItem>,
    },
}

enum ClassItem {
    Char(char),
    Range(char, char),
    Named(String),
}

fn glob_tokens(pattern: &str) -> Vec<Glob> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                if !matches!(tokens.last(), Some(Glob::Star)) {
                    tokens.push(Glob::Star);
                }
                i += 1;
            }
            '?' => {
                tokens.push(Glob::Any);
                i += 1;
            }
            '\\' if i + 1 < chars.len() => {
                tokens.push(Glob::Char(chars[i + 1]));
                i += 2;
            }
            '[' => match glob_class(&chars, i + 1) {
                Some((class, next)) => {
                    tokens.push(class);
                    i = next;
                }
                None => {
                    tokens.push(Glob::Char('['));
                    i += 1;
                }
            },
            c => {
                tokens.push(Glob::Char(c));
                i += 1;
            }
        }
    }
    tokens
}

/// Parse a bracket expression starting just after `[`; `None` when it is unterminated (then the
/// `[` is literal, as in fnmatch).
fn glob_class(chars: &[char], mut i: usize) -> Option<(Glob, usize)> {
    let negated = matches!(chars.get(i), Some('!' | '^'));
    if negated {
        i += 1;
    }
    let mut items = Vec::new();
    let mut first = true;
    loop {
        let c = *chars.get(i)?;
        if c == ']' && !first {
            return Some((Glob::Class { negated, items }, i + 1));
        }
        first = false;
        if c == '['
            && chars.get(i + 1) == Some(&':')
            && let Some(end) = (i + 2..chars.len().saturating_sub(1))
                .find(|&j| chars[j] == ':' && chars[j + 1] == ']')
        {
            items.push(ClassItem::Named(chars[i + 2..end].iter().collect()));
            i = end + 2;
            continue;
        }
        let mut low = c;
        if c == '\\' && i + 1 < chars.len() {
            i += 1;
            low = chars[i];
        }
        i += 1;
        if chars.get(i) == Some(&'-') && chars.get(i + 1).is_some_and(|&next| next != ']') {
            let mut high = chars[i + 1];
            i += 2;
            if high == '\\' && i < chars.len() {
                high = chars[i];
                i += 1;
            }
            items.push(ClassItem::Range(low, high));
        } else {
            items.push(ClassItem::Char(low));
        }
    }
}

fn named_class(name: &str, c: char) -> bool {
    match name {
        "alpha" => c.is_alphabetic(),
        "digit" => c.is_ascii_digit(),
        "alnum" => c.is_alphanumeric(),
        "upper" => c.is_uppercase(),
        "lower" => c.is_lowercase(),
        "space" => c.is_whitespace(),
        "blank" => c == ' ' || c == '\t',
        "punct" => c.is_ascii_punctuation(),
        "print" => !c.is_control(),
        "graph" => !c.is_control() && !c.is_whitespace(),
        "cntrl" => c.is_control(),
        "xdigit" => c.is_ascii_hexdigit(),
        _ => false,
    }
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn class_matches(negated: bool, items: &[ClassItem], c: char, ignore_case: bool) -> bool {
    let candidates = if ignore_case {
        vec![c, fold(c), c.to_uppercase().next().unwrap_or(c)]
    } else {
        vec![c]
    };
    let hit = items.iter().any(|item| {
        candidates.iter().any(|&c| match item {
            ClassItem::Char(x) => *x == c,
            ClassItem::Range(low, high) => (*low..=*high).contains(&c),
            ClassItem::Named(name) => named_class(name, c),
        })
    });
    hit != negated
}

fn token_matches(token: &Glob, c: char, ignore_case: bool) -> bool {
    match token {
        Glob::Any => true,
        Glob::Star => false,
        Glob::Char(x) => *x == c || (ignore_case && fold(*x) == fold(c)),
        Glob::Class { negated, items } => class_matches(*negated, items, c, ignore_case),
    }
}

/// `fnmatch(3)` with no flags, as GNU find's `-name`/`-path` use it: `*` and `?` match any
/// character including `/` and a leading `.`; bracket expressions take `!`/`^`, ranges and
/// `[:class:]`; a backslash quotes the next character.
pub(crate) fn fnmatch(pattern: &str, text: &str, ignore_case: bool) -> bool {
    let tokens = glob_tokens(pattern);
    let text: Vec<char> = text.chars().collect();
    let (mut t, mut s) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while s < text.len() {
        if t < tokens.len() {
            if matches!(tokens[t], Glob::Star) {
                backtrack = Some((t, s));
                t += 1;
                continue;
            }
            if token_matches(&tokens[t], text[s], ignore_case) {
                t += 1;
                s += 1;
                continue;
            }
        }
        match backtrack {
            Some((star, from)) => {
                t = star + 1;
                s = from + 1;
                backtrack = Some((star, from + 1));
            }
            None => return false,
        }
    }
    tokens[t..].iter().all(|token| matches!(token, Glob::Star))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RegexType {
    Emacs,
    Basic,
    Extended,
}

impl RegexType {
    const NAMES: [&'static str; 13] = [
        "findutils-default",
        "ed",
        "emacs",
        "gnu-awk",
        "grep",
        "posix-awk",
        "awk",
        "posix-basic",
        "posix-egrep",
        "egrep",
        "posix-extended",
        "posix-minimal-basic",
        "sed",
    ];

    fn parse(name: &str) -> Option<Self> {
        match name {
            "findutils-default" | "emacs" => Some(Self::Emacs),
            "ed" | "grep" | "posix-basic" | "posix-minimal-basic" | "sed" => Some(Self::Basic),
            "gnu-awk" | "posix-awk" | "awk" | "posix-egrep" | "egrep" | "posix-extended" => {
                Some(Self::Extended)
            }
            _ => None,
        }
    }
}

/// Translate GNU find's default (Emacs) regex syntax: `\(`/`\)`/`\|` group and alternate,
/// `+`/`?`/`*` are operators, `{`, `(`, `|` are literal, brackets have no classes and treat `\`
/// literally.
fn emacs_regex(pattern: &str) -> Result<String, &'static str> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut depth = 0usize;
    let mut at_start = true;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        i += 1;
        let starting = at_start;
        at_start = false;
        match c {
            '\\' => {
                let Some(&next) = chars.get(i) else {
                    return Err("Trailing backslash");
                };
                i += 1;
                match next {
                    '(' => {
                        out.push('(');
                        depth += 1;
                        at_start = true;
                    }
                    ')' => {
                        if depth == 0 {
                            return Err("Unmatched ) or \\)");
                        }
                        depth -= 1;
                        out.push(')');
                    }
                    '|' => {
                        out.push('|');
                        at_start = true;
                    }
                    d @ '1'..='9' => {
                        out.push_str("(?:\\");
                        out.push(d);
                        out.push(')');
                    }
                    'w' | 'W' | 'b' | 'B' => {
                        out.push('\\');
                        out.push(next);
                    }
                    '<' => out.push_str("\\b"),
                    '>' => out.push_str("\\b"),
                    '`' => out.push_str("\\A"),
                    '\'' => out.push_str("\\z"),
                    other => out.push_str(&regex::escape(&other.to_string())),
                }
            }
            '[' => {
                out.push('[');
                if chars.get(i) == Some(&'^') {
                    out.push('^');
                    i += 1;
                }
                if chars.get(i) == Some(&']') {
                    out.push_str("\\]");
                    i += 1;
                }
                loop {
                    let Some(&inner) = chars.get(i) else {
                        return Err("Invalid regular expression");
                    };
                    i += 1;
                    match inner {
                        ']' => {
                            out.push(']');
                            break;
                        }
                        '\\' | '[' | '&' | '~' => {
                            out.push('\\');
                            out.push(inner);
                        }
                        other => out.push(other),
                    }
                }
            }
            '*' | '+' | '?' if starting => {
                out.push('\\');
                out.push(c);
            }
            '*' | '+' | '?' | '.' => out.push(c),
            '^' if starting => {
                out.push('^');
                at_start = true;
            }
            '$' if i == chars.len()
                || (chars[i] == '\\' && matches!(chars.get(i + 1), Some(')' | '|'))) =>
            {
                out.push('$');
            }
            other => out.push_str(&regex::escape(&other.to_string())),
        }
    }
    if depth > 0 {
        return Err("Unmatched ( or \\(");
    }
    Ok(out)
}

enum PathRegex {
    Plain(regex::Regex),
    Fancy(fancy_regex::Regex),
}

impl PathRegex {
    fn is_match(&self, text: &str) -> bool {
        match self {
            Self::Plain(regex) => regex.is_match(text),
            Self::Fancy(regex) => regex.is_match(text).unwrap_or(false),
        }
    }
}

fn compile_regex(kind: RegexType, pattern: &str, ignore_case: bool) -> Result<PathRegex, Usage> {
    let failure = |reason: &str| {
        fail(format!(
            "failed to compile regular expression '{pattern}': {reason}"
        ))
    };
    let translated = match kind {
        RegexType::Emacs => emacs_regex(pattern).map_err(failure)?,
        RegexType::Basic => super::grep::basic(pattern),
        RegexType::Extended => super::grep::extended(pattern),
    };
    let flags = if ignore_case { "(?si)" } else { "(?s)" };
    let full = format!("{flags}^(?:{translated})$");
    let reason = |message: String| {
        if message.contains("unclosed group") {
            "Unmatched ( or \\("
        } else if message.contains("unopened group") {
            "Unmatched ) or \\)"
        } else if message.contains("unclosed character class") {
            "Unmatched [, [^, [:, [., or [="
        } else {
            "Invalid regular expression"
        }
    };
    if super::grep::has_back_reference(&translated) {
        fancy_regex::Regex::new(&full)
            .map(PathRegex::Fancy)
            .map_err(|error| failure(reason(error.to_string())))
    } else {
        regex::Regex::new(&full)
            .map(PathRegex::Plain)
            .map_err(|error| failure(reason(error.to_string())))
    }
}

// ------------------------------------------------------------------------------------------------
// File metadata
// ------------------------------------------------------------------------------------------------

/// Nanoseconds since the Unix epoch.
type Ts = i128;

fn timestamp(time: std::io::Result<SystemTime>) -> Option<Ts> {
    let time = time.ok()?;
    Some(match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i128::try_from(after.as_nanos()).unwrap_or(i128::MAX),
        Err(before) => -i128::try_from(before.duration().as_nanos()).unwrap_or(i128::MAX),
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(unix),
    allow(dead_code, reason = "only unix exposes devices, FIFOs and sockets")
)]
enum Kind {
    File,
    Dir,
    Link,
    Block,
    Char,
    Fifo,
    Socket,
    Unknown,
}

impl Kind {
    fn of(file_type: std::fs::FileType) -> Self {
        if file_type.is_symlink() {
            Self::Link
        } else if file_type.is_dir() {
            Self::Dir
        } else if file_type.is_file() {
            Self::File
        } else {
            platform::special(file_type)
        }
    }

    const fn letter(self) -> char {
        match self {
            Self::File => 'f',
            Self::Dir => 'd',
            Self::Link => 'l',
            Self::Block => 'b',
            Self::Char => 'c',
            Self::Fifo => 'p',
            Self::Socket => 's',
            Self::Unknown => 'U',
        }
    }
}

#[derive(Clone, Copy)]
enum TimeField {
    Access,
    Change,
    Modify,
}

#[derive(Clone)]
struct Stat {
    kind: Kind,
    size: u64,
    mtime: Ts,
    atime: Ts,
    ctime: Ts,
    mode: Option<u32>,
}

impl Stat {
    /// `ctime` asks for the status-change time, which costs a second stat on WASI.
    fn read(path: &Path, follow: bool, ctime: bool) -> std::io::Result<Self> {
        let meta = if follow {
            std::fs::metadata(path)?
        } else {
            std::fs::symlink_metadata(path)?
        };
        let mtime = timestamp(meta.modified()).unwrap_or(0);
        let (ctime, mode) = platform::extra(&meta, path, follow, ctime);
        let kind = match Kind::of(meta.file_type()) {
            Kind::Unknown => platform::kind(path, follow),
            kind => kind,
        };
        Ok(Self {
            kind,
            size: meta.len(),
            mtime,
            atime: timestamp(meta.accessed()).unwrap_or(mtime),
            // WASI exposes no status-change time through std; the modification time is the
            // closest stand-in when the libc stat below cannot supply one.
            ctime: ctime.unwrap_or(mtime),
            mode,
        })
    }

    const fn time(&self, field: TimeField) -> Ts {
        match field {
            TimeField::Access => self.atime,
            TimeField::Change => self.ctime,
            TimeField::Modify => self.mtime,
        }
    }
}

mod platform {
    use super::{Kind, Ts};
    use std::path::Path;

    #[cfg(unix)]
    pub(super) fn special(file_type: std::fs::FileType) -> Kind {
        use std::os::unix::fs::FileTypeExt;
        if file_type.is_block_device() {
            Kind::Block
        } else if file_type.is_char_device() {
            Kind::Char
        } else if file_type.is_fifo() {
            Kind::Fifo
        } else if file_type.is_socket() {
            Kind::Socket
        } else {
            Kind::Unknown
        }
    }

    #[cfg(not(unix))]
    pub(super) fn special(_: std::fs::FileType) -> Kind {
        Kind::Unknown
    }

    /// The kind of a file std cannot name (std keeps WASI's device types unstable): from the C
    /// library's stat, which answers for the virtual `/dev` (see `devices`).
    #[cfg(target_os = "wasi")]
    pub(super) fn kind(path: &Path, follow: bool) -> Kind {
        match raw_stat(path, follow).map(|stat| stat.st_mode & libc::S_IFMT) {
            Some(libc::S_IFCHR) => Kind::Char,
            Some(libc::S_IFBLK) => Kind::Block,
            Some(libc::S_IFIFO) => Kind::Fifo,
            Some(libc::S_IFSOCK) => Kind::Socket,
            _ => Kind::Unknown,
        }
    }

    #[cfg(not(target_os = "wasi"))]
    pub(super) fn kind(_: &Path, _: bool) -> Kind {
        Kind::Unknown
    }

    /// Status-change time and permission bits, where the platform exposes them.
    #[cfg(unix)]
    pub(super) fn extra(
        meta: &std::fs::Metadata,
        _: &Path,
        _: bool,
        _: bool,
    ) -> (Option<Ts>, Option<u32>) {
        use std::os::unix::fs::MetadataExt;
        let ctime = i128::from(meta.ctime()) * super::NANOS + i128::from(meta.ctime_nsec());
        (Some(ctime), Some(meta.mode()))
    }

    #[cfg(target_os = "wasi")]
    pub(super) fn extra(
        _: &std::fs::Metadata,
        path: &Path,
        follow: bool,
        ctime: bool,
    ) -> (Option<Ts>, Option<u32>) {
        if !ctime {
            return (None, None);
        }
        (
            raw_stat(path, follow).and_then(|stat| {
                let ctime = i128::from(stat.st_ctim.tv_sec) * super::NANOS
                    + i128::from(stat.st_ctim.tv_nsec);
                (ctime != 0).then_some(ctime)
            }),
            None,
        )
    }

    #[cfg(not(any(unix, target_os = "wasi")))]
    pub(super) fn extra(
        _: &std::fs::Metadata,
        _: &Path,
        _: bool,
        _: bool,
    ) -> (Option<Ts>, Option<u32>) {
        (None, None)
    }

    /// Identity of a directory for `-L` loop detection.
    #[cfg(unix)]
    pub(super) fn identity(path: &Path) -> Option<(u64, u64)> {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path)
            .ok()
            .map(|meta| (meta.dev(), meta.ino()))
    }

    #[cfg(target_os = "wasi")]
    #[allow(unsafe_code, reason = "fstat of a descriptor the File owns")]
    pub(super) fn identity(path: &Path) -> Option<(u64, u64)> {
        use std::os::fd::AsRawFd;
        // A path's own identity on WASI is that of its last component, even a symbolic link
        // stat follows: the directory it names is opened, and its descriptor tells.
        let directory = std::fs::File::open(path).ok()?;
        // SAFETY: `libc::stat` is plain old data; an all-zero value is a valid instance.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: the descriptor is open for the call and `stat` is a valid, writable buffer.
        let status = unsafe { libc::fstat(directory.as_raw_fd(), &mut stat) };
        (status == 0).then_some((stat.st_dev, stat.st_ino))
    }

    #[cfg(not(any(unix, target_os = "wasi")))]
    pub(super) fn identity(_: &Path) -> Option<(u64, u64)> {
        None
    }

    /// wasi-libc's stat exposes the WASI link and status-change fields std keeps unstable.
    #[cfg(target_os = "wasi")]
    fn raw_stat(path: &Path, follow: bool) -> Option<libc::stat> {
        let path = std::ffi::CString::new(path.to_str()?).ok()?;
        // SAFETY: `libc::stat` is plain old data; an all-zero value is a valid instance.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `path` is a NUL-terminated C string that outlives the call and `stat` is a
        // valid, writable `libc::stat`; the functions only write into that buffer.
        let status = unsafe {
            if follow {
                libc::stat(path.as_ptr(), &raw mut stat)
            } else {
                libc::lstat(path.as_ptr(), &raw mut stat)
            }
        };
        (status == 0).then_some(stat)
    }
}

// ------------------------------------------------------------------------------------------------
// The expression
// ------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cmp {
    Less,
    Equal,
    Greater,
}

/// Split a leading `+`/`-` comparison sign.
fn comparison(text: &str) -> (Cmp, &str) {
    if let Some(rest) = text.strip_prefix('+') {
        (Cmp::Greater, rest)
    } else if let Some(rest) = text.strip_prefix('-') {
        (Cmp::Less, rest)
    } else {
        (Cmp::Equal, text)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Follow {
    Never,
    CommandLine,
    Always,
}

enum Target {
    Stdout,
    Output(usize),
}

enum Output {
    Stdout,
    Stderr,
    Null,
    File(std::io::BufWriter<std::fs::File>),
}

struct Exec {
    words: Vec<String>,
    batch: bool,
    in_dir: bool,
}

enum Primary {
    True,
    False,
    Name {
        pattern: String,
        ignore_case: bool,
    },
    Path {
        pattern: String,
        ignore_case: bool,
    },
    LinkName {
        pattern: String,
        ignore_case: bool,
    },
    Regex(PathRegex),
    Type {
        kinds: Vec<char>,
        other_side: bool,
    },
    Empty,
    Size {
        cmp: Cmp,
        count: u64,
        unit: u64,
    },
    Age {
        field: TimeField,
        cmp: Cmp,
        amount: f64,
        minutes: bool,
        origin: Ts,
    },
    Newer {
        field: TimeField,
        reference: Ts,
    },
    Prune,
    Quit,
    Print {
        zero: bool,
        target: Target,
    },
    Printf {
        format: Vec<Segment>,
        target: Target,
    },
    Delete,
    Exec(Exec),
}

enum Node {
    Leaf(usize),
    Not(Box<Node>),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Comma(Box<Node>, Box<Node>),
}

#[derive(Clone, Copy)]
enum Op {
    Eval(usize),
    Not,
    JumpIfFalse(usize),
    JumpIfTrue(usize),
}

fn emit(node: Node, code: &mut Vec<Op>) {
    match node {
        Node::Leaf(index) => code.push(Op::Eval(index)),
        Node::Not(inner) => {
            emit(*inner, code);
            code.push(Op::Not);
        }
        Node::And(left, right) => {
            emit(*left, code);
            let jump = code.len();
            code.push(Op::JumpIfFalse(0));
            emit(*right, code);
            code[jump] = Op::JumpIfFalse(code.len());
        }
        Node::Or(left, right) => {
            emit(*left, code);
            let jump = code.len();
            code.push(Op::JumpIfTrue(0));
            emit(*right, code);
            code[jump] = Op::JumpIfTrue(code.len());
        }
        Node::Comma(left, right) => {
            emit(*left, code);
            emit(*right, code);
        }
    }
}

/// A diagnostic that ends the command before anything runs.
#[derive(Debug)]
pub(crate) struct Usage {
    lines: Vec<String>,
    code: u8,
}

fn fail(message: impl Into<String>) -> Usage {
    Usage {
        lines: vec![message.into()],
        code: 1,
    }
}

fn refuse(feature: &str) -> Usage {
    Usage {
        lines: vec![format!("{feature} is unsupported in bash-tool")],
        code: 2,
    }
}

/// Everything parsed from the command line.
struct Program {
    starts: Vec<String>,
    follow: Follow,
    max_depth: Option<usize>,
    min_depth: usize,
    depth_first: bool,
    primaries: Vec<Primary>,
    code: Vec<Op>,
    outputs: Vec<Output>,
    warnings: Vec<String>,
    /// Whether any test or directive reads the status-change time.
    needs_ctime: bool,
}

/// Does `arg` begin the expression (GNU `looks_like_expression` for a leading argument)?
fn starts_expression(arg: &str) -> bool {
    (arg.starts_with('-') && arg.len() > 1) || arg == "!" || arg == "("
}

struct Parser<'a> {
    args: &'a [String],
    pos: usize,
    resolve: &'a dyn Fn(&str) -> PathBuf,
    now: Ts,
    regex_type: RegexType,
    day_start: bool,
    follow: Follow,
    max_depth: Option<usize>,
    min_depth: usize,
    depth_first: bool,
    primaries: Vec<Primary>,
    outputs: Vec<Output>,
    warnings: Vec<String>,
    has_action: bool,
    has_prune: bool,
    has_delete: bool,
    /// The most recent predicate, named in GNU's unquoted-pattern hint.
    last_predicate: Option<String>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&str> {
        self.args.get(self.pos).map(String::as_str)
    }

    /// No operand follows: the end, or a token that closes the current operand list.
    fn operand_missing(&self) -> bool {
        matches!(self.peek(), None | Some(")" | ","))
    }

    fn leaf(&mut self, primary: Primary) -> Node {
        self.primaries.push(primary);
        Node::Leaf(self.primaries.len() - 1)
    }

    fn expression(&mut self) -> Result<Node, Usage> {
        let mut left = self.alternation()?;
        while self.peek() == Some(",") {
            self.pos += 1;
            if self.operand_missing() {
                return Err(fail("expected an expression after ','"));
            }
            let right = self.alternation()?;
            left = Node::Comma(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn alternation(&mut self) -> Result<Node, Usage> {
        let mut left = self.conjunction()?;
        while let Some(operator @ ("-o" | "-or")) = self.peek() {
            let operator = operator.to_string();
            self.pos += 1;
            if self.operand_missing() {
                return Err(fail(format!("expected an expression after '{operator}'")));
            }
            let right = self.conjunction()?;
            left = Node::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn conjunction(&mut self) -> Result<Node, Usage> {
        let mut left = self.negation()?;
        loop {
            match self.peek() {
                None | Some(")" | "," | "-o" | "-or") => break,
                Some(operator @ ("-a" | "-and")) => {
                    let operator = operator.to_string();
                    self.pos += 1;
                    if self.operand_missing() {
                        return Err(fail(format!("expected an expression after '{operator}'")));
                    }
                }
                Some(_) => {}
            }
            let right = self.negation()?;
            left = Node::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn negation(&mut self) -> Result<Node, Usage> {
        if let Some(operator @ ("!" | "-not")) = self.peek() {
            let operator = operator.to_string();
            self.pos += 1;
            if self.operand_missing() {
                return Err(fail(format!("expected an expression after '{operator}'")));
            }
            return Ok(Node::Not(Box::new(self.negation()?)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Node, Usage> {
        let Some(token) = self.peek().map(str::to_string) else {
            return Err(fail("expected an expression"));
        };
        self.pos += 1;
        match token.as_str() {
            "(" => {
                if self.peek() == Some(")") {
                    return Err(fail(
                        "invalid expression; empty parentheses are not allowed.",
                    ));
                }
                if self.peek().is_none() {
                    return Err(fail(
                        "invalid expression; expected to find a ')' but didn't see one. Perhaps \
                         you need an extra predicate after '('",
                    ));
                }
                let inner = self.expression()?;
                if self.peek() != Some(")") {
                    return Err(fail(
                        "invalid expression; I was expecting to find a ')' somewhere but did not \
                         see one.",
                    ));
                }
                self.pos += 1;
                Ok(inner)
            }
            ")" => Err(fail("you have too many ')'")),
            "-o" | "-or" | "-a" | "-and" | "," => Err(fail(format!(
                "invalid expression; you have used a binary operator '{token}' with nothing \
                 before it."
            ))),
            _ if token.starts_with('-') && token.len() > 1 => {
                let node = self.predicate(&token)?;
                self.last_predicate = Some(token);
                Ok(node)
            }
            _ => {
                let mut lines = vec![format!("paths must precede expression: `{token}'")];
                // GNU hints when the stray operand exists: likely a glob the shell expanded.
                if let Some(previous) = &self.last_predicate
                    && (self.resolve)(&token).exists()
                {
                    lines.push(format!(
                        "possible unquoted pattern after predicate `{previous}'?"
                    ));
                }
                Err(Usage { lines, code: 1 })
            }
        }
    }

    fn argument(&mut self, predicate: &str) -> Result<String, Usage> {
        let value = self
            .args
            .get(self.pos)
            .cloned()
            .ok_or_else(|| fail(format!("missing argument to `{predicate}'")))?;
        self.pos += 1;
        Ok(value)
    }

    fn depth_argument(&mut self, predicate: &str) -> Result<usize, Usage> {
        let value = self.argument(predicate)?;
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(fail(format!(
                "Expected a positive decimal integer argument to {predicate}, but got {}",
                quote(&value)
            )));
        }
        value.parse().map_err(|_| {
            fail(format!(
                "Expected a positive decimal integer argument to {predicate}, but got {}",
                quote(&value)
            ))
        })
    }

    fn types(&mut self, predicate: &str) -> Result<Vec<char>, Usage> {
        let value = self.argument(predicate)?;
        if value.is_empty() {
            return Err(fail(format!(
                "Arguments to {predicate} should contain at least one letter"
            )));
        }
        let chars: Vec<char> = value.chars().collect();
        let mut kinds = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            let letter = chars[i];
            if !"bcdpflsD".contains(letter) {
                return Err(fail(format!("Unknown argument to {predicate}: {letter}")));
            }
            kinds.push(letter);
            i += 1;
            match chars.get(i) {
                None => {}
                Some(',') if i + 1 == chars.len() => {
                    return Err(fail(format!(
                        "Last file type in list argument to {predicate} is missing, i.e., list \
                         is ending on: ','"
                    )));
                }
                Some(',') => i += 1,
                Some(_) => {
                    return Err(fail(format!(
                        "Must separate multiple arguments to {predicate} using: ','"
                    )));
                }
            }
        }
        Ok(kinds)
    }

    fn size(&mut self) -> Result<Primary, Usage> {
        let value = self.argument("-size")?;
        let Some(suffix) = value.chars().last() else {
            return Err(fail("invalid null argument to -size"));
        };
        let (unit, number) = match suffix {
            'b' => (512, &value[..value.len() - 1]),
            'c' => (1, &value[..value.len() - 1]),
            'w' => (2, &value[..value.len() - 1]),
            'k' => (1024, &value[..value.len() - 1]),
            'M' => (1024 * 1024, &value[..value.len() - 1]),
            'G' => (1024 * 1024 * 1024, &value[..value.len() - 1]),
            c if c.is_ascii_digit() => (512, value.as_str()),
            c => return Err(fail(format!("invalid -size type `{c}'"))),
        };
        let (cmp, digits) = comparison(number);
        let count = digits
            .parse::<u64>()
            .ok()
            .filter(|_| digits.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(|| fail(format!("Invalid argument `{value}' to -size")))?;
        Ok(Primary::Size { cmp, count, unit })
    }

    fn age(&mut self, predicate: &str, field: TimeField, minutes: bool) -> Result<Primary, Usage> {
        let value = self.argument(predicate)?;
        let (cmp, number) = comparison(&value);
        let amount = number
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && !number.starts_with(['+', '-']))
            .ok_or_else(|| fail(format!("invalid argument `{value}' to `{predicate}'")))?;
        let origin = if self.day_start {
            start_of_tomorrow(self.now)
        } else {
            self.now
        };
        Ok(Primary::Age {
            field,
            cmp,
            amount,
            minutes,
            origin,
        })
    }

    /// Timestamp of a reference file for `-newer` and friends.
    fn reference(&mut self, predicate: &str, field: TimeField) -> Result<Ts, Usage> {
        let file = self.argument(predicate)?;
        let path = (self.resolve)(&file);
        let follow = self.follow != Follow::Never;
        let ctime = matches!(field, TimeField::Change);
        read_stat(&path, follow, ctime)
            .map(|stat| stat.time(field))
            .map_err(|error| fail(format!("{}: {}", quote(&file), super::io_message(&error))))
    }

    fn output(&mut self, predicate: &str) -> Result<usize, Usage> {
        let file = self.argument(predicate)?;
        let output = match file.as_str() {
            "/dev/stdout" => Output::Stdout,
            "/dev/stderr" => Output::Stderr,
            "/dev/null" => Output::Null,
            _ => std::fs::File::create((self.resolve)(&file))
                .map(|handle| Output::File(std::io::BufWriter::new(handle)))
                .map_err(|error| {
                    fail(format!("{}: {}", quote(&file), super::io_message(&error)))
                })?,
        };
        self.outputs.push(output);
        Ok(self.outputs.len() - 1)
    }

    fn exec(&mut self, name: &'static str, in_dir: bool) -> Result<Primary, Usage> {
        let start = self.pos;
        let mut end = None;
        for index in start..self.args.len() {
            match self.args[index].as_str() {
                ";" => {
                    end = Some((index, false));
                    break;
                }
                "+" if index > start => {
                    let previous = &self.args[index - 1];
                    if previous == "{}" {
                        end = Some((index, true));
                        break;
                    }
                    if previous.contains("{}") {
                        return Err(fail(format!(
                            "In {} the {} must appear by itself, but you specified {}",
                            quote(&format!("{name} ... {{}} +")),
                            quote("{}"),
                            quote(previous)
                        )));
                    }
                }
                _ => {}
            }
        }
        let Some((end, batch)) = end else {
            return Err(fail(format!("missing argument to `{name}'")));
        };
        if end == start {
            return Err(fail(format!(
                "invalid argument `{}' to `{name}'",
                self.args[end]
            )));
        }
        let mut words = self.args[start..end].to_vec();
        if batch {
            words.pop();
            if words.is_empty() || words.iter().any(|word| word.contains("{}")) {
                return Err(fail(format!(
                    "Only one instance of {{}} is supported with {name} ... +"
                )));
            }
        }
        self.pos = end + 1;
        Ok(Primary::Exec(Exec {
            words,
            batch,
            in_dir,
        }))
    }

    fn format(&mut self, predicate: &str) -> Result<Vec<Segment>, Usage> {
        let format = self.argument(predicate)?;
        parse_format(&format, &mut self.warnings)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one arm per GNU predicate keeps the grammar in a single table"
    )]
    fn predicate(&mut self, token: &str) -> Result<Node, Usage> {
        let primary = match token {
            // Global options: they apply everywhere and evaluate to true in place.
            "-maxdepth" => {
                self.max_depth = Some(self.depth_argument(token)?);
                Primary::True
            }
            "-mindepth" => {
                self.min_depth = self.depth_argument(token)?;
                Primary::True
            }
            "-depth" | "-d" => {
                self.depth_first = true;
                Primary::True
            }
            "-follow" => {
                self.follow = Follow::Always;
                Primary::True
            }
            "-daystart" => {
                self.day_start = true;
                Primary::True
            }
            // A single WASI filesystem has no mount points; these change nothing here.
            "-xdev"
            | "-mount"
            | "-noleaf"
            | "-ignore_readdir_race"
            | "-noignore_readdir_race"
            | "-warn"
            | "-nowarn" => Primary::True,
            "-regextype" => {
                let name = self.argument(token)?;
                self.regex_type = RegexType::parse(&name).ok_or_else(|| {
                    let valid: Vec<String> =
                        RegexType::NAMES.iter().map(|name| quote(name)).collect();
                    fail(format!(
                        "Unknown regular expression type {}; valid types are {}.",
                        quote(&name),
                        valid.join(", ")
                    ))
                })?;
                Primary::True
            }
            "-true" => Primary::True,
            "-false" => Primary::False,
            "-name" | "-iname" => Primary::Name {
                pattern: self.argument(token)?,
                ignore_case: token == "-iname",
            },
            "-path" | "-wholename" | "-ipath" | "-iwholename" => Primary::Path {
                pattern: self.argument(token)?,
                ignore_case: token.starts_with("-i"),
            },
            "-lname" | "-ilname" => Primary::LinkName {
                pattern: self.argument(token)?,
                ignore_case: token == "-ilname",
            },
            "-regex" | "-iregex" => {
                let pattern = self.argument(token)?;
                Primary::Regex(compile_regex(
                    self.regex_type,
                    &pattern,
                    token == "-iregex",
                )?)
            }
            "-type" | "-xtype" => Primary::Type {
                kinds: self.types(token)?,
                other_side: token == "-xtype",
            },
            "-empty" => Primary::Empty,
            "-size" => self.size()?,
            "-mtime" => self.age(token, TimeField::Modify, false)?,
            "-atime" => self.age(token, TimeField::Access, false)?,
            "-ctime" => self.age(token, TimeField::Change, false)?,
            "-mmin" => self.age(token, TimeField::Modify, true)?,
            "-amin" => self.age(token, TimeField::Access, true)?,
            "-cmin" => self.age(token, TimeField::Change, true)?,
            "-newer" => Primary::Newer {
                field: TimeField::Modify,
                reference: self.reference(token, TimeField::Modify)?,
            },
            "-anewer" => Primary::Newer {
                field: TimeField::Access,
                reference: self.reference(token, TimeField::Modify)?,
            },
            "-cnewer" => Primary::Newer {
                field: TimeField::Change,
                reference: self.reference(token, TimeField::Modify)?,
            },
            _ if token.len() == 8 && token.starts_with("-newer") => {
                let letters: Vec<char> = token[6..].chars().collect();
                let field = |letter: char| match letter {
                    'a' => Some(TimeField::Access),
                    'c' => Some(TimeField::Change),
                    'm' => Some(TimeField::Modify),
                    _ => None,
                };
                if letters.contains(&'B') {
                    return Err(Usage {
                        lines: vec![
                            "This system does not provide a way to find the birth time of a \
                             file."
                                .into(),
                            format!("invalid predicate `{token}'"),
                        ],
                        code: 1,
                    });
                }
                let Some(own) = field(letters[0]) else {
                    return Err(fail(format!("unknown predicate `{token}'")));
                };
                let reference = if letters[1] == 't' {
                    let text = self.argument(token)?;
                    parse_date(&text).ok_or_else(|| {
                        fail(format!(
                            "I cannot figure out how to interpret {} as a date or time",
                            quote(&text)
                        ))
                    })?
                } else {
                    let Some(theirs) = field(letters[1]) else {
                        return Err(fail(format!("unknown predicate `{token}'")));
                    };
                    self.reference(token, theirs)?
                };
                Primary::Newer {
                    field: own,
                    reference,
                }
            }
            "-prune" => {
                self.has_prune = true;
                Primary::Prune
            }
            "-quit" => Primary::Quit,
            "-print" | "-print0" => {
                self.has_action = true;
                Primary::Print {
                    zero: token == "-print0",
                    target: Target::Stdout,
                }
            }
            "-fprint" | "-fprint0" => {
                self.has_action = true;
                Primary::Print {
                    zero: token == "-fprint0",
                    target: Target::Output(self.output(token)?),
                }
            }
            "-printf" => {
                self.has_action = true;
                Primary::Printf {
                    format: self.format(token)?,
                    target: Target::Stdout,
                }
            }
            "-fprintf" => {
                self.has_action = true;
                let target = Target::Output(self.output(token)?);
                Primary::Printf {
                    format: self.format(token)?,
                    target,
                }
            }
            "-delete" => {
                self.has_action = true;
                self.has_delete = true;
                Primary::Delete
            }
            "-exec" => {
                self.has_action = true;
                self.exec("-exec", false)?
            }
            "-execdir" => {
                self.has_action = true;
                self.exec("-execdir", true)?
            }
            // WASI has no permission bits, owners, inode numbers or link counts to test, and
            // the agent has no terminal to prompt on.
            "-perm" | "-user" | "-group" | "-uid" | "-gid" | "-nouser" | "-nogroup"
            | "-readable" | "-writable" | "-executable" | "-links" | "-inum" | "-samefile"
            | "-used" | "-fstype" | "-context" | "-ok" | "-okdir" | "-ls" | "-fls" => {
                return Err(refuse(token));
            }
            _ => return Err(fail(format!("unknown predicate `{token}'"))),
        };
        Ok(self.leaf(primary))
    }
}

/// Parse the whole command line into a runnable program.
fn parse(args: &[String], resolve: &dyn Fn(&str) -> PathBuf, now: Ts) -> Result<Program, Usage> {
    let mut pos = 0;
    let mut follow = Follow::Never;
    while let Some(arg) = args.get(pos) {
        match arg.as_str() {
            "-P" => follow = Follow::Never,
            "-H" => follow = Follow::CommandLine,
            "-L" => follow = Follow::Always,
            "--" => {
                pos += 1;
                break;
            }
            "-D" => return Err(refuse("-D")),
            option if option.starts_with("-O") => {}
            _ => break,
        }
        pos += 1;
    }
    let mut starts = Vec::new();
    while let Some(arg) = args.get(pos) {
        if starts_expression(arg) {
            break;
        }
        starts.push(arg.clone());
        pos += 1;
    }
    if starts.is_empty() {
        starts.push(".".to_string());
    }
    let mut parser = Parser {
        args,
        pos,
        resolve,
        now,
        regex_type: RegexType::Emacs,
        day_start: false,
        follow,
        max_depth: None,
        min_depth: 0,
        depth_first: false,
        primaries: Vec::new(),
        outputs: Vec::new(),
        warnings: Vec::new(),
        has_action: false,
        has_prune: false,
        has_delete: false,
        last_predicate: None,
    };
    let tree = if parser.peek().is_some() {
        let tree = parser.expression()?;
        if parser.peek().is_some() {
            return Err(fail("you have too many ')'"));
        }
        Some(tree)
    } else {
        None
    };
    if parser.has_delete && parser.has_prune && !parser.depth_first {
        return Err(fail(
            "The -delete action automatically turns on -depth, but -prune does nothing when \
             -depth is in effect.  If you want to carry on anyway, just explicitly use the -depth \
             option.",
        ));
    }
    // Without an action the whole expression is `( EXPR ) -print`.
    let tree = match tree {
        Some(tree) if parser.has_action => tree,
        tree => {
            let print = parser.leaf(Primary::Print {
                zero: false,
                target: Target::Stdout,
            });
            match tree {
                Some(tree) => Node::And(Box::new(tree), Box::new(print)),
                None => print,
            }
        }
    };
    let mut code = Vec::new();
    emit(tree, &mut code);
    let needs_ctime = parser.primaries.iter().any(uses_ctime);
    Ok(Program {
        starts,
        follow: parser.follow,
        max_depth: parser.max_depth,
        min_depth: parser.min_depth,
        depth_first: parser.depth_first || parser.has_delete,
        primaries: parser.primaries,
        code,
        outputs: parser.outputs,
        warnings: parser.warnings,
        needs_ctime,
    })
}

/// Whether evaluating `primary` reads an entry's status-change time.
fn uses_ctime(primary: &Primary) -> bool {
    match primary {
        Primary::Age { field, .. } | Primary::Newer { field, .. } => {
            matches!(field, TimeField::Change)
        }
        Primary::Printf { format, .. } => format.iter().any(|segment| {
            matches!(
                segment,
                Segment::Field(
                    _,
                    Field::Stamp(TimeField::Change) | Field::Time(TimeField::Change, _)
                )
            )
        }),
        _ => false,
    }
}

// ------------------------------------------------------------------------------------------------
// -printf formats and time
// ------------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Field {
    Path,
    Base,
    Parent,
    Relative,
    Start,
    Size,
    Depth,
    Kind,
    KindFollowed,
    LinkTarget,
    Mode,
    ModeSymbolic,
    /// A time field in `ctime(3)` style with GNU's fraction (`%t`, `%a`, `%c`).
    Stamp(TimeField),
    /// A `%Tk`-style directive.
    Time(TimeField, char),
}

struct Spec {
    left: bool,
    width: usize,
    precision: Option<usize>,
}

enum Segment {
    Text(Vec<u8>),
    /// `\c`: stop this format's output.
    Stop,
    Field(Spec, Field),
}

const TIME_DIRECTIVES: &str = "@aAbBcdDeFgGhHIjklmMnprRsStTuUVwWxXyYzZ+";

/// `pad` turns a `-printf` field width or precision straight into that many bytes of padding or
/// truncation; cap it at the shell's shared in-memory limit instead of letting one
/// `%999999999999p` try to allocate an unbounded string.
const MAX_PRINTF_WIDTH: usize = crate::commands::MAX_STDIN_BYTES;

/// Parse a `-printf` format; unknown escapes and directives are GNU warnings and print literally.
fn parse_format(format: &str, warnings: &mut Vec<String>) -> Result<Vec<Segment>, Usage> {
    // The bytes the format stands for: its text is printed as those bytes.
    let bytes = &*super::shell_bytes::encode(format);
    let mut segments = Vec::new();
    let mut text = Vec::new();
    let mut i = 0;
    let at_end = || fail("error: % at end of format string");
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 1;
                let Some(&escape) = bytes.get(i) else {
                    text.push(b'\\');
                    break;
                };
                i += 1;
                match escape {
                    b'a' => text.push(7),
                    b'b' => text.push(8),
                    b'f' => text.push(12),
                    b'n' => text.push(b'\n'),
                    b'r' => text.push(b'\r'),
                    b't' => text.push(b'\t'),
                    b'v' => text.push(11),
                    b'\\' => text.push(b'\\'),
                    b'c' => {
                        segments.push(Segment::Text(std::mem::take(&mut text)));
                        segments.push(Segment::Stop);
                    }
                    b'0'..=b'7' => {
                        let mut value = u32::from(escape - b'0');
                        for _ in 0..2 {
                            match bytes.get(i) {
                                Some(&digit @ b'0'..=b'7') => {
                                    value = value * 8 + u32::from(digit - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        text.push(u8::try_from(value & 0xff).unwrap_or(0));
                    }
                    other => {
                        warnings.push(format!(
                            "warning: unrecognized escape `\\{}'",
                            char::from(other)
                        ));
                        text.push(b'\\');
                        text.push(other);
                    }
                }
            }
            b'%' => {
                let begin = i;
                i += 1;
                if bytes.get(i) == Some(&b'%') {
                    text.push(b'%');
                    i += 1;
                    continue;
                }
                let mut spec = Spec {
                    left: false,
                    width: 0,
                    precision: None,
                };
                while let Some(&flag @ (b'-' | b'+' | b' ' | b'#' | b'0')) = bytes.get(i) {
                    spec.left |= flag == b'-';
                    i += 1;
                }
                while let Some(&digit @ b'0'..=b'9') = bytes.get(i) {
                    // Saturate instead of wrapping: `pad` turns this into that many bytes of
                    // padding, so an overflowed (and possibly tiny, or on a 32-bit usize even
                    // negative-looking-when-cast) width from a literal like `%999999999999p`
                    // would silently do the wrong thing instead of erroring below.
                    spec.width = spec
                        .width
                        .saturating_mul(10)
                        .saturating_add(usize::from(digit - b'0'));
                    i += 1;
                }
                if spec.width > MAX_PRINTF_WIDTH {
                    return Err(refuse("-printf with a field width this large"));
                }
                if bytes.get(i) == Some(&b'.') {
                    i += 1;
                    let mut precision: usize = 0;
                    while let Some(&digit @ b'0'..=b'9') = bytes.get(i) {
                        precision = precision
                            .saturating_mul(10)
                            .saturating_add(usize::from(digit - b'0'));
                        i += 1;
                    }
                    if precision > MAX_PRINTF_WIDTH {
                        return Err(refuse("-printf with a field precision this large"));
                    }
                    spec.precision = Some(precision);
                }
                let Some(&directive) = bytes.get(i) else {
                    return Err(at_end());
                };
                i += 1;
                let field = match directive {
                    b'p' => Field::Path,
                    b'f' => Field::Base,
                    b'h' => Field::Parent,
                    b'P' => Field::Relative,
                    b'H' => Field::Start,
                    b's' => Field::Size,
                    b'd' => Field::Depth,
                    b'y' => Field::Kind,
                    b'Y' => Field::KindFollowed,
                    b'l' => Field::LinkTarget,
                    b'm' => Field::Mode,
                    b'M' => Field::ModeSymbolic,
                    b't' => Field::Stamp(TimeField::Modify),
                    b'a' => Field::Stamp(TimeField::Access),
                    b'c' => Field::Stamp(TimeField::Change),
                    b'T' | b'A' | b'C' => {
                        let Some(&which) = bytes.get(i) else {
                            return Err(at_end());
                        };
                        i += 1;
                        let field = match directive {
                            b'T' => TimeField::Modify,
                            b'A' => TimeField::Access,
                            _ => TimeField::Change,
                        };
                        if which.is_ascii() && TIME_DIRECTIVES.contains(char::from(which)) {
                            Field::Time(field, char::from(which))
                        } else {
                            let raw = String::from_utf8_lossy(&bytes[begin..i]).into_owned();
                            warnings
                                .push(format!("warning: unrecognized format directive `{raw}'"));
                            text.extend_from_slice(raw.as_bytes());
                            continue;
                        }
                    }
                    // Owners, inode numbers, link counts, block counts and devices are not part of
                    // WASI file metadata.
                    b'u' | b'g' | b'U' | b'G' | b'i' | b'n' | b'k' | b'b' | b'D' | b'F' | b'S'
                    | b'Z' => {
                        return Err(refuse(&format!("-printf %{}", char::from(directive))));
                    }
                    _ => {
                        let raw = String::from_utf8_lossy(&bytes[begin..i]).into_owned();
                        warnings.push(format!("warning: unrecognized format directive `{raw}'"));
                        text.extend_from_slice(raw.as_bytes());
                        continue;
                    }
                };
                segments.push(Segment::Text(std::mem::take(&mut text)));
                segments.push(Segment::Field(spec, field));
            }
            byte => {
                text.push(byte);
                i += 1;
            }
        }
    }
    segments.push(Segment::Text(text));
    Ok(segments)
}

fn pad(value: &str, spec: &Spec) -> String {
    let value: String = match spec.precision {
        Some(precision) => value.chars().take(precision).collect(),
        None => value.to_string(),
    };
    let length = value.chars().count();
    if length >= spec.width {
        value
    } else if spec.left {
        format!("{value}{}", " ".repeat(spec.width - length))
    } else {
        format!("{}{value}", " ".repeat(spec.width - length))
    }
}

type LocalTime = chrono::DateTime<chrono::FixedOffset>;

fn local_time(ns: Ts) -> LocalTime {
    use chrono::TimeZone;
    let seconds = i64::try_from(ns.div_euclid(NANOS)).unwrap_or(0);
    let nanos = u32::try_from(ns.rem_euclid(NANOS)).unwrap_or(0);
    chrono::Local
        .timestamp_opt(seconds, nanos)
        .single()
        .map(|time| time.fixed_offset())
        .or_else(|| {
            chrono::Utc
                .timestamp_opt(seconds, nanos)
                .single()
                .map(|time| time.fixed_offset())
        })
        .unwrap_or_default()
}

/// Midnight at the end of the current local day, GNU's `-daystart` origin.
fn start_of_tomorrow(now: Ts) -> Ts {
    use chrono::Timelike;
    let time = local_time(now);
    let elapsed = i128::from(time.num_seconds_from_midnight()) * NANOS
        + i128::from(time.nanosecond() % 1_000_000_000);
    now - elapsed + 86_400 * NANOS
}

/// GNU's date grammar for `-newerXt`, through uutils' `parse_datetime`: ISO forms, `@EPOCH`,
/// `Jun 1 2020`, `20200601`, zone offsets, and relative dates (`yesterday`, `2 days ago`,
/// `next monday`, `now`), read in the shell's local time.
fn parse_date(text: &str) -> Option<Ts> {
    match parse_datetime::parse_datetime(text.trim()).ok()? {
        parse_datetime::ParsedDateTime::InRange(zoned) => Some(zoned.timestamp().as_nanosecond()),
        // Years outside the range a timestamp can hold.
        parse_datetime::ParsedDateTime::Extended(_) => None,
    }
}

const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Render one time directive as GNU find does in the C locale.
fn format_time(ns: Ts, directive: char) -> String {
    use chrono::{Datelike, Timelike};
    let fraction = format!("{:09}0", ns.rem_euclid(NANOS));
    let seconds = ns.div_euclid(NANOS);
    if directive == '@' {
        return format!("{seconds}.{fraction}");
    }
    let time = local_time(ns);
    let weekday = time.weekday().num_days_from_sunday() as usize;
    let month = time.month0() as usize;
    let hour12 = match time.hour() % 12 {
        0 => 12,
        hour => hour,
    };
    let meridiem = if time.hour() < 12 { "AM" } else { "PM" };
    let yday = time.ordinal0();
    let offset = time.offset().local_minus_utc();
    let zone = format!(
        "{}{:02}{:02}",
        if offset < 0 { '-' } else { '+' },
        offset.abs() / 3600,
        offset.abs() % 3600 / 60
    );
    match directive {
        'a' => WEEKDAYS[weekday][..3].to_string(),
        'A' => WEEKDAYS[weekday].to_string(),
        'b' | 'h' => MONTHS[month][..3].to_string(),
        'B' => MONTHS[month].to_string(),
        'c' => format!(
            "{} {} {:>2} {:02}:{:02}:{:02} {}",
            &WEEKDAYS[weekday][..3],
            &MONTHS[month][..3],
            time.day(),
            time.hour(),
            time.minute(),
            time.second(),
            time.year()
        ),
        'd' => format!("{:02}", time.day()),
        'D' | 'x' => format!(
            "{:02}/{:02}/{:02}",
            time.month(),
            time.day(),
            time.year().rem_euclid(100)
        ),
        'e' => format!("{:>2}", time.day()),
        'F' => format!("{}-{:02}-{:02}", time.year(), time.month(), time.day()),
        'g' => format!("{:02}", time.iso_week().year().rem_euclid(100)),
        'G' => time.iso_week().year().to_string(),
        'H' => format!("{:02}", time.hour()),
        'I' => format!("{hour12:02}"),
        'j' => format!("{:03}", yday + 1),
        'k' => format!("{:>2}", time.hour()),
        'l' => format!("{hour12:>2}"),
        'm' => format!("{:02}", time.month()),
        'M' => format!("{:02}", time.minute()),
        'n' => "\n".to_string(),
        'p' => meridiem.to_string(),
        'r' => format!(
            "{hour12:02}:{:02}:{:02} {meridiem}",
            time.minute(),
            time.second()
        ),
        'R' => format!("{:02}:{:02}", time.hour(), time.minute()),
        's' => seconds.to_string(),
        'S' => format!("{:02}.{fraction}", time.second()),
        't' => "\t".to_string(),
        'T' | 'X' => format!(
            "{:02}:{:02}:{:02}.{fraction}",
            time.hour(),
            time.minute(),
            time.second()
        ),
        'u' => time.weekday().number_from_monday().to_string(),
        'U' => format!(
            "{:02}",
            (yday + 7 - time.weekday().num_days_from_sunday()) / 7
        ),
        'V' => format!("{:02}", time.iso_week().week()),
        'w' => time.weekday().num_days_from_sunday().to_string(),
        'W' => format!(
            "{:02}",
            (yday + 7 - time.weekday().num_days_from_monday()) / 7
        ),
        'y' => format!("{:02}", time.year().rem_euclid(100)),
        'Y' => time.year().to_string(),
        'z' => zone,
        'Z' => {
            if offset == 0 {
                "UTC".to_string()
            } else {
                zone
            }
        }
        '+' => format!(
            "{}-{:02}-{:02}+{:02}:{:02}:{:02}.{fraction}",
            time.year(),
            time.month(),
            time.day(),
            time.hour(),
            time.minute(),
            time.second()
        ),
        // `%t`/`%a`/`%c`: ctime(3) with GNU's ten-digit fraction.
        _ => format!(
            "{} {} {:>2} {:02}:{:02}:{:02}.{fraction} {}",
            &WEEKDAYS[weekday][..3],
            &MONTHS[month][..3],
            time.day(),
            time.hour(),
            time.minute(),
            time.second(),
            time.year()
        ),
    }
}

/// `ls -l`-style permission string.
fn symbolic_mode(kind: Kind, mode: u32) -> String {
    let mut out = String::with_capacity(10);
    out.push(match kind {
        Kind::File | Kind::Unknown => '-',
        other => other.letter(),
    });
    let bits = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    for (bit, letter) in bits {
        out.push(if mode & bit != 0 { letter } else { '-' });
    }
    out
}

/// gnulib `last_component`: the final name, keeping trailing slashes; all slashes give `/`.
fn last_component(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { "" } else { "/" };
    }
    let start = trimmed.rfind('/').map_or(0, |index| index + 1);
    &path[start..]
}

/// The name `-name` matches: the last component without trailing slashes.
fn match_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { "" } else { "/" };
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

// ------------------------------------------------------------------------------------------------
// Traversal and evaluation
// ------------------------------------------------------------------------------------------------

struct Entry {
    display: String,
    fs: PathBuf,
    depth: usize,
    start: usize,
    stat: Stat,
    /// Whether `stat` followed a symbolic link at this entry.
    followed: bool,
}

struct Frame {
    entry: Entry,
    names: Vec<std::ffi::OsString>,
    next: usize,
    identity: Option<(u64, u64)>,
}

/// Pending `-exec ... {} +` arguments for one predicate.
struct Batch {
    args: Vec<String>,
    bytes: usize,
    dir: Option<PathBuf>,
}

enum Launch {
    Exited(u8),
    /// Not started or killed by a signal; already diagnosed.
    Failed,
}

fn child_display(parent: &str, name: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn read_stat(path: &Path, follow: bool, ctime: bool) -> std::io::Result<Stat> {
    // A dangling link under -L/-H is reported as the link itself, as GNU does.
    Stat::read(path, follow, ctime).or_else(|error| {
        if follow {
            Stat::read(path, false, ctime)
        } else {
            Err(error)
        }
    })
}

fn compare(value: u64, cmp: Cmp, against: u64) -> bool {
    match cmp {
        Cmp::Less => value < against,
        Cmp::Equal => value == against,
        Cmp::Greater => value > against,
    }
}

#[allow(
    clippy::cast_precision_loss,
    reason = "ages are compared at sub-second precision, far inside f64's range"
)]
fn age_matches(origin: Ts, time: Ts, cmp: Cmp, amount: f64, minutes: bool) -> bool {
    // GNU find compares `-mtime`/`-mmin` windows using each timestamp's whole-second value;
    // it never looks at either one's sub-second part. Taking the nanosecond-precise
    // difference of `origin` and `time` (as this used to) instead of truncating each one
    // first means a file written by `touch -d 'N days ago'` and then matched by a script a
    // few milliseconds later lands a hair on the wrong side of the exact N*86400 boundary,
    // where GNU (having thrown away the sub-second parts of both timestamps before
    // subtracting) still sees it exactly on the boundary.
    let origin_secs = origin.div_euclid(NANOS);
    let time_secs = time.div_euclid(NANOS);
    let age = (origin_secs - time_secs) as f64;
    if minutes {
        // GNU: `-mmin N` is the window (N-1, N] minutes ago.
        match cmp {
            Cmp::Less => age < amount * 60.0,
            Cmp::Equal => age > (amount - 1.0) * 60.0 && age <= amount * 60.0,
            Cmp::Greater => age > amount * 60.0,
        }
    } else {
        // GNU: `-mtime N` counts whole days, ignoring the fractional part.
        let day = 86_400.0;
        match cmp {
            Cmp::Less => age < amount * day,
            Cmp::Equal => age >= amount * day && age < (amount + 1.0) * day,
            Cmp::Greater => age >= (amount + 1.0) * day,
        }
    }
}

struct Run<'a, SE: ShellExtensions> {
    context: ExecutionContext<'a, SE>,
    stdout: OpenFile,
    stderr: OpenFile,
    out: Vec<u8>,
    outputs: Vec<Output>,
    batches: Vec<Option<Batch>>,
    status: u8,
    quit: bool,
    pruned: bool,
}

impl<SE: ShellExtensions> Run<'_, SE> {
    async fn flush(&mut self) -> Result<(), Error> {
        if !self.out.is_empty() {
            self.stdout.async_io().write_all(&self.out).await?;
            self.out.clear();
        }
        Ok(())
    }

    async fn diagnose(&mut self, message: &str) -> Result<(), Error> {
        self.flush().await?;
        self.stderr
            .async_io()
            .write_all(format!("find: {message}\n").as_bytes())
            .await?;
        Ok(())
    }

    async fn write(&mut self, target: &Target, bytes: &[u8]) -> Result<(), Error> {
        let output = match target {
            Target::Stdout => None,
            Target::Output(index) => Some(*index),
        };
        match output.map(|index| &mut self.outputs[index]) {
            None | Some(Output::Stdout) => {
                self.out.extend_from_slice(bytes);
                if self.out.len() >= 8192 {
                    self.flush().await?;
                }
            }
            Some(Output::Stderr) => {
                self.flush().await?;
                self.stderr.async_io().write_all(bytes).await?;
            }
            Some(Output::Null) => {}
            Some(Output::File(file)) => {
                std::io::Write::write_all(file, bytes)?;
            }
        }
        Ok(())
    }

    async fn walk(&mut self, program: &Program, start: usize) -> Result<(), Error> {
        let display = program.starts[start].clone();
        let fs = self.context.shell.absolute_path(Path::new(&display));
        let followed = program.follow != Follow::Never;
        let stat = if display.is_empty() {
            Err("No such file or directory".to_string())
        } else {
            read_stat(&fs, followed, program.needs_ctime).map_err(|error| super::io_message(&error))
        };
        let stat = match stat {
            Ok(stat) => stat,
            Err(error) => {
                let message = format!("{}: {error}", quote(&display));
                self.diagnose(&message).await?;
                self.status = 1;
                return Ok(());
            }
        };
        let entry = Entry {
            display,
            fs,
            depth: 0,
            start,
            followed: followed && stat.kind != Kind::Link,
            stat,
        };
        let mut stack = Vec::new();
        self.visit(program, entry, &mut stack).await?;
        let follow = program.follow == Follow::Always;
        while !self.quit {
            let Some(frame) = stack.last_mut() else {
                break;
            };
            if let Some(name) = frame.names.get(frame.next).cloned() {
                frame.next += 1;
                let display = child_display(&frame.entry.display, &name.to_string_lossy());
                let fs = frame.entry.fs.join(&name);
                let depth = frame.entry.depth + 1;
                match read_stat(&fs, follow, program.needs_ctime) {
                    Ok(stat) => {
                        let entry = Entry {
                            display,
                            fs,
                            depth,
                            start,
                            followed: follow && stat.kind != Kind::Link,
                            stat,
                        };
                        self.visit(program, entry, &mut stack).await?;
                    }
                    Err(error) => {
                        let message = format!("{}: {}", quote(&display), super::io_message(&error));
                        self.diagnose(&message).await?;
                        self.status = 1;
                    }
                }
            } else if let Some(frame) = stack.pop()
                && program.depth_first
                && frame.entry.depth >= program.min_depth
            {
                self.evaluate(program, &frame.entry).await?;
            }
        }
        Ok(())
    }

    async fn visit(
        &mut self,
        program: &Program,
        entry: Entry,
        stack: &mut Vec<Frame>,
    ) -> Result<(), Error> {
        let descend =
            entry.stat.kind == Kind::Dir && program.max_depth.is_none_or(|max| entry.depth < max);
        let mut identity = None;
        if descend && entry.followed {
            identity = platform::identity(&entry.fs);
            if let Some(ancestor) = stack
                .iter()
                .find(|frame| identity.is_some() && frame.identity == identity)
            {
                let message = format!(
                    "File system loop detected; {} is part of the same file system loop as {}.",
                    quote(&entry.display),
                    quote(&ancestor.entry.display)
                );
                self.diagnose(&message).await?;
                self.status = 1;
                return Ok(());
            }
        }
        if !program.depth_first {
            if entry.depth >= program.min_depth {
                self.pruned = false;
                self.evaluate(program, &entry).await?;
            }
            let pruned = std::mem::take(&mut self.pruned);
            if self.quit || !descend || pruned {
                return Ok(());
            }
            self.open(program, entry, identity, stack).await
        } else if descend {
            self.open(program, entry, identity, stack).await
        } else if entry.depth >= program.min_depth {
            self.evaluate(program, &entry).await
        } else {
            Ok(())
        }
    }

    async fn open(
        &mut self,
        program: &Program,
        entry: Entry,
        identity: Option<(u64, u64)>,
        stack: &mut Vec<Frame>,
    ) -> Result<(), Error> {
        match std::fs::read_dir(&entry.fs) {
            Ok(reader) => {
                let mut names: Vec<std::ffi::OsString> = reader
                    .filter_map(|item| item.ok().map(|item| item.file_name()))
                    .collect();
                // GNU walks in readdir order; sorting keeps our output deterministic.
                names.sort_by(|a, b| a.as_encoded_bytes().cmp(b.as_encoded_bytes()));
                stack.push(Frame {
                    entry,
                    names,
                    next: 0,
                    identity,
                });
            }
            Err(error) => {
                let message = format!("{}: {}", quote(&entry.display), super::io_message(&error));
                self.diagnose(&message).await?;
                self.status = 1;
                if program.depth_first && entry.depth >= program.min_depth {
                    self.evaluate(program, &entry).await?;
                }
            }
        }
        Ok(())
    }

    async fn evaluate(&mut self, program: &Program, entry: &Entry) -> Result<(), Error> {
        let mut value = true;
        let mut pc = 0;
        while pc < program.code.len() && !self.quit {
            match program.code[pc] {
                Op::Eval(index) => value = self.primary(program, index, entry).await?,
                Op::Not => value = !value,
                Op::JumpIfFalse(target) if !value => {
                    pc = target;
                    continue;
                }
                Op::JumpIfTrue(target) if value => {
                    pc = target;
                    continue;
                }
                Op::JumpIfFalse(_) | Op::JumpIfTrue(_) => {}
            }
            pc += 1;
        }
        Ok(())
    }

    async fn primary(
        &mut self,
        program: &Program,
        index: usize,
        entry: &Entry,
    ) -> Result<bool, Error> {
        Ok(match &program.primaries[index] {
            Primary::True => true,
            Primary::False => false,
            Primary::Name {
                pattern,
                ignore_case,
            } => fnmatch(pattern, match_name(&entry.display), *ignore_case),
            Primary::Path {
                pattern,
                ignore_case,
            } => fnmatch(pattern, &entry.display, *ignore_case),
            Primary::LinkName {
                pattern,
                ignore_case,
            } => {
                entry.stat.kind == Kind::Link
                    && std::fs::read_link(&entry.fs).is_ok_and(|target| {
                        fnmatch(pattern, &target.to_string_lossy(), *ignore_case)
                    })
            }
            Primary::Regex(regex) => regex.is_match(&entry.display),
            Primary::Type { kinds, other_side } => {
                let kind = if *other_side {
                    // -xtype: the type as seen with the opposite link policy.
                    let follow = !entry.followed && entry.stat.kind == Kind::Link;
                    if follow || entry.followed {
                        read_stat(&entry.fs, follow, false).map_or(Kind::Link, |stat| stat.kind)
                    } else {
                        entry.stat.kind
                    }
                } else {
                    entry.stat.kind
                };
                kinds.contains(&kind.letter())
            }
            Primary::Empty => match entry.stat.kind {
                Kind::File => entry.stat.size == 0,
                Kind::Dir => {
                    std::fs::read_dir(&entry.fs).is_ok_and(|mut reader| reader.next().is_none())
                }
                _ => false,
            },
            Primary::Size { cmp, count, unit } => {
                compare(entry.stat.size.div_ceil(*unit), *cmp, *count)
            }
            Primary::Age {
                field,
                cmp,
                amount,
                minutes,
                origin,
            } => age_matches(*origin, entry.stat.time(*field), *cmp, *amount, *minutes),
            Primary::Newer { field, reference } => entry.stat.time(*field) > *reference,
            Primary::Prune => {
                // With -depth the children were already visited, so -prune changes nothing.
                self.pruned = !program.depth_first;
                true
            }
            Primary::Quit => {
                self.quit = true;
                true
            }
            Primary::Print { zero, target } => {
                let mut bytes = entry.display.as_bytes().to_vec();
                bytes.push(if *zero { 0 } else { b'\n' });
                self.write(target, &bytes).await?;
                true
            }
            Primary::Printf { format, target } => {
                let bytes = render(format, entry, &program.starts[entry.start]);
                self.write(target, &bytes).await?;
                true
            }
            Primary::Delete => self.delete(entry).await?,
            Primary::Exec(exec) => self.exec(program, index, exec, entry).await?,
        })
    }

    async fn delete(&mut self, entry: &Entry) -> Result<bool, Error> {
        // GNU never removes the starting point `.`.
        if entry.display == "." {
            return Ok(true);
        }
        let is_dir = std::fs::symlink_metadata(&entry.fs).is_ok_and(|meta| meta.is_dir());
        let removed = if is_dir {
            std::fs::remove_dir(&entry.fs)
        } else {
            std::fs::remove_file(&entry.fs)
        };
        match removed {
            Ok(()) => Ok(true),
            Err(error) => {
                let message = format!(
                    "cannot delete {}: {}",
                    quote(&entry.display),
                    super::io_message(&error)
                );
                self.diagnose(&message).await?;
                self.status = 1;
                Ok(false)
            }
        }
    }

    async fn exec(
        &mut self,
        program: &Program,
        index: usize,
        exec: &Exec,
        entry: &Entry,
    ) -> Result<bool, Error> {
        let (argument, dir) = if exec.in_dir {
            let dir = entry
                .fs
                .parent()
                .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
            (format!("./{}", match_name(&entry.display)), Some(dir))
        } else {
            (entry.display.clone(), None)
        };
        if exec.batch {
            let full = self.batches[index].as_ref().is_some_and(|batch| {
                batch.dir != dir || batch.bytes + argument.len() + 1 > BATCH_BYTES
            });
            if full {
                self.run_batch(program, index).await?;
            }
            let batch = self.batches[index].get_or_insert_with(|| Batch {
                args: Vec::new(),
                bytes: 0,
                dir,
            });
            batch.bytes += argument.len() + 1;
            batch.args.push(argument);
            return Ok(true);
        }
        let words: Vec<String> = exec
            .words
            .iter()
            .map(|word| word.replace("{}", &argument))
            .collect();
        Ok(matches!(
            self.launch(&words, dir.as_deref()).await?,
            Launch::Exited(0)
        ))
    }

    async fn run_batch(&mut self, program: &Program, index: usize) -> Result<(), Error> {
        let Some(batch) = self.batches[index].take() else {
            return Ok(());
        };
        let Primary::Exec(exec) = &program.primaries[index] else {
            return Ok(());
        };
        let mut words = exec.words.clone();
        words.extend(batch.args);
        if !matches!(
            self.launch(&words, batch.dir.as_deref()).await?,
            Launch::Exited(0)
        ) {
            self.status = 1;
        }
        Ok(())
    }

    async fn launch(&mut self, words: &[String], dir: Option<&Path>) -> Result<Launch, Error> {
        self.flush().await?;
        let cwd = dir.map_or_else(
            || self.context.shell.working_dir().to_path_buf(),
            Path::to_path_buf,
        );
        match super::xargs::lookup(self.context.shell, &words[0], &cwd) {
            super::xargs::Lookup::Found => {}
            super::xargs::Lookup::Missing => {
                let message = format!("{}: No such file or directory", quote(&words[0]));
                self.diagnose(&message).await?;
                return Ok(Launch::Failed);
            }
            super::xargs::Lookup::Denied => {
                let message = format!("{}: Permission denied", quote(&words[0]));
                self.diagnose(&message).await?;
                return Ok(Launch::Failed);
            }
        }
        let line = super::xargs::command_line(words);
        let result = super::xargs::run_child(
            &mut *self.context.shell,
            &self.context.params,
            None,
            line,
            dir,
            None,
            false,
        )
        .await?;
        if let Some(signal) = result.terminating_signal {
            let message = format!("{} terminated by signal {signal}", quote(&words[0]));
            self.diagnose(&message).await?;
            return Ok(Launch::Failed);
        }
        Ok(Launch::Exited(u8::from(result.exit_code)))
    }

    async fn finish(&mut self, program: &Program) -> Result<(), Error> {
        for index in 0..self.batches.len() {
            self.run_batch(program, index).await?;
        }
        for output in &mut self.outputs {
            if let Output::File(file) = output {
                std::io::Write::flush(file)?;
            }
        }
        self.flush().await
    }
}

/// Expand a `-printf` format for one entry.
fn render(format: &[Segment], entry: &Entry, start: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for segment in format {
        let (spec, field) = match segment {
            Segment::Text(text) => {
                out.extend_from_slice(text);
                continue;
            }
            Segment::Stop => break,
            Segment::Field(spec, field) => (spec, field),
        };
        let stat = &entry.stat;
        let value = match *field {
            Field::Path => entry.display.clone(),
            Field::Base => last_component(&entry.display).to_string(),
            Field::Parent => entry.display.rfind('/').map_or_else(
                || ".".to_string(),
                |index| entry.display[..index].to_string(),
            ),
            Field::Relative => {
                if entry.depth == 0 {
                    String::new()
                } else {
                    let rest = entry.display.get(start.len()..).unwrap_or_default();
                    rest.strip_prefix('/').unwrap_or(rest).to_string()
                }
            }
            Field::Start => start.to_string(),
            Field::Size => stat.size.to_string(),
            Field::Depth => entry.depth.to_string(),
            Field::Kind => stat.kind.letter().to_string(),
            Field::KindFollowed => {
                if stat.kind == Kind::Link {
                    match Stat::read(&entry.fs, true, false) {
                        Ok(target) => target.kind.letter().to_string(),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "N".into(),
                        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => "L".into(),
                        Err(_) => "?".into(),
                    }
                } else {
                    stat.kind.letter().to_string()
                }
            }
            Field::LinkTarget => std::fs::read_link(&entry.fs)
                .map(|target| target.to_string_lossy().into_owned())
                .unwrap_or_default(),
            // WASI carries no permission bits: `-` marks the value as unavailable.
            Field::Mode => stat
                .mode
                .map_or_else(|| "-".to_string(), |mode| format!("{:o}", mode & 0o7777)),
            Field::ModeSymbolic => stat
                .mode
                .map_or_else(|| "-".to_string(), |mode| symbolic_mode(stat.kind, mode)),
            Field::Stamp(time) => format_time(stat.time(time), '\0'),
            Field::Time(time, directive) => format_time(stat.time(time), directive),
        };
        out.extend_from_slice(pad(&value, spec).as_bytes());
    }
    out
}

// ------------------------------------------------------------------------------------------------
// The builtin
// ------------------------------------------------------------------------------------------------

const HELP: &str = "\
Usage: find [-H] [-L] [-P] [starting-point...] [expression]

Operators (by precedence): ( EXPR )  ! EXPR  -not EXPR  EXPR1 -a EXPR2  EXPR1 EXPR2
  EXPR1 -o EXPR2  EXPR1 , EXPR2
Options: -maxdepth N -mindepth N -depth -d -follow -daystart -regextype TYPE -xdev -mount
  -noleaf -ignore_readdir_race -warn -nowarn
Tests (N can be +N, -N or N): -name GLOB -iname GLOB -path GLOB -ipath GLOB -wholename GLOB
  -lname GLOB -ilname GLOB -regex RE -iregex RE -type [fdlbcps] -xtype [fdlbcps] -empty
  -size N[bcwkMG] -mtime N -atime N -ctime N -mmin N -amin N -cmin N -newer FILE -anewer FILE
  -cnewer FILE -newerXY REF -true -false
Actions: -print -print0 -printf FORMAT -fprint FILE -fprint0 FILE -fprintf FILE FORMAT -delete
  -prune -quit -exec COMMAND ; -exec COMMAND {} + -execdir COMMAND ; -execdir COMMAND {} +

-exec runs COMMAND through the shell as a child command; its exit status is the test's value.
Siblings are visited in sorted order. Not supported (no WASI metadata or terminal): -perm -user
-group -uid -gid -nouser -nogroup -readable -writable -executable -links -inum -samefile -used
-fstype -context -ls -fls -ok -okdir, and -printf %u %g %U %G %i %n %k %b %D %F %S %Z.
";

fn content(name: &str, content_type: ContentType, _: &ContentOptions) -> Result<String, Error> {
    match content_type {
        ContentType::ShortDescription => Ok(format!("{name} - {SYNOPSIS}\n")),
        ContentType::ShortUsage => Ok(format!(
            "{name}: {name} [-H] [-L] [-P] [starting-point...] [expression]\n"
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
        if args.first().map(String::as_str) == Some("--help") {
            context
                .stdout()
                .async_io()
                .write_all(HELP.as_bytes())
                .await?;
            return Ok(ExecutionResult::success());
        }
        let now = timestamp(Ok(SystemTime::now())).unwrap_or(0);
        let parsed = {
            let shell = &*context.shell;
            parse(&args, &|path| shell.absolute_path(Path::new(path)), now)
        };
        let mut program = match parsed {
            Ok(program) => program,
            Err(usage) => {
                let mut text = String::new();
                for line in &usage.lines {
                    text.push_str("find: ");
                    text.push_str(line);
                    text.push('\n');
                }
                stderr.async_io().write_all(text.as_bytes()).await?;
                return Ok(ExecutionResult::new(usage.code));
            }
        };
        let outputs = std::mem::take(&mut program.outputs);
        let mut run = Run {
            stdout: context.stdout(),
            stderr,
            context,
            out: Vec::new(),
            outputs,
            batches: program.primaries.iter().map(|_| None).collect(),
            status: 0,
            quit: false,
            pruned: false,
        };
        for warning in &program.warnings {
            run.diagnose(warning).await?;
        }
        for start in 0..program.starts.len() {
            if run.quit {
                break;
            }
            run.walk(&program, start).await?;
        }
        run.finish(&program).await?;
        Ok(ExecutionResult::new(run.status))
    })
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
    vec![(NAME.into(), registration)]
}

pub(crate) fn manifests() -> Vec<Manifest> {
    vec![Manifest::builtin(NAME, SYNOPSIS).with_help(
        "find [-H|-L|-P] [PATH...] [EXPRESSION] — GNU find: operators ( ) ! -a -o , with POSIX \
         precedence; tests -name/-iname/-path/-ipath/-regex/-iregex (emacs default, \
         -regextype posix-extended), -type/-xtype, -empty, -size, -mtime/-mmin/-atime/-amin/\
         -ctime/-cmin, -newer/-newerXY; options -maxdepth/-mindepth/-depth; actions -print, \
         -print0, -printf, -fprint, -delete, -prune, -quit, -exec/-execdir with ; or {} + (run \
         through the shell). Siblings are listed in sorted order. Refused (no WASI metadata): \
         -perm, -user, -group, -links, -readable/-writable/-executable, -ls, -ok.",
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Program, Usage> {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        parse(&args, &|path| PathBuf::from("/").join(path), 0)
    }

    fn message(args: &[&str]) -> String {
        match parse_args(args) {
            Ok(_) => String::new(),
            Err(usage) => usage.lines.join("\n"),
        }
    }

    #[test]
    fn fnmatch_follows_gnu_glob_rules() {
        assert!(fnmatch("*.txt", "a.txt", false));
        assert!(!fnmatch("*.txt", "a.txt.bak", false));
        assert!(fnmatch("*", ".hidden", false));
        assert!(fnmatch("f?le", "file", false));
        assert!(!fnmatch("f?le", "fiile", false));
        assert!(fnmatch("[ab]x", "ax", false));
        assert!(!fnmatch("[!ab]x", "ax", false));
        assert!(fnmatch("[^ab]x", "cx", false));
        assert!(fnmatch("[[:digit:]]*", "1abc", false));
        assert!(fnmatch("[a-c]", "b", false));
        assert!(fnmatch("[]]", "]", false));
        assert!(fnmatch("\\*", "*", false));
        assert!(!fnmatch("\\*", "a", false));
        assert!(fnmatch("a[", "a[", false));
        assert!(fnmatch("*.TXT", "a.txt", true));
        assert!(fnmatch("*/b/*", "a/b/c", false));
        assert!(fnmatch("a*b*c", "aXbYbZc", false));
    }

    #[test]
    fn emacs_regex_translation() {
        let full = |pattern: &str| format!("^(?:{})$", emacs_regex(pattern).unwrap());
        let matches = |pattern: &str, text: &str| {
            regex::Regex::new(&format!("(?s){}", full(pattern)))
                .unwrap()
                .is_match(text)
        };
        assert!(matches(r"\./a+", "./aaa"));
        assert!(matches(r"\./a\+", "./a+"));
        assert!(matches(r"\./a{2}", "./a{2}"));
        assert!(matches(r"\./\(a\|b\)1*", "./a1"));
        assert!(matches(r"\./a|b", "./a|b"));
        assert!(matches(r"\./a[\]b", "./a\\b"));
        assert!(matches(r"*a1", "*a1"));
        assert!(!matches(r"\./a[[:digit:]]", "./a1"));
        assert_eq!(emacs_regex(r"a\(b"), Err("Unmatched ( or \\("));
        assert_eq!(emacs_regex("a["), Err("Invalid regular expression"));
        assert_eq!(emacs_regex("a\\"), Err("Trailing backslash"));
    }

    #[test]
    fn expression_precedence_and_defaults() {
        let program = parse_args(&["a", "b", "-name", "x", "-o", "-name", "y"]).unwrap();
        assert_eq!(program.starts, vec!["a", "b"]);
        // ( -name x -o -name y ) -print: the implicit -print is AND-ed around the whole OR.
        assert!(matches!(program.code.last(), Some(Op::Eval(_))));
        let program = parse_args(&[]).unwrap();
        assert_eq!(program.starts, vec!["."]);
        let program = parse_args(&["-L", "-name", "x", "-delete"]).unwrap();
        assert!(program.follow == Follow::Always && program.depth_first);
    }

    #[test]
    fn gnu_diagnostics() {
        assert_eq!(message(&["t", "-foo"]), "unknown predicate `-foo'");
        assert_eq!(
            message(&["-type", "f", "t"]),
            "paths must precede expression: `t'"
        );
        let args: Vec<String> = ["-print", "tmp"]
            .iter()
            .map(|arg| (*arg).to_string())
            .collect();
        // Any existing directory: WASM has no temp_dir(), and its tests run with /tmp preopened.
        let hinted = parse(&args, &|_| std::path::PathBuf::from("/tmp"), 0)
            .err()
            .map(|usage| usage.lines);
        assert_eq!(
            hinted,
            Some(vec![
                "paths must precede expression: `tmp'".to_string(),
                "possible unquoted pattern after predicate `-print'?".to_string()
            ])
        );
        assert_eq!(
            message(&["-print", "t"]),
            "paths must precede expression: `t'"
        );
        assert_eq!(message(&["t", "-name"]), "missing argument to `-name'");
        assert_eq!(
            message(&["t", "-o"]),
            "invalid expression; you have used a binary operator '-o' with nothing before it."
        );
        assert_eq!(
            message(&["t", "-name", "x", "-o"]),
            "expected an expression after '-o'"
        );
        assert_eq!(message(&["t", "!"]), "expected an expression after '!'");
        assert_eq!(
            message(&["t", "(", ")"]),
            "invalid expression; empty parentheses are not allowed."
        );
        assert_eq!(message(&["t", "-name", "x", ")"]), "you have too many ')'");
        assert_eq!(
            message(&["t", "-type", "ff"]),
            "Must separate multiple arguments to -type using: ','"
        );
        assert_eq!(message(&["t", "-size", "1q"]), "invalid -size type `q'");
        assert_eq!(
            message(&["t", "-mtime", "x"]),
            "invalid argument `x' to `-mtime'"
        );
        assert_eq!(
            message(&["t", "-maxdepth", "-1"]),
            "Expected a positive decimal integer argument to -maxdepth, but got \u{2018}-1\u{2019}"
        );
        assert_eq!(
            message(&["t", "-exec", "echo", "{}", "{}", "+"]),
            "Only one instance of {} is supported with -exec ... +"
        );
        assert_eq!(
            message(&["t", "-exec", "echo"]),
            "missing argument to `-exec'"
        );
        assert_eq!(
            message(&["t", "-perm", "644"]),
            "-perm is unsupported in bash-tool"
        );
        assert_eq!(
            parse_args(&["t", "-perm", "644"])
                .err()
                .map(|usage| usage.code),
            Some(2)
        );
    }

    #[test]
    fn printf_formats_and_paths() {
        let mut warnings = Vec::new();
        let format = parse_format("%p|%-4f|%.2h|\\t\\101\\q%z\\cX", &mut warnings).unwrap();
        assert_eq!(
            warnings,
            vec![
                "warning: unrecognized escape `\\q'".to_string(),
                "warning: unrecognized format directive `%z'".to_string()
            ]
        );
        let entry = Entry {
            display: "t/sub/b.log".into(),
            fs: PathBuf::from("/t/sub/b.log"),
            depth: 2,
            start: 0,
            stat: Stat {
                kind: Kind::File,
                size: 2,
                mtime: 0,
                atime: 0,
                ctime: 0,
                mode: None,
            },
            followed: false,
        };
        assert_eq!(
            String::from_utf8(render(&format, &entry, "t")).unwrap(),
            "t/sub/b.log|b.log|t/|\tA\\q%z"
        );
        assert!(parse_format("x%", &mut warnings).is_err());
        assert_eq!(last_component("t/"), "t/");
        assert_eq!(last_component("/"), "/");
        assert_eq!(match_name("t/"), "t");
        assert_eq!(
            format_time(1_583_298_367 * NANOS + 123_456_789, '@'),
            "1583298367.1234567890"
        );
    }

    #[test]
    fn size_and_age_rounding() {
        assert!(compare(1025_u64.div_ceil(1024), Cmp::Equal, 2));
        assert!(compare(0_u64.div_ceil(1024 * 1024), Cmp::Less, 1));
        let day = 86_400 * NANOS;
        assert!(age_matches(
            10 * day,
            10 * day - NANOS,
            Cmp::Equal,
            0.0,
            false
        ));
        assert!(age_matches(10 * day, 8 * day, Cmp::Greater, 1.0, false));
        assert!(!age_matches(10 * day, 9 * day, Cmp::Greater, 1.0, false));
        assert!(age_matches(
            10 * day,
            10 * day - 30 * NANOS,
            Cmp::Equal,
            1.0,
            true
        ));
        assert!(!age_matches(
            10 * day,
            10 * day - NANOS,
            Cmp::Equal,
            0.0,
            true
        ));
    }

    #[test]
    fn newer_dates_use_gnu_grammar() {
        use chrono::TimeZone;
        let june = chrono::Local
            .with_ymd_and_hms(2020, 6, 1, 0, 0, 0)
            .single()
            .unwrap()
            .timestamp_nanos_opt()
            .unwrap();
        for text in ["2020-06-01", "Jun 1 2020", "20200601", "1 June 2020 00:00"] {
            assert_eq!(parse_date(text), Some(i128::from(june)), "{text}");
        }
        assert_eq!(
            parse_date("2020-06-01 12:00:00"),
            Some(i128::from(june) + 12 * 3600 * NANOS)
        );
        assert_eq!(parse_date("@1590969600"), Some(1_590_969_600 * NANOS));
        // 12:00 at UTC+2 is 10:00 UTC.
        assert_eq!(
            parse_date("2020-06-01 12:00:00 +0200"),
            Some(1_591_005_600 * NANOS)
        );
        let now = timestamp(Ok(std::time::SystemTime::now())).unwrap();
        let yesterday = parse_date("yesterday").unwrap();
        assert!(yesterday < now && now - yesterday < 2 * 86_400 * NANOS);
        assert!((parse_date("now").unwrap() - now).abs() < 60 * NANOS);
        assert!(parse_date("next monday").unwrap() > now);
        assert_eq!(parse_date("xyz"), None);
        // GNU reads an empty date as now.
        assert!((parse_date("").unwrap() - now).abs() < 60 * NANOS);
    }
}
