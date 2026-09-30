//! `diff` and `cmp`: GNU-compatible file comparison.
//!
//! `diff` drives uutils/diffutils' library engines (`diffutilslib`: unified, context, normal,
//! ed and side-by-side formatters, all pure functions over byte slices — never the crate's CLI
//! entry point, which calls `process::exit`). We add what the library doesn't do: binary
//! detection, a recursive directory walk with GNU's `Only in X: name` / `diff -rX a/x b/x`
//! conventions, and `--label` / correct per-format mtime headers (the library always formats
//! headers in the unified style, which is wrong for `-c`; see [`apply_header`]).
//!
//! `cmp` has no safely reusable library: `diffutilslib::cmp` writes straight to the real
//! process stdout and reads the real process stdin for `-`, both of which bypass Brush's
//! captured streams (and a real stdin read traps a WASM agent). It is reimplemented here from
//! GNU's documented byte-for-byte semantics instead.

use diffutilslib::params::{Format, Params};
use similar::{Algorithm, DiffOp, DiffTag};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A refusal before any input is read: the command's message and exit status.
pub(crate) struct Refusal {
    pub(crate) code: i32,
    pub(crate) message: String,
}

fn refusal(code: i32, message: String) -> Refusal {
    Refusal { code, message }
}

/// WASM hard rule: refuse unsupported features loudly rather than silently ignoring them.
fn unsupported(cmd: &str, feature: &str) -> Refusal {
    refusal(2, format!("{cmd}: {feature} is unsupported in bash-tool\n"))
}

/// GNU wraps a *value* it's complaining about in Unicode single quotes (`‘…’`) — never the
/// plain ASCII `'` it uses around an *option name* in the same diagnostic family. Verified
/// against the oracle for `invalid context length`, `invalid --ignore-initial value`,
/// `missing operand after`, `extra operand`, and `EOF on`.
fn fancy_quote(s: &str) -> String {
    format!("\u{2018}{s}\u{2019}")
}

/// GNU's two-line usage diagnostic shape (verified against the oracle for `diff`, `cmp` and
/// `patch` alike): the specific complaint, then a `Try '<cmd> --help' for more information.`
/// follow-up — both to stderr, exit 2.
pub(crate) fn usage_error(cmd: &str, message: String) -> Refusal {
    refusal(
        2,
        format!("{cmd}: {message}\n{cmd}: Try '{cmd} --help' for more information.\n"),
    )
}

// ---------------------------------------------------------------------------------------------
// diff
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutFormat {
    Normal,
    Unified(usize),
    Context(usize),
    Ed,
    SideBySide(usize),
}

struct DiffOptions {
    format: OutFormat,
    brief: bool,
    report_identical: bool,
    new_file: bool,
    recursive: bool,
    text_mode: bool,
    ignore_case: bool,
    ignore_all_space: bool,
    ignore_space_change: bool,
    label_a: Option<String>,
    label_b: Option<String>,
    /// Every argv token that wasn't one of the two top-level operands, verbatim and in order —
    /// GNU's `-r` echoes exactly these (shell-escaped) in its `diff <opts> a/x b/x` header
    /// before a differing pair, not a reconstruction of the parsed flags.
    option_tokens: Vec<String>,
}

/// Parse a numeric argument to a flag, either attached (`-U3`) or as the next token (`-U 3`) —
/// getopt always consumes the very next token as the value once it commits to reading one, even
/// if that token itself looks like a flag (`diff -U -1` reports an *invalid* value, not a
/// missing one). `letter` is the bare option letter (`"U"`) for the missing-argument message;
/// `kind` names what the value means (`"context length"`, `"width"`) for the invalid-value one —
/// both verified against the oracle.
fn numeric_flag(
    cmd: &str,
    letter: &str,
    kind: &str,
    attached: &str,
    args: &[String],
    i: &mut usize,
) -> Result<usize, Refusal> {
    let text = if attached.is_empty() {
        *i += 1;
        args.get(*i)
            .cloned()
            .ok_or_else(|| usage_error(cmd, format!("option requires an argument -- '{letter}'")))?
    } else {
        attached.to_owned()
    };
    text.parse::<usize>()
        .map_err(|_| usage_error(cmd, format!("invalid {kind} {}", fancy_quote(&text))))
}

/// Agents write bundled short options constantly (`-ruN`, `-Naur`, `-urN`, `-rq`) — parsed
/// getopt-style: each character in a single-dash token is its own flag, and an argument-taking
/// one (`-U`/`-C`/`-W`) consumes the rest of the token if anything follows it there, else the
/// next token, and ends the bundle (nothing can follow a value in the same token).
#[allow(clippy::too_many_lines)] // a flat CLI switch reads more clearly than a fragmented one
fn parse_diff(argv: &[String]) -> Result<(DiffOptions, [String; 2]), Refusal> {
    let cmd = "diff";
    let args = &argv[1..];
    let mut format: Option<OutFormat> = None;
    let mut pending_width: Option<usize> = None;
    let mut brief = false;
    let mut report_identical = false;
    let mut new_file = false;
    let mut recursive = false;
    let mut text_mode = false;
    let mut labels: Vec<String> = Vec::new();
    let mut operands: Vec<String> = Vec::new();
    let mut option_tokens: Vec<String> = Vec::new();
    let mut end_of_opts = false;
    let mut ignore_case = false;
    let mut ignore_all_space = false;
    let mut ignore_space_change = false;

    let mut i = 0;
    while i < args.len() {
        let a = args[i].clone();
        if end_of_opts || a == "-" || !a.starts_with('-') {
            operands.push(a);
            i += 1;
            continue;
        }
        option_tokens.push(a.clone());
        if a == "--" {
            end_of_opts = true;
            i += 1;
            continue;
        }
        if let Some(long) = a.strip_prefix("--") {
            match long {
                "unified" => format = Some(OutFormat::Unified(3)),
                "context" => format = Some(OutFormat::Context(3)),
                "ed" => format = Some(OutFormat::Ed),
                "side-by-side" => {
                    format = Some(OutFormat::SideBySide(pending_width.unwrap_or(130)));
                }
                "brief" => brief = true,
                "report-identical-files" => report_identical = true,
                "new-file" => new_file = true,
                "recursive" => recursive = true,
                "text" => text_mode = true,
                "ignore-case" => ignore_case = true,
                "ignore-all-space" => ignore_all_space = true,
                "ignore-space-change" => ignore_space_change = true,
                "ignore-blank-lines" => return Err(unsupported(cmd, "--ignore-blank-lines")),
                // -d/--minimal only ever asks for a different (usually slower) LCS algorithm
                // in search of a visually smaller diff; it can't change what's semantically
                // correct output, and every case this fork has been checked against gets the
                // identical result either way, so it's accepted as a no-op rather than refused.
                // --speed-large-files is a pure performance hint, invisible to any observable
                // output. --normal explicitly requests the format that's already the default.
                "minimal" | "speed-large-files" => {}
                "normal" => format = Some(OutFormat::Normal),
                "rcs" => return Err(unsupported(cmd, "-n/--rcs")),
                "show-c-function" => return Err(unsupported(cmd, "-p/--show-c-function")),
                "ignore-matching-lines" => {
                    return Err(unsupported(cmd, "-I/--ignore-matching-lines"));
                }
                "show-function-line" => {
                    return Err(unsupported(cmd, "-F/--show-function-line"));
                }
                _ if long.starts_with("ignore-matching-lines=") => {
                    return Err(unsupported(cmd, "-I/--ignore-matching-lines"));
                }
                _ if long.starts_with("show-function-line=") => {
                    return Err(unsupported(cmd, "-F/--show-function-line"));
                }
                "color" => {}
                "label" => {
                    i += 1;
                    let value = args.get(i).cloned().ok_or_else(|| {
                        usage_error(cmd, "option '--label' requires an argument".to_owned())
                    })?;
                    option_tokens.push(value.clone());
                    labels.push(value);
                }
                _ if long.starts_with("label=") => labels.push(long["label=".len()..].to_owned()),
                _ if long.starts_with("color=") => {}
                _ if long.starts_with("unified=") => {
                    let n = long["unified=".len()..]
                        .parse()
                        .map_err(|_| refusal(2, format!("{cmd}: invalid --unified argument\n")))?;
                    format = Some(OutFormat::Unified(n));
                }
                _ if long.starts_with("context=") => {
                    let n = long["context=".len()..]
                        .parse()
                        .map_err(|_| refusal(2, format!("{cmd}: invalid --context argument\n")))?;
                    format = Some(OutFormat::Context(n));
                }
                _ if long.starts_with("width=") => {
                    let n = long["width=".len()..]
                        .parse()
                        .map_err(|_| refusal(2, format!("{cmd}: invalid --width argument\n")))?;
                    pending_width = Some(n);
                    if let Some(OutFormat::SideBySide(_)) = format {
                        format = Some(OutFormat::SideBySide(n));
                    }
                }
                _ => return Err(usage_error(cmd, format!("unrecognized option '{a}'"))),
            }
            i += 1;
            continue;
        }

        let bytes = a.as_bytes();
        let mut ci = 1;
        while ci < bytes.len() {
            let c = bytes[ci] as char;
            match c {
                'u' => format = Some(OutFormat::Unified(3)),
                'c' => format = Some(OutFormat::Context(3)),
                'e' => format = Some(OutFormat::Ed),
                'y' => format = Some(OutFormat::SideBySide(pending_width.unwrap_or(130))),
                'q' => brief = true,
                's' => report_identical = true,
                'N' => new_file = true,
                'r' => recursive = true,
                'a' => text_mode = true,
                'i' => ignore_case = true,
                'w' => ignore_all_space = true,
                'b' => ignore_space_change = true,
                'B' => return Err(unsupported(cmd, "-B")),
                // See the matching `"minimal"` long-option arm above.
                'd' => {}
                'n' => return Err(unsupported(cmd, "-n/--rcs")),
                'p' => return Err(unsupported(cmd, "-p/--show-c-function")),
                'I' => return Err(unsupported(cmd, "-I/--ignore-matching-lines")),
                'F' => return Err(unsupported(cmd, "-F/--show-function-line")),
                'U' => {
                    let before = i;
                    let n = numeric_flag(cmd, "U", "context length", &a[ci + 1..], args, &mut i)?;
                    if i > before {
                        option_tokens.push(args[i].clone());
                    }
                    format = Some(OutFormat::Unified(n));
                    ci = bytes.len();
                }
                'C' => {
                    let before = i;
                    let n = numeric_flag(cmd, "C", "context length", &a[ci + 1..], args, &mut i)?;
                    if i > before {
                        option_tokens.push(args[i].clone());
                    }
                    format = Some(OutFormat::Context(n));
                    ci = bytes.len();
                }
                'W' => {
                    let before = i;
                    let n = numeric_flag(cmd, "W", "width", &a[ci + 1..], args, &mut i)?;
                    if i > before {
                        option_tokens.push(args[i].clone());
                    }
                    // A parse of "0" succeeds as a usize, but GNU rejects zero specifically for
                    // width (unlike context length, where 0 is a meaningful radius) — verified
                    // against the oracle.
                    if n == 0 {
                        return Err(usage_error(
                            cmd,
                            format!("invalid width {}", fancy_quote("0")),
                        ));
                    }
                    pending_width = Some(n);
                    if let Some(OutFormat::SideBySide(_)) = format {
                        format = Some(OutFormat::SideBySide(n));
                    }
                    ci = bytes.len();
                }
                _ => return Err(usage_error(cmd, format!("invalid option -- '{c}'"))),
            }
            ci += 1;
        }
        i += 1;
    }

    match operands.len() {
        2 => {}
        0 => {
            return Err(usage_error(
                cmd,
                format!("missing operand after {}", fancy_quote(cmd)),
            ));
        }
        1 => {
            return Err(usage_error(
                cmd,
                format!("missing operand after {}", fancy_quote(&operands[0])),
            ));
        }
        _ => {
            return Err(usage_error(
                cmd,
                format!("extra operand {}", fancy_quote(&operands[2])),
            ));
        }
    }
    let mut label_iter = labels.into_iter();
    let opts = DiffOptions {
        format: format.unwrap_or(OutFormat::Normal),
        brief,
        report_identical,
        new_file,
        recursive,
        text_mode,
        ignore_case,
        ignore_all_space,
        ignore_space_change,
        label_a: label_iter.next(),
        label_b: label_iter.next(),
        option_tokens,
    };
    Ok((opts, [operands[0].clone(), operands[1].clone()]))
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|&b| b == 0)
}

fn build_params(
    a_name: &str,
    b_name: &str,
    format: Format,
    context_count: usize,
    width: usize,
) -> Params {
    Params {
        executable: OsString::from("diff"),
        from: OsString::from(a_name),
        to: OsString::from(b_name),
        format,
        context_count,
        report_identical_files: false,
        brief: false,
        expand_tabs: false,
        tabsize: 8,
        width: if width == 0 { 130 } else { width },
    }
}

/// GNU's timestamp style differs by format: unified/side-by-side use
/// `%Y-%m-%d %H:%M:%S%.9f %z`; context uses ctime-style `%a %b %e %H:%M:%S %Y`. The
/// `diffutilslib` engines always format in the first style regardless of selected format —
/// [`apply_header`] replaces the header line it produced with this instead.
fn format_mtime(format: Format, mtime: Option<SystemTime>) -> String {
    use chrono::{DateTime, Local};
    let t: DateTime<Local> = mtime.unwrap_or_else(SystemTime::now).into();
    match format {
        Format::Context => t.format("%a %b %e %H:%M:%S %Y").to_string(),
        _ => t.format("%Y-%m-%d %H:%M:%S%.9f %z").to_string(),
    }
}

fn header_prefixes(format: Format) -> (&'static str, &'static str) {
    match format {
        Format::Context => ("*** ", "--- "),
        _ => ("--- ", "+++ "),
    }
}

/// Replace the two-line file header `unified_diff`/`context_diff` produced (`--- name\ttime` /
/// `+++ name\ttime`, or `***`/`---` for context) with either the GNU `--label` text (no
/// timestamp at all — matches real `diff --label`) or the same name with a correctly
/// format-specific timestamp. Leaves `body` untouched if its shape doesn't match what we
/// expect (defensive: never corrupt output we don't understand).
fn apply_header(
    body: Vec<u8>,
    format: Format,
    a_name: &str,
    b_name: &str,
    label_a: Option<&str>,
    label_b: Option<&str>,
    a_mtime: Option<SystemTime>,
    b_mtime: Option<SystemTime>,
) -> Vec<u8> {
    let (p1, p2) = header_prefixes(format);
    let old1 = format!("{p1}{a_name}\t");
    let Some(after1) = body.strip_prefix(old1.as_bytes()) else {
        return body;
    };
    let nl1 = after1
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(after1.len());
    let rest1 = &after1[(nl1 + 1).min(after1.len())..];
    let old2 = format!("{p2}{b_name}\t");
    let Some(after2) = rest1.strip_prefix(old2.as_bytes()) else {
        return body;
    };
    let nl2 = after2
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(after2.len());
    let tail = &after2[(nl2 + 1).min(after2.len())..];

    let line1 = match label_a {
        Some(l) => format!("{p1}{l}\n"),
        None => format!("{p1}{a_name}\t{}\n", format_mtime(format, a_mtime)),
    };
    let line2 = match label_b {
        Some(l) => format!("{p2}{l}\n"),
        None => format!("{p2}{b_name}\t{}\n", format_mtime(format, b_mtime)),
    };
    let mut out = Vec::with_capacity(line1.len() + line2.len() + tail.len());
    out.extend_from_slice(line1.as_bytes());
    out.extend_from_slice(line2.as_bytes());
    out.extend_from_slice(tail);
    out
}

/// Split into lines that keep their trailing `\n` (so a missing final newline shows up as the
/// last line alone lacking one), for the UTF-8 grouped renderers below. `None` for content that
/// isn't valid UTF-8 (rare for text files, and already excluded from the binary-file path,
/// which checks for a NUL byte rather than valid UTF-8) — callers fall back to `diffutilslib`.
fn text_lines(bytes: &[u8]) -> Option<Vec<&str>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest.find('\n').map_or(rest.len(), |p| p + 1);
        let (line, remaining) = rest.split_at(end);
        lines.push(line);
        rest = remaining;
    }
    Some(lines)
}

/// GNU's unified-format hunk-range display: `N` for a single-line range, `N,COUNT` otherwise —
/// and, for an empty range (a pure insertion or deletion point), the line number *before* it
/// rather than at it. Verified against the oracle: unified's second number is a line *count*.
fn hunk_range(start: usize, end: usize) -> String {
    let len = end - start;
    let mut beginning = start + 1;
    if len == 1 {
        beginning.to_string()
    } else {
        if len == 0 {
            beginning -= 1;
        }
        format!("{beginning},{len}")
    }
}

/// GNU's context-format hunk-range display: same single-line collapse as [`hunk_range`], but the
/// second number of a multi-line range is the 1-based *end* line, not a count (verified against
/// the oracle: `-C1` on a change at old lines 2-4 prints `2,4`, not `2,3`) — a real difference
/// from unified format, not a typo.
fn context_hunk_range(start: usize, end: usize) -> String {
    let len = end - start;
    if len == 1 {
        (start + 1).to_string()
    } else if len == 0 {
        format!("{start},{end}")
    } else {
        format!("{},{end}", start + 1)
    }
}

/// The key `-i`/`-w`/`-b` compare two lines by, instead of their literal text — the line minus
/// its trailing `\n`, then: `-w` drops every whitespace character; `-b` collapses each run of
/// whitespace to one space and drops trailing whitespace entirely, but a run at the very start
/// of the line still counts as different from no leading whitespace at all (verified against the
/// oracle: `-b` treats "  a" and "   a" as equal but " a" and "a" as different — collapsing the
/// *amount* of whitespace, not its presence); `-i` lowercases the result. The line actually
/// *rendered* is never touched by this — see [`render_unified`]/[`render_context`], which always
/// pull context/changed lines from the original `a`/`b` slices, never from these keys.
fn comparison_key<'a>(line: &'a str, opts: &DiffOptions) -> std::borrow::Cow<'a, str> {
    if !opts.ignore_case && !opts.ignore_all_space && !opts.ignore_space_change {
        return std::borrow::Cow::Borrowed(line);
    }
    let s = line.strip_suffix('\n').unwrap_or(line);
    let mut result = String::with_capacity(s.len());
    if opts.ignore_all_space {
        result.extend(s.chars().filter(|c| !c.is_whitespace()));
    } else if opts.ignore_space_change {
        let mut chars = s.trim_end().chars().peekable();
        while let Some(c) = chars.next() {
            if c.is_whitespace() {
                result.push(' ');
                while chars.peek().is_some_and(|next| next.is_whitespace()) {
                    chars.next();
                }
            } else {
                result.push(c);
            }
        }
    } else {
        result.push_str(s);
    }
    if opts.ignore_case {
        result = result.to_lowercase();
    }
    std::borrow::Cow::Owned(result)
}

/// Whether two files actually differ, honoring `-i`/`-w`/`-b`: a plain byte comparison when none
/// of them are set (the common, cheap case — and the only thing that makes sense for content
/// that isn't valid UTF-8, since the ignore options work line-by-line on text), else a
/// line-by-line comparison under [`comparison_key`].
/// Whether `v` is non-empty and doesn't end in a newline -- the condition `diff -e` always
/// warns about and exits 2 for, even when the two files are otherwise byte-identical (real `ed`
/// scripts are line-based and can't represent a file missing its final newline at all, so this
/// is unconditional on ed format, unlike the content comparison every other format uses).
fn missing_trailing_newline(v: &[u8]) -> bool {
    !v.is_empty() && *v.last().unwrap() != b'\n'
}

fn content_differs(a_bytes: &[u8], b_bytes: &[u8], opts: &DiffOptions) -> bool {
    if !opts.ignore_case && !opts.ignore_all_space && !opts.ignore_space_change {
        return a_bytes != b_bytes;
    }
    match (text_lines(a_bytes), text_lines(b_bytes)) {
        (Some(al), Some(bl)) => {
            al.len() != bl.len()
                || al
                    .iter()
                    .zip(bl.iter())
                    .any(|(x, y)| comparison_key(x, opts) != comparison_key(y, opts))
        }
        _ => a_bytes != b_bytes,
    }
}

/// `diffutilslib` finds its edit script with the `diff` crate, which fills a table with a cell for
/// each pair of lines left once the common head and tail are set aside: 30,000 differing lines a
/// side is 3.6 GB, past what a WASM component can allocate. Formats still rendered through it
/// refuse inputs whose table would exceed this many cells (64 MiB).
const MAX_LCS_CELLS: usize = 1 << 24;

/// The cells the `diff` crate's table would need for these inputs.
fn lcs_cells(a: &[u8], b: &[u8]) -> usize {
    let a: Vec<&[u8]> = a.split_inclusive(|&c| c == b'\n').collect();
    let b: Vec<&[u8]> = b.split_inclusive(|&c| c == b'\n').collect();
    let head = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    (a.len() - head - tail + 1).saturating_mul(b.len() - head - tail + 1)
}

/// Group the line-level edit script the way GNU does: two changes share a hunk when the
/// unchanged gap between them is at most `2*n` (verified against the oracle: with `-U3`, gaps of
/// 5 and 6 merge, a gap of 7 splits). `similar::group_diff_ops` implements exactly this rule (its
/// `len > n * 2` check) — `diffutilslib`'s own `unified_diff`/`context_diff` don't group at all,
/// starting a new hunk at every change regardless of how close the previous one is, which real
/// `diff -u`/`diff -c` never do. This is why `-u`/`-c` are rendered here instead of through it.
/// Lines compare equal (and so end up `Equal`, not `Replace`/`Delete`/`Insert`) under
/// [`comparison_key`], which is the identity function unless an ignore option is set.
fn grouped_ops(a: &[&str], b: &[&str], n: usize, opts: &DiffOptions) -> Vec<Vec<DiffOp>> {
    let ak: Vec<_> = a.iter().map(|l| comparison_key(l, opts)).collect();
    let bk: Vec<_> = b.iter().map(|l| comparison_key(l, opts)).collect();
    let ops = similar::capture_diff_slices(Algorithm::Myers, &ak, &bk);
    similar::group_diff_ops(ops, n)
}

fn header_line(
    prefix: &str,
    name: &str,
    label: Option<&str>,
    mtime: Option<SystemTime>,
    format: Format,
) -> String {
    match label {
        Some(l) => format!("{prefix}{l}\n"),
        None => format!("{prefix}{name}\t{}\n", format_mtime(format, mtime)),
    }
}

fn write_unified_line(out: &mut Vec<u8>, prefix: u8, line: &str) {
    out.push(prefix);
    out.extend_from_slice(line.as_bytes());
    if !line.ends_with('\n') {
        out.push(b'\n');
        out.extend_from_slice(b"\\ No newline at end of file\n");
    }
}

/// Render grouped hunks in unified format: one `@@ -a,b +c,d @@` header per group, then each
/// op's lines (` `/`-`/`+`, a `Replace` op emitting its deleted lines then its inserted ones).
fn render_unified(a: &[&str], b: &[&str], groups: &[Vec<DiffOp>]) -> Vec<u8> {
    let mut out = Vec::new();
    for group in groups {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let old = (first.old_range().start, last.old_range().end);
        let new = (first.new_range().start, last.new_range().end);
        let _ = writeln!(
            out,
            "@@ -{} +{} @@",
            hunk_range(old.0, old.1),
            hunk_range(new.0, new.1)
        );
        for op in group {
            match op.tag() {
                DiffTag::Equal => {
                    for line in &a[op.old_range()] {
                        write_unified_line(&mut out, b' ', line);
                    }
                }
                DiffTag::Delete => {
                    for line in &a[op.old_range()] {
                        write_unified_line(&mut out, b'-', line);
                    }
                }
                DiffTag::Insert => {
                    for line in &b[op.new_range()] {
                        write_unified_line(&mut out, b'+', line);
                    }
                }
                DiffTag::Replace => {
                    for line in &a[op.old_range()] {
                        write_unified_line(&mut out, b'-', line);
                    }
                    for line in &b[op.new_range()] {
                        write_unified_line(&mut out, b'+', line);
                    }
                }
            }
        }
    }
    out
}

fn write_context_line(out: &mut Vec<u8>, marker: u8, line: &str) {
    out.push(marker);
    out.push(b' ');
    out.extend_from_slice(line.as_bytes());
    if !line.ends_with('\n') {
        out.push(b'\n');
        out.extend_from_slice(b"\\ No newline at end of file\n");
    }
}

/// Render grouped hunks in context format. Verified against the oracle: a block (the `***`/old
/// side or the `---`/new side) is only printed with body lines when it actually contains a
/// change (`-`/`!` for old, `+`/`!` for new) — a pure insertion prints an empty old block (just
/// its header line) and a pure deletion prints an empty new block, rather than repeating the
/// shared context under both.
fn render_context(a: &[&str], b: &[&str], groups: &[Vec<DiffOp>]) -> Vec<u8> {
    let mut out = Vec::new();
    for group in groups {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let old = (first.old_range().start, last.old_range().end);
        let new = (first.new_range().start, last.new_range().end);
        out.extend_from_slice(b"***************\n");
        let _ = writeln!(out, "*** {} ****", context_hunk_range(old.0, old.1));
        if group
            .iter()
            .any(|op| matches!(op.tag(), DiffTag::Delete | DiffTag::Replace))
        {
            for op in group {
                match op.tag() {
                    DiffTag::Equal => {
                        for line in &a[op.old_range()] {
                            write_context_line(&mut out, b' ', line);
                        }
                    }
                    DiffTag::Delete => {
                        for line in &a[op.old_range()] {
                            write_context_line(&mut out, b'-', line);
                        }
                    }
                    DiffTag::Replace => {
                        for line in &a[op.old_range()] {
                            write_context_line(&mut out, b'!', line);
                        }
                    }
                    DiffTag::Insert => {}
                }
            }
        }
        let _ = writeln!(out, "--- {} ----", context_hunk_range(new.0, new.1));
        if group
            .iter()
            .any(|op| matches!(op.tag(), DiffTag::Insert | DiffTag::Replace))
        {
            for op in group {
                match op.tag() {
                    DiffTag::Equal => {
                        for line in &b[op.new_range()] {
                            write_context_line(&mut out, b' ', line);
                        }
                    }
                    DiffTag::Insert => {
                        for line in &b[op.new_range()] {
                            write_context_line(&mut out, b'+', line);
                        }
                    }
                    DiffTag::Replace => {
                        for line in &b[op.new_range()] {
                            write_context_line(&mut out, b'!', line);
                        }
                    }
                    DiffTag::Delete => {}
                }
            }
        }
    }
    out
}

/// Render normal format from an (ungrouped — normal format has no context lines to merge a gap
/// across, so each contiguous change is its own command) edit script. Only used for the `-i`/
/// `-w`/`-b` path (see [`comparison_key`]); the ordinary path still goes through `diffutilslib`
/// (via [`collapse_redundant_ranges`]) since that's already correct once its range-collapse bug
/// is worked around. Ranges use [`context_hunk_range`]'s `start,end` convention — GNU's normal
/// format shares it with context format, not unified format's `start,count`.
fn render_normal(a: &[&str], b: &[&str], ops: &[DiffOp]) -> Vec<u8> {
    let mut out = Vec::new();
    for op in ops {
        let old = (op.old_range().start, op.old_range().end);
        let new = (op.new_range().start, op.new_range().end);
        match op.tag() {
            DiffTag::Equal => {}
            DiffTag::Delete => {
                let _ = writeln!(out, "{}d{}", context_hunk_range(old.0, old.1), new.0);
                for line in &a[op.old_range()] {
                    write_context_line(&mut out, b'<', line);
                }
            }
            DiffTag::Insert => {
                let _ = writeln!(out, "{}a{}", old.0, context_hunk_range(new.0, new.1));
                for line in &b[op.new_range()] {
                    write_context_line(&mut out, b'>', line);
                }
            }
            DiffTag::Replace => {
                let _ = writeln!(
                    out,
                    "{}c{}",
                    context_hunk_range(old.0, old.1),
                    context_hunk_range(new.0, new.1)
                );
                for line in &a[op.old_range()] {
                    write_context_line(&mut out, b'<', line);
                }
                out.extend_from_slice(b"---\n");
                for line in &b[op.new_range()] {
                    write_context_line(&mut out, b'>', line);
                }
            }
        }
    }
    out
}

/// Render an ed script (`diff -e`) from grouped ops, in GNU's own bottom-to-top hunk order:
/// `groups` is iterated in reverse so each hunk's line numbers -- always against the
/// *untouched original* file, since that's the only numbering an unapplied later hunk can
/// still trust -- are emitted before any hunk whose own application would shift them. Within a
/// hunk: an empty old range is a pure insertion (`Na`, `N` the 0-based old insertion point,
/// already GNU's "insert after old line N"); an empty new range is a pure deletion (`d`, no
/// body); otherwise it's a change (`c`), body from the new range either way. A body line that
/// is *only* a literal `.` (which GNU escapes, doubling it and following with an ed `s/.//` to
/// undo the doubling once inserted) is left unescaped here -- rare enough in practice, and
/// GNU's own escape sometimes splits one hunk into several ed commands to keep `s/.//`
/// addressed at the exact line it must fix, which is a materially bigger rewrite for a gap this
/// narrow.
fn render_ed(b: &[&str], groups: &[Vec<DiffOp>]) -> Vec<u8> {
    let mut out = Vec::new();
    for group in groups.iter().rev() {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let old = (first.old_range().start, last.old_range().end);
        let new = (first.new_range().start, last.new_range().end);
        if old.0 == old.1 {
            let _ = writeln!(out, "{}a", old.0);
        } else if new.0 == new.1 {
            let _ = writeln!(out, "{}d", context_hunk_range(old.0, old.1));
            continue;
        } else {
            let _ = writeln!(out, "{}c", context_hunk_range(old.0, old.1));
        }
        write_ed_body(&mut out, &b[new.0..new.1]);
    }
    out
}

/// Write an `a`/`c` command's body text, GNU's way: a line that is *only* a literal `.` would
/// otherwise read as `a`/`c`'s own end-of-text terminator, so it's doubled (`..`) and followed
/// by an ed `s/.//` to strip the extra `.` back off once the line has actually been inserted.
/// `s/.//` addresses no line explicitly, so it always acts on ed's *current* line -- the last
/// line just written -- which only lands on the right line if the escaped `.` really was the
/// last thing in that block. So each escaped `.` forces a break: the pending block is
/// terminated right there (even if that's every line so far), fixed up, and -- if any lines of
/// this body remain -- a fresh, address-less `a` (ed's current line is already correctly
/// positioned right after the fix-up) starts the next block.
fn write_ed_body(out: &mut Vec<u8>, lines: &[&str]) {
    let mut open = false;
    let mut iter = lines.iter().peekable();
    while let Some(line) = iter.next() {
        let text = line.strip_suffix('\n').unwrap_or(line);
        if text == "." {
            out.extend_from_slice(b"..\n.\ns/.//\n");
            open = false;
            if iter.peek().is_some() {
                out.extend_from_slice(b"a\n");
            }
        } else {
            out.extend_from_slice(text.as_bytes());
            out.push(b'\n');
            open = true;
        }
    }
    if open {
        out.extend_from_slice(b".\n");
    }
}

/// `normal_diff`/`ed_diff` always print `N,N` for a single-line `a`/`d`/`c` range (`0a1,1`,
/// `5,5d3`) instead of GNU's `N` (`0a1`, `5d3`) — the sibling `unified_diff`/`context_diff`
/// formatters already collapse it, so this is a formatting gap specific to those two. Collapse
/// it ourselves on any line that looks like a whole command (so hunk content, which can start
/// with arbitrary text, is never touched).
fn collapse_redundant_ranges(body: Vec<u8>) -> Vec<u8> {
    // Bytes, not `str`: a *content* line (an actual diffed line, `<`/`>`/`---`-prefixed) can
    // contain invalid UTF-8 (this is exactly the fallback path for a file `text_lines` refused,
    // because it wasn't valid UTF-8) — requiring the *whole* body to decode as one `str` made
    // this silently skip every line, command lines included, the moment any content line had an
    // invalid byte anywhere in the file. A command line (`\d+(,\d+)?[acd]...`) is always
    // plain ASCII on its own, so matching it byte-by-byte needs no such assumption about its
    // neighbors.
    static COMMAND: std::sync::OnceLock<regex::bytes::Regex> = std::sync::OnceLock::new();
    static PAIR: std::sync::OnceLock<regex::bytes::Regex> = std::sync::OnceLock::new();
    let command = COMMAND
        .get_or_init(|| regex::bytes::Regex::new(r"^\d+(,\d+)?[acd](\d+(,\d+)?)?$").unwrap());
    let pair = PAIR.get_or_init(|| regex::bytes::Regex::new(r"(\d+),(\d+)").unwrap());

    let mut out = Vec::with_capacity(body.len());
    for line in body.split_inclusive(|&b| b == b'\n') {
        let trimmed = line.strip_suffix(b"\n").unwrap_or(line);
        if command.is_match(trimmed) {
            let replaced = pair.replace_all(trimmed, |c: &regex::bytes::Captures| {
                if c[1] == c[2] {
                    c[1].to_vec()
                } else {
                    [&c[1], b",".as_slice(), &c[2]].concat()
                }
            });
            out.extend_from_slice(&replaced);
            out.extend_from_slice(&line[trimmed.len()..]);
        } else {
            out.extend_from_slice(line);
        }
    }
    out
}

/// Reorder an ed script's hunks from the library's forward (top-to-bottom) order to GNU's own
/// bottom-to-top order.
///
/// An ed script is meant to be fed to `ed` one hunk at a time; every hunk's line numbers refer
/// to the *original* file, so applying an earlier hunk first would shift the line numbers an
/// unapplied later hunk still expects. GNU's `diff -e` sidesteps this by listing hunks from the
/// end of the file backwards, so each one lands before any hunk that could renumber it out from
/// under itself; `diffutilslib::ed_diff`'s hunks are individually correct (right ranges, right
/// `a`/`c`/`d` command, right `.` terminator) but left in forward order, so this only needs to
/// regroup and reverse them, not touch their content.
fn reverse_ed_hunks(body: Vec<u8>) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(&body) else {
        return body;
    };
    static COMMAND: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let command = COMMAND.get_or_init(|| regex::Regex::new(r"^\d+(,\d+)?[acd]$").unwrap());

    let mut hunks: Vec<String> = Vec::new();
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_end_matches('\n');
        if !command.is_match(trimmed) {
            // Shouldn't happen for well-formed ed-script output; keep any such line as its own
            // "hunk" rather than losing or misattaching it.
            hunks.push(line.to_owned());
            continue;
        }
        let mut hunk = line.to_owned();
        if trimmed.ends_with(['a', 'c']) {
            // Consume the replacement/insertion text up to and including the lone-`.`
            // terminator; a literal `.` line in the text itself is escaped (doubled, with a
            // following `s/.//`) by `ed_diff`, so it never reads as the terminator here.
            for body_line in lines.by_ref() {
                hunk.push_str(body_line);
                if body_line.trim_end_matches('\n') == "." {
                    break;
                }
            }
        }
        hunks.push(hunk);
    }
    hunks.reverse();
    hunks.concat().into_bytes()
}

/// Read one `diff`/`cmp` operand: `-` reads `stdin` (once — a second `-` on the same
/// invocation, e.g. two-stdin, reads nothing further), a missing path is emptied under `-N`,
/// and everything else is [`super::read_file`] (which already treats `/dev/null` as empty —
/// there is no `/dev` in the WASM filesystem).
fn read_operand(
    name: &str,
    resolve: &dyn Fn(&str) -> PathBuf,
    stdin: &mut dyn Read,
    new_file: bool,
) -> Result<(Vec<u8>, Option<SystemTime>), (String, std::io::Error)> {
    if name == "-" {
        let mut buf = Vec::new();
        stdin
            .read_to_end(&mut buf)
            .map_err(|e| (name.to_owned(), e))?;
        return Ok((buf, None));
    }
    let path = resolve(name);
    match super::read_file(&path) {
        Ok(bytes) => {
            let mtime = std::fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok());
            Ok((bytes, mtime))
        }
        Err(e) if new_file && e.kind() == std::io::ErrorKind::NotFound => Ok((Vec::new(), None)),
        Err(e) => Err((name.to_owned(), e)),
    }
}

/// Compare one file pair and write the result. `recursive_prefix`, when set, is the
/// `diff -rX a/x b/x` line `-r` mode prints immediately before a full (non-brief, non-binary)
/// diff body, so callers other than the directory walk pass `None`.
#[allow(clippy::too_many_arguments)]
fn compare_pair(
    a_name: &str,
    b_name: &str,
    stdin: &mut dyn Read,
    resolve: &dyn Fn(&str) -> PathBuf,
    opts: &DiffOptions,
    recursive_prefix: Option<&str>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> std::io::Result<i32> {
    let (a_bytes, a_mtime) = match read_operand(a_name, resolve, stdin, opts.new_file) {
        Ok(v) => v,
        Err((name, e)) => {
            writeln!(err, "diff: {name}: {}", super::io_message(&e))?;
            return Ok(2);
        }
    };
    let (b_bytes, b_mtime) = match read_operand(b_name, resolve, stdin, opts.new_file) {
        Ok(v) => v,
        Err((name, e)) => {
            writeln!(err, "diff: {name}: {}", super::io_message(&e))?;
            return Ok(2);
        }
    };

    // `diff -e` must still report a missing final newline even when the files are otherwise
    // byte-identical (see `missing_trailing_newline`); every other format has nothing to say
    // about that case, so only ed format's own branch below needs the early return skipped.
    let ed_newline_only = matches!(opts.format, OutFormat::Ed)
        && (missing_trailing_newline(&a_bytes) || missing_trailing_newline(&b_bytes));
    if !content_differs(&a_bytes, &b_bytes, opts) && !ed_newline_only {
        if opts.report_identical {
            writeln!(out, "Files {a_name} and {b_name} are identical")?;
        }
        return Ok(0);
    }

    if !opts.text_mode && (is_binary(&a_bytes) || is_binary(&b_bytes)) {
        writeln!(out, "Binary files {a_name} and {b_name} differ")?;
        return Ok(1);
    }

    if opts.brief {
        writeln!(out, "Files {a_name} and {b_name} differ")?;
        return Ok(1);
    }

    if let Some(prefix) = recursive_prefix {
        writeln!(out, "{prefix}")?;
    }

    // What `diffutilslib` still renders (non-UTF-8 input, `-y`, `-e`) runs its quadratic engine.
    let utf8 = std::str::from_utf8(&a_bytes).is_ok() && std::str::from_utf8(&b_bytes).is_ok();
    let quadratic = match opts.format {
        OutFormat::SideBySide(_) => Some("-y on files this large"),
        OutFormat::Ed => Some("-e on files this large"),
        _ if !utf8 => Some("comparing non-UTF-8 files this large"),
        _ => None,
    };
    if let Some(feature) = quadratic
        && lcs_cells(&a_bytes, &b_bytes) > MAX_LCS_CELLS
    {
        let refusal = unsupported("diff", feature);
        err.write_all(refusal.message.as_bytes())?;
        return Ok(refusal.code);
    }

    match opts.format {
        OutFormat::Normal => {
            let body = match (text_lines(&a_bytes), text_lines(&b_bytes)) {
                // Rendered here rather than by `diffutilslib::normal_diff`, whose engine is
                // quadratic in memory (see `MAX_LCS_CELLS`).
                (Some(al), Some(bl)) => {
                    let ak: Vec<_> = al.iter().map(|l| comparison_key(l, opts)).collect();
                    let bk: Vec<_> = bl.iter().map(|l| comparison_key(l, opts)).collect();
                    let ops = similar::capture_diff_slices(Algorithm::Myers, &ak, &bk);
                    render_normal(&al, &bl, &ops)
                }
                _ => {
                    let params = build_params(a_name, b_name, Format::Normal, 0, 0);
                    collapse_redundant_ranges(diffutilslib::normal_diff(
                        &a_bytes, &b_bytes, &params,
                    ))
                }
            };
            out.write_all(&body)?;
            Ok(1)
        }
        OutFormat::Unified(n) => {
            let body = match (text_lines(&a_bytes), text_lines(&b_bytes)) {
                (Some(al), Some(bl)) => {
                    let mut body = header_line(
                        "--- ",
                        a_name,
                        opts.label_a.as_deref(),
                        a_mtime,
                        Format::Unified,
                    )
                    .into_bytes();
                    body.extend(
                        header_line(
                            "+++ ",
                            b_name,
                            opts.label_b.as_deref(),
                            b_mtime,
                            Format::Unified,
                        )
                        .into_bytes(),
                    );
                    body.extend(render_unified(&al, &bl, &grouped_ops(&al, &bl, n, opts)));
                    body
                }
                // Non-UTF-8 (rare for text content): diffutilslib's ungrouped unified diff is
                // still correct, just not merged into GNU's larger hunks — see `grouped_ops`.
                _ => {
                    let params = build_params(a_name, b_name, Format::Unified, n, 0);
                    let body = diffutilslib::unified_diff(&a_bytes, &b_bytes, &params);
                    apply_header(
                        body,
                        Format::Unified,
                        a_name,
                        b_name,
                        opts.label_a.as_deref(),
                        opts.label_b.as_deref(),
                        a_mtime,
                        b_mtime,
                    )
                }
            };
            out.write_all(&body)?;
            Ok(1)
        }
        OutFormat::Context(n) => {
            let body = match (text_lines(&a_bytes), text_lines(&b_bytes)) {
                (Some(al), Some(bl)) => {
                    let mut body = header_line(
                        "*** ",
                        a_name,
                        opts.label_a.as_deref(),
                        a_mtime,
                        Format::Context,
                    )
                    .into_bytes();
                    body.extend(
                        header_line(
                            "--- ",
                            b_name,
                            opts.label_b.as_deref(),
                            b_mtime,
                            Format::Context,
                        )
                        .into_bytes(),
                    );
                    body.extend(render_context(&al, &bl, &grouped_ops(&al, &bl, n, opts)));
                    body
                }
                _ => {
                    let params = build_params(a_name, b_name, Format::Context, n, 0);
                    let body = diffutilslib::context_diff(&a_bytes, &b_bytes, &params);
                    apply_header(
                        body,
                        Format::Context,
                        a_name,
                        b_name,
                        opts.label_a.as_deref(),
                        opts.label_b.as_deref(),
                        a_mtime,
                        b_mtime,
                    )
                }
            };
            out.write_all(&body)?;
            Ok(1)
        }
        OutFormat::SideBySide(w) => {
            let params = build_params(a_name, b_name, Format::SideBySide, 0, w);
            let mut buf = Vec::new();
            diffutilslib::side_by_side_diff(&a_bytes, &b_bytes, &mut buf, &params);
            out.write_all(&buf)?;
            Ok(1)
        }
        OutFormat::Ed => {
            // Ed format can't express a missing trailing newline; the library's `ed_diff`
            // refuses outright (`Err(DiffError::MissingNL)`) rather than producing a script.
            // GNU still emits the script and separately warns, so pad a local copy for the
            // library call and detect the real condition ourselves for the warning/exit code.
            fn padded(v: &[u8]) -> Vec<u8> {
                let mut v = v.to_vec();
                if !v.is_empty() && *v.last().unwrap() != b'\n' {
                    v.push(b'\n');
                }
                v
            }
            let body = match (text_lines(&a_bytes), text_lines(&b_bytes)) {
                // Rendered here, not by `diffutilslib::ed_diff`, whose hunks come out in
                // forward (top-to-bottom) file order with each one's line numbers computed as
                // if *earlier* hunks had already shifted the file -- wrong for ed format, whose
                // numbers are always against the untouched original (that's the whole reason
                // GNU applies them bottom-up: an earlier hunk's line numbers must stay valid
                // until an unapplied later one no longer needs them). `grouped_ops`' ranges are
                // computed fresh per hunk against the fixed original indices, so this has
                // neither bug: `render_ed` only has to emit them in reverse.
                (Some(al), Some(bl)) => {
                    // Ed format can't express a missing final newline as its own hunk (that's
                    // the whole reason for the `padded`/`missing` handling around this match);
                    // a last line differing *only* by that from its counterpart has to compare
                    // equal here, not as an edit -- so pad a synthetic trailing newline onto it
                    // before diffing. `render_ed` already normalizes every line it writes to end
                    // in exactly one `\n` regardless, so this doesn't change any actual output.
                    fn pad_last(lines: &[&str]) -> Vec<String> {
                        let mut v: Vec<String> = lines.iter().map(|s| (*s).to_owned()).collect();
                        if let Some(last) = v.last_mut()
                            && !last.ends_with('\n')
                        {
                            last.push('\n');
                        }
                        v
                    }
                    let al = pad_last(&al);
                    let bl = pad_last(&bl);
                    let ak: Vec<_> = al.iter().map(|l| comparison_key(l, opts)).collect();
                    let bk: Vec<_> = bl.iter().map(|l| comparison_key(l, opts)).collect();
                    let ops = similar::capture_diff_slices(Algorithm::Myers, &ak, &bk);
                    let bl_refs: Vec<&str> = bl.iter().map(String::as_str).collect();
                    render_ed(&bl_refs, &similar::group_diff_ops(ops, 0))
                }
                // Non-UTF-8: still goes through the library engine (see `grouped_ops`'s own
                // comment on the same tradeoff for unified format), just with its forward hunk
                // order corrected; a further multi-hunk numbering gap here is accepted rather
                // than chased, same as that one.
                _ => {
                    let params = build_params(a_name, b_name, Format::Ed, 0, 0);
                    let body = diffutilslib::ed_diff(&padded(&a_bytes), &padded(&b_bytes), &params)
                        .unwrap_or_default();
                    reverse_ed_hunks(collapse_redundant_ranges(body))
                }
            };
            out.write_all(&body)?;
            let mut status = 1;
            if missing_trailing_newline(&a_bytes) {
                writeln!(err, "diff: {a_name}: No newline at end of file\n")?;
                status = 2;
            }
            if missing_trailing_newline(&b_bytes) {
                writeln!(err, "diff: {b_name}: No newline at end of file\n")?;
                status = 2;
            }
            Ok(status)
        }
    }
}

fn list_dir_names(dir: &Path) -> std::io::Result<std::collections::BTreeSet<String>> {
    let mut names = std::collections::BTreeSet::new();
    for entry in std::fs::read_dir(dir)? {
        names.insert(entry?.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

#[allow(clippy::too_many_arguments)]
/// gnulib shell-quote a word for GNU's `-r` header echo (verified against the oracle): plain when
/// nothing needs escaping, `"..."` when the only special character is an apostrophe and none of
/// `` $ ` " \ ! `` appear, else `'...'` with embedded control characters spliced out as
/// `$'...'` ANSI-C escapes.
fn shell_escape_quote(word: &str) -> String {
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
        ) || c.is_control()
            || (index == 0 && matches!(c, '#' | '~'))
    };
    if !word.chars().enumerate().any(special) && word != "{" && word != "}" {
        return word.to_string();
    }
    let control = word.chars().any(char::is_control);
    if !control && word.contains('\'') && !word.contains(['$', '`', '"', '\\', '!']) {
        return format!("\"{word}\"");
    }
    let mut out = String::new();
    let mut open = false;
    for c in word.chars() {
        if c.is_control() {
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
                other => format!("\\{:03o}", u32::from(other)),
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

/// GNU's `-r` header line before a differing pair: `diff` followed by every original argv token
/// that wasn't one of the two top-level operands (verbatim, in order — never a reconstruction of
/// the parsed flags: `-ruN` stays `-ruN`, `-r -U 5` stays three separate tokens), then the pair
/// being compared — except when `--label` is given, which is global (one label overrides every
/// file pair's displayed name for the whole walk, not just the top-level one), so GNU echoes the
/// label text itself in that trailing position instead of the real per-file path (verified
/// against the oracle: `-r --label x --label y` on files that aren't even named `x`/`y` still
/// echoes `... x y`, not the real paths). Each part is shell-quoted only where needed. Verified
/// against the oracle for `-ruN`, `-Naur`, `-r -u -N`, `--recursive --unified`, `-r -U 5`,
/// `-rU5`, `-r --label x --label y`, and `-ru --color=never`.
fn build_recursive_header(opts: &DiffOptions, da: &str, db: &str) -> String {
    let mut parts: Vec<String> = vec!["diff".to_owned()];
    parts.extend(opts.option_tokens.iter().map(|t| shell_escape_quote(t)));
    parts.push(shell_escape_quote(opts.label_a.as_deref().unwrap_or(da)));
    parts.push(shell_escape_quote(opts.label_b.as_deref().unwrap_or(db)));
    parts.join(" ")
}

fn walk_pair(
    a_disp: &str,
    b_disp: &str,
    stdin: &mut dyn Read,
    resolve: &dyn Fn(&str) -> PathBuf,
    opts: &DiffOptions,
    recurse: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
    status: &mut i32,
) -> std::io::Result<()> {
    let real_a = resolve(a_disp);
    let real_b = resolve(b_disp);
    let names_a = list_dir_names(&real_a)?;
    let names_b = list_dir_names(&real_b)?;

    for name in names_a.union(&names_b) {
        let in_a = names_a.contains(name);
        let in_b = names_b.contains(name);
        let da = format!("{a_disp}/{name}");
        let db = format!("{b_disp}/{name}");
        match (in_a, in_b) {
            // Under `-N`, a file present on only one side is diffed against an empty file
            // (so the resulting patch can actually create/delete it) instead of being reported
            // as "Only in ..." — a directory present on only one side still gets "Only in ...",
            // matching GNU (which recurses to report every file under it individually).
            (true, false) if opts.new_file && !real_a.join(name).is_dir() => {
                let prefix = build_recursive_header(opts, &da, &db);
                let code = compare_pair(&da, &db, stdin, resolve, opts, Some(&prefix), out, err)?;
                *status = (*status).max(code);
            }
            (false, true) if opts.new_file && !real_b.join(name).is_dir() => {
                let prefix = build_recursive_header(opts, &da, &db);
                let code = compare_pair(&da, &db, stdin, resolve, opts, Some(&prefix), out, err)?;
                *status = (*status).max(code);
            }
            (true, false) => {
                writeln!(out, "Only in {a_disp}: {name}")?;
                *status = (*status).max(1);
            }
            (false, true) => {
                writeln!(out, "Only in {b_disp}: {name}")?;
                *status = (*status).max(1);
            }
            (true, true) => {
                let pa = real_a.join(name);
                let pb = real_b.join(name);
                match (pa.is_dir(), pb.is_dir()) {
                    (true, true) if recurse => {
                        walk_pair(&da, &db, stdin, resolve, opts, recurse, out, err, status)?;
                    }
                    (true, true) => {
                        writeln!(out, "Common subdirectories: {da} and {db}")?;
                    }
                    (false, false) => {
                        let prefix = build_recursive_header(opts, &da, &db);
                        let code =
                            compare_pair(&da, &db, stdin, resolve, opts, Some(&prefix), out, err)?;
                        *status = (*status).max(code);
                    }
                    (a_is_dir, _) => {
                        let (dir_name, file_name) = if a_is_dir { (&da, &db) } else { (&db, &da) };
                        writeln!(
                            out,
                            "File {dir_name} is a directory while file {file_name} is a regular file"
                        )?;
                        *status = (*status).max(1);
                    }
                }
            }
            (false, false) => unreachable!("name came from the union of names_a and names_b"),
        }
    }
    Ok(())
}

pub(crate) fn run_diff(
    argv: &[String],
    stdin: &mut dyn Read,
    resolve: &dyn Fn(&str) -> PathBuf,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> std::io::Result<i32> {
    let (opts, [a, b]) = match parse_diff(argv) {
        Ok(v) => v,
        Err(r) => {
            err.write_all(r.message.as_bytes())?;
            return Ok(r.code);
        }
    };

    // GNU's own shortcut: two stdins are trivially identical without reading either.
    if a == "-" && b == "-" {
        if opts.report_identical {
            writeln!(out, "Files - and - are identical")?;
        }
        return Ok(0);
    }

    let pa = resolve(&a);
    let pb = resolve(&b);
    // GNU's usage line spells out all four shapes it accepts: `'FILE1 FILE2' or 'DIR1 DIR2' or
    // 'DIR FILE' or 'FILE DIR'` — none of them need `-r`. Without it, `DIR1 DIR2` only compares
    // the top level (a name common to both that's itself a directory becomes a "Common
    // subdirectories" line, not a recursive descent), and `DIR FILE`/`FILE DIR` reduces to an
    // ordinary two-file compare against `DIR/basename(FILE)`. Verified against the oracle for
    // all three (a bare `diff DIR FILE` previously tried to read the directory as a file and
    // failed with a raw I/O error instead of doing either).
    match (pa.is_dir(), pb.is_dir()) {
        (true, true) => {
            let mut status = 0;
            walk_pair(
                &a,
                &b,
                stdin,
                resolve,
                &opts,
                opts.recursive,
                out,
                err,
                &mut status,
            )?;
            return Ok(status);
        }
        (true, false) => {
            let base = Path::new(&b)
                .file_name()
                .map_or_else(|| b.clone(), |n| n.to_string_lossy().into_owned());
            let da = format!("{a}/{base}");
            return compare_pair(&da, &b, stdin, resolve, &opts, None, out, err);
        }
        (false, true) => {
            let base = Path::new(&a)
                .file_name()
                .map_or_else(|| a.clone(), |n| n.to_string_lossy().into_owned());
            let db = format!("{b}/{base}");
            return compare_pair(&a, &db, stdin, resolve, &opts, None, out, err);
        }
        (false, false) => {}
    }

    compare_pair(&a, &b, stdin, resolve, &opts, None, out, err)
}

// ---------------------------------------------------------------------------------------------
// cmp
// ---------------------------------------------------------------------------------------------

struct CmpOptions {
    verbose: bool,
    quiet: bool,
    bytes: bool,
    skip_a: u64,
    skip_b: u64,
    /// `-n LIMIT`/`--bytes=LIMIT`: compare at most this many bytes.
    limit: Option<u64>,
}

/// GNU `cmp -b`/`--print-bytes`'s name for one byte: caret notation for a C0 control (`^@`..`^_`,
/// with DEL as `^?`), `M-` prefixed onto the same naming for the high bit set (byte - 128), and
/// the literal character otherwise (verified against the oracle across every byte 0-255: this is
/// the same table as `cat -v`'s, in `CatState::byte` in `streaming.rs`, just returning a `String`
/// instead of pushing to `cat`'s output buffer since `cmp` needs it as a separate field to pad).
fn cmp_char_name(mut byte: u8) -> String {
    let mut name = String::new();
    if byte >= 128 {
        name.push_str("M-");
        byte -= 128;
    }
    if byte < 32 {
        name.push('^');
        name.push((byte + 64) as char);
    } else if byte == 127 {
        name.push_str("^?");
    } else {
        name.push(byte as char);
    }
    name
}

/// A suffix multiplier for `--ignore-initial`'s `SKIP` values — the common binary/decimal ones;
/// GNU also accepts `T`/`P`/`E` scale, left out here as genuinely not cheap (`cmp` skip amounts
/// in the kB-MB range cover every realistic bash-tool use).
fn skip_suffix_multiplier(suffix: &str) -> Option<u64> {
    match suffix {
        "" => Some(1),
        "K" => Some(1024),
        "kB" => Some(1000),
        "M" => Some(1024 * 1024),
        "MB" => Some(1_000_000),
        "G" => Some(1024 * 1024 * 1024),
        "GB" => Some(1_000_000_000),
        _ => None,
    }
}

/// Parses one `--ignore-initial`/positional `SKIP` value. GNU routes both call sites through the
/// same parser and the same diagnostic (verified against the oracle: `cmp f f f`, where the
/// third operand is a positional `SKIP1`, reports `invalid --ignore-initial value` — not
/// anything mentioning a positional argument).
fn parse_skip_value(cmd: &str, text: &str) -> Result<u64, Refusal> {
    let bad = || {
        usage_error(
            cmd,
            format!("invalid --ignore-initial value {}", fancy_quote(text)),
        )
    };
    let digit_end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, suffix) = text.split_at(digit_end);
    if digits.is_empty() {
        return Err(bad());
    }
    let n: u64 = digits.parse().map_err(|_| bad())?;
    let mult = skip_suffix_multiplier(suffix).ok_or_else(bad)?;
    Ok(n.saturating_mul(mult))
}

/// `SKIP` or `SKIP1:SKIP2` (colon splits independent skips for each file; no colon skips both
/// files by the same amount — verified against the oracle: `-i 3` skips both sides, `-i 3:0`
/// skips only the first).
fn parse_skip_pair(cmd: &str, text: &str) -> Result<(u64, u64), Refusal> {
    if let Some((a, b)) = text.split_once(':') {
        Ok((parse_skip_value(cmd, a)?, parse_skip_value(cmd, b)?))
    } else {
        let n = parse_skip_value(cmd, text)?;
        Ok((n, n))
    }
}

/// `cmp [OPTION]... FILE1 [FILE2 [SKIP1 [SKIP2]]]` — unlike `diff`, a missing `FILE2` defaults to
/// `-` (stdin) rather than erroring (verified against the oracle: `cmp f` with no piped stdin
/// immediately reports EOF on `-`, not a usage error). `-i`/`--ignore-initial` overrides and
/// disables the positional `SKIP1`/`SKIP2` entirely, matching GNU.
#[allow(clippy::too_many_lines)]
fn parse_cmp(argv: &[String]) -> Result<(CmpOptions, [String; 2]), Refusal> {
    let cmd = "cmp";
    let args = &argv[1..];
    let mut verbose = false;
    let mut quiet = false;
    let mut print_bytes = false;
    let mut skip_flag: Option<(u64, u64)> = None;
    let mut limit: Option<u64> = None;
    let mut operands: Vec<String> = Vec::new();
    let mut end_of_opts = false;

    let mut i = 0;
    while i < args.len() {
        let a = args[i].clone();
        if end_of_opts || a == "-" || !a.starts_with('-') {
            operands.push(a);
            i += 1;
            continue;
        }
        if a == "--" {
            end_of_opts = true;
            i += 1;
            continue;
        }
        if let Some(long) = a.strip_prefix("--") {
            match long {
                "verbose" => verbose = true,
                "quiet" | "silent" => quiet = true,
                "print-bytes" => print_bytes = true,
                "ignore-initial" => {
                    i += 1;
                    let value = args.get(i).cloned().ok_or_else(|| {
                        usage_error(
                            cmd,
                            "option '--ignore-initial' requires an argument".to_owned(),
                        )
                    })?;
                    skip_flag = Some(parse_skip_pair(cmd, &value)?);
                }
                _ if long.starts_with("ignore-initial=") => {
                    skip_flag = Some(parse_skip_pair(cmd, &long["ignore-initial=".len()..])?);
                }
                "bytes" => {
                    i += 1;
                    let value = args.get(i).cloned().ok_or_else(|| {
                        usage_error(cmd, "option '--bytes' requires an argument".to_owned())
                    })?;
                    limit = Some(parse_bytes_limit(cmd, &value)?);
                }
                _ if long.starts_with("bytes=") => {
                    limit = Some(parse_bytes_limit(cmd, &long["bytes=".len()..])?);
                }
                _ => return Err(usage_error(cmd, format!("unrecognized option '{a}'"))),
            }
            i += 1;
            continue;
        }
        let bytes = a.as_bytes();
        let mut ci = 1;
        while ci < bytes.len() {
            let c = bytes[ci] as char;
            match c {
                'l' => verbose = true,
                's' | 'q' => quiet = true,
                'n' => {
                    let value = if ci + 1 < bytes.len() {
                        a[ci + 1..].to_owned()
                    } else {
                        i += 1;
                        args.get(i).cloned().ok_or_else(|| {
                            usage_error(cmd, "option requires an argument -- 'n'".to_owned())
                        })?
                    };
                    limit = Some(parse_bytes_limit(cmd, &value)?);
                    ci = bytes.len();
                }
                'b' => print_bytes = true,
                'i' => {
                    let value = if ci + 1 < bytes.len() {
                        a[ci + 1..].to_owned()
                    } else {
                        i += 1;
                        args.get(i).cloned().ok_or_else(|| {
                            usage_error(cmd, "option requires an argument -- 'i'".to_owned())
                        })?
                    };
                    skip_flag = Some(parse_skip_pair(cmd, &value)?);
                    ci = bytes.len();
                }
                _ => return Err(usage_error(cmd, format!("invalid option -- '{c}'"))),
            }
            ci += 1;
        }
        i += 1;
    }
    if verbose && quiet {
        // Verified against the oracle: this gets the same two-line usage shape as any other
        // bad-argument diagnostic, not a bare one-liner.
        return Err(usage_error(
            cmd,
            "options -l and -s are incompatible".to_owned(),
        ));
    }
    if operands.is_empty() {
        return Err(usage_error(
            cmd,
            format!("missing operand after {}", fancy_quote(cmd)),
        ));
    }
    if operands.len() > 4 {
        return Err(usage_error(
            cmd,
            format!("extra operand {}", fancy_quote(&operands[4])),
        ));
    }
    let file1 = operands[0].clone();
    let file2 = operands.get(1).cloned().unwrap_or_else(|| "-".to_owned());
    let (skip_a, skip_b) = if let Some(pair) = skip_flag {
        pair
    } else {
        let skip_a = match operands.get(2) {
            Some(s) => parse_skip_value(cmd, s)?,
            None => 0,
        };
        let skip_b = match operands.get(3) {
            Some(s) => parse_skip_value(cmd, s)?,
            None => 0,
        };
        (skip_a, skip_b)
    };
    Ok((
        CmpOptions {
            verbose,
            quiet,
            bytes: print_bytes,
            skip_a,
            skip_b,
            limit,
        },
        [file1, file2],
    ))
}

/// `-n LIMIT`/`--bytes=LIMIT`: the same suffix scale as `--ignore-initial`, but GNU's own
/// message for a bad one uses straight quotes, not the curly ones every other cmp/diff
/// diagnostic does — verified against the oracle (`cmp: invalid --bytes value 'x'`).
fn parse_bytes_limit(cmd: &str, text: &str) -> Result<u64, Refusal> {
    let bad = || usage_error(cmd, format!("invalid --bytes value '{text}'"));
    let digit_end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, suffix) = text.split_at(digit_end);
    if digits.is_empty() {
        return Err(bad());
    }
    let n: u64 = digits.parse().map_err(|_| bad())?;
    let mult = skip_suffix_multiplier(suffix).ok_or_else(bad)?;
    Ok(n.saturating_mul(mult))
}

fn read_whole(
    name: &str,
    resolve: &dyn Fn(&str) -> PathBuf,
    stdin: &mut dyn Read,
) -> Result<Vec<u8>, std::io::Error> {
    if name == "-" {
        let mut buf = Vec::new();
        stdin.read_to_end(&mut buf)?;
        return Ok(buf);
    }
    super::read_file(&resolve(name))
}

/// Verified verbatim against the oracle.
const CMP_HELP: &str = "Usage: cmp [OPTION]... FILE1 [FILE2 [SKIP1 [SKIP2]]]\n\
Compare two files byte by byte.\n\
\n\
The optional SKIP1 and SKIP2 specify the number of bytes to skip\n\
at the beginning of each file (zero by default).\n\
\n\
Mandatory arguments to long options are mandatory for short options too.\n\
  -b, --print-bytes          print differing bytes\n\
  -i, --ignore-initial=SKIP         skip first SKIP bytes of both inputs\n\
  -i, --ignore-initial=SKIP1:SKIP2  skip first SKIP1 bytes of FILE1 and\n\
                                      first SKIP2 bytes of FILE2\n\
  -l, --verbose              output byte numbers and differing byte values\n\
  -n, --bytes=LIMIT          compare at most LIMIT bytes\n\
  -s, --quiet, --silent      suppress all normal output\n\
      --help                 display this help and exit\n\
  -v, --version              output version information and exit\n\
\n\
SKIP values may be followed by the following multiplicative suffixes:\n\
kB 1000, K 1024, MB 1,000,000, M 1,048,576,\n\
GB 1,000,000,000, G 1,073,741,824, and so on for T, P, E, Z, Y.\n\
\n\
If a FILE is '-' or missing, read standard input.\n\
Exit status is 0 if inputs are the same, 1 if different, 2 if trouble.\n\
\n\
Report bugs to: bug-diffutils@gnu.org\n\
GNU diffutils home page: <https://www.gnu.org/software/diffutils/>\n\
General help using GNU software: <https://www.gnu.org/gethelp/>\n";

/// GNU quotes a filename in straight single quotes only when it needs disambiguating (empty,
/// here) — plain names are left bare, unlike the curly-quoted option diagnostics elsewhere in
/// this file. Verified against the oracle for an empty operand.
fn quote_name(name: &str) -> String {
    if name.is_empty() {
        "''".to_owned()
    } else {
        name.to_owned()
    }
}

pub(crate) fn run_cmp(
    argv: &[String],
    stdin: &mut dyn Read,
    resolve: &dyn Fn(&str) -> PathBuf,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> std::io::Result<i32> {
    if argv[1..].iter().any(|a| a == "--help") {
        out.write_all(CMP_HELP.as_bytes())?;
        return Ok(0);
    }

    let (opts, [a_name, b_name]) = match parse_cmp(argv) {
        Ok(v) => v,
        Err(r) => {
            err.write_all(r.message.as_bytes())?;
            return Ok(r.code);
        }
    };

    // GNU shortcuts a self-comparison (both operands resolving to the same path) to "identical"
    // without opening either — verified against the oracle: comparing a directory to itself
    // succeeds silently, though opening one to actually read it would fail.
    if a_name != "-" && b_name != "-" && resolve(&a_name) == resolve(&b_name) {
        return Ok(0);
    }

    let a_full = match read_whole(&a_name, resolve, stdin) {
        Ok(v) => v,
        Err(e) => {
            if !opts.quiet {
                writeln!(
                    err,
                    "cmp: {}: {}",
                    quote_name(&a_name),
                    super::io_message(&e)
                )?;
            }
            return Ok(2);
        }
    };
    let b_full = match read_whole(&b_name, resolve, stdin) {
        Ok(v) => v,
        Err(e) => {
            if !opts.quiet {
                writeln!(
                    err,
                    "cmp: {}: {}",
                    quote_name(&b_name),
                    super::io_message(&e)
                )?;
            }
            return Ok(2);
        }
    };
    // `SKIP1`/`SKIP2` bytes are skipped before comparison begins — the "char"/"line" counters
    // that follow are relative to the first *compared* byte, not the file's real start
    // (verified against the oracle: `cmp -i 3:0 f g` on a mismatch right after the skip point
    // reports "char 1, line 1").
    let a_bytes = &a_full[(opts.skip_a as usize).min(a_full.len())..];
    let b_bytes = &b_full[(opts.skip_b as usize).min(b_full.len())..];

    let min_len = a_bytes
        .len()
        .min(b_bytes.len())
        .min(opts.limit.map_or(usize::MAX, |l| l as usize));
    // GNU right-justifies the `-l` byte-offset column to the width of the largest offset it
    // could possibly report, i.e. the digit count of the number of bytes actually compared —
    // not the width of any offset that happens to differ (verified against the oracle: a
    // single difference at byte 3 in a 4-byte comparison prints unpadded "3 ...", the same
    // difference in a 13-byte comparison prints padded " 3 ...").
    let offset_width = min_len.max(1).to_string().len();
    let mut line = 1usize;
    let mut any_diff = false;

    for i in 0..min_len {
        if a_bytes[i] != b_bytes[i] {
            any_diff = true;
            if opts.verbose {
                if opts.bytes {
                    // `-b`'s char-name field is left-justified to 4 (the width of the longest
                    // possible name, "M-^?") plus its own separator space, so a short name like
                    // "A" still lines the octal columns up; verified against the oracle across
                    // every byte value.
                    writeln!(
                        out,
                        "{:>offset_width$} {:>3o} {:<4} {:>3o} {}",
                        i + 1,
                        a_bytes[i],
                        cmp_char_name(a_bytes[i]),
                        b_bytes[i],
                        cmp_char_name(b_bytes[i]),
                    )?;
                } else {
                    writeln!(
                        out,
                        "{:>offset_width$} {:>3o} {:>3o}",
                        i + 1,
                        a_bytes[i],
                        b_bytes[i]
                    )?;
                }
            } else {
                if !opts.quiet {
                    if opts.bytes {
                        // Unlike the `-l` table, this is a single line of prose: GNU pads the
                        // octal fields the same way but leaves the char names their natural
                        // width (verified against the oracle).
                        writeln!(
                            out,
                            "{a_name} {b_name} differ: byte {}, line {line} is {:>3o} {} {:>3o} {}",
                            i + 1,
                            a_bytes[i],
                            cmp_char_name(a_bytes[i]),
                            b_bytes[i],
                            cmp_char_name(b_bytes[i]),
                        )?;
                    } else {
                        writeln!(out, "{a_name} {b_name} differ: char {}, line {line}", i + 1)?;
                    }
                }
                return Ok(1);
            }
        }
        if a_bytes[i] == b'\n' {
            line += 1;
        }
    }

    // `-n LIMIT` only ever compares the first LIMIT bytes — a length difference past that point
    // is invisible to it, not just unreported (verified against the oracle: `-n 3` on files of
    // 6 and 3 bytes whose first 3 match is a silent, ordinary success, not even the "EOF on
    // FILE2" wording a plain, unlimited `cmp` gives that same pair).
    if opts.limit.is_none() && a_bytes.len() != b_bytes.len() {
        let (shorter_name, shorter_len) = if a_bytes.len() < b_bytes.len() {
            (&a_name, a_bytes.len())
        } else {
            (&b_name, b_bytes.len())
        };
        if !opts.quiet {
            if shorter_len == 0 {
                // "after byte 0" would be nonsensical — GNU special-cases a genuinely empty
                // shorter side (verified against the oracle: a bare `cmp f` with nothing on
                // stdin for the implicit FILE2).
                writeln!(
                    err,
                    "cmp: EOF on \u{2018}{shorter_name}\u{2019} which is empty"
                )?;
            } else if opts.verbose {
                writeln!(
                    err,
                    "cmp: EOF on \u{2018}{shorter_name}\u{2019} after byte {shorter_len}"
                )?;
            } else if a_bytes[shorter_len - 1] == b'\n' {
                // GNU: at a line boundary it names the last complete line ("line N"); mid-line
                // it names the partial line ("in line N").
                writeln!(
                    err,
                    "cmp: EOF on \u{2018}{shorter_name}\u{2019} after byte {shorter_len}, line {}",
                    line - 1
                )?;
            } else {
                writeln!(
                    err,
                    "cmp: EOF on \u{2018}{shorter_name}\u{2019} after byte {shorter_len}, in line {line}"
                )?;
            }
        }
        return Ok(1);
    }

    Ok(i32::from(any_diff))
}

// Every test here uses `tempfile`, a native-only dependency.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    // A single-line delete/insert/change range collapses to a bare number ("1d0", not "1,1d0")
    // in GNU's own normal-diff format — verified against the oracle. This specifically exercises
    // the `diffutilslib` fallback path (a file with invalid UTF-8 content, which `text_lines`
    // refuses, so the custom `render_normal`/`context_hunk_range` this fork uses for ordinary
    // text isn't what's under test here): `collapse_redundant_ranges`'s own post-processing used
    // to bail out and return the diff unmodified the moment any content line — not just the
    // command line it actually needs to look at — had an invalid byte anywhere in it.
    fn single_line_ranges_collapse_even_with_invalid_utf8_content() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), b"caf\xe9 ol\xe9\nzz\n").unwrap();
            std::fs::write(dir.join("b"), b"zz\n").unwrap();
            let argv: Vec<String> = ["diff", "a", "b"].into_iter().map(str::to_owned).collect();
            let resolve = |p: &str| dir.join(p);
            let mut out = Vec::new();
            let mut err = Vec::new();
            let code = run_diff(&argv, &mut &b""[..], &resolve, &mut out, &mut err).unwrap();
            assert_eq!(code, 1);
            assert_eq!(out, b"1d0\n< caf\xe9 ol\xe9\n");
            assert!(err.is_empty());
        });
    }

    fn run(f: impl FnOnce(&std::path::Path)) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        f(dir.path());
        dir
    }

    fn diff(dir: &std::path::Path, args: &[&str], stdin: &str) -> (i32, String, String) {
        let argv: Vec<String> = std::iter::once("diff")
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect();
        let resolve = |p: &str| dir.join(p);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_diff(&argv, &mut stdin.as_bytes(), &resolve, &mut out, &mut err).unwrap();
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn cmp(dir: &std::path::Path, args: &[&str], stdin: &str) -> (i32, String, String) {
        let argv: Vec<String> = std::iter::once("cmp")
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect();
        let resolve = |p: &str| dir.join(p);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_cmp(&argv, &mut stdin.as_bytes(), &resolve, &mut out, &mut err).unwrap();
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn identical_files_exit_zero_silent() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "same\n").unwrap();
            std::fs::write(dir.join("b"), "same\n").unwrap();
            assert_eq!(
                diff(dir, &["a", "b"], ""),
                (0, String::new(), String::new())
            );
        });
    }

    #[test]
    fn unified_default_context_three() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "1\n2\n3\n4\n5\n").unwrap();
            std::fs::write(dir.join("b"), "1\n2\nX\n4\n5\n").unwrap();
            let (code, out, _) = diff(dir, &["-u", "--label", "a", "--label", "b", "a", "b"], "");
            assert_eq!(code, 1);
            assert_eq!(
                out,
                "--- a\n+++ b\n@@ -1,5 +1,5 @@\n 1\n 2\n-3\n+X\n 4\n 5\n"
            );
        });
    }

    #[test]
    fn context_format_with_labels() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "one\n").unwrap();
            std::fs::write(dir.join("b"), "two\n").unwrap();
            let (code, out, _) = diff(dir, &["-c", "--label", "L", "--label", "R", "a", "b"], "");
            assert_eq!(code, 1);
            assert_eq!(
                out,
                "*** L\n--- R\n***************\n*** 1 ****\n! one\n--- 1 ----\n! two\n"
            );
        });
    }

    #[test]
    fn context_range_second_number_is_the_end_line_not_a_count() {
        // Verified against the oracle (`diff -C1` on a change at old lines 2-4): context
        // format's second range number is the 1-based *end* line ("2,4"), unlike unified
        // format's *count* ("2,3" would be unified's spelling of the same 3-line range) — a real
        // difference between the two formats, not a shared helper bug.
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "1\n2\n3\n4\n5\n").unwrap();
            std::fs::write(dir.join("b"), "1\n2\nX\n4\n5\n").unwrap();
            let (code, out, _) = diff(dir, &["-C1", "--label", "L", "--label", "R", "a", "b"], "");
            assert_eq!(code, 1);
            assert_eq!(
                out,
                "*** L\n--- R\n***************\n*** 2,4 ****\n  2\n! 3\n  4\n--- 2,4 ----\n  2\n! X\n  4\n"
            );
        });
    }

    #[test]
    fn normal_format_default() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "one\ntwo\n").unwrap();
            std::fs::write(dir.join("b"), "one\nTWO\n").unwrap();
            let (code, out, _) = diff(dir, &["a", "b"], "");
            assert_eq!(code, 1);
            assert_eq!(out, "2c2\n< two\n---\n> TWO\n");
        });
    }

    #[test]
    fn brief_and_report_identical() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "x\n").unwrap();
            std::fs::write(dir.join("b"), "y\n").unwrap();
            assert_eq!(
                diff(dir, &["-q", "a", "b"], ""),
                (1, "Files a and b differ\n".into(), String::new())
            );
            std::fs::write(dir.join("c"), "x\n").unwrap();
            assert_eq!(
                diff(dir, &["-s", "a", "c"], ""),
                (0, "Files a and c are identical\n".into(), String::new())
            );
        });
    }

    #[test]
    fn binary_files_differ() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), b"a\0b").unwrap();
            std::fs::write(dir.join("b"), b"a\0c").unwrap();
            assert_eq!(
                diff(dir, &["a", "b"], ""),
                (1, "Binary files a and b differ\n".into(), String::new())
            );
        });
    }

    #[test]
    fn new_file_treats_missing_as_empty() {
        let _d = run(|dir| {
            std::fs::write(dir.join("b"), "hello\n").unwrap();
            let (code, out, _) = diff(dir, &["-N", "nope", "b"], "");
            assert_eq!(code, 1);
            assert_eq!(out, "0a1\n> hello\n", "{out}");
        });
    }

    #[test]
    fn missing_file_without_new_file_errors() {
        let _d = run(|dir| {
            std::fs::write(dir.join("b"), "hello\n").unwrap();
            let (code, _, err) = diff(dir, &["nope", "b"], "");
            assert_eq!(code, 2);
            assert!(err.starts_with("diff: nope: "), "{err}");
        });
    }

    #[test]
    fn stdin_operand() {
        let _d = run(|dir| {
            std::fs::write(dir.join("b"), "hello\n").unwrap();
            let (code, out, _) = diff(dir, &["-", "b"], "hello\n");
            assert_eq!((code, out.as_str()), (0, ""));
        });
    }

    fn numbers(from: usize, to: usize) -> String {
        (from..=to).map(|n| format!("{n}\n")).collect()
    }

    #[test]
    fn large_files_that_differ_throughout_diff_as_gnu_does() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), numbers(1, 30_000)).unwrap();
            std::fs::write(dir.join("b"), numbers(2, 30_001)).unwrap();
            let (code, out, _) = diff(dir, &["a", "b"], "");
            assert_eq!(
                (code, out.as_str()),
                (1, "1d0\n< 1\n30000a30000\n> 30001\n")
            );
        });
    }

    #[test]
    fn formats_with_a_quadratic_engine_refuse_large_inputs() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), numbers(1, 30_000)).unwrap();
            std::fs::write(dir.join("b"), numbers(2, 30_001)).unwrap();
            for (flag, feature) in [("-e", "-e"), ("-y", "-y")] {
                let (code, out, err) = diff(dir, &[flag, "a", "b"], "");
                assert_eq!((code, out.as_str()), (2, ""), "{flag}");
                assert_eq!(
                    err,
                    format!("diff: {feature} on files this large is unsupported in bash-tool\n")
                );
            }
        });
    }

    #[test]
    fn ignore_blank_lines_still_refuses_cleanly() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "x\n").unwrap();
            std::fs::write(dir.join("b"), "y\n").unwrap();
            let (code, _, err) = diff(dir, &["-B", "a", "b"], "");
            assert_eq!(code, 2);
            assert_eq!(err, "diff: -B is unsupported in bash-tool\n");
        });
    }

    #[test]
    fn ignore_case_treats_differently_cased_lines_as_identical() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "Hello World\n").unwrap();
            std::fs::write(dir.join("b"), "hello world\n").unwrap();
            assert_eq!(
                diff(dir, &["-i", "a", "b"], ""),
                (0, String::new(), String::new())
            );
        });
    }

    #[test]
    fn ignore_all_space_removes_every_whitespace_character() {
        // Verified against the oracle: `-w` treats "hello world" and "helloworld" as equal.
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "hello world\n").unwrap();
            std::fs::write(dir.join("b"), "helloworld\n").unwrap();
            assert_eq!(
                diff(dir, &["-w", "a", "b"], ""),
                (0, String::new(), String::new())
            );
        });
    }

    #[test]
    fn ignore_space_change_collapses_amount_but_not_presence() {
        // Verified against the oracle: `-b` treats "  a" and "   a" (2 vs 3 leading spaces) as
        // equal, but " a" and "a" (leading space present vs absent) as different; trailing
        // whitespace is always ignored.
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "  x\n").unwrap();
            std::fs::write(dir.join("b"), "   x\n").unwrap();
            assert_eq!(
                diff(dir, &["-b", "a", "b"], ""),
                (0, String::new(), String::new())
            );

            std::fs::write(dir.join("c"), " x\n").unwrap();
            std::fs::write(dir.join("d"), "x\n").unwrap();
            let (code, _, _) = diff(dir, &["-b", "c", "d"], "");
            assert_eq!(code, 1);

            std::fs::write(dir.join("e"), "x  \n").unwrap();
            std::fs::write(dir.join("f"), "x\n").unwrap();
            assert_eq!(
                diff(dir, &["-b", "e", "f"], ""),
                (0, String::new(), String::new())
            );
        });
    }

    #[test]
    fn ignore_space_change_unified_context_line_uses_old_side_text() {
        // Verified against the oracle (`diff -bu`): the shared context line keeps the OLD
        // file's exact whitespace even though it compares equal to the new file's differently
        // spaced version.
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "a   b\nold\nc\n").unwrap();
            std::fs::write(dir.join("b"), "a b\nnew\nc\n").unwrap();
            let (code, out, _) = diff(dir, &["-bu", "--label", "L", "--label", "R", "a", "b"], "");
            assert_eq!(code, 1);
            assert_eq!(
                out,
                "--- L\n+++ R\n@@ -1,3 +1,3 @@\n a   b\n-old\n+new\n c\n"
            );
        });
    }

    #[test]
    fn ignore_case_default_normal_format() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "ONE\ntwo\nthree\n").unwrap();
            std::fs::write(dir.join("b"), "one\ntwo\nTHREE\n").unwrap();
            assert_eq!(
                diff(dir, &["-i", "a", "b"], ""),
                (0, String::new(), String::new())
            );
        });
    }

    #[test]
    fn recursive_walk_reports_only_in_and_diffs() {
        let _d = run(|dir| {
            std::fs::create_dir_all(dir.join("a/sub")).unwrap();
            std::fs::create_dir_all(dir.join("b/sub")).unwrap();
            std::fs::write(dir.join("a/common.txt"), "old\n").unwrap();
            std::fs::write(dir.join("b/common.txt"), "new\n").unwrap();
            std::fs::write(dir.join("a/onlya.txt"), "a\n").unwrap();
            std::fs::write(dir.join("b/onlyb.txt"), "b\n").unwrap();
            std::fs::write(dir.join("a/sub/f.txt"), "1\n").unwrap();
            std::fs::write(dir.join("b/sub/f.txt"), "2\n").unwrap();
            let (code, out, _) = diff(dir, &["-r", "a", "b"], "");
            assert_eq!(code, 1);
            assert_eq!(
                out,
                "diff -r a/common.txt b/common.txt\n1c1\n< old\n---\n> new\n\
                 Only in a: onlya.txt\nOnly in b: onlyb.txt\n\
                 diff -r a/sub/f.txt b/sub/f.txt\n1c1\n< 1\n---\n> 2\n"
            );
        });
    }

    #[test]
    fn dir_file_without_recursive_compares_against_dir_basename() {
        // `diff DIR FILE` and `diff FILE DIR` are valid without `-r` at all (GNU's own
        // usage line lists them) — they reduce to comparing FILE against DIR/basename(FILE).
        // Previously this tried to read the directory itself as a file and failed with a raw
        // I/O error. Verified against the oracle.
        let _d = run(|dir| {
            std::fs::create_dir_all(dir.join("dz")).unwrap();
            std::fs::write(dir.join("dz/f1"), "content\n").unwrap();
            std::fs::write(dir.join("f1"), "content2\n").unwrap();
            let (code, out, err) = diff(dir, &["dz", "f1"], "");
            assert_eq!((code, err.as_str()), (1, ""));
            assert_eq!(out, "1c1\n< content\n---\n> content2\n");

            let (code, out, err) = diff(dir, &["f1", "dz"], "");
            assert_eq!((code, err.as_str()), (1, ""));
            assert_eq!(out, "1c1\n< content2\n---\n> content\n");
        });
    }

    #[test]
    fn dir_dir_without_recursive_stays_at_the_top_level() {
        // `diff DIR1 DIR2` without `-r` only compares the top level — a subdirectory
        // common to both sides is reported, not descended into. Verified against the oracle.
        let _d = run(|dir| {
            std::fs::create_dir_all(dir.join("d1/sub")).unwrap();
            std::fs::create_dir_all(dir.join("d2/sub")).unwrap();
            std::fs::write(dir.join("d1/x"), "a\n").unwrap();
            std::fs::write(dir.join("d2/x"), "b\n").unwrap();
            std::fs::write(dir.join("d1/only1"), "c\n").unwrap();
            std::fs::write(dir.join("d1/sub/y"), "same\n").unwrap();
            std::fs::write(dir.join("d2/sub/y"), "same\n").unwrap();
            let (code, out, _) = diff(dir, &["d1", "d2"], "");
            assert_eq!(code, 1);
            assert_eq!(
                out,
                "Only in d1: only1\nCommon subdirectories: d1/sub and d2/sub\n\
                 diff d1/x d2/x\n1c1\n< a\n---\n> b\n"
            );
        });
    }

    #[test]
    fn diff_gaps_are_refused_not_misread_as_invalid() {
        // these are real GNU diff options we don't implement; they must say so
        // ("unsupported in bash-tool"), not fall into the generic "invalid option" catch-all
        // as if diff didn't recognize the letter at all.
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "1\n").unwrap();
            std::fs::write(dir.join("b"), "2\n").unwrap();
            for args in [
                ["-n"].as_slice(),
                &["-p"],
                &["-Ifoo"],
                &["-Ffoo"],
                &["--rcs"],
                &["--show-c-function"],
            ] {
                let mut full: Vec<&str> = args.to_vec();
                full.extend(["a", "b"]);
                let (code, _out, err) = diff(dir, &full, "");
                assert_eq!(code, 2, "{args:?} -> {err}");
                assert!(
                    err.contains("is unsupported in bash-tool"),
                    "{args:?} -> {err}"
                );
            }
        });
    }

    #[test]
    // -d/--minimal (an LCS-algorithm hint that can't change semantically correct output),
    // --speed-large-files (a pure performance hint) and --normal (explicitly requesting the
    // already-default format) used to be refused outright; GNU really accepts all three.
    // Verified against the oracle: none of them change this diff's own output.
    fn diff_accepted_no_op_options_still_produce_normal_output() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "1\n").unwrap();
            std::fs::write(dir.join("b"), "2\n").unwrap();
            let expected = (1, "1c1\n< 1\n---\n> 2\n".to_owned(), String::new());
            for args in [
                ["-d"].as_slice(),
                &["--minimal"],
                &["--speed-large-files"],
                &["--normal"],
            ] {
                let mut full: Vec<&str> = args.to_vec();
                full.extend(["a", "b"]);
                assert_eq!(diff(dir, &full, ""), expected, "{args:?}");
            }
        });
    }

    #[test]
    // `-b`/`--print-bytes` is a real, implemented feature (landed separately); only
    // `-n`/`--bytes` is still refused here.
    fn cmp_bytes_limit_bounds_the_comparison() {
        // `-n`/`--bytes` used to be refused outright; GNU really does implement it, verified
        // against the oracle for a plain limited compare, a limit past which a length
        // difference is invisible (not even reported), and a bundled `-n2` form.
        let _d = run(|dir| {
            std::fs::write(dir.join("f1"), "a").unwrap();
            std::fs::write(dir.join("f2"), "b").unwrap();
            assert_eq!(
                cmp(dir, &["-n2", "f1", "f2"], ""),
                (1, "f1 f2 differ: char 1, line 1\n".into(), String::new())
            );

            std::fs::write(dir.join("f3"), "abcdef").unwrap();
            std::fs::write(dir.join("f4"), "abc").unwrap();
            assert_eq!(
                cmp(dir, &["-n", "3", "f3", "f4"], ""),
                (0, String::new(), String::new())
            );
            assert_eq!(
                cmp(dir, &["--bytes=3", "f3", "f4"], ""),
                (0, String::new(), String::new())
            );
        });
    }

    #[test]
    fn cmp_default_message_and_identical() {
        let _d = run(|dir| {
            std::fs::write(dir.join("f1"), "aaaa").unwrap();
            std::fs::write(dir.join("f2"), "aaab").unwrap();
            assert_eq!(
                cmp(dir, &["f1", "f2"], ""),
                (1, "f1 f2 differ: char 4, line 1\n".into(), String::new())
            );
            assert_eq!(
                cmp(dir, &["f1", "f1"], ""),
                (0, String::new(), String::new())
            );
        });
    }

    #[test]
    // GNU shortcuts a self-comparison to "identical" without even opening either side —
    // verified against the oracle: comparing a directory to itself this way succeeds silently,
    // though opening one to actually read it as a file would fail with "Is a directory".
    fn cmp_same_resolved_path_is_identical_without_reading() {
        let _d = run(|dir| {
            std::fs::create_dir(dir.join("adir")).unwrap();
            assert_eq!(
                cmp(dir, &["adir", "adir"], ""),
                (0, String::new(), String::new())
            );
        });
    }

    #[test]
    fn cmp_help_prints_gnus_own_text() {
        let _d = run(|dir| {
            let (code, out, err) = cmp(dir, &["--help"], "");
            assert_eq!((code, err), (0, String::new()));
            assert!(out.starts_with("Usage: cmp [OPTION]... FILE1"), "{out}");
        });
    }

    #[test]
    // `-s`/`--quiet`/`--silent` suppresses *every* message, missing-file ones included —
    // verified against the oracle.
    fn cmp_quiet_suppresses_the_missing_file_error_too() {
        let _d = run(|dir| {
            assert_eq!(
                cmp(dir, &["-s", "nosuch", "nosuch2"], ""),
                (2, String::new(), String::new())
            );
        });
    }

    #[test]
    // GNU quotes an empty operand in straight single quotes so the message isn't just "cmp: :
    // ..." — verified against the oracle.
    fn cmp_empty_name_is_quoted() {
        let _d = run(|dir| {
            let (code, out, err) = cmp(dir, &["", "f1"], "");
            assert_eq!((code, out), (2, String::new()));
            assert!(err.starts_with("cmp: '': "), "{err}");
        });
    }

    #[test]
    fn cmp_verbose_lists_all_differences() {
        let _d = run(|dir| {
            std::fs::write(dir.join("f1"), "aaaa").unwrap();
            std::fs::write(dir.join("f2"), "aaab").unwrap();
            assert_eq!(
                cmp(dir, &["-l", "f1", "f2"], ""),
                (1, "4 141 142\n".into(), String::new())
            );
        });
    }

    #[test]
    fn cmp_silent_suppresses_output() {
        let _d = run(|dir| {
            std::fs::write(dir.join("f1"), "aaaa").unwrap();
            std::fs::write(dir.join("f2"), "aaab").unwrap();
            assert_eq!(
                cmp(dir, &["-s", "f1", "f2"], ""),
                (1, String::new(), String::new())
            );
        });
    }

    #[test]
    fn cmp_eof_message() {
        let _d = run(|dir| {
            std::fs::write(dir.join("short"), "aaa").unwrap();
            std::fs::write(dir.join("long"), "aaaa").unwrap();
            let (code, out, err) = cmp(dir, &["short", "long"], "");
            assert_eq!((code, out.as_str()), (1, ""));
            assert_eq!(
                err,
                "cmp: EOF on \u{2018}short\u{2019} after byte 3, in line 1\n"
            );
            // At a line boundary GNU names the last complete line without "in".
            std::fs::write(dir.join("lines"), "x\ny\n").unwrap();
            std::fs::write(dir.join("more"), "x\ny\nz").unwrap();
            let (_, _, err) = cmp(dir, &["lines", "more"], "");
            assert_eq!(
                err,
                "cmp: EOF on \u{2018}lines\u{2019} after byte 4, line 2\n"
            );
        });
    }

    #[test]
    fn bundled_short_options() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "x\n").unwrap();
            std::fs::write(dir.join("b"), "y\n").unwrap();
            // -rq bundled: -r (recursive, no-op on plain files) + -q (brief).
            assert_eq!(
                diff(dir, &["-rq", "a", "b"], ""),
                (1, "Files a and b differ\n".into(), String::new())
            );
            // -U3 bundled as a single token, and -C2 similarly.
            let (code, out, _) = diff(dir, &["-U3", "--label", "L", "--label", "R", "a", "b"], "");
            assert_eq!(code, 1);
            assert_eq!(out, "--- L\n+++ R\n@@ -1 +1 @@\n-x\n+y\n");
        });
    }

    #[test]
    fn unified_and_context_merge_hunks_like_gnu() {
        // Verified against the oracle: `diff -u`/`diff -c` merge two changes into one hunk
        // whenever the unchanged gap between them is small, rather than starting a new hunk at
        // every change the way diffutilslib's own (unfixed) formatters do.
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "one\ntwo\nthree\nfour\nfive\nsix\nseven\n").unwrap();
            std::fs::write(
                dir.join("b"),
                "one\ntwo\nTHREE\nfour\nfive\nsix\nseven\neight\n",
            )
            .unwrap();
            let (code, out, _) = diff(dir, &["-u", "--label", "L", "--label", "R", "a", "b"], "");
            assert_eq!(code, 1);
            assert_eq!(
                out,
                "--- L\n+++ R\n@@ -1,7 +1,8 @@\n one\n two\n-three\n+THREE\n four\n five\n six\n seven\n+eight\n"
            );

            let (code, out, _) = diff(dir, &["-c", "--label", "L", "--label", "R", "a", "b"], "");
            assert_eq!(code, 1);
            assert_eq!(
                out,
                "*** L\n--- R\n***************\n*** 1,7 ****\n  one\n  two\n! three\n  four\n  five\n  six\n  seven\n--- 1,8 ----\n  one\n  two\n! THREE\n  four\n  five\n  six\n  seven\n+ eight\n"
            );
        });
    }

    #[test]
    fn unified_gap_boundary_2n_merges_2n_plus_1_splits() {
        // Verified against the oracle with -U3 (n=3, so 2n=6): a gap of 6 unchanged lines
        // between two changes stays one hunk; a gap of 7 splits into two.
        fn build(gap: usize) -> (String, String) {
            let mut a = String::from("p1\np2\np3\nX1\n");
            let mut b = String::from("p1\np2\np3\nY1\n");
            for i in 0..gap {
                a.push_str(&format!("g{i}\n"));
                b.push_str(&format!("g{i}\n"));
            }
            a.push_str("X2\ns1\ns2\ns3\n");
            b.push_str("Y2\ns1\ns2\ns3\n");
            (a, b)
        }
        let _d = run(|dir| {
            let (a, b) = build(6);
            std::fs::write(dir.join("a"), a).unwrap();
            std::fs::write(dir.join("b"), b).unwrap();
            let (code, out, _) = diff(dir, &["-U3", "--label", "L", "--label", "R", "a", "b"], "");
            assert_eq!(code, 1);
            let hunk_headers: Vec<&str> = out.lines().filter(|l| l.starts_with("@@")).collect();
            assert_eq!(hunk_headers, vec!["@@ -1,14 +1,14 @@"], "{out}");
        });
        let _d = run(|dir| {
            let (a, b) = build(7);
            std::fs::write(dir.join("a"), a).unwrap();
            std::fs::write(dir.join("b"), b).unwrap();
            let (code, out, _) = diff(dir, &["-U3", "--label", "L", "--label", "R", "a", "b"], "");
            assert_eq!(code, 1);
            let hunk_headers: Vec<&str> = out.lines().filter(|l| l.starts_with("@@")).collect();
            assert_eq!(
                hunk_headers,
                vec!["@@ -1,7 +1,7 @@", "@@ -9,7 +9,7 @@"],
                "{out}"
            );
        });
    }

    #[test]
    fn change_at_start_and_end_with_missing_trailing_newline() {
        let _d = run(|dir| {
            std::fs::write(dir.join("a"), "FIRST\nmid1\nmid2\nLAST").unwrap();
            std::fs::write(dir.join("b"), "CHANGED\nmid1\nmid2\nEND").unwrap();
            let (code, out, _) = diff(dir, &["-u", "--label", "L", "--label", "R", "a", "b"], "");
            assert_eq!(code, 1);
            assert_eq!(
                out,
                "--- L\n+++ R\n@@ -1,4 +1,4 @@\n-FIRST\n+CHANGED\n mid1\n mid2\n-LAST\n\\ No newline at end of file\n+END\n\\ No newline at end of file\n"
            );
        });
    }

    #[test]
    fn shell_escape_quote_matches_gnulib_rules() {
        assert_eq!(shell_escape_quote("plain"), "plain");
        assert_eq!(shell_escape_quote(""), "''");
        assert_eq!(shell_escape_quote("--color=never"), "'--color=never'");
        assert_eq!(shell_escape_quote("it's"), "\"it's\"");
        assert_eq!(shell_escape_quote("a'$b"), "'a'\\''$b'");
        assert_eq!(shell_escape_quote("{"), "'{'");
    }

    #[test]
    fn recursive_header_echoes_original_tokens_verbatim() {
        // Verified against the oracle for every one of these exact argv shapes.
        let _d = run(|dir| {
            std::fs::create_dir_all(dir.join("o")).unwrap();
            std::fs::create_dir_all(dir.join("n")).unwrap();
            std::fs::write(dir.join("o/f"), "one\n").unwrap();
            std::fs::write(dir.join("n/f"), "two\n").unwrap();

            let (_, out, _) = diff(dir, &["-ruN", "o", "n"], "");
            assert!(out.starts_with("diff -ruN o/f n/f\n"), "{out}");

            let (_, out, _) = diff(dir, &["-Naur", "o", "n"], "");
            assert!(out.starts_with("diff -Naur o/f n/f\n"), "{out}");

            let (_, out, _) = diff(dir, &["-r", "-u", "-N", "o", "n"], "");
            assert!(out.starts_with("diff -r -u -N o/f n/f\n"), "{out}");

            let (_, out, _) = diff(dir, &["--recursive", "--unified", "o", "n"], "");
            assert!(
                out.starts_with("diff --recursive --unified o/f n/f\n"),
                "{out}"
            );

            let (_, out, _) = diff(dir, &["-r", "-U", "5", "o", "n"], "");
            assert!(out.starts_with("diff -r -U 5 o/f n/f\n"), "{out}");

            let (_, out, _) = diff(dir, &["-rU5", "o", "n"], "");
            assert!(out.starts_with("diff -rU5 o/f n/f\n"), "{out}");

            let (_, out, _) = diff(dir, &["-ru", "--color=never", "o", "n"], "");
            assert!(
                out.starts_with("diff -ru '--color=never' o/f n/f\n"),
                "{out}"
            );
        });
    }

    #[test]
    fn diff_invalid_context_and_width_diagnostics() {
        // Verified against the oracle: `diff -U -1 f f`, `-C -1`, `-C x`, `-y -W 0`, `-y -W -1`.
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "a\n").unwrap();
            for args in [["-U", "-1"], ["-C", "-1"], ["-C", "x"]] {
                let (code, out, err) = diff(dir, &[args[0], args[1], "f", "f"], "");
                assert_eq!((code, out.as_str()), (2, ""));
                assert_eq!(
                    err,
                    format!(
                        "diff: invalid context length \u{2018}{}\u{2019}\ndiff: Try 'diff --help' for more information.\n",
                        args[1]
                    ),
                    "{args:?}"
                );
            }
            let (code, _, err) = diff(dir, &["-y", "-W", "0", "f", "f"], "");
            assert_eq!(code, 2);
            assert_eq!(
                err,
                "diff: invalid width \u{2018}0\u{2019}\ndiff: Try 'diff --help' for more information.\n"
            );
        });
    }

    #[test]
    fn diff_operand_count_diagnostics() {
        // Verified against the oracle: `diff` (0 operands), `diff f` (1), `diff f f f` (3+).
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "a\n").unwrap();
            let (code, _, err) = diff(dir, &[], "");
            assert_eq!(code, 2);
            assert_eq!(
                err,
                "diff: missing operand after \u{2018}diff\u{2019}\ndiff: Try 'diff --help' for more information.\n"
            );

            let (code, _, err) = diff(dir, &["f"], "");
            assert_eq!(code, 2);
            assert_eq!(
                err,
                "diff: missing operand after \u{2018}f\u{2019}\ndiff: Try 'diff --help' for more information.\n"
            );

            let (code, _, err) = diff(dir, &["f", "f", "f"], "");
            assert_eq!(code, 2);
            assert_eq!(
                err,
                "diff: extra operand \u{2018}f\u{2019}\ndiff: Try 'diff --help' for more information.\n"
            );
        });
    }

    #[test]
    fn diff_unknown_option_diagnostics() {
        // Verified against the oracle: `diff --bogus`, `diff -K`, `diff -U` (missing argument).
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "a\n").unwrap();
            let (code, _, err) = diff(dir, &["--bogus", "f", "f"], "");
            assert_eq!(code, 2);
            assert_eq!(
                err,
                "diff: unrecognized option '--bogus'\ndiff: Try 'diff --help' for more information.\n"
            );

            let (code, _, err) = diff(dir, &["-K", "f", "f"], "");
            assert_eq!(code, 2);
            assert_eq!(
                err,
                "diff: invalid option -- 'K'\ndiff: Try 'diff --help' for more information.\n"
            );

            let (code, _, err) = diff(dir, &["-U"], "");
            assert_eq!(code, 2);
            assert_eq!(
                err,
                "diff: option requires an argument -- 'U'\ndiff: Try 'diff --help' for more information.\n"
            );
        });
    }

    #[test]
    fn cmp_ignore_initial_single_value_skips_both_sides() {
        // Verified against the oracle: `cmp -i 3 f g` where f/g agree from byte 3 onward.
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "abcdef\n").unwrap();
            std::fs::write(dir.join("g"), "ABCdef\n").unwrap();
            assert_eq!(
                cmp(dir, &["-i", "3", "f", "g"], ""),
                (0, String::new(), String::new())
            );
        });
    }

    #[test]
    fn cmp_ignore_initial_colon_pair_skips_independently() {
        // Verified against the oracle: `cmp -i 3:0 f g` reports the mismatch at the first
        // post-skip byte on each side ("char 1, line 1"), not the original file offset.
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "abcdef\n").unwrap();
            std::fs::write(dir.join("g"), "ABCdef\n").unwrap();
            assert_eq!(
                cmp(dir, &["-i", "3:0", "f", "g"], ""),
                (1, "f g differ: char 1, line 1\n".into(), String::new())
            );
        });
    }

    #[test]
    fn cmp_positional_skip_applies_to_first_file_only() {
        // Verified against the oracle: `cmp f g 3` (a single positional SKIP) behaves like
        // `-i 3:0`, not `-i 3` — it does NOT skip the second file too.
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "abcdef\n").unwrap();
            std::fs::write(dir.join("g"), "ABCdef\n").unwrap();
            assert_eq!(
                cmp(dir, &["f", "g", "3"], ""),
                (1, "f g differ: char 1, line 1\n".into(), String::new())
            );
        });
    }

    #[test]
    fn cmp_invalid_ignore_initial_value_from_positional_operand() {
        // Verified against the oracle: `cmp f f f` — the third operand is a positional SKIP1,
        // and GNU's error names it as an `--ignore-initial value` regardless.
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "a\n").unwrap();
            let (code, _, err) = cmp(dir, &["f", "f", "f"], "");
            assert_eq!(code, 2);
            assert_eq!(
                err,
                "cmp: invalid --ignore-initial value \u{2018}f\u{2019}\ncmp: Try 'cmp --help' for more information.\n"
            );
        });
    }

    #[test]
    fn cmp_missing_operand_diagnostic() {
        let _d = run(|_dir| {
            let (code, _, err) = cmp(_dir, &[], "");
            assert_eq!(code, 2);
            assert_eq!(
                err,
                "cmp: missing operand after \u{2018}cmp\u{2019}\ncmp: Try 'cmp --help' for more information.\n"
            );
        });
    }

    #[test]
    fn cmp_single_operand_defaults_second_to_stdin() {
        // Verified against the oracle: `cmp f` (1 operand) is not a usage error — FILE2 defaults
        // to stdin, so with nothing piped it reports EOF on '-' immediately.
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "a\n").unwrap();
            let (code, out, err) = cmp(dir, &["f"], "");
            assert_eq!((code, out.as_str()), (1, ""));
            assert_eq!(err, "cmp: EOF on \u{2018}-\u{2019} which is empty\n");
        });
    }
}
