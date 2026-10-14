//! `sed`: the uutils sed engine, driven one line at a time.
//!
//! uutils sed (forked with a line-at-a-time engine) supplies GNU-compatible scripts, regex
//! dialects and error messages. The shell delivers input itself, so an endless producer still
//! reaches `q`. `-i` and `-s` need whole files and run through uutils' own entry point instead.

use std::io::{BufRead, Write};
use uu_sed::sed::incremental::{Engine, Flow};

/// A refusal before any input is read: sed's message and status.
pub(crate) struct Refusal {
    pub(crate) code: i32,
    pub(crate) message: String,
}

/// Whether `argv` asks to read the sed script itself from stdin: `-f -`, `--script-file -`
/// or `--script-file=-` — the same `-` convention every other file operand in this tool
/// uses for "read the pipeline" — or `/dev/stdin` in their place. The fork's own script-file reader treats `-` as stdin too,
/// but reads the process's real stdin directly rather than the shell's piped/redirected
/// one; the caller must stage that in for the compile (see
/// `coreutils::with_shell_stdin_staged`) exactly when this is true, since draining the
/// shell's stdin when nothing asked for it would starve the main input's own `-` operand.
///
/// Only the wasm cooperative driver needs this: on native, `run_uu`'s process-fd swap (or,
/// outside `-i`/`-s`, the plain process stdin) already lines up without it.
#[cfg(target_arch = "wasm32")]
pub(crate) fn wants_stdin_script(argv: &[String]) -> bool {
    // `-` or a name for standard input, which the staged input serves (see `devices`).
    let stdin = |value: &str| {
        value == "-" || super::devices::operand(value) == Some(super::devices::Device::Stream(0))
    };
    let mut args = argv.iter();
    while let Some(arg) = args.next() {
        if (arg == "-f" || arg == "--script-file") && args.next().is_some_and(|value| stdin(value))
        {
            return true;
        }
        if arg.strip_prefix("--script-file=").is_some_and(stdin) {
            return true;
        }
    }
    false
}

/// The locale sed runs in: the first non-empty of the exported `LC_ALL`, `LC_CTYPE` and `LANG`,
/// else UTF-8. A process-level `C` default would make `.` split multibyte characters.
pub(crate) fn locale(env: &[(String, String)]) -> String {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|name| {
            env.iter()
                .find(|(key, value)| key == name && !value.is_empty())
                .map(|(_, value)| value.clone())
        })
        .unwrap_or_else(|| "C.UTF-8".to_owned())
}

/// Parse and compile `argv` (the command name first) in `locale`. The operands are returned as
/// given; `-` is standard input.
pub(crate) fn prepare(argv: &[String], locale: String) -> Result<(Engine, Vec<String>), Refusal> {
    // Also governs `uumain` runs (`-i`, `-s`) that follow on this thread.
    uu_sed::sed::set_locale(Some(locale));
    // `OsString::from` would keep the shell's own marker characters for a raw (non-UTF-8) byte
    // literally, rather than the byte they stand for (see `shell_bytes`): a script built from
    // one (`x=$'\xff'; sed "s/$x/X/"`) would then search for the marker's own UTF-8 encoding,
    // never the byte actually in the data. `to_os_string` decodes it to the real byte instead,
    // which the fork's own `script`/`expression` arguments (now `OsString`-valued, not
    // `String`) and `ScriptValue::StringVal` (now `Vec<u8>`) can carry all the way through.
    let args = argv
        .iter()
        .map(|arg| crate::tools::shell_bytes::to_os_string(arg));
    match Engine::new(args) {
        Ok((engine, files)) => Ok((
            engine,
            files
                .into_iter()
                .map(|file| file.to_string_lossy().into_owned())
                .collect(),
        )),
        Err(error) => {
            let text = error.to_string();
            // the fork's own compiler already refuses `e`/`s///e` at compile time (no
            // input line is ever read), which already matches this tool's "refuse the whole
            // command before any of it runs" rule — but it does so with its own status (1) and
            // wording, not the `… is unsupported in bash-tool` convention every other refusal in
            // this tool uses (and the one the README documents for this exact feature).
            // Reworded and re-statused here rather than upstream in the fork, since every other
            // compile error should keep going through unchanged.
            if text.contains("the 'e' command and substitute flag are unsupported here") {
                return Err(Refusal {
                    code: 2,
                    message: "sed: the 'e' command and substitute flag are unsupported in bash-tool: no shell to run\n".to_owned(),
                });
            }
            // `--help`/`--version` come back through this same `Err` path (clap's own
            // control-flow convention — not really an error), and exit 0 exactly then (see
            // `Engine::new`'s own status-code note). GNU writes that text to stdout, bare, with
            // no `sed: ` prefix at all — unlike every other diagnostic here, which belongs on
            // stderr with one. `Refusal` doesn't carry a stream of its own, so the caller reads
            // `code == 0` as "stdout, no prefix" instead.
            let code = error.code();
            let message = if code == 0 {
                format!("{}\n", text.trim_end())
            } else {
                diagnostic(&text)
            };
            Err(Refusal { code, message })
        }
    }
}

/// clap reports usage errors with their own `error:` prefix; other errors get sed's name.
fn diagnostic(message: &str) -> String {
    let message = message.trim_end();
    if message.starts_with("error:") {
        format!("{message}\n")
    } else {
        format!("sed: {message}\n")
    }
}

/// GNU's wording for an operand that cannot be opened; sed reports it, continues, and exits 2.
pub(crate) fn unreadable(name: &str, error: &std::io::Error) -> String {
    format!("sed: can't read {name}: {}\n", super::io_message(error))
}

/// Drive `engine` over records, reading one ahead only when the script asks about the last
/// line. Returns sed's exit status.
pub(crate) fn run_records(
    engine: &mut Engine,
    records: &mut dyn Iterator<Item = std::io::Result<Vec<u8>>>,
    out: &mut dyn Write,
    err: &mut dyn Write,
    status: i32,
) -> std::io::Result<i32> {
    let mut output = Vec::new();
    let lookahead = engine.needs_last();
    let mut current = records.next().transpose()?;
    while let Some(record) = current {
        let next = if lookahead {
            records.next().transpose()?
        } else {
            None
        };
        output.clear();
        match engine.record(&record, lookahead && next.is_none(), &mut output) {
            Ok(Flow::Continue) => (),
            Ok(Flow::Quit) => {
                out.write_all(&output)?;
                break;
            }
            Err(error) => {
                out.write_all(&output)?;
                err.write_all(diagnostic(&error.to_string()).as_bytes())?;
                return Ok(error.code());
            }
        }
        out.write_all(&output)?;
        current = if lookahead {
            next
        } else {
            records.next().transpose()?
        };
    }
    output.clear();
    if let Err(error) = engine.finish(&mut output) {
        out.write_all(&output)?;
        err.write_all(diagnostic(&error.to_string()).as_bytes())?;
        return Ok(error.code());
    }
    out.write_all(&output)?;
    Ok(match engine.exit_code() {
        0 => status,
        code => code,
    })
}

/// Split a reader into records that keep their delimiter.
pub(crate) fn records(
    mut input: impl BufRead,
    delimiter: u8,
) -> impl Iterator<Item = std::io::Result<Vec<u8>>> {
    std::iter::from_fn(move || {
        let mut record = Vec::new();
        match input.read_until(delimiter, &mut record) {
            Ok(0) => None,
            Ok(_) => Some(Ok(record)),
            Err(error) => Some(Err(error)),
        }
    })
}

/// The native driver: operands are read synchronously. `open` resolves an operand against the
/// shell's working directory.
pub(crate) fn run_sync(
    engine: &mut Engine,
    files: &[String],
    stdin: &mut dyn std::io::Read,
    out: &mut dyn Write,
    err: &mut dyn Write,
    open: &dyn Fn(&str) -> std::io::Result<std::fs::File>,
) -> std::io::Result<i32> {
    let delimiter = engine.delimiter();
    let mut status = 0;
    let mut inputs: Vec<Box<dyn std::io::Read + '_>> = Vec::new();
    let mut stdin = Some(stdin);
    for file in files {
        if file == "-" {
            if let Some(stdin) = stdin.take() {
                inputs.push(Box::new(stdin));
            }
            continue;
        }
        match open(file) {
            Ok(handle) => inputs.push(Box::new(handle)),
            Err(error) => {
                err.write_all(unreadable(file, &error).as_bytes())?;
                status = 2;
            }
        }
    }
    let chained = inputs.into_iter().fold(
        Box::new(std::io::empty()) as Box<dyn std::io::Read + '_>,
        |all, next| Box::new(std::io::Read::chain(all, next)),
    );
    let mut records = records(std::io::BufReader::new(chained), delimiter);
    run_records(engine, &mut records, out, err, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sed(args: &[&str], input: &str) -> (i32, String, String) {
        let argv: Vec<String> = std::iter::once("sed")
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = match prepare(&argv, "C.UTF-8".into()) {
            Ok((mut engine, files)) => run_sync(
                &mut engine,
                &files,
                &mut input.as_bytes(),
                &mut out,
                &mut err,
                &|path| std::fs::File::open(path),
            )
            .unwrap(),
            Err(refusal) => {
                err.extend(refusal.message.bytes());
                refusal.code
            }
        };
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    #[cfg(target_arch = "wasm32")]
    // `-f -` (and its long-option spellings) must be recognized so the caller
    // stages the shell's own stdin in for the compile, instead of leaving the fork to read
    // the process's real (and on wasm, forbidden-to-read) stdin resource directly.
    fn wants_stdin_script_recognizes_every_spelling() {
        let owned = |args: &[&str]| args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(super::wants_stdin_script(&owned(&["sed", "-f", "-"])));
        assert!(super::wants_stdin_script(&owned(&[
            "sed",
            "--script-file",
            "-"
        ])));
        assert!(super::wants_stdin_script(&owned(&[
            "sed",
            "--script-file=-"
        ])));
        // Not the main input's own `-` operand, and not a script file merely named `-x`.
        assert!(!super::wants_stdin_script(&owned(&["sed", "s/a/b/", "-"])));
        assert!(!super::wants_stdin_script(&owned(&["sed", "-f", "-x"])));
        assert!(!super::wants_stdin_script(&owned(&["sed", "-e", "-"])));
    }

    #[test]
    fn basic_and_extended_regular_expressions() {
        assert_eq!(sed(&[r"s/\(a\)\(b\)/\2\1/"], "ab\n").1, "ba\n");
        assert_eq!(sed(&["-E", r"s/(a+)b/[\1]/"], "aab\n").1, "[aa]\n");
        assert_eq!(sed(&[r"s/a\|b/X/g"], "abc\n").1, "XXc\n");
        assert_eq!(sed(&["s/a+/X/"], "aa+\n").1, "aX\n");
    }

    #[test]
    fn commands_and_addresses() {
        assert_eq!(sed(&["-n", "$p"], "a\nb\nc").1, "c");
        assert_eq!(sed(&["n;d"], "a\nb\nc\n").1, "a\nc\n");
        assert_eq!(sed(&["1!G;h;$!d"], "a\nb\nc\n").1, "c\nb\na\n");
        assert_eq!(sed(&["y/abc/xyz/"], "cab\n").1, "zxy\n");
        assert_eq!(sed(&["2i\\\nnew"], "a\nb\n").1, "a\nnew\nb\n");
        assert_eq!(sed(&["0,/b/d"], "a\nb\nc\n").1, "c\n");
    }

    #[test]
    fn quit_status_and_errors() {
        assert_eq!(
            sed(&["2q5"], "a\nb\nc\n"),
            (5, "a\nb\n".into(), String::new())
        );
        let (code, out, err) = sed(&["s/a/b"], "a\n");
        assert_eq!((code, out.as_str()), (1, ""));
        assert!(
            err.starts_with("sed: ") && err.contains("unterminated"),
            "{err}"
        );
        // refused up front (no input line read — GNU semantics for `e` need a real
        // shell, which this tool doesn't have), with this tool's own `… is unsupported in
        // bash-tool` wording and status 2, not the fork's own status/wording verbatim.
        let (code, out, err) = sed(&["s/x/y/e"], "x\n");
        assert_eq!((code, out.as_str()), (2, ""));
        assert_eq!(
            err,
            "sed: the 'e' command and substitute flag are unsupported in bash-tool: no shell to run\n"
        );
        // The bare `e` command (not just `s///e`) is refused the same way.
        let (code, out, err) = sed(&["1e date"], "x\n");
        assert_eq!((code, out.as_str()), (2, ""));
        assert_eq!(
            err,
            "sed: the 'e' command and substitute flag are unsupported in bash-tool: no shell to run\n"
        );
        let (code, out, err) = sed(&["p", "/nonexistent/file", "-"], "x\n");
        assert_eq!((code, out.as_str()), (2, "x\nx\n"));
        assert_eq!(
            err,
            "sed: can't read /nonexistent/file: No such file or directory\n"
        );
    }
}
