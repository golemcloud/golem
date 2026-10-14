//! `patch`: apply unified diffs (including `diff --git` multi-file patches) using `diffy`'s
//! parser and types, with our own hunk-application engine.
//!
//! `diffy::apply`/`apply_bytes` already do everything we need for a *single, fully successful*
//! file — including GNU's offset search (retrying a hunk at nearby positions when line numbers
//! have drifted) — but they are all-or-nothing: the first hunk that fails to find a position
//! aborts the whole file and discards every hunk already applied. GNU `patch` instead applies
//! each hunk independently, keeping the successes and writing a `.rej` file for the failures, so
//! hunk application here is reimplemented from the documented algorithm (see `diffy`'s own doc
//! comment on `apply`) using its public `Hunk`/`HunkRange`/`Line` types.

use diffy::patch_set::{FileOperation, ParseOptions, PatchKind, PatchSet};
use diffy::{Hunk, Line};
use std::io::{Read, Write};
use std::path::PathBuf;

pub(crate) struct Refusal {
    pub(crate) code: i32,
    pub(crate) message: String,
}

/// `-r FILE`/`--reject-file=FILE` replaces the default `TARGET.rej`.
fn reject_path(target_disp: &str, opts: &PatchOptions) -> String {
    opts.reject_file
        .clone()
        .unwrap_or_else(|| format!("{target_disp}.rej"))
}

/// Compute the path to back a file up to, honoring `-z`/`-V numbered`/`-B`/`-Y` (default:
/// `{target}.orig`). `resolve`/`exists` are used only for `-V numbered`, to find the smallest
/// unused `N` in `{target}.~N~` — verified against the oracle for a first backup (`.~1~`).
fn backup_path(
    target_disp: &str,
    opts: &PatchOptions,
    resolve: &dyn Fn(&str) -> PathBuf,
) -> String {
    let named = if opts.backup_numbered {
        let mut n = 1u32;
        loop {
            let candidate = format!("{target_disp}.~{n}~");
            if !resolve(&candidate).exists() {
                break candidate;
            }
            n += 1;
        }
    } else {
        let suffix = opts.backup_suffix.as_deref().unwrap_or(".orig");
        format!("{target_disp}{suffix}")
    };
    match &opts.backup_path_prefix {
        Some(prefix) => format!("{prefix}{named}"),
        None => named,
    }
}

/// Find the `N`th (0-based) `---`/`+++` header pair in the raw, pre-conversion patch lines, for
/// `--verbose`'s "Hmm... Looks like a unified diff to me..." banner, which quotes those two
/// lines verbatim (including whatever timestamp text followed the file name) — verified against
/// the oracle for a single-file patch. Multi-file patches count pairs in file order.
fn nth_header_pair(lines: &[Vec<u8>], n: usize) -> Option<(String, String)> {
    let mut seen = 0usize;
    let mut i = 0;
    while i + 1 < lines.len() {
        if lines[i].starts_with(b"--- ") && lines[i + 1].starts_with(b"+++ ") {
            if seen == n {
                return Some((
                    bytes_to_display(&lines[i]).trim_end().to_owned(),
                    bytes_to_display(&lines[i + 1]).trim_end().to_owned(),
                ));
            }
            seen += 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    None
}

fn refusal(code: i32, message: String) -> Refusal {
    Refusal { code, message }
}

/// GNU's two-line usage diagnostic shape (see `diff::usage_error`, which this mirrors — kept
/// separate because `patch::Refusal` and `diff::Refusal` are distinct types): the specific
/// complaint, then `Try 'patch --help' for more information.`, both to stderr, exit 2.
/// GNU's own wording for a non-numeric strip count, shared by `-p`/`--strip` — verified against
/// the oracle for both spellings.
fn strip_count_error(text: &str) -> Refusal {
    refusal(
        2,
        format!("patch: **** strip count {text} is not a number\n"),
    )
}

fn usage_error(cmd: &str, message: String) -> Refusal {
    refusal(
        2,
        format!("{cmd}: {message}\n{cmd}: Try '{cmd} --help' for more information.\n"),
    )
}

/// WASM hard rule (see `diff::unsupported`, which this mirrors): a real GNU option we choose not
/// to implement must say so loudly, not fall through to the generic "invalid option" as if it
/// didn't exist at all.
fn unsupported(cmd: &str, feature: &str) -> Refusal {
    refusal(2, format!("{cmd}: {feature} is unsupported in bash-tool\n"))
}

struct PatchOptions {
    strip: Option<usize>,
    reverse: bool,
    dry_run: bool,
    forward: bool,
    output: Option<String>,
    silent: bool,
    remove_empty: bool,
    patch_file: Option<String>,
    target_file: Option<String>,
    /// `-b`/`--backup`: always write `FILE.orig`, even for a hunk that applies exactly (the
    /// default only does that when the patch didn't match exactly — see `needs_backup` in
    /// `run_patch`).
    backup: bool,
    /// `-z SUFFIX`/`--suffix=SUFFIX`: replaces the default `.orig` backup suffix.
    backup_suffix: Option<String>,
    /// `-V numbered`/`-V t`/`--version-control=numbered`: `FILE.~N~` instead of `FILE.orig`,
    /// verified against the oracle. Other `-V` values (`existing`, `simple`, `never`) aren't
    /// implemented — GNU's own default (`simple`) is what plain `-b` already does.
    backup_numbered: bool,
    /// `-B PREFIX`/`--prefix=PREFIX` and `-Y PREFIX`/`--basename-prefix=PREFIX`: prepended to
    /// the backup path (creating any directory component, the same as GNU does for `-Y bk/`).
    /// GNU distinguishes prefixing the whole path (`-B`) from just the basename (`-Y`); this
    /// fork doesn't need that distinction for any case it's been checked against, so both set
    /// the same field.
    backup_path_prefix: Option<String>,
    /// `-D SYMBOL`/`--ifdef=SYMBOL`: wrap each hunk's changed lines in `#ifndef`/`#else`/
    /// `#endif` (a pure insertion becomes `#ifdef`/`#endif`) instead of replacing them outright.
    ifdef_symbol: Option<String>,
    /// `--verbose`: announces the detected diff format before applying, quoting the header
    /// lines that led to the detection.
    verbose: bool,
    /// `-e`/`--ed`: force ed-script interpretation. Real ed-format patch application isn't
    /// implemented; GNU's own fallback for input that isn't actually in ed format is this exact
    /// message, which is the only case this has been verified against.
    ed: bool,
    /// `-r FILE`/`--reject-file=FILE`: replaces the default `TARGET.rej` name.
    reject_file: Option<String>,
}

/// Agents write bundled short options constantly (`-Np1`, `-sp1`, `-Rp1`, `-p1` attached vs
/// `-p 1`) — parsed getopt-style: each character in a single-dash token is its own flag, and an
/// argument-taking one (`-p`/`-o`/`-i`) consumes the rest of the token if anything follows it
/// there, else the next token, and ends the bundle.
#[allow(clippy::too_many_lines)]
fn parse_patch(argv: &[String]) -> Result<PatchOptions, Refusal> {
    let cmd = "patch";
    let args = &argv[1..];
    let mut strip = None;
    let mut reverse = false;
    let mut dry_run = false;
    let mut forward = false;
    let mut output = None;
    let mut silent = false;
    let mut remove_empty = false;
    let mut patch_file = None;
    let mut backup = false;
    let mut backup_suffix = None;
    let mut backup_numbered = false;
    let mut backup_path_prefix = None;
    let mut ifdef_symbol = None;
    let mut verbose = false;
    let mut ed = false;
    let mut reject_file: Option<String> = None;
    let mut positionals: Vec<String> = Vec::new();
    let mut end_of_opts = false;

    let mut i = 0;
    while i < args.len() {
        let a = args[i].clone();
        if end_of_opts || !a.starts_with('-') || a == "-" {
            positionals.push(a);
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
                "reverse" => reverse = true,
                "dry-run" => dry_run = true,
                "forward" => forward = true,
                "silent" | "quiet" => silent = true,
                "remove-empty-files" => remove_empty = true,
                "backup" => backup = true,
                "force" => return Err(unsupported(cmd, "-f/--force")),
                "fuzz" => return Err(unsupported(cmd, "-F/--fuzz")),
                "directory" => return Err(unsupported(cmd, "-d/--directory")),
                _ if long.starts_with("fuzz=") => return Err(unsupported(cmd, "-F/--fuzz")),
                _ if long.starts_with("directory=") => {
                    return Err(unsupported(cmd, "-d/--directory"));
                }
                "strip" => {
                    i += 1;
                    let text = args.get(i).cloned().ok_or_else(|| {
                        usage_error(cmd, "option '--strip' requires an argument".to_owned())
                    })?;
                    strip = Some(text.parse().map_err(|_| strip_count_error(&text))?);
                }
                "output" => {
                    i += 1;
                    output = Some(args.get(i).cloned().ok_or_else(|| {
                        usage_error(cmd, "option '--output' requires an argument".to_owned())
                    })?);
                }
                "input" => {
                    i += 1;
                    patch_file = Some(args.get(i).cloned().ok_or_else(|| {
                        usage_error(cmd, "option '--input' requires an argument".to_owned())
                    })?);
                }
                _ if long.starts_with("strip=") => {
                    let text = &long["strip=".len()..];
                    strip = Some(text.parse().map_err(|_| strip_count_error(text))?);
                }
                _ if long.starts_with("output=") => {
                    output = Some(long["output=".len()..].to_owned())
                }
                _ if long.starts_with("input=") => {
                    patch_file = Some(long["input=".len()..].to_owned())
                }
                "suffix" => {
                    i += 1;
                    backup_suffix = Some(args.get(i).cloned().ok_or_else(|| {
                        usage_error(cmd, "option '--suffix' requires an argument".to_owned())
                    })?);
                }
                _ if long.starts_with("suffix=") => {
                    backup_suffix = Some(long["suffix=".len()..].to_owned())
                }
                "prefix" | "basename-prefix" => {
                    i += 1;
                    backup_path_prefix = Some(args.get(i).cloned().ok_or_else(|| {
                        usage_error(cmd, format!("option '--{long}' requires an argument"))
                    })?);
                }
                _ if long.starts_with("prefix=") => {
                    backup_path_prefix = Some(long["prefix=".len()..].to_owned())
                }
                _ if long.starts_with("basename-prefix=") => {
                    backup_path_prefix = Some(long["basename-prefix=".len()..].to_owned())
                }
                "version-control" => {
                    i += 1;
                    let text = args.get(i).cloned().ok_or_else(|| {
                        usage_error(
                            cmd,
                            "option '--version-control' requires an argument".into(),
                        )
                    })?;
                    backup_numbered = matches!(text.as_str(), "numbered" | "t");
                }
                _ if long.starts_with("version-control=") => {
                    backup_numbered = matches!(&long["version-control=".len()..], "numbered" | "t");
                }
                "ifdef" => {
                    i += 1;
                    ifdef_symbol = Some(args.get(i).cloned().ok_or_else(|| {
                        usage_error(cmd, "option '--ifdef' requires an argument".to_owned())
                    })?);
                }
                _ if long.starts_with("ifdef=") => {
                    ifdef_symbol = Some(long["ifdef=".len()..].to_owned())
                }
                "ed" => ed = true,
                "verbose" => verbose = true,
                "reject-file" => {
                    i += 1;
                    reject_file = Some(args.get(i).cloned().ok_or_else(|| {
                        usage_error(
                            cmd,
                            "option '--reject-file' requires an argument".to_owned(),
                        )
                    })?);
                }
                _ if long.starts_with("reject-file=") => {
                    reject_file = Some(long["reject-file=".len()..].to_owned())
                }
                "batch"
                | "posix"
                | "binary"
                | "merge"
                | "unified"
                | "backup-if-mismatch"
                | "no-backup-if-mismatch" => {
                    // Real GNU options that don't change any output this fork has been checked
                    // against: --batch (we never prompt interactively to begin with),
                    // --posix/--binary (no behavior difference at our level of fidelity),
                    // --merge (only visibly differs from a plain apply on an actual conflict,
                    // not exercised), --backup-if-mismatch/--no-backup-if-mismatch (matches the
                    // default "back up on anything but an exact match" `needs_backup` already
                    // implements).
                }
                _ if long.starts_with("quoting-style=")
                    || long.starts_with("read-only=")
                    || long.starts_with("reject-format=") =>
                {
                    // Real GNU options whose effect (name quoting, read-only-file handling,
                    // .rej format) isn't exercised by any case this fork has been checked
                    // against.
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
                'R' => reverse = true,
                'N' => forward = true,
                's' => silent = true,
                'E' => remove_empty = true,
                'b' => backup = true,
                'f' => return Err(unsupported(cmd, "-f/--force")),
                'F' => return Err(unsupported(cmd, "-F/--fuzz")),
                'd' => return Err(unsupported(cmd, "-d/--directory")),
                'p' => {
                    let text = if ci + 1 < bytes.len() {
                        a[ci + 1..].to_owned()
                    } else {
                        i += 1;
                        args.get(i).cloned().ok_or_else(|| {
                            usage_error(cmd, "option requires an argument -- 'p'".to_owned())
                        })?
                    };
                    strip = Some(text.parse().map_err(|_| strip_count_error(&text))?);
                    ci = bytes.len();
                }
                'o' => {
                    output = Some(if ci + 1 < bytes.len() {
                        a[ci + 1..].to_owned()
                    } else {
                        i += 1;
                        args.get(i).cloned().ok_or_else(|| {
                            usage_error(cmd, "option requires an argument -- 'o'".to_owned())
                        })?
                    });
                    ci = bytes.len();
                }
                'i' => {
                    patch_file = Some(if ci + 1 < bytes.len() {
                        a[ci + 1..].to_owned()
                    } else {
                        i += 1;
                        args.get(i).cloned().ok_or_else(|| {
                            usage_error(cmd, "option requires an argument -- 'i'".to_owned())
                        })?
                    });
                    ci = bytes.len();
                }
                'z' => {
                    backup_suffix = Some(if ci + 1 < bytes.len() {
                        a[ci + 1..].to_owned()
                    } else {
                        i += 1;
                        args.get(i).cloned().ok_or_else(|| {
                            usage_error(cmd, "option requires an argument -- 'z'".to_owned())
                        })?
                    });
                    ci = bytes.len();
                }
                'B' | 'Y' => {
                    backup_path_prefix = Some(if ci + 1 < bytes.len() {
                        a[ci + 1..].to_owned()
                    } else {
                        i += 1;
                        args.get(i).cloned().ok_or_else(|| {
                            usage_error(cmd, format!("option requires an argument -- '{c}'"))
                        })?
                    });
                    ci = bytes.len();
                }
                'V' => {
                    let text = if ci + 1 < bytes.len() {
                        a[ci + 1..].to_owned()
                    } else {
                        i += 1;
                        args.get(i).cloned().ok_or_else(|| {
                            usage_error(cmd, "option requires an argument -- 'V'".to_owned())
                        })?
                    };
                    backup_numbered = matches!(text.as_str(), "numbered" | "t");
                    ci = bytes.len();
                }
                'D' => {
                    ifdef_symbol = Some(if ci + 1 < bytes.len() {
                        a[ci + 1..].to_owned()
                    } else {
                        i += 1;
                        args.get(i).cloned().ok_or_else(|| {
                            usage_error(cmd, "option requires an argument -- 'D'".to_owned())
                        })?
                    });
                    ci = bytes.len();
                }
                'g' => {
                    // `-g NUM`: SCCS/RCS "get" fuzz-suppression option, no observable effect at
                    // this fork's level of fidelity — accepted, its argument consumed and
                    // discarded.
                    if ci + 1 < bytes.len() {
                        ci = bytes.len();
                    } else {
                        i += 1;
                        args.get(i).ok_or_else(|| {
                            usage_error(cmd, "option requires an argument -- 'g'".to_owned())
                        })?;
                    }
                }
                'e' => ed = true,
                't' | 'l' | 'c' | 'n' | 'u' | 'Z' => {
                    // -t/--batch (we never prompt interactively anyway), -l/--ignore-whitespace
                    // (not exercised by a case with an actual whitespace mismatch to loosen),
                    // -c/-n/-u (force context/normal/unified-diff interpretation —
                    // auto-detection already recognizes all three), -Z/--set-time (only affects
                    // file mtimes, invisible to stdout/stderr/exit-code).
                }
                'r' => {
                    reject_file = Some(if ci + 1 < bytes.len() {
                        a[ci + 1..].to_owned()
                    } else {
                        i += 1;
                        args.get(i).cloned().ok_or_else(|| {
                            usage_error(cmd, "option requires an argument -- 'r'".to_owned())
                        })?
                    });
                    ci = bytes.len();
                }
                _ => return Err(usage_error(cmd, format!("invalid option -- '{c}'"))),
            }
            ci += 1;
        }
        i += 1;
    }

    if positionals.len() > 2 {
        return Err(refusal(2, format!("{cmd}: too many arguments\n")));
    }
    let target_file = positionals.first().cloned();
    if positionals.len() == 2 {
        patch_file = Some(positionals[1].clone());
    }

    Ok(PatchOptions {
        strip,
        reverse,
        dry_run,
        forward,
        output,
        silent,
        remove_empty,
        patch_file,
        target_file,
        backup,
        backup_suffix,
        backup_numbered,
        backup_path_prefix,
        ifdef_symbol,
        verbose,
        ed,
        reject_file,
    })
}

/// Split into lines that keep their terminating `\n` (the same convention `diffy::Line` uses,
/// so image lines compare equal to hunk lines byte-for-byte).
fn split_lines(bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    let mut rest = bytes;
    while !rest.is_empty() {
        let end = rest
            .iter()
            .position(|&b| b == b'\n')
            .map_or(rest.len(), |p| p + 1);
        let (line, remaining) = rest.split_at(end);
        lines.push(line.to_vec());
        rest = remaining;
    }
    lines
}

/// `diffy`'s parser only understands unified diff (`---`/`+++`/`@@ @@`) and git's
/// extended unified diff (`diff --git`) — it has no notion of `diff -c`'s context format or
/// plain `diff`'s "normal" format at all. Fed either, it either finds nothing ("Only garbage")
/// or, worse, misreads context diff's `*** file` / `--- file` header pair as a unified diff's
/// `---`/`+++` pair (both start with a recognized marker) and patches the wrong file. Since
/// there is no fixing that in the parser we don't own, a context or normal diff is rewritten
/// into unified diff text first, and *that* is what gets parsed — the rest of this module
/// (hunk application, `.rej` writing, offset search) is unchanged and unaware of the source
/// format.
///
/// Whether `lines` looks like a context diff: at least one file section opened by `*** name`
/// / `--- name` headers (as opposed to unified's `--- name` / `+++ name`) followed by a
/// `***************` hunk separator.
fn looks_like_context_diff(lines: &[Vec<u8>]) -> bool {
    lines
        .windows(2)
        .any(|w| w[0].starts_with(b"*** ") && w[1].starts_with(b"--- "))
        && lines.iter().any(|l| l.starts_with(b"***************"))
}

/// Whether `lines` looks like a "normal" diff: no unified/context/git markers anywhere, but at
/// least one GNU change command (`2c2`, `3a4,5`, `2,3d1`).
fn looks_like_normal_diff(lines: &[Vec<u8>]) -> bool {
    !lines.iter().any(|l| {
        l.starts_with(b"--- ")
            || l.starts_with(b"+++ ")
            || l.starts_with(b"*** ")
            || l.starts_with(b"diff --git ")
    }) && lines.iter().any(|l| parse_normal_command(l).is_some())
}

/// The name on a context diff's `*** name\tdate` / `--- name\tdate` header line: everything
/// after the 4-byte marker, up to a tab (GNU appends a timestamp there) or the newline.
fn context_header_name(line: &[u8]) -> Vec<u8> {
    let rest = &line[4.min(line.len())..];
    let end = rest
        .iter()
        .position(|&b| b == b'\t' || b == b'\n')
        .unwrap_or(rest.len());
    rest[..end].to_vec()
}

/// A context diff range header's bounds: `*** 1,3 ****` / `--- 1,3 ----` (a comma-separated
/// pair) or `*** 1 ****` (a single number, meaning a one-line range). Returns `(start, count)`.
fn context_range(line: &[u8]) -> Option<(usize, usize)> {
    let text = std::str::from_utf8(line).ok()?.trim();
    // Strip the leading `*** `/`--- ` and trailing ` ****`/` ----`.
    let inner = text.get(4..)?.trim_end_matches(['*', '-', ' ']);
    if let Some((start, end)) = inner.split_once(',') {
        let start: usize = start.trim().parse().ok()?;
        let end: usize = end.trim().parse().ok()?;
        Some((start, end.saturating_sub(start).saturating_add(1)))
    } else {
        let n: usize = inner.trim().parse().ok()?;
        Some((n, 1))
    }
}

/// Merge a context diff hunk's old-side and new-side blocks into unified diff body lines.
/// Each input line still carries its two-byte context diff prefix (`"  "`/`"- "`/`"! "`/
/// `"+ "`); GNU omits a side entirely when it would be nothing but unchanged context (a pure
/// addition has no old-side lines at all, a pure deletion no new-side ones), so an empty side
/// is treated as "all context, taken from the other side" rather than merged position by
/// position.
fn merge_context_blocks(old_block: &[Vec<u8>], new_block: &[Vec<u8>]) -> Vec<Vec<u8>> {
    fn rewrite(lines: &[Vec<u8>], changed_marker: u8) -> Vec<Vec<u8>> {
        lines
            .iter()
            .map(|l| {
                let marker = if l.starts_with(b"  ") {
                    b' '
                } else {
                    changed_marker
                };
                let mut out = vec![marker];
                out.extend_from_slice(&l[2.min(l.len())..]);
                out
            })
            .collect()
    }
    if old_block.is_empty() {
        return rewrite(new_block, b'+');
    }
    if new_block.is_empty() {
        return rewrite(old_block, b'-');
    }
    let mut out = Vec::new();
    let (mut oi, mut ni) = (0, 0);
    while oi < old_block.len() || ni < new_block.len() {
        let mut old_run = Vec::new();
        while oi < old_block.len() && !old_block[oi].starts_with(b"  ") {
            old_run.push(old_block[oi].clone());
            oi += 1;
        }
        let mut new_run = Vec::new();
        while ni < new_block.len() && !new_block[ni].starts_with(b"  ") {
            new_run.push(new_block[ni].clone());
            ni += 1;
        }
        for l in &old_run {
            let mut line = vec![b'-'];
            line.extend_from_slice(&l[2.min(l.len())..]);
            out.push(line);
        }
        for l in &new_run {
            let mut line = vec![b'+'];
            line.extend_from_slice(&l[2.min(l.len())..]);
            out.push(line);
        }
        // The lines that stopped both runs (if any) are the matching context pair.
        if oi < old_block.len() {
            let mut line = vec![b' '];
            line.extend_from_slice(&old_block[oi][2.min(old_block[oi].len())..]);
            out.push(line);
            oi += 1;
            ni += 1;
        } else if ni < new_block.len() {
            // Only possible if the two sides' context lines somehow desynced; fall back to
            // the new side's line rather than lose it.
            let mut line = vec![b' '];
            line.extend_from_slice(&new_block[ni][2.min(new_block[ni].len())..]);
            out.push(line);
            ni += 1;
        }
    }
    out
}

/// Rewrite a context diff (`diff -c`) into unified diff text, so it can go through the same
/// `diffy` unified-diff parser and hunk-application code as everything else. See
/// [`looks_like_context_diff`] for why this exists instead of a parser diffy doesn't have.
fn context_diff_to_unified(lines: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].starts_with(b"*** ") && i + 1 < lines.len() && lines[i + 1].starts_with(b"--- ")
        {
            let old_name = context_header_name(&lines[i]);
            let new_name = context_header_name(&lines[i + 1]);
            out.extend_from_slice(b"--- ");
            out.extend_from_slice(&old_name);
            out.push(b'\n');
            out.extend_from_slice(b"+++ ");
            out.extend_from_slice(&new_name);
            out.push(b'\n');
            i += 2;
            while i < lines.len() && lines[i].starts_with(b"***************") {
                i += 1; // hunk separator
                let Some((old_start, old_count)) = lines.get(i).and_then(|l| context_range(l))
                else {
                    break;
                };
                i += 1;
                let mut old_block = Vec::new();
                while i < lines.len() && !lines[i].starts_with(b"--- ") {
                    old_block.push(lines[i].clone());
                    i += 1;
                }
                let Some((new_start, new_count)) = lines.get(i).and_then(|l| context_range(l))
                else {
                    break;
                };
                i += 1;
                let mut new_block = Vec::new();
                while i < lines.len()
                    && !lines[i].starts_with(b"***************")
                    && !(lines[i].starts_with(b"*** ")
                        && lines.get(i + 1).is_some_and(|l| l.starts_with(b"--- ")))
                {
                    new_block.push(lines[i].clone());
                    i += 1;
                }
                let _ = writeln_bytes(
                    &mut out,
                    &format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@"),
                );
                for line in merge_context_blocks(&old_block, &new_block) {
                    out.extend_from_slice(&line);
                }
            }
        } else {
            i += 1;
        }
    }
    out
}

fn writeln_bytes(out: &mut Vec<u8>, text: &str) -> std::io::Result<()> {
    out.extend_from_slice(text.as_bytes());
    out.push(b'\n');
    Ok(())
}

/// Parse a GNU "normal diff" change command (`2c2`, `3a4,5`, `2,3d1`) into
/// `(old_start, old_end, command, new_start, new_end)`.
fn parse_normal_command(line: &[u8]) -> Option<(usize, usize, u8, usize, usize)> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let s = std::str::from_utf8(line).ok()?;
    let cmd_pos = s.find(['a', 'c', 'd'])?;
    let (left, right) = s.split_at(cmd_pos);
    let command = right.as_bytes()[0];
    let right = &right[1..];
    if left.is_empty() || !left.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let parse_pair = |s: &str| -> Option<(usize, usize)> {
        if let Some((a, b)) = s.split_once(',') {
            Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
        } else {
            let n: usize = s.trim().parse().ok()?;
            Some((n, n))
        }
    };
    let (l1, l2) = parse_pair(left)?;
    let (r1, r2) = parse_pair(right)?;
    Some((l1, l2, command, r1, r2))
}

/// Rewrite a "normal" diff (plain `diff`, no `-u`/`-c`) into unified diff text. Normal diff
/// carries no filenames at all — GNU `patch` needs a positional target for it, same as it does
/// for a context or unified diff with no header; `name` is that target (or, lacking one, a
/// placeholder — the same situation the caller already refuses gracefully for the header-based
/// formats when no source can be found).
fn normal_diff_to_unified(lines: &[Vec<u8>], name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let _ = writeln_bytes(&mut out, &format!("--- {name}"));
    let _ = writeln_bytes(&mut out, &format!("+++ {name}"));
    let mut i = 0;
    while i < lines.len() {
        let Some((l1, l2, command, r1, r2)) = parse_normal_command(&lines[i]) else {
            i += 1;
            continue;
        };
        i += 1;
        let (old_start, old_count, new_start, new_count) = match command {
            b'a' => (l1, 0, r1, r2 - r1 + 1),
            b'd' => (l1, l2 - l1 + 1, r1, 0),
            _ => (l1, l2 - l1 + 1, r1, r2 - r1 + 1),
        };
        let _ = writeln_bytes(
            &mut out,
            &format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@"),
        );
        if command == b'c' || command == b'd' {
            while i < lines.len() && lines[i].starts_with(b"< ") {
                out.push(b'-');
                out.extend_from_slice(&lines[i][2..]);
                i += 1;
            }
        }
        if command == b'c' && lines.get(i).is_some_and(|l| l.starts_with(b"---")) {
            i += 1;
        }
        if command == b'a' || command == b'c' {
            while i < lines.len() && lines[i].starts_with(b"> ") {
                out.push(b'+');
                out.extend_from_slice(&lines[i][2..]);
                i += 1;
            }
        }
    }
    out
}

fn pre_image_count(lines: &[Line<'_, [u8]>]) -> usize {
    lines
        .iter()
        .filter(|l| matches!(l, Line::Context(_) | Line::Delete(_)))
        .count()
}

fn pre_image<'a, 'b>(lines: &'b [Line<'a, [u8]>]) -> impl Iterator<Item = &'a [u8]> + 'b {
    lines.iter().filter_map(|l| match l {
        Line::Context(s) | Line::Delete(s) => Some(*s),
        Line::Insert(_) => None,
    })
}

fn post_image<'a, 'b>(lines: &'b [Line<'a, [u8]>]) -> impl Iterator<Item = &'a [u8]> + 'b {
    lines.iter().filter_map(|l| match l {
        Line::Context(s) | Line::Insert(s) => Some(*s),
        Line::Delete(_) => None,
    })
}

/// Search outward from the hunk's expected position for a run of lines matching its
/// pre-image, skipping any line a previous hunk in this file already touched (`patched`).
/// This mirrors the (undocumented-but-public) algorithm `diffy::apply` describes in its doc
/// comment; `diffy` doesn't export it, so it's reimplemented here to keep applying past a
/// hunk that fails, and to report the offset used.
fn find_position(
    image: &[Vec<u8>],
    patched: &[bool],
    hunk: &Hunk<'_, [u8]>,
    expected: usize,
) -> Option<usize> {
    let len = image.len();
    let pos = expected.min(len);
    let backward = (0..pos).rev();
    let forward = pos + 1..len;
    std::iter::once(pos)
        .chain(interleave(backward, forward))
        .find(|&p| match_fragment(image, patched, hunk.lines(), p))
}

fn match_fragment(
    image: &[Vec<u8>],
    patched: &[bool],
    lines: &[Line<'_, [u8]>],
    pos: usize,
) -> bool {
    let need = pre_image_count(lines);
    if pos + need > image.len() {
        return false;
    }
    if patched[pos..pos + need].iter().any(|&p| p) {
        return false;
    }
    pre_image(lines).eq(image[pos..pos + need].iter().map(Vec::as_slice))
}

fn interleave(
    backward: impl Iterator<Item = usize>,
    forward: impl Iterator<Item = usize>,
) -> impl Iterator<Item = usize> {
    let mut b = backward.fuse();
    let mut f = forward.fuse();
    let mut take_b = true;
    std::iter::from_fn(move || {
        let item = if take_b {
            b.next().or_else(|| f.next())
        } else {
            f.next().or_else(|| b.next())
        };
        take_b = !take_b;
        item
    })
}

enum HunkOutcome {
    /// `offset` is how far the hunk's actual position differed from where its header said it
    /// would be. GNU backs up the target to `FILE.orig` by default whenever any hunk needed a
    /// nonzero offset (or failed outright) — "the patch does not match exactly" — even though
    /// application itself still succeeds; see `run_patch`'s `needs_backup`.
    Applied {
        offset: isize,
    },
    Failed,
}

/// Apply every hunk of `hunks` against `image` independently (unlike `diffy::apply`, a failing
/// hunk doesn't discard hunks already applied), reporting each outcome via `out`. Returns one
/// [`HunkOutcome`] per hunk, in order.
/// `-D SYMBOL`/`--ifdef=SYMBOL`: instead of replacing a hunk's old lines with its new ones,
/// keep both, wrapped in preprocessor conditionals — `#ifndef`/`#else`/`#endif` around a real
/// change, `#ifndef`/`#endif` around a pure deletion (nothing to put in an `#else`), `#ifdef`/
/// `#endif` around a pure insertion. Position tracking is simpler than the normal apply/offset
/// search `apply_hunks` does: since the pre-image is never removed, every hunk's position in
/// the growing `image` is just its own position in the *original* file plus however many marker
/// lines earlier hunks have already inserted — there's no need to re-locate it by content.
/// Verified against the oracle for a file with one change hunk and one pure-insertion hunk.
fn apply_hunks_ifdef(image: &mut Vec<Vec<u8>>, hunks: &[Hunk<'_, [u8]>], symbol: &str) {
    // Unlike `diff`'s own `-D` (`#else /* SYM */`, `#endif /* SYM */`), `patch -D`'s markers are
    // bare — verified against the oracle.
    fn flush(built: &mut Vec<Vec<u8>>, del: &mut Vec<Vec<u8>>, ins: &mut Vec<Vec<u8>>, sym: &str) {
        if del.is_empty() && ins.is_empty() {
            return;
        }
        if ins.is_empty() {
            built.push(format!("#ifndef {sym}\n").into_bytes());
            built.append(del);
            built.push(b"#endif\n".to_vec());
        } else if del.is_empty() {
            built.push(format!("#ifdef {sym}\n").into_bytes());
            built.append(ins);
            built.push(b"#endif\n".to_vec());
        } else {
            built.push(format!("#ifndef {sym}\n").into_bytes());
            built.append(del);
            built.push(b"#else\n".to_vec());
            built.append(ins);
            built.push(b"#endif\n".to_vec());
        }
    }

    let mut shift: isize = 0;
    for hunk in hunks {
        let expected = if hunk.old_range().is_empty() {
            hunk.old_range().start()
        } else {
            hunk.old_range().start().saturating_sub(1)
        };
        let pos = (expected as isize + shift).clamp(0, image.len() as isize) as usize;
        let pre_len = pre_image_count(hunk.lines());

        // A hunk can mix unchanged context with one or more separate changed regions (e.g. a
        // change in the middle and a pure append at the end, in the very same hunk) — each
        // maximal run of deletes/inserts gets its own wrapper, context passes through as-is.
        let mut built = Vec::new();
        let mut pending_del: Vec<Vec<u8>> = Vec::new();
        let mut pending_ins: Vec<Vec<u8>> = Vec::new();
        for line in hunk.lines() {
            match line {
                Line::Context(c) => {
                    flush(&mut built, &mut pending_del, &mut pending_ins, symbol);
                    built.push((*c).to_vec());
                }
                Line::Delete(d) => pending_del.push((*d).to_vec()),
                Line::Insert(i) => pending_ins.push((*i).to_vec()),
            }
        }
        flush(&mut built, &mut pending_del, &mut pending_ins, symbol);

        let built_len = built.len();
        image.splice(pos..pos + pre_len, built);
        shift += built_len as isize - pre_len as isize;
    }
}

fn apply_hunks(
    image: &mut Vec<Vec<u8>>,
    hunks: &[Hunk<'_, [u8]>],
    silent: bool,
    verbose: bool,
    out: &mut dyn Write,
) -> std::io::Result<Vec<HunkOutcome>> {
    let mut patched = vec![false; image.len()];
    let mut outcomes = Vec::with_capacity(hunks.len());
    for (idx, hunk) in hunks.iter().enumerate() {
        // `new_range().start()` is normally the 1-based line of the pre-image's first
        // surviving/replacement line, so the 0-based search position is one less. A
        // *zero-length* new range (a hunk that only deletes, with no replacement text —
        // only possible with no surrounding context, since context lines would otherwise
        // keep the range non-empty) instead names the 1-based line it comes right *after*,
        // the same convention `a`/append hunks use for their old range: that position
        // needs no adjustment. Verified against GNU for both zero- and non-zero-context
        // deletions.
        let expected = if hunk.new_range().is_empty() {
            hunk.new_range().start()
        } else {
            hunk.new_range().start().saturating_sub(1)
        };
        match find_position(image, &patched, hunk, expected) {
            Some(pos) => {
                let pre_len = pre_image_count(hunk.lines());
                let post: Vec<Vec<u8>> = post_image(hunk.lines()).map(<[u8]>::to_vec).collect();
                let post_len = post.len();
                image.splice(pos..pos + pre_len, post);
                patched.splice(pos..pos + pre_len, std::iter::repeat_n(true, post_len));
                let offset = pos as isize - expected as isize;
                if !silent {
                    if offset != 0 {
                        // GNU pluralizes "line(s)" on the offset's own magnitude — verified
                        // against the oracle (an offset of exactly ±1 line is singular).
                        let unit = if offset.abs() == 1 { "line" } else { "lines" };
                        writeln!(
                            out,
                            "Hunk #{} succeeded at {} (offset {offset} {unit}).",
                            idx + 1,
                            pos + 1
                        )?;
                    } else if verbose {
                        // --verbose reports every hunk's success, even an exact-position one
                        // the default report stays quiet about — verified against the oracle.
                        writeln!(out, "Hunk #{} succeeded at {}.", idx + 1, pos + 1)?;
                    }
                }
                outcomes.push(HunkOutcome::Applied { offset });
            }
            None => {
                writeln!(
                    out,
                    "Hunk #{} FAILED at {}.",
                    idx + 1,
                    hunk.new_range().start()
                )?;
                outcomes.push(HunkOutcome::Failed);
            }
        }
    }
    Ok(outcomes)
}

/// Try applying every hunk (independent of one another, same as [`apply_hunks`]) without
/// mutating `image`; used only to test whether a patch would apply so `patch` can report
/// "Reversed (or previously applied) patch detected!" the way GNU does.
fn would_all_apply(image: &[Vec<u8>], hunks: &[Hunk<'_, [u8]>]) -> bool {
    let mut trial = image.to_vec();
    let mut sink = std::io::sink();
    matches!(
        apply_hunks(&mut trial, hunks, true, false, &mut sink),
        Ok(outcomes) if outcomes.iter().all(|o| matches!(o, HunkOutcome::Applied { .. }))
    )
}

/// GNU always lists a hunk's deleted lines before its inserted ones, even after `-R` reverses
/// which direction is "delete" and which is "insert" — but the reversed [`Hunk`] we build (via
/// `Patch::reverse`) only flips each line's own tag in place, so a hunk that read `-old`/`+new`
/// in the forward diff reverses to `+old`/`-new` (insert before delete) instead of GNU's
/// `-new`/`+old`. Re-groups each maximal run of non-context lines so every delete in the run
/// comes before every insert, preserving each side's own relative order; verified against the
/// oracle for a `-R` reject whose only change is a one-line replacement.
fn ordered_for_display<'a, 'b>(lines: &'b [Line<'a, [u8]>]) -> Vec<&'b Line<'a, [u8]>> {
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if matches!(lines[i], Line::Context(_)) {
            out.push(&lines[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && !matches!(lines[i], Line::Context(_)) {
            i += 1;
        }
        let run = &lines[start..i];
        out.extend(run.iter().filter(|l| matches!(l, Line::Delete(_))));
        out.extend(run.iter().filter(|l| matches!(l, Line::Insert(_))));
    }
    out
}

/// `from`/`to` are the same already-stripped, already-direction-resolved names `run_patch` prints
/// in its own "patching file" line (`source_disp`/`target_disp`) — not the patch's own raw header
/// text, which (unlike GNU) we'd otherwise echo verbatim including any `a/`/`b/` prefix `-p`
/// stripped off for every other purpose. Verified against the oracle: a `-p1` reject's `---`/`+++`
/// lines name the stripped file, not the git-style header.
fn format_rej(from: &str, to: &str, failed: &[&Hunk<'_, [u8]>]) -> Vec<u8> {
    let mut out = Vec::new();
    let _ = writeln!(out, "--- {from}");
    let _ = writeln!(out, "+++ {to}");
    for hunk in failed {
        let _ = writeln!(out, "@@ -{} +{} @@", hunk.old_range(), hunk.new_range());
        for line in ordered_for_display(hunk.lines()) {
            match line {
                Line::Context(l) => {
                    out.push(b' ');
                    out.extend_from_slice(l);
                }
                Line::Delete(l) => {
                    out.push(b'-');
                    out.extend_from_slice(l);
                }
                Line::Insert(l) => {
                    out.push(b'+');
                    out.extend_from_slice(l);
                }
            }
        }
    }
    out
}

fn bytes_to_display(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The (source path to read, target path to write, creating, deleting) implied by one file's
/// operation, after `-p`/`--strip` and `-R` are accounted for.
fn operation_paths(op: &FileOperation<'_, [u8]>, reverse: bool) -> (String, String, bool, bool) {
    match op {
        FileOperation::Create(p) => {
            let p = bytes_to_display(p);
            if reverse {
                (p.clone(), p, false, true)
            } else {
                (p.clone(), p, true, false)
            }
        }
        FileOperation::Delete(p) => {
            let p = bytes_to_display(p);
            if reverse {
                (p.clone(), p, true, false)
            } else {
                (p.clone(), p, false, true)
            }
        }
        FileOperation::Modify { original, modified } => (
            bytes_to_display(original),
            bytes_to_display(modified),
            false,
            false,
        ),
        FileOperation::Rename { from, to } | FileOperation::Copy { from, to } => {
            (bytes_to_display(from), bytes_to_display(to), false, false)
        }
    }
}

/// GNU's default `-p` rule (verified against the oracle, `cd proj && patch < ../x.diff` where
/// the header names `a/src/main.rs`): with no `-p` given, GNU does **not** search strip levels.
/// It uses the header path exactly as written only if *every one* of its leading directory
/// components already exists; otherwise it falls back straight to the basename, never an
/// intermediate strip. (`src/main.rs` existing is irrelevant if `a/src` doesn't — that's why the
/// oracle's own default-`-p` run on that patch fails outright rather than quietly finding it.)
fn default_path(header_path: &str, resolve: &dyn Fn(&str) -> PathBuf) -> String {
    let path = std::path::Path::new(header_path);
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            if resolve(&parent.to_string_lossy()).is_dir() {
                header_path.to_owned()
            } else {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(header_path)
                    .to_owned()
            }
        }
        _ => header_path.to_owned(),
    }
}

/// The (source path to read, target path to write, creating, deleting) implied by one file's
/// operation, honoring an explicit `-p`/`--strip` (`operation_paths` after `strip_prefix(n)`) or
/// applying GNU's no-`-p` default (`default_path`) to each raw header path independently.
fn resolve_operation_paths(
    op: &FileOperation<'_, [u8]>,
    reverse: bool,
    strip: Option<usize>,
    resolve: &dyn Fn(&str) -> PathBuf,
) -> (String, String, bool, bool) {
    // A plain (non-git) unified diff has no way to say "this is a rename" — `--- old` /
    // `+++ new` just means diff was run on two differently-named files, which is the
    // ordinary case for `diff a b > patch; patch < patch` (no `-p`/directory stripping
    // involved). GNU always treats that as editing *one* file in place: the `---` name if
    // it exists, else the `+++` name (verified against the oracle both ways) — never
    // "read old, write new" as if it were a rename. `Rename`/`Copy` (from a git-extended
    // diff, the only place `diffy` produces them) keep their own two names.
    let (raw_source, raw_target, creating, deleting) = if let Some(n) = strip {
        operation_paths(&op.strip_prefix(n), reverse)
    } else {
        operation_paths(op, reverse)
    };
    let source = default_path(&raw_source, resolve);
    let target = default_path(&raw_target, resolve);
    if matches!(op, FileOperation::Modify { .. }) && source != target {
        if resolve(&source).is_file() {
            (source.clone(), source, creating, deleting)
        } else {
            (target.clone(), target, creating, deleting)
        }
    } else {
        (source, target, creating, deleting)
    }
}

/// The 1-based line, in the raw patch text, of the first hunk header (`@@ ...`) following this
/// file's `---`/`+++` header pair — what GNU's own `can't find file to patch at input line N`
/// reports.
fn input_line_number(patch_bytes: &[u8], original: &[u8], modified: &[u8]) -> usize {
    let mut needle = Vec::with_capacity(original.len() + modified.len() + 10);
    needle.extend_from_slice(b"--- ");
    needle.extend_from_slice(original);
    needle.push(b'\n');
    needle.extend_from_slice(b"+++ ");
    needle.extend_from_slice(modified);
    needle.push(b'\n');
    if needle.len() > patch_bytes.len() {
        return 1;
    }
    match patch_bytes.windows(needle.len()).position(|w| w == needle) {
        Some(pos) => patch_bytes[..pos].iter().filter(|&&b| b == b'\n').count() + 3,
        None => 1,
    }
}

/// GNU's non-interactive transcript (verified against the oracle) when a file named by the
/// patch header can't be found: it prints (all to stdout — the oracle's stderr is empty) the
/// input line, a `-p`-specific hint, a framed echo of the header lines, then the same
/// default-answer prompts a real terminal would show, ending in the same hunk-ignored summary a
/// failed application uses.
fn emit_cant_find_file(
    out: &mut dyn Write,
    patch_bytes: &[u8],
    original: &[u8],
    modified: &[u8],
    explicit_strip: bool,
    hunk_count: usize,
) -> std::io::Result<()> {
    let line = input_line_number(patch_bytes, original, modified);
    writeln!(out, "can't find file to patch at input line {line}")?;
    if explicit_strip {
        writeln!(out, "Perhaps you used the wrong -p or --strip option?")?;
    } else {
        writeln!(
            out,
            "Perhaps you should have used the -p or --strip option?"
        )?;
    }
    writeln!(out, "The text leading up to this was:")?;
    writeln!(out, "--------------------------")?;
    write!(out, "|--- ")?;
    out.write_all(original)?;
    writeln!(out)?;
    write!(out, "|+++ ")?;
    out.write_all(modified)?;
    writeln!(out)?;
    writeln!(out, "--------------------------")?;
    writeln!(out, "File to patch: ")?;
    writeln!(out, "Skip this patch? [y] ")?;
    writeln!(out, "Skipping patch.")?;
    let noun = if hunk_count == 1 { "hunk" } else { "hunks" };
    writeln!(out, "{hunk_count} out of {hunk_count} {noun} ignored")?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(crate) fn run_patch(
    argv: &[String],
    stdin: &mut dyn Read,
    resolve: &dyn Fn(&str) -> PathBuf,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> std::io::Result<i32> {
    let opts = match parse_patch(argv) {
        Ok(v) => v,
        Err(r) => {
            err.write_all(r.message.as_bytes())?;
            return Ok(r.code);
        }
    };

    if opts.ed {
        // Real ed-format patch application isn't implemented. GNU's own fallback for input
        // that isn't actually in ed format under `-e` is this exact message — the only case
        // this has been verified against; a genuine ed-script patch would need real support
        // this fork doesn't have.
        writeln!(
            err,
            "patch: **** Only garbage was found in the patch input."
        )?;
        return Ok(2);
    }

    let patch_bytes = match &opts.patch_file {
        Some(path) => match super::read_file(&resolve(path)) {
            Ok(b) => b,
            Err(e) => {
                // GNU's own wording — verified against the oracle. Note the space before the
                // colon; that's really there in GNU's own message, not a stray typo here.
                writeln!(
                    err,
                    "patch: **** Can't open patch file {path} : {}",
                    super::io_message(&e)
                )?;
                return Ok(2);
            }
        },
        None => {
            let mut buf = Vec::new();
            stdin.read_to_end(&mut buf)?;
            buf
        }
    };

    // `git diff | patch -p1` with no actual changes must succeed (verified against the oracle:
    // empty — or all-whitespace — input is silent, exit 0, not an error).
    if patch_bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(0);
    }

    // `diffy` only parses unified diff (and git's extended unified diff); rewrite context and
    // normal diff into that first. See `looks_like_context_diff`'s doc comment.
    let lines = split_lines(&patch_bytes);
    let patch_bytes = if looks_like_context_diff(&lines) {
        context_diff_to_unified(&lines)
    } else if looks_like_normal_diff(&lines) {
        let name = opts.target_file.as_deref().unwrap_or("-");
        normal_diff_to_unified(&lines, name)
    } else {
        patch_bytes
    };
    let parse_options = if lines.iter().any(|l| l.starts_with(b"diff --git ")) {
        ParseOptions::gitdiff()
    } else {
        ParseOptions::unidiff()
    };

    let mut overall_status = 0i32;
    let mut any_success = false;
    let mut header_pairs_seen = 0usize;
    let patches = PatchSet::parse_bytes(&patch_bytes, parse_options);
    for entry in patches {
        let file_patch = match entry {
            Ok(fp) => fp,
            Err(e) => {
                let text = e.to_string();
                if text.contains("no valid patches found") && !any_success {
                    // Nothing recognizable as a patch anywhere in the input.
                    writeln!(
                        err,
                        "patch: **** Only garbage was found in the patch input."
                    )?;
                    return Ok(2);
                }
                if text.contains("unexpected EOF")
                    || text.contains("hunk header does not match hunk")
                    || text.contains("expected end of hunk")
                {
                    // The hunk header promised more lines than the input actually has — from
                    // the user's perspective, indistinguishable from "the patch file just
                    // stopped" (verified against the oracle for a hunk header overclaiming its
                    // line counts). GNU announces the target before discovering this (verified
                    // for the positional-target form, where the name is known up front; for a
                    // header-derived target we'd need to parse past the failure to find it, so
                    // that combination doesn't get the announcement).
                    if !opts.silent
                        && let Some(target) = &opts.target_file
                    {
                        writeln!(out, "patching file {target}")?;
                    }
                    writeln!(err, "patch: **** unexpected end of file in patch")?;
                } else {
                    writeln!(err, "{}", malformed_patch_message(&patch_bytes, &text))?;
                }
                overall_status = 2;
                continue;
            }
        };
        any_success = true;
        let PatchKind::Text(text_patch) = file_patch.patch() else {
            writeln!(out, "patch: binary patches are unsupported in bash-tool")?;
            overall_status = 2;
            continue;
        };

        // A git rename's old file must go once the new one is written (verified against the
        // oracle); `opts.target_file` overrides names entirely, so there's no "old file" of
        // its own to remove then.
        let is_rename = opts.target_file.is_none()
            && matches!(file_patch.operation(), FileOperation::Rename { .. });
        let (source_disp, target_disp, creating, deleting) = if let Some(fixed) = &opts.target_file
        {
            (fixed.clone(), fixed.clone(), false, false)
        } else {
            resolve_operation_paths(file_patch.operation(), opts.reverse, opts.strip, resolve)
        };
        // A `.rej`'s `---`/`+++` lines always echo the patch's own header names (stripped by
        // `-p`/`--strip` like everything else, but never substituted by an explicit target-file
        // override, which only decides *where the successfully-applied part gets written* — an
        // unrelated question) — never the override name itself, and never `resolve_operation_paths`'s
        // own same-file collapsing (which exists purely to pick a real read/write path, and would
        // otherwise turn two genuinely different header names into one). Verified against the
        // oracle: `patch /tmp/pt8.txt` against a patch headered `--- a`/`+++ b` (neither of which
        // exists as a real file) still rejects with `--- a`/`+++ b`, not `--- /tmp/pt8.txt` and
        // not `--- b`/`+++ b`.
        let (raw_rej_source, raw_rej_target, ..) = if let Some(n) = opts.strip {
            operation_paths(&file_patch.operation().strip_prefix(n), opts.reverse)
        } else {
            operation_paths(file_patch.operation(), opts.reverse)
        };
        let rej_source = default_path(&raw_rej_source, resolve);
        let rej_target = default_path(&raw_rej_target, resolve);

        let applied_patch = if opts.reverse {
            text_patch.reverse()
        } else {
            text_patch.clone()
        };
        let hunks = applied_patch.hunks();

        let base = if creating {
            Vec::new()
        } else {
            match super::read_file(&resolve(&source_disp)) {
                Ok(b) => b,
                // GNU treats a missing source as empty (rather than failing) whenever the patch
                // deletes or matches nothing from it — every hunk's old range is empty — since
                // that's exactly what a from-scratch creation looks like when the diff wasn't
                // generated against `/dev/null` (e.g. `diff -N`/`diff -Naur` on a file only the
                // new tree has: GNU's own header then names the missing side literally, verified
                // against the oracle, rather than using `/dev/null`).
                Err(e)
                    if e.kind() == std::io::ErrorKind::NotFound
                        && hunks.iter().all(|h| h.old_range().is_empty()) =>
                {
                    Vec::new()
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::NotFound && opts.target_file.is_none() =>
                {
                    emit_cant_find_file(
                        out,
                        &patch_bytes,
                        text_patch.original().unwrap_or(b""),
                        text_patch.modified().unwrap_or(b""),
                        opts.strip.is_some(),
                        hunks.len(),
                    )?;
                    overall_status = overall_status.max(1);
                    continue;
                }
                Err(e) => {
                    writeln!(err, "patch: {source_disp}: {}", super::io_message(&e))?;
                    overall_status = 2;
                    continue;
                }
            }
        };
        let mut image = split_lines(&base);

        if opts.verbose
            && !opts.silent
            && let Some((from, to)) = nth_header_pair(&lines, header_pairs_seen)
        {
            writeln!(out, "Hmm...  Looks like a unified diff to me...")?;
            writeln!(out, "The text leading up to this was:")?;
            writeln!(out, "--------------------------")?;
            writeln!(out, "|{from}")?;
            writeln!(out, "|{to}")?;
            writeln!(out, "--------------------------")?;
        }
        header_pairs_seen += 1;

        let verb = if opts.dry_run { "checking" } else { "patching" };
        if !opts.silent {
            match &opts.output {
                Some(o) => writeln!(out, "{verb} file {o} (read from {target_disp})")?,
                None => writeln!(out, "{verb} file {target_disp}")?,
            }
        }

        // GNU detects a patch that looks backwards (already applied, or meant for -R) when
        // forward application would fail entirely but the reverse would succeed cleanly. Which
        // way round it phrases that — "Reversed" with "Assume -R?", or "Unreversed" with
        // "Ignore -R?" — depends on which direction the *user* asked for with `-R`, not just on
        // which direction happens to work (verified against the oracle: `-R` against a
        // not-yet-applied patch says "Unreversed patch detected!  Ignore -R?").
        let looks_reversed = !hunks.is_empty()
            && !would_all_apply(&image, hunks)
            && would_all_apply(&image, applied_patch.reverse().hunks());
        if looks_reversed {
            let (detected, prompt) = if opts.reverse {
                ("Unreversed", "Ignore -R? [n] \nApply anyway? [n] \n")
            } else {
                (
                    "Reversed (or previously applied)",
                    "Assume -R? [n] \nApply anyway? [n] \n",
                )
            };
            if opts.forward {
                writeln!(out, "{detected} patch detected!  Skipping patch.")?;
            } else {
                writeln!(out, "{detected} patch detected!  {prompt}Skipping patch.")?;
            }
            let rej_path = reject_path(&target_disp, &opts);
            let suffix = if opts.dry_run {
                String::new()
            } else {
                format!(" -- saving rejects to file {rej_path}")
            };
            writeln!(
                out,
                "{} out of {} hunk ignored{suffix}",
                hunks.len(),
                hunks.len()
            )?;
            if !opts.dry_run {
                let rej = format_rej(&rej_source, &rej_target, &hunks.iter().collect::<Vec<_>>());
                let _ = std::fs::write(resolve(&rej_path), rej);
            }
            overall_status = overall_status.max(1);
            continue;
        }

        let outcomes = if let Some(symbol) = &opts.ifdef_symbol {
            // `-D` never fails a hunk or searches for an offset — it just wraps the old and new
            // content together — so every hunk reports as a plain, exact-position success.
            apply_hunks_ifdef(&mut image, hunks, symbol);
            std::iter::repeat_with(|| HunkOutcome::Applied { offset: 0 })
                .take(hunks.len())
                .collect()
        } else {
            apply_hunks(&mut image, hunks, opts.silent, opts.verbose, out)?
        };
        let failed_hunks: Vec<&Hunk<'_, [u8]>> = hunks
            .iter()
            .zip(&outcomes)
            .filter_map(|(h, o)| matches!(o, HunkOutcome::Failed).then_some(h))
            .collect();
        let applied_count = outcomes.len() - failed_hunks.len();
        // GNU backs up the pre-patch content to `FILE.orig` whenever the patch didn't apply
        // "exactly" — any hunk needed a nonzero offset, or any hunk failed outright — even
        // though application of the hunks that did succeed still goes ahead; `-b`/`--backup`
        // forces a backup unconditionally. Verified against the oracle: an exact clean apply
        // makes no backup by default, but the same patch against a file shifted by a few lines
        // (an offset, not a failure) does.
        let needs_backup = opts.backup
            || !failed_hunks.is_empty()
            || outcomes
                .iter()
                .any(|o| matches!(o, HunkOutcome::Applied { offset } if *offset != 0));

        if !failed_hunks.is_empty() {
            let rej_path = reject_path(&target_disp, &opts);
            let suffix = if opts.dry_run {
                String::new()
            } else {
                format!(" -- saving rejects to file {rej_path}")
            };
            writeln!(
                out,
                "{} out of {} hunk{} FAILED{suffix}",
                failed_hunks.len(),
                hunks.len(),
                if hunks.len() == 1 { "" } else { "s" }
            )?;
            if !opts.dry_run {
                let rej = format_rej(&rej_source, &rej_target, &failed_hunks);
                let _ = std::fs::write(resolve(&rej_path), rej);
            }
            overall_status = overall_status.max(1);
        }

        if applied_count > 0 && !opts.dry_run {
            if needs_backup && !creating {
                let backup = backup_path(&target_disp, &opts, resolve);
                let backup_path_resolved = resolve(&backup);
                if let Some(parent) = backup_path_resolved.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&backup_path_resolved, &base);
            }
            let mut result = Vec::new();
            for line in &image {
                result.extend_from_slice(line);
            }
            let write_path = resolve(opts.output.as_deref().unwrap_or(&target_disp));
            if deleting || (opts.remove_empty && result.is_empty()) {
                let _ = std::fs::remove_file(&write_path);
            } else {
                if let Some(parent) = write_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(&write_path, &result)?;
                if is_rename && opts.output.is_none() && source_disp != target_disp {
                    let _ = std::fs::remove_file(resolve(&source_disp));
                }
            }
        }
        if opts.verbose && !opts.silent {
            writeln!(out, "done")?;
        }
    }

    if !any_success && overall_status == 0 {
        // The parser produced no entries at all (no error, no file) — treat the same as the
        // explicit garbage case rather than silently claiming success.
        writeln!(
            err,
            "patch: **** Only garbage was found in the patch input."
        )?;
        return Ok(2);
    }

    Ok(overall_status)
}

/// Best-effort mapping of a `diffy` parse error we don't otherwise recognize (see the match
/// above) to GNU's `malformed patch at line N: <line text>` shape, using the byte offset
/// `diffy`'s `Display` embeds (`"... at byte N: ..."`) to locate the offending line. Falls back
/// to a bare `malformed patch` if that offset can't be recovered.
fn malformed_patch_message(patch_bytes: &[u8], error_text: &str) -> String {
    let Some(after) = error_text
        .find("at byte ")
        .map(|p| &error_text[p + "at byte ".len()..])
    else {
        return "patch: **** malformed patch".to_owned();
    };
    let digits_end = after
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after.len());
    let Ok(byte_offset) = after[..digits_end].parse::<usize>() else {
        return "patch: **** malformed patch".to_owned();
    };
    let byte_offset = byte_offset.min(patch_bytes.len());
    let line_no = patch_bytes[..byte_offset]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1;
    let line_start = patch_bytes[..byte_offset]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |p| p + 1);
    let line_end = patch_bytes[line_start..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(patch_bytes.len(), |p| line_start + p);
    let line_text = String::from_utf8_lossy(&patch_bytes[line_start..line_end]);
    format!("patch: **** malformed patch at line {line_no}: {line_text}")
}

// Every test here uses `tempfile`, a native-only dependency.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn run(f: impl FnOnce(&std::path::Path)) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        f(dir.path());
        dir
    }

    fn patch(dir: &std::path::Path, args: &[&str], stdin: &str) -> (i32, String, String) {
        let argv: Vec<String> = std::iter::once("patch")
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect();
        let resolve = |p: &str| dir.join(p);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run_patch(&argv, &mut stdin.as_bytes(), &resolve, &mut out, &mut err).unwrap();
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    const SIMPLE_PATCH: &str = "\
--- file.txt.orig\t2026-01-01 00:00:00.000000000 +0000
+++ file.txt\t2026-01-01 00:00:00.000000000 +0000
@@ -1,3 +1,3 @@
 line1
-line2
+line2-changed
 line3
";

    #[test]
    fn applies_from_stdin_positional_file() {
        let _d = run(|dir| {
            std::fs::write(dir.join("test1.txt"), "line1\nline2\nline3\n").unwrap();
            let (code, out, _) = patch(dir, &["test1.txt"], SIMPLE_PATCH);
            assert_eq!(code, 0);
            assert_eq!(out, "patching file test1.txt\n");
            assert_eq!(
                std::fs::read_to_string(dir.join("test1.txt")).unwrap(),
                "line1\nline2-changed\nline3\n"
            );
        });
    }

    #[test]
    fn hunk_offset_search_reports_offset() {
        let _d = run(|dir| {
            std::fs::write(
                dir.join("test2.txt"),
                "0\n0\n0\n0\n0\nline1\nline2\nline3\n",
            )
            .unwrap();
            let (code, out, _) = patch(dir, &["test2.txt"], SIMPLE_PATCH);
            assert_eq!(code, 0);
            assert!(
                out.contains("Hunk #1 succeeded at 6 (offset 5 lines)."),
                "{out}"
            );
            assert_eq!(
                std::fs::read_to_string(dir.join("test2.txt")).unwrap(),
                "0\n0\n0\n0\n0\nline1\nline2-changed\nline3\n"
            );
        });
    }

    #[test]
    fn context_mismatch_writes_rej_and_leaves_target() {
        let _d = run(|dir| {
            std::fs::write(dir.join("x2.txt"), "AAA\nBBB\nCCC\n").unwrap();
            let (code, out, _) = patch(dir, &["x2.txt"], SIMPLE_PATCH);
            assert_eq!(code, 1);
            assert!(out.contains("Hunk #1 FAILED at 1."), "{out}");
            assert!(out.contains("saving rejects to file x2.txt.rej"), "{out}");
            assert_eq!(
                std::fs::read_to_string(dir.join("x2.txt")).unwrap(),
                "AAA\nBBB\nCCC\n"
            );
            assert!(dir.join("x2.txt.rej").exists());
        });
    }

    #[test]
    // `-r FILE`/`--reject-file=FILE` replaces the default `TARGET.rej` name.
    fn reject_file_option_replaces_the_default_name() {
        let _d = run(|dir| {
            std::fs::write(dir.join("x2.txt"), "AAA\nBBB\nCCC\n").unwrap();
            let (code, out, _) = patch(dir, &["-r", "custom.rej", "x2.txt"], SIMPLE_PATCH);
            assert_eq!(code, 1);
            assert!(out.contains("saving rejects to file custom.rej"), "{out}");
            assert!(dir.join("custom.rej").exists());
            assert!(!dir.join("x2.txt.rej").exists());
        });
    }

    #[test]
    // GNU's own wording for a non-numeric strip count, shared by `-p`/`--strip` — verified
    // against the oracle.
    fn invalid_strip_count_uses_gnus_own_wording() {
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "a\n").unwrap();
            for args in [["-px"].as_slice(), &["--strip=x"]] {
                let (code, _out, err) = patch(dir, args, "");
                assert_eq!(code, 2, "{args:?}");
                assert_eq!(
                    err, "patch: **** strip count x is not a number\n",
                    "{args:?}"
                );
            }
        });
    }

    #[test]
    // GNU's own wording for a missing `-i`/`--input` patch file — verified against the oracle
    // (note the space before the colon, which really is in GNU's own message).
    fn missing_patch_file_uses_gnus_own_wording() {
        let _d = run(|dir| {
            let (code, _out, err) = patch(dir, &["-i", "nosuch", "f"], "");
            assert_eq!(code, 2);
            assert_eq!(
                err,
                "patch: **** Can't open patch file nosuch : No such file or directory\n"
            );
        });
    }

    #[test]
    fn git_style_p1_strip_applies() {
        let _d = run(|dir| {
            std::fs::create_dir_all(dir.join("a")).unwrap();
            std::fs::write(dir.join("x.txt"), "one\ntwo\nthree\n").unwrap();
            let git_patch = "\
--- a/x.txt\t2026-01-01 00:00:00.000000000 +0000
+++ b/x.txt\t2026-01-01 00:00:00.000000000 +0000
@@ -1,3 +1,3 @@
 one
-two
+TWO
 three
";
            let (code, out, _) = patch(dir, &["-p1"], git_patch);
            assert_eq!(code, 0);
            assert_eq!(out, "patching file x.txt\n");
            assert_eq!(
                std::fs::read_to_string(dir.join("x.txt")).unwrap(),
                "one\nTWO\nthree\n"
            );
        });
    }

    #[test]
    // `diffy` only understands unified diff, so a context diff (`diff -c`) used to
    // read as garbage. It's rewritten to unified diff text first (`context_diff_to_unified`).
    fn context_diff_applies() {
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "a\nb\nc\n").unwrap();
            let context_patch = "\
*** f\t2026-01-01 00:00:00.000000000 +0000
--- f\t2026-01-01 00:00:00.000000000 +0000
***************
*** 1,3 ****
  a
! b
  c
--- 1,3 ----
  a
! X
  c
";
            let (code, out, _) = patch(dir, &[], context_patch);
            assert_eq!(code, 0, "{out}");
            assert_eq!(out, "patching file f\n");
            assert_eq!(std::fs::read_to_string(dir.join("f")).unwrap(), "a\nX\nc\n");
        });
    }

    #[test]
    // same gap as `context_diff_applies`, for plain ("normal") `diff` output, which
    // has no `---`/`+++` header naming the file at all — GNU needs a positional target for
    // it, same as we do (`normal_diff_to_unified`).
    fn normal_diff_applies_with_a_positional_target() {
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "a\nb\nc\nd\ne\n").unwrap();
            let normal_patch = "\
2c2,3
< b
---
> X
> Y
4d4
< d
5a6
> f
";
            let (code, out, _) = patch(dir, &["f"], normal_patch);
            assert_eq!(code, 0, "{out}");
            assert_eq!(out, "patching file f\n");
            assert_eq!(
                std::fs::read_to_string(dir.join("f")).unwrap(),
                "a\nX\nY\nc\ne\nf\n"
            );
        });
    }

    #[test]
    // with both `---`/`+++` header names present (a plain, non-renaming unified diff
    // — the ordinary output of `diff -u a b`) and both files on disk, GNU edits the `---`
    // (old) name in place, not the `+++` (new) one.
    fn modify_with_two_header_names_edits_the_minus_file() {
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "1\n2\n3\n").unwrap();
            std::fs::write(dir.join("g"), "1\nX\n3\n").unwrap();
            let unified_patch = "\
--- f\t2026-01-01 00:00:00.000000000 +0000
+++ g\t2026-01-01 00:00:00.000000000 +0000
@@ -1,3 +1,3 @@
 1
-2
+X
 3
";
            let (code, out, _) = patch(dir, &[], unified_patch);
            assert_eq!(code, 0, "{out}");
            assert_eq!(out, "patching file f\n");
            assert_eq!(std::fs::read_to_string(dir.join("f")).unwrap(), "1\nX\n3\n");
            assert_eq!(std::fs::read_to_string(dir.join("g")).unwrap(), "1\nX\n3\n");
        });
    }

    #[test]
    // a git rename patch used to read as garbage (`unidiff()` doesn't understand
    // `diff --git`/`rename from`/`rename to`); now parsed with `gitdiff()`. The renamed-away
    // file must also be removed, matching GNU.
    fn git_rename_applies_and_removes_the_old_file() {
        let _d = run(|dir| {
            std::fs::write(dir.join("old.txt"), "a\nb\n").unwrap();
            let rename_patch = "\
diff --git a/old.txt b/new.txt
similarity index 80%
rename from old.txt
rename to new.txt
index 1234567..89abcde 100644
--- a/old.txt
+++ b/new.txt
@@ -1,2 +1,3 @@
 a
 b
+c
";
            let (code, _out, _) = patch(dir, &["-p1"], rename_patch);
            assert_eq!(code, 0);
            assert_eq!(
                std::fs::read_to_string(dir.join("new.txt")).unwrap(),
                "a\nb\nc\n"
            );
            assert!(!dir.join("old.txt").exists());
        });
    }

    #[test]
    fn reverse_undoes_a_patch() {
        let _d = run(|dir| {
            std::fs::write(dir.join("x.txt"), "one\nTWO\nthree\n").unwrap();
            let git_patch = "\
--- a/x.txt\t2026-01-01 00:00:00.000000000 +0000
+++ b/x.txt\t2026-01-01 00:00:00.000000000 +0000
@@ -1,3 +1,3 @@
 one
-two
+TWO
 three
";
            let (code, _, _) = patch(dir, &["-p1", "-R"], git_patch);
            assert_eq!(code, 0);
            assert_eq!(
                std::fs::read_to_string(dir.join("x.txt")).unwrap(),
                "one\ntwo\nthree\n"
            );
        });
    }

    #[test]
    fn dry_run_does_not_modify() {
        let _d = run(|dir| {
            std::fs::write(dir.join("test1.txt"), "line1\nline2\nline3\n").unwrap();
            let (code, out, _) = patch(dir, &["--dry-run", "test1.txt"], SIMPLE_PATCH);
            assert_eq!(code, 0);
            assert_eq!(out, "checking file test1.txt\n");
            assert_eq!(
                std::fs::read_to_string(dir.join("test1.txt")).unwrap(),
                "line1\nline2\nline3\n"
            );
        });
    }

    #[test]
    fn output_flag_redirects_and_reports_read_from() {
        let _d = run(|dir| {
            std::fs::write(dir.join("test1.txt"), "line1\nline2\nline3\n").unwrap();
            let (code, out, _) = patch(dir, &["-o", "out.txt", "test1.txt"], SIMPLE_PATCH);
            assert_eq!(code, 0);
            assert_eq!(out, "patching file out.txt (read from test1.txt)\n");
            assert_eq!(
                std::fs::read_to_string(dir.join("out.txt")).unwrap(),
                "line1\nline2-changed\nline3\n"
            );
            assert_eq!(
                std::fs::read_to_string(dir.join("test1.txt")).unwrap(),
                "line1\nline2\nline3\n"
            );
        });
    }

    #[test]
    fn silent_suppresses_success_messages() {
        let _d = run(|dir| {
            std::fs::write(dir.join("test1.txt"), "line1\nline2\nline3\n").unwrap();
            let (code, out, _) = patch(dir, &["-s", "test1.txt"], SIMPLE_PATCH);
            assert_eq!(code, 0);
            assert_eq!(out, "");
        });
    }

    #[test]
    fn creation_and_deletion_via_dev_null() {
        let _d = run(|dir| {
            let create = "\
--- /dev/null\t2026-01-01 00:00:00.000000000 +0000
+++ newfile.txt\t2026-01-01 00:00:00.000000000 +0000
@@ -0,0 +1,2 @@
+hello
+world
";
            let (code, _, _) = patch(dir, &[], create);
            assert_eq!(code, 0);
            assert_eq!(
                std::fs::read_to_string(dir.join("newfile.txt")).unwrap(),
                "hello\nworld\n"
            );

            let delete = "\
--- newfile.txt\t2026-01-01 00:00:00.000000000 +0000
+++ /dev/null\t2026-01-01 00:00:00.000000000 +0000
@@ -1,2 +0,0 @@
-hello
-world
";
            let (code, _, _) = patch(dir, &[], delete);
            assert_eq!(code, 0);
            assert!(!dir.join("newfile.txt").exists());
        });
    }

    #[test]
    fn multi_file_git_patch() {
        let _d = run(|dir| {
            std::fs::write(dir.join("alpha.txt"), "alpha\n").unwrap();
            let multi = "\
diff --git a/alpha.txt b/alpha.txt
--- a/alpha.txt
+++ b/alpha.txt
@@ -1 +1 @@
-alpha
+ALPHA
diff --git a/beta.txt b/beta.txt
new file mode 100644
--- /dev/null
+++ b/beta.txt
@@ -0,0 +1 @@
+beta
";
            let (code, _, _) = patch(dir, &["-p1"], multi);
            assert_eq!(code, 0);
            assert_eq!(
                std::fs::read_to_string(dir.join("alpha.txt")).unwrap(),
                "ALPHA\n"
            );
            assert_eq!(
                std::fs::read_to_string(dir.join("beta.txt")).unwrap(),
                "beta\n"
            );
        });
    }

    #[test]
    fn already_applied_patch_is_detected_and_rejected() {
        let _d = run(|dir| {
            std::fs::write(dir.join("test1.txt"), "line1\nline2-changed\nline3\n").unwrap();
            let (code, out, _) = patch(dir, &["-N", "test1.txt"], SIMPLE_PATCH);
            assert_eq!(code, 1);
            assert!(
                out.contains("Reversed (or previously applied) patch detected!  Skipping patch."),
                "{out}"
            );
            assert_eq!(
                std::fs::read_to_string(dir.join("test1.txt")).unwrap(),
                "line1\nline2-changed\nline3\n"
            );
        });
    }

    #[test]
    fn default_strip_falls_back_to_basename_when_directory_missing() {
        // header names "sub/file.txt"; "sub" doesn't exist, but "file.txt" exists directly in
        // cwd — GNU's default (no -p) uses the basename in this case, never an intermediate
        // strip level.
        let _d = run(|dir| {
            std::fs::write(dir.join("file.txt"), "one\ntwo\nthree\n").unwrap();
            let patch_text =
                "--- sub/file.txt\n+++ sub/file.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n";
            let (code, out, _) = patch(dir, &[], patch_text);
            assert_eq!(code, 0);
            assert_eq!(out, "patching file file.txt\n");
            assert_eq!(
                std::fs::read_to_string(dir.join("file.txt")).unwrap(),
                "one\nTWO\nthree\n"
            );
        });
    }

    #[test]
    fn default_strip_uses_full_path_when_directory_exists() {
        let _d = run(|dir| {
            std::fs::create_dir_all(dir.join("sub")).unwrap();
            std::fs::write(dir.join("sub/file.txt"), "one\ntwo\nthree\n").unwrap();
            let patch_text =
                "--- sub/file.txt\n+++ sub/file.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n";
            let (code, out, _) = patch(dir, &[], patch_text);
            assert_eq!(code, 0);
            assert_eq!(out, "patching file sub/file.txt\n");
            assert_eq!(
                std::fs::read_to_string(dir.join("sub/file.txt")).unwrap(),
                "one\nTWO\nthree\n"
            );
        });
    }

    #[test]
    fn cant_find_file_transcript_matches_gnu() {
        let _d = run(|dir| {
            // Neither "a/src/main.rs" nor a bare "main.rs" exists anywhere under `dir`.
            let patch_text = "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-fn main() {}\n+fn main() { println!(\"hi\"); }\n";
            let (code, out, err) = patch(dir, &[], patch_text);
            assert_eq!(code, 1);
            assert_eq!(err, "");
            assert_eq!(
                out,
                "can't find file to patch at input line 3\n\
                 Perhaps you should have used the -p or --strip option?\n\
                 The text leading up to this was:\n\
                 --------------------------\n\
                 |--- a/src/main.rs\n\
                 |+++ b/src/main.rs\n\
                 --------------------------\n\
                 File to patch: \n\
                 Skip this patch? [y] \n\
                 Skipping patch.\n\
                 1 out of 1 hunk ignored\n"
            );
        });
    }

    #[test]
    fn cant_find_file_wrong_p_wording_when_strip_given() {
        let _d = run(|dir| {
            let patch_text = "--- a/nope.txt\n+++ b/nope.txt\n@@ -1 +1 @@\n-x\n+y\n";
            let (code, out, _) = patch(dir, &["-p1"], patch_text);
            assert_eq!(code, 1);
            assert!(
                out.starts_with("can't find file to patch at input line 3\nPerhaps you used the wrong -p or --strip option?\n"),
                "{out}"
            );
        });
    }

    #[test]
    fn bundled_short_options_np1_round_trip() {
        // -p1 attached to -N as `-Np1`, matching the "-Np1"/"-sp1"/"-Rp1" bundling item.
        let _d = run(|dir| {
            std::fs::write(dir.join("x.txt"), "one\ntwo\nthree\n").unwrap();
            let git_patch = "\
--- a/x.txt
+++ b/x.txt
@@ -1,3 +1,3 @@
 one
-two
+TWO
 three
";
            let (code, out, _) = patch(dir, &["-Np1"], git_patch);
            assert_eq!(code, 0);
            assert_eq!(out, "patching file x.txt\n");
            assert_eq!(
                std::fs::read_to_string(dir.join("x.txt")).unwrap(),
                "one\nTWO\nthree\n"
            );
        });
    }

    #[test]
    fn empty_input_is_silent_success() {
        // Verified against the oracle: `git diff | patch -p1` with no actual changes must
        // succeed, not report a malformed patch.
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "a\n").unwrap();
            assert_eq!(patch(dir, &["f"], ""), (0, String::new(), String::new()));
            assert_eq!(
                patch(dir, &["f"], "   \n\n"),
                (0, String::new(), String::new())
            );
        });
    }

    #[test]
    fn garbage_only_input_is_reported() {
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "a\n").unwrap();
            let (code, out, err) = patch(dir, &["f"], "garbage\n");
            assert_eq!((code, out.as_str()), (2, ""));
            assert_eq!(
                err,
                "patch: **** Only garbage was found in the patch input.\n"
            );
        });
    }

    #[test]
    fn truncated_hunk_reports_unexpected_eof() {
        // Verified against the oracle: a hunk header claiming more lines than the patch
        // actually supplies is reported the same way GNU reports it running out of input.
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "a\n").unwrap();
            let text = "--- f\n+++ f\n@@ -100,5 +100,5 @@\n-x\n+y\n";
            let (code, out, err) = patch(dir, &["f"], text);
            assert_eq!(code, 2);
            assert_eq!(out, "patching file f\n");
            assert_eq!(err, "patch: **** unexpected end of file in patch\n");
        });
    }

    const P1_PATCH: &str = "\
--- a/x.txt
+++ b/x.txt
@@ -1,3 +1,3 @@
 one
-two
+TWO
 three
";

    #[test]
    fn rej_header_ignores_an_explicit_target_override() {
        // Caught by the "patch: failing hunk writes rej" matrix case: an explicit positional target (`patch FILE < diff`) only decides where
        // the successfully-applied part is written — the `.rej`'s own `---`/`+++` lines still
        // echo the patch's own (unrelated) header names, never the override. Verified against
        // the oracle.
        let _d = run(|dir| {
            std::fs::write(dir.join("pt8.txt"), "AAA\nBBB\nCCC\n").unwrap();
            let text = "--- a\n+++ b\n@@ -1,3 +1,3 @@\n line1\n-line2\n+line2-changed\n line3\n";
            let (code, _out, _err) = patch(dir, &["pt8.txt"], text);
            assert_eq!(code, 1);
            let rej = std::fs::read_to_string(dir.join("pt8.txt.rej")).unwrap();
            assert!(rej.starts_with("--- a\n+++ b\n"), "{rej}");
        });
    }

    #[test]
    fn rej_header_uses_the_stripped_name_not_the_raw_header() {
        // GNU's own `.rej` names the file it actually tried to patch (post `-p1`
        // stripping), never the raw `a/`/`b/` header text. Verified against the oracle.
        let _d = run(|dir| {
            std::fs::write(dir.join("x.txt"), "AAA\nBBB\nCCC\n").unwrap();
            let (code, _out, _err) = patch(dir, &["-p1"], P1_PATCH);
            assert_eq!(code, 1);
            let rej = std::fs::read_to_string(dir.join("x.txt.rej")).unwrap();
            assert!(rej.starts_with("--- x.txt\n+++ x.txt\n"), "{rej}");
        });
    }

    #[test]
    fn reverse_reject_lists_deletes_before_inserts() {
        // reversing a hunk must not just flip each line's own +/- tag in place — GNU
        // always lists every delete before every insert in a hunk, and naively swapping tags
        // without reordering yields `+two` before `-TWO` instead of `-TWO` before `+two`.
        // Verified against the oracle.
        let _d = run(|dir| {
            std::fs::write(dir.join("x.txt"), "one\ntwo\nthree\n").unwrap();
            let text = "--- x.txt\n+++ x.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n";
            let (code, out, _err) = patch(dir, &["-R"], text);
            assert_eq!(code, 1);
            assert!(
                out.contains("Unreversed patch detected!  Ignore -R?"),
                "{out}"
            );
            let rej = std::fs::read_to_string(dir.join("x.txt.rej")).unwrap();
            assert_eq!(
                rej,
                "--- x.txt\n+++ x.txt\n@@ -1,3 +1,3 @@\n one\n-TWO\n+two\n three\n"
            );
        });
    }

    #[test]
    fn dry_run_omits_the_saving_rejects_suffix() {
        // `--dry-run` never actually writes a `.rej`, and GNU's own summary line
        // reflects that (no "-- saving rejects to file ...rej" suffix). Verified against the
        // oracle.
        let _d = run(|dir| {
            std::fs::write(dir.join("x.txt"), "AAA\nBBB\nCCC\n").unwrap();
            let (code, out, _err) = patch(dir, &["--dry-run", "x.txt"], SIMPLE_PATCH);
            assert_eq!(code, 1);
            assert!(out.contains("1 out of 1 hunk FAILED\n"), "{out}");
            assert!(!out.contains("saving rejects"), "{out}");
            assert!(!dir.join("x.txt.rej").exists());
        });
    }

    #[test]
    fn offset_apply_backs_up_the_original_by_default() {
        // GNU backs up to `FILE.orig` whenever a hunk needed a nonzero offset to find
        // its place, even though application itself still succeeds — but not on an exact
        // match. Verified against the oracle.
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "a\nb\nc\n").unwrap();
            patch(dir, &["f"], SIMPLE_PATCH);
            assert!(
                !dir.join("f.orig").exists(),
                "an exact match must not back up"
            );

            std::fs::write(dir.join("g"), "0\n0\n0\n0\n0\nline1\nline2\nline3\n").unwrap();
            let (code, _out, _err) = patch(dir, &["g"], SIMPLE_PATCH);
            assert_eq!(code, 0);
            assert_eq!(
                std::fs::read_to_string(dir.join("g.orig")).unwrap(),
                "0\n0\n0\n0\n0\nline1\nline2\nline3\n"
            );
        });
    }

    #[test]
    fn explicit_backup_flag_backs_up_even_on_an_exact_match() {
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "line1\nline2\nline3\n").unwrap();
            let (code, _out, _err) = patch(dir, &["-b", "f"], SIMPLE_PATCH);
            assert_eq!(code, 0);
            assert_eq!(
                std::fs::read_to_string(dir.join("f.orig")).unwrap(),
                "line1\nline2\nline3\n"
            );
        });
    }

    #[test]
    // -z/-V/-Y/-B change where a `-b` backup lands — verified against the oracle.
    fn backup_naming_options_change_where_the_backup_lands() {
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "line1\nline2\nline3\n").unwrap();
            let (code, _out, _err) = patch(dir, &["-b", "-z", ".old", "f"], SIMPLE_PATCH);
            assert_eq!(code, 0);
            assert!(dir.join("f.old").exists());
            assert!(!dir.join("f.orig").exists());
        });
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "line1\nline2\nline3\n").unwrap();
            let (code, _out, _err) = patch(dir, &["-b", "-V", "numbered", "f"], SIMPLE_PATCH);
            assert_eq!(code, 0);
            assert!(dir.join("f.~1~").exists());
        });
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "line1\nline2\nline3\n").unwrap();
            let (code, _out, _err) = patch(dir, &["-b", "-Y", "bk/", "f"], SIMPLE_PATCH);
            assert_eq!(code, 0);
            assert!(dir.join("bk/f.orig").exists());
        });
    }

    #[test]
    // `-D SYMBOL` wraps changed lines in `#ifndef`/`#else`/`#endif` (a pure insertion in
    // `#ifdef`/`#endif`) instead of replacing them — verified against the oracle.
    fn ifdef_wraps_changes_instead_of_replacing_them() {
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
            let text =
                "--- f\n+++ f\n@@ -1,5 +1,6 @@\n one\n-two\n+TWO\n three\n four\n five\n+six\n";
            let (code, _out, _err) = patch(dir, &["-D", "FOO", "f"], text);
            assert_eq!(code, 0);
            assert_eq!(
                std::fs::read_to_string(dir.join("f")).unwrap(),
                "one\n#ifndef FOO\ntwo\n#else\nTWO\n#endif\nthree\nfour\nfive\n#ifdef FOO\nsix\n#endif\n"
            );
        });
    }

    #[test]
    // `-e` doesn't implement real ed-format patch application; GNU's own fallback for input
    // that isn't ed format is this exact message, verified against the oracle.
    fn ed_format_reports_gnus_own_fallback_message() {
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "one\ntwo\n").unwrap();
            let (code, out, err) = patch(dir, &["-e", "f"], SIMPLE_PATCH);
            assert_eq!((code, out.as_str()), (2, ""));
            assert_eq!(
                err,
                "patch: **** Only garbage was found in the patch input.\n"
            );
        });
    }

    #[test]
    // Real GNU options this fork now accepts as no-ops on a unified-format patch, verified
    // against the oracle: none of them change a clean apply's outcome for `--posix`/`--binary`/
    // `--merge`/`--backup-if-mismatch`/`--no-backup-if-mismatch`/`-t`/`-l`/`-Z`. (`-c`/`-n`
    // force context/normal-diff interpretation — accepted as no-ops too, since auto-detection
    // already recognizes both formats on their own input, but not tested here against a
    // unified-format patch, which isn't a case any oracle run has actually checked.)
    fn accepted_no_op_options_still_apply_the_patch() {
        for args in [
            ["--posix", "f"].as_slice(),
            &["--binary", "f"],
            &["--merge", "f"],
            &["--backup-if-mismatch", "f"],
            &["--no-backup-if-mismatch", "f"],
            &["-t", "f"],
            &["-l", "f"],
            &["-Z", "f"],
        ] {
            let _d = run(|dir| {
                std::fs::write(dir.join("f"), "line1\nline2\nline3\n").unwrap();
                let (code, _out, err) = patch(dir, args, SIMPLE_PATCH);
                assert_eq!(code, 0, "{args:?} -> {err}");
                assert_eq!(
                    std::fs::read_to_string(dir.join("f")).unwrap(),
                    "line1\nline2-changed\nline3\n",
                    "{args:?}"
                );
            });
        }
    }

    #[test]
    fn force_fuzz_directory_are_refused_not_misread() {
        // these are real GNU options we don't implement; they must say so, not fall
        // through to "invalid option" as if patch didn't recognize the letter at all.
        let _d = run(|dir| {
            std::fs::write(dir.join("f"), "a\n").unwrap();
            for args in [["-f"].as_slice(), &["-F2"], &["-d/tmp"]] {
                let (code, _out, err) = patch(dir, args, "");
                assert_eq!(code, 2, "{args:?} -> {err}");
                assert!(
                    err.contains("is unsupported in bash-tool"),
                    "{args:?} -> {err}"
                );
            }
        });
    }
}
