//! The finite-invocation profile used by bash-tool. Validate code before Brush can run it,
//! including expanded code introduced by shell builtins. Background jobs are allowed and are
//! stopped when the invocation ends; coprocesses are refused.
use std::collections::HashMap;
use std::io::Read as _;
#[cfg(not(target_arch = "wasm32"))]
use std::io::Write as _;
use std::sync::LazyLock;

use brush_core::builtins::{BoxFuture, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionExitCode, ExecutionResult};
use brush_parser::ast::{Command, CompoundCommand, CompoundList};
use brush_parser::word::{Parameter, ParameterExpr, WordPiece};

impl super::Session {
    /// Require every shell operation to finish within its invocation. Background jobs are stopped
    /// when it ends; job control, history execution, and sourcing nonregular files are refused
    /// with exit 2.
    pub fn enable_stateless_mode(&mut self) {
        for name in [
            "eval",
            "source",
            ".",
            "alias",
            "trap",
            "bg",
            "fg",
            "wait",
            "fc",
            "shopt",
            "enable",
            "exec",
            "suspend",
            "read",
            "mapfile",
            "readarray",
            "jobs",
            "cd",
            "help",
            "bind",
            "compgen",
        ] {
            if let Some(registration) = self.shell.builtin_mut(name) {
                registration.execute_func = execute;
            }
        }
        // Commands this platform has no equivalent for are refused by name, not missing.
        for name in ["umask", "ulimit"] {
            let mut registration = ORIGINALS["true"].clone();
            registration.execute_func = execute;
            self.shell.register_builtin(name, registration);
        }
        self.shell.set_prompt_guard(Some(validate_prompt));
    }
}

/// Checks what a prompt string (`PS4`, `${x@P}`) expands before the shell runs any of it, as
/// `eval` checks its text: its command substitutions are code the script did not name. Text that
/// is not a word is the shell's to report as it expands it.
fn validate_prompt(text: &str) -> Result<(), String> {
    match brush_parser::word::parse(text, &brush_parser::ParserOptions::default()) {
        // Text too deep to parse is refused as any code nested too deeply is.
        Err(brush_parser::WordParseError::NestedTooDeeply) => {
            return Err(diagnostic(
                "bash",
                "shell code is nested too deeply for bash-tool",
            ));
        }
        Err(_) => return Ok(()),
        Ok(_) => {}
    }
    // A substitution in a prompt that does not parse runs nothing: the shell reports its syntax
    // error when it expands the prompt, as bash does, rather than the whole command being refused.
    PARSED_WHEN_RUN.with(|later| later.set(true));
    let result = word_at_depth(text, 0);
    PARSED_WHEN_RUN.with(|later| later.set(false));
    result.map_err(|refusal| diagnostic("bash", &refusal.text()))
}

thread_local! {
    /// Whether the code being checked is parsed only when it runs (a prompt's substitutions, a
    /// trap's action, a `mapfile` callback), so a syntax error in it is the shell's to report
    /// then (see [`validate_prompt`], [`validate_when_run`]).
    static PARSED_WHEN_RUN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// [`validate`] for code the shell parses only when it runs it, as bash does a trap's action and
/// a `mapfile` callback: a syntax error in it is reported when (and each time) it runs, so only
/// what bash-tool refuses is checked now.
fn validate_when_run(script: &str, origin: &str) -> Result<(), String> {
    PARSED_WHEN_RUN.with(|later| later.set(true));
    let result = validate(script, origin);
    PARSED_WHEN_RUN.with(|later| later.set(false));
    result
}

/// Why bash-tool refuses `name` with these arguments, if it does: a feature it does not
/// implement. The same check runs before a script starts, on the commands its text names
/// literally, and again when a command runs.
fn refusal(name: &str, words: &[String]) -> Option<String> {
    let option = |letters: &str| {
        words
            .iter()
            .take_while(|word| *word != "--")
            .any(|word| word.starts_with('-') && word.chars().skip(1).any(|c| letters.contains(c)))
    };
    match name {
        "bg" | "fg" | "fc" => Some(format!(
            "background/history execution ({name}) is unsupported in bash-tool"
        )),
        "umask" | "ulimit" => Some(format!("{name} is unsupported in bash-tool")),
        // `enable -n` alone only lists disabled builtins, and naming anything but one of bash's
        // builtins (`enable -n cat`) is bash's `not a shell builtin` error, not a disabling.
        "enable"
            if option("n")
                && words.iter().any(|word| {
                    !word.starts_with('-') && crate::tools::programs::is_bash_builtin(word)
                }) =>
        {
            Some("enable -n is unsupported in bash-tool".into())
        }
        // WASI cannot load a shared object, so there is no builtin to load (or unload).
        "enable" if option("f") => Some("enable -f is unsupported in bash-tool".into()),
        "exec" if exec_option(words, "acl") => {
            Some("exec -a, -c and -l are unsupported in bash-tool".into())
        }
        "wait" if option("pf") => Some("wait -p and wait -f are unsupported in bash-tool".into()),
        // It would stop the call's every process until a CONT nothing outside the call can send.
        "suspend" if option("f") => Some("suspend -f is unsupported in bash-tool".into()),
        // The rest are Brush's own "not yet implemented" placeholders (status 99): a feature
        // gap, not a WASI limit, but one this canonical refusal makes visible and documented
        // instead of leaking Brush's internals.
        "cd" if option("@") => Some("cd -@ is unsupported in bash-tool".into()),
        "help" if option("m") => Some("help -m is unsupported in bash-tool".into()),
        // `bind`'s *query* forms (list/find/print against readline's tables: -l, -p, -P, -s,
        // -S, -v, -V, -q, -u, -X) need readline's default emacs/vi keymap and variable tables to
        // answer faithfully. Bash's own tables are compiled in and answer these queries even
        // without a terminal (confirmed against the oracle: `bind -q abort` succeeds under
        // `bash -c`, printing the same warning this refusal's sibling binding-form path already
        // reproduces, then its real answer) -- this is Brush not having built an equivalent
        // table, not a WASI/terminal limit, and reproducing bash's actual defaults just for this
        // is disproportionate given that. A *binding* form (a literal `KEYSEQ: TARGET`, -x, -f,
        // -r, -m) has nothing to query, so it reaches Brush's own `bind`, which warns ("line
        // editing not enabled") and succeeds, matching bash's own non-interactive behavior
        // exactly (also verified against the oracle).
        "bind" if option("lpPsSvVquX") => {
            Some("bind's query options (-l, -p, -P, -s, -S, -v, -V, -q, -u, -X) are unsupported in bash-tool".into())
        }
        _ => None,
    }
}

/// Whether `exec`'s own leading options -- the words before the command name it will run --
/// include any of `letters`. Unlike [`refusal`]'s generic `option` check, this stops at the
/// first word that isn't one of `exec`'s own options: everything after that belongs to the
/// command being exec'd, and coincidentally looking like `-l` or `-c` there (`exec sort -c`,
/// `exec ls -l`) must not refuse a perfectly ordinary command. `-a` itself takes a NAME
/// argument, which is not a further option and not the command name either.
fn exec_option(words: &[String], letters: &str) -> bool {
    let mut iter = words.iter();
    while let Some(word) = iter.next() {
        if word == "--" {
            break;
        }
        let Some(flags) = word.strip_prefix('-').filter(|flags| !flags.is_empty()) else {
            break;
        };
        if flags.chars().any(|c| letters.contains(c)) {
            return true;
        }
        if flags.contains('a') {
            iter.next();
        }
    }
    false
}

static ORIGINALS: LazyLock<HashMap<String, Registration<DefaultShellExtensions>>> =
    LazyLock::new(|| brush_builtins::default_builtins(brush_builtins::BuiltinSet::BashMode));

fn execute(
    context: ExecutionContext<'_>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let name = context.command_name.as_str();
        let words: Vec<String> = args.iter().skip(1).map(ToString::to_string).collect();
        let words = words.strip_prefix(&["--".into()]).unwrap_or(&words);
        // Each refusal with the status bash-tool exits with: 2, or a syntax error's own status.
        let refused = |error: String| (error, 2);
        // Bash's warnings for here-documents an eval's text leaves open, and the text closing them.
        let mut here_documents = None;
        let checked = match name {
            _ if let Some(refusal) = refusal(name, words) => Err(refused(refusal)),
            // An option, which eval has none of, is eval's own error to report, before its text.
            "eval"
                if args.get(1).is_some_and(|arg| {
                    let arg = arg.to_string();
                    arg.len() > 1 && arg.starts_with('-') && arg != "--"
                }) =>
            {
                Ok(())
            }
            // Bash numbers an eval's lines from the line the eval is on.
            "eval" => {
                let line = context
                    .shell
                    .call_stack()
                    .current_frame()
                    .and_then(|frame| frame.current_line())
                    .unwrap_or(1);
                let text = words.join(" ");
                // Here-documents left open run to the end of the text, as in a script.
                let closed = close_here_documents(&text);
                let script = closed.as_ref().map_or(text.as_str(), |(script, _)| script);
                let checked = validate_with_status(script, "eval").map_err(|(error, status)| {
                    let error = match closed {
                        Some(_) => within_script(&error, text.lines().count() + 1),
                        None => error,
                    };
                    (numbered_from(&error, line), status)
                });
                here_documents = closed.map(|(script, warnings)| {
                    (script, diagnostic("bash", &numbered_from(&warnings, line)))
                });
                checked
            }
            "source" | "." => words
                .first()
                .map_or(Ok(()), |path| validate_source(&context, path)),
            "alias" => words
                .iter()
                .filter_map(|word| word.split_once('=').map(|(_, value)| value))
                .try_for_each(validate_alias)
                .map_err(refused),
            // `compgen -C` runs its command, code the script did not name.
            "compgen" => compgen_command(words)
                .map_or(Ok(()), |command| validate(command, "-c"))
                .map_err(refused),
            // So does `mapfile -C` (and `readarray -C`): its callback.
            "mapfile" | "readarray" => option_argument(words, "dnOsuCc", 'C')
                .map_or(Ok(()), |callback| validate_when_run(callback, "-c"))
                .map_err(refused),
            "trap"
                if words.len() >= 2
                    && words[0] != "-"
                    && (!words[0].starts_with('-')
                        || args.get(1).is_some_and(|arg| arg.to_string() == "--")) =>
            {
                validate_when_run(&words[0], &trap_origin(&words[1])).map_err(refused)
            }
            _ => Ok(()),
        };
        // Bash warns as it reads the here-documents, before it reaches a syntax error.
        let mut text = here_documents
            .as_ref()
            .map_or_else(String::new, |(_, warnings)| warnings.clone());
        if let Err((error, _)) = &checked {
            log::warn!("refused: {error}");
            // `source`/`.` diagnostics are already complete text (see `validate_source`): bash
            // never adds its own name in front of a sourced file's syntax error, unlike every
            // other diagnostic here, which does need the usual `NAME: ` wrap.
            if matches!(name, "source" | ".") {
                text.push_str(&format!("{error}\n"));
            } else {
                text.push_str(&diagnostic("bash", error));
            }
        }
        if !text.is_empty() {
            #[cfg(target_arch = "wasm32")]
            {
                use futures::io::AsyncWriteExt;
                context
                    .stderr()
                    .async_io()
                    .write_all(text.as_bytes())
                    .await?;
            }
            #[cfg(not(target_arch = "wasm32"))]
            write!(context.stderr(), "{text}")?;
        }
        if let Err((error, status)) = checked {
            let mut result: ExecutionResult = ExecutionExitCode::from(status).into();
            if status == SUBSTITUTION_STATUS && matches!(name, "eval" | "source" | ".") {
                // A command substitution that does not parse ends the shell in bash, and a
                // subshell with status 1.
                if context.shell.is_subshell() {
                    result = ExecutionExitCode::from(1).into();
                }
                result.next_control_flow = brush_core::ExecutionControlFlow::ExitShell;
            } else if name == "eval"
                && error.starts_with("eval: ")
                && context.shell.options().posix_mode
                && !context.shell.options().interactive
            {
                // A syntax error in `eval`'s text ends a non-interactive POSIX-mode shell, as a
                // special builtin's error does in bash.
                result.next_control_flow = brush_core::ExecutionControlFlow::ExitShell;
            }
            return Ok(result);
        }
        // An eval's text runs with its here-documents closed.
        let args = match here_documents {
            Some((script, _)) => {
                let mut closed = vec![args[0].clone()];
                closed.extend(args.get(1).filter(|arg| arg.to_string() == "--").cloned());
                closed.push(CommandArg::String(script));
                closed
            }
            None => args,
        };
        // Preserve Brush's argument handling, source positional parameters, and control flow.
        (ORIGINALS[name].execute_func)(context, args).await
    })
}

/// The command `compgen -C COMMAND` runs, if it is given one: `-C` alone or ending a cluster of
/// options takes the next word, and otherwise the rest of its own.
fn compgen_command(words: &[String]) -> Option<&str> {
    option_argument(words, "AGWFCXPSo", 'C')
}

/// The argument a command's option `wanted` is given, if any, where `takes_argument` lists the
/// command's options that take one: the rest of the word, or the next one when the option ends a
/// cluster. Options end at `--` or the first operand.
fn option_argument<'a>(words: &'a [String], takes_argument: &str, wanted: char) -> Option<&'a str> {
    let mut found = None;
    let mut index = 0;
    while let Some(word) = words.get(index) {
        index += 1;
        let Some(letters) = word.strip_prefix('-').filter(|letters| !letters.is_empty()) else {
            break;
        };
        if letters == "-" {
            break;
        }
        // The options that take an argument: the rest of the word, or the next one.
        if let Some(at) = letters.find(|c| takes_argument.contains(c)) {
            let rest = &letters[at + 1..];
            let argument = if rest.is_empty() {
                index += 1;
                words.get(index - 1).map(String::as_str)
            } else {
                Some(rest)
            };
            if letters[at..].starts_with(wanted) {
                found = argument;
            }
        }
    }
    found
}

/// Unlike [`validate`], the returned error is the complete diagnostic text bash-tool prints,
/// with no further `NAME: ` wrapping: a syntax error from [`validate`] already carries the
/// sourced file's own name (matching bash, which never adds `bash: ` in front of one), while
/// every other error here is one of bash-tool's own canonical refusals and names itself.
fn validate_source(context: &ExecutionContext<'_>, operand: &str) -> Result<(), (String, u8)> {
    let refused = |error: String| (error, 2);
    let path = context.shell.absolute_path(std::path::Path::new(operand));
    // `/dev/stdin`/`/dev/stdout`/`/dev/stderr` (and their `/dev/fd/0-2` aliases) are live
    // streams: reading one synchronously here, before the pipeline feeding it has a chance to
    // run, would block forever in this single-threaded cooperative model. A buffered path -- a
    // process substitution's `/dev/fd/N` for N > 2, or a regular file -- has no such problem, so
    // only a live stream is refused.
    if matches!(
        crate::tools::devices::classify(&path),
        Some(crate::tools::devices::Device::Stream(_))
    ) {
        return Err(refused(
            "bash: source of /dev/stdin, /dev/stdout or /dev/stderr is unsupported in bash-tool"
                .into(),
        ));
    }
    // The null device reads as an empty script, which runs as nothing.
    if crate::tools::devices::classify(&path) == Some(crate::tools::devices::Device::Null) {
        return Ok(());
    }
    // A missing or unreadable file, or a directory, is `source`'s own error to report.
    let Ok(file) = std::fs::File::open(&path) else {
        return Ok(());
    };
    let metadata = file.metadata().map_err(|e| refused(format!("bash: {e}")))?;
    if metadata.is_dir() {
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(refused(
            "bash: source requires a regular file in bash-tool".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(crate::commands::MAX_STDIN_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| refused(format!("bash: {e}")))?;
    if bytes.len() > crate::commands::MAX_STDIN_BYTES {
        return Err(refused(
            "bash: sourced script exceeds the bash-tool input limit".into(),
        ));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| refused("bash: sourced script is not UTF-8".to_owned()))?;
    // Bash never adds `bash: ` in front of a sourced file's own syntax error; `validate` already
    // names the file itself (`origin`), matching bash exactly.
    validate_with_status(text, operand)
}

/// The script with its unterminated here-documents closed at the end of the input, as bash
/// closes them, and bash's warning for each (lines to print after `bash: `). `None` when every
/// here-document is closed.
pub(crate) fn close_here_documents(script: &str) -> Option<(String, String)> {
    // Text nested too deeply to tokenize is refused by `validate` instead.
    if lexical_depth(script) > MAX_LEXICAL_DEPTH {
        return None;
    }
    let Err(brush_parser::TokenizerError::UnterminatedHereDocuments(documents)) =
        brush_parser::tokenize_str(script)
    else {
        return None;
    };
    let last_line = script.lines().count();
    let mut closed = script.to_owned();
    let mut warnings = String::new();
    for document in documents {
        if !closed.ends_with('\n') {
            closed.push('\n');
        }
        closed.push_str(&document.delimiter);
        closed.push('\n');
        warnings.push_str(&format!(
            "line {last_line}: warning: here-document at line {} delimited by end-of-file \
             (wanted `{}')\n",
            document.line, document.delimiter
        ));
    }
    Some((closed, warnings))
}

/// A [`validate`] error for a script whose here-documents [`close_here_documents`] closed, with
/// the lines the closing delimiters added (past `end_line`) given as `end_line`, the line after
/// the script's last, where bash reached its end.
pub(crate) fn within_script(error: &str, end_line: usize) -> String {
    let mut out = String::with_capacity(error.len());
    let mut rest = error;
    while let Some(at) = rest.find("line ") {
        let (before, after) = rest.split_at(at + "line ".len());
        out.push_str(before);
        let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let (number, tail) = after.split_at(digits);
        match number.parse::<usize>() {
            Ok(line) if line > end_line && tail.starts_with(':') => {
                out.push_str(&end_line.to_string());
            }
            _ => out.push_str(number),
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// `error`'s line numbers counted from line `first` rather than line 1: the `line N:` each line
/// names, and in its message the line an unfinished command or a here-document started on.
fn numbered_from(error: &str, first: usize) -> String {
    let shifted = |number: &str| {
        number
            .parse::<usize>()
            .ok()
            .map(|number| number + first.saturating_sub(1))
    };
    error
        .lines()
        .map(|line| {
            let Some((before, after)) = line.split_once("line ") else {
                return line.to_owned();
            };
            let Some((number, mut message)) = after
                .split_once(':')
                .and_then(|(number, message)| Some((shifted(number)?, message.to_owned())))
            else {
                return line.to_owned();
            };
            for marker in [" command on line ", " here-document at line "] {
                let Some((head, tail)) = message.split_once(marker) else {
                    continue;
                };
                let digits =
                    tail.len() - tail.trim_start_matches(|c: char| c.is_ascii_digit()).len();
                if let Some(start) = shifted(&tail[..digits]) {
                    message = format!("{head}{marker}{start}{}", &tail[digits..]);
                }
            }
            format!("{before}line {number}:{message}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A [`validate`] error as the shell `name` prints it: `NAME: ` before each line.
pub(crate) fn diagnostic(name: &str, error: &str) -> String {
    error
        .lines()
        .map(|line| format!("{name}: {line}\n"))
        .collect()
}

/// How bash names a trap handler in its diagnostics: `exit trap`, `int trap`.
fn trap_origin(signal: &str) -> String {
    let name = signal.to_ascii_lowercase();
    let name = name.strip_prefix("sig").unwrap_or(&name);
    format!("{} trap", if name == "0" { "exit" } else { name })
}

/// Checks an alias body as a script only when it is one: bash defines an alias whatever its text,
/// and a body that does not parse fails where the alias is used.
fn validate_alias(value: &str) -> Result<(), String> {
    // A body nested too deeply to parse is refused by `validate` without parsing it.
    if lexical_depth(value) <= MAX_LEXICAL_DEPTH {
        let options = brush_parser::ParserOptions::default();
        let mut reader = std::io::BufReader::new(value.as_bytes());
        if brush_parser::Parser::new(&mut reader, &options)
            .parse_program()
            .is_err()
        {
            return Ok(());
        }
    }
    validate(value, "-c")
}

/// Checks `script` before it runs, as [`validate`] does, and gives the status bash exits with for
/// what it refuses.
pub(crate) fn validate_with_status(script: &str, origin: &str) -> Result<(), (String, u8)> {
    script_at_depth(script, 0, Code::Script(origin)).map_err(|refusal| match refusal {
        Refusal::Text(error) => (error, syntax_status(script)),
        Refusal::Substitution { open, command } => (
            substitution_diagnostic(script, origin, open, &command),
            SUBSTITUTION_STATUS,
        ),
    })
}

/// The status bash exits with for a command substitution that does not parse: it ends the shell
/// (a subshell exits with 1).
pub(crate) const SUBSTITUTION_STATUS: u8 = 127;

/// Why [`validate`] refuses a script.
enum Refusal {
    /// The error to print.
    Text(String),
    /// A substitution opened with `open` (`$(`, `<(` or `>(`) whose `command` does not parse.
    /// Bash parses it with the command it is in, so the error is the script's: see
    /// [`substitution_diagnostic`].
    Substitution { open: &'static str, command: String },
}

impl Refusal {
    /// The error without the script around it: a substitution's is its own command's.
    fn text(self) -> String {
        match self {
            Self::Text(error) => error,
            Self::Substitution { command, .. } => {
                let options = brush_parser::ParserOptions::default();
                let mut reader = std::io::BufReader::new(command.as_bytes());
                match brush_parser::Parser::new(&mut reader, &options).parse_program() {
                    Err(error) => brush_parser::bash_diagnostic(&error, &command, &options)
                        .iter()
                        .map(|line| format!("-c: {line}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    Ok(_) => "-c: syntax error".to_owned(),
                }
            }
        }
    }
}

impl From<String> for Refusal {
    fn from(error: String) -> Self {
        Self::Text(error)
    }
}

impl From<&str> for Refusal {
    fn from(error: &str) -> Self {
        Self::Text(error.to_owned())
    }
}

/// Bash's syntax error for `script`, named `origin`, whose substitution `open` + `command` does
/// not parse: at the line of the script where the error is, with the whole of that line quoted,
/// and naming the `)` bash was looking for when the error is at another token.
fn substitution_diagnostic(script: &str, origin: &str, open: &str, command: &str) -> String {
    // The substitution bash meets first is the first with this text.
    let first_line = script
        .find(&format!("{open}{command}"))
        .map_or(1, |at| 1 + script[..at].matches('\n').count());
    let options = brush_parser::ParserOptions::default();
    let lines =
        brush_parser::command_substitution_diagnostic(&format!("{command})"), first_line, &options);
    if lines.is_empty() {
        return Refusal::Substitution {
            open: "$(",
            command: command.to_owned(),
        }
        .text();
    }
    let last = lines.len() - 1;
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            // Bash quotes the line of the script it was reading, not the substitution's.
            let quoted = line
                .strip_prefix("line ")
                .and_then(|rest| rest.split_once(": `"))
                .filter(|_| index == last)
                .and_then(|(number, _)| {
                    let text = script
                        .lines()
                        .nth(number.parse::<usize>().ok()?.checked_sub(1)?)?;
                    Some(format!("line {number}: `{text}'"))
                });
            format!("{origin}: {}", quoted.as_deref().unwrap_or(line))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The status bash exits with for `script` refused by [`validate`]: a syntax error's own (1 in a
/// compound array assignment, see `brush_parser::syntax_error_status`), otherwise 2.
fn syntax_status(script: &str) -> u8 {
    if lexical_depth(script) > MAX_LEXICAL_DEPTH {
        return 2;
    }
    let options = brush_parser::ParserOptions::default();
    let mut reader = std::io::BufReader::new(script.as_bytes());
    match brush_parser::Parser::new(&mut reader, &options).parse_program() {
        Err(error) => brush_parser::syntax_error_status(&error, script, &options),
        Ok(_) => 2,
    }
}

/// Checks `script` before it runs. `origin` names it in a syntax error as bash does: `-c`,
/// `eval`, `exit trap`, or a sourced file's path. The error is one or more lines, each printed
/// after the shell's name with [`diagnostic`].
pub(crate) fn validate(script: &str, origin: &str) -> Result<(), String> {
    validate_with_status(script, origin).map_err(|(error, _)| error)
}

/// Where the code [`script_at_depth`] checks comes from.
#[derive(Clone, Copy)]
enum Code<'a> {
    /// A script, named in its syntax errors as bash names it: `-c`, `eval`, a file.
    Script(&'a str),
    /// The command of a substitution opened with `$(`, `<(` or `>(`, which bash parses with the
    /// command it is in.
    Substitution(&'static str),
    /// A command in backquotes, which bash parses only as it runs it.
    Backquoted,
}

fn script_at_depth(script: &str, depth: usize, code: Code<'_>) -> Result<(), Refusal> {
    if depth > 64 || lexical_depth(script) > MAX_LEXICAL_DEPTH {
        return Err("shell code is nested too deeply for bash-tool".into());
    }
    let options = brush_parser::ParserOptions::default();
    let mut reader = std::io::BufReader::new(script.as_bytes());
    let parsed = brush_parser::Parser::new(&mut reader, &options).parse_program();
    if parsed.is_err() && PARSED_WHEN_RUN.with(std::cell::Cell::get) {
        return Ok(());
    }
    let program = match (parsed, code) {
        (Ok(program), _) => program,
        // The shell reports it when it runs the substitution, and the command goes on.
        (Err(_), Code::Backquoted) => return Ok(()),
        (Err(_), Code::Substitution(open)) => {
            return Err(Refusal::Substitution {
                open,
                command: script.to_owned(),
            });
        }
        (Err(error), Code::Script(origin)) => {
            return Err(Refusal::Text(
                brush_parser::bash_diagnostic(&error, script, &options)
                    .iter()
                    .map(|line| format!("{origin}: {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ));
        }
    };
    for list in &program.complete_commands {
        list_has_background(list, depth)?;
    }
    Ok(())
}

/// How deeply [`lexical_depth`] lets a script nest before it is refused unparsed. The parser
/// recurses once per level on Wasmtime's 512 KiB native stack and runs out a little past 200
/// (a trap nothing after parsing could prevent); the checks on the parsed script refuse anything
/// past 64 anyway, so this only has to stop what the parser cannot hold.
const MAX_LEXICAL_DEPTH: usize = 128;

/// The deepest nesting of brackets, substitutions and compound commands in `script`, found in one
/// pass without parsing. Quotes, escapes, comments and here-document bodies are skipped; a
/// reserved word counts only where a command starts. It can undercount (a `case` pattern's `)`
/// closes nothing) but never overcounts balanced code.
fn lexical_depth(script: &str) -> usize {
    let bytes = script.as_bytes();
    let (mut depth, mut deepest) = (0_usize, 0_usize);
    // Double quotes enclosing the current position, each with the depth it opened at: inside
    // them only `$(` and the closing quote matter.
    let mut quoted: Vec<usize> = Vec::new();
    // Here-documents whose bodies start after the current line: (delimiter, strip tabs).
    let mut here_documents: Vec<(Vec<u8>, bool)> = Vec::new();
    // Parameter expansions enclosing the current position, each with the depth it opened at.
    let mut expansions: Vec<usize> = Vec::new();
    let mut command_start = true;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let in_quotes = quoted.last().is_some_and(|opened| *opened == depth);
        match byte {
            b'\\' => index += 1,
            b'"' if in_quotes => {
                quoted.pop();
            }
            b'"' => quoted.push(depth),
            b'$' if bytes.get(index + 1) == Some(&b'(') => {
                depth += 1;
                index += 1;
                command_start = true;
            }
            b'$' if bytes.get(index + 1) == Some(&b'{') => {
                expansions.push(depth);
                depth += 1;
                index += 1;
            }
            b'}' if expansions.last().is_some_and(|opened| opened + 1 == depth) => {
                expansions.pop();
                depth -= 1;
            }
            _ if in_quotes => {}
            b'\'' => {
                index += bytes[index + 1..]
                    .iter()
                    .position(|&b| b == b'\'')
                    .map_or(bytes.len(), |end| end + 1);
            }
            b'#' if index == 0 || bytes[index - 1].is_ascii_whitespace() => {
                index += bytes[index..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .unwrap_or(bytes.len() - index);
                continue;
            }
            // A here-string's word is an ordinary word.
            b'<' if bytes[index + 1..].starts_with(b"<<") => index += 2,
            b'<' if bytes.get(index + 1) == Some(&b'<') => {
                index += 2;
                let strip = bytes.get(index) == Some(&b'-');
                index += usize::from(strip);
                while bytes.get(index).is_some_and(|b| *b == b' ' || *b == b'\t') {
                    index += 1;
                }
                let end = bytes[index..]
                    .iter()
                    .position(|b| b.is_ascii_whitespace() || b";&|()<>".contains(b))
                    .map_or(bytes.len(), |end| index + end);
                let delimiter = bytes[index..end]
                    .iter()
                    .copied()
                    .filter(|b| !matches!(b, b'\'' | b'"' | b'\\'))
                    .collect();
                here_documents.push((delimiter, strip));
                index = end;
                continue;
            }
            b'\n' if !here_documents.is_empty() => {
                // Skip each pending body, up to and including its delimiter line.
                index += 1;
                for (delimiter, strip) in std::mem::take(&mut here_documents) {
                    while index < bytes.len() {
                        let end = bytes[index..]
                            .iter()
                            .position(|&b| b == b'\n')
                            .map_or(bytes.len(), |end| index + end);
                        let mut line = &bytes[index..end];
                        while strip && line.first() == Some(&b'\t') {
                            line = &line[1..];
                        }
                        index = end + 1;
                        if line == delimiter.as_slice() {
                            break;
                        }
                    }
                }
                command_start = true;
                continue;
            }
            b'(' => {
                depth += 1;
                command_start = true;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                // After a `case` pattern a command starts.
                command_start = true;
            }
            b';' | b'&' | b'|' | b'\n' => command_start = true,
            _ if byte.is_ascii_whitespace() => {}
            _ => {
                let end = bytes[index..]
                    .iter()
                    .position(|b| b.is_ascii_whitespace() || b"();&|<>\"'$}".contains(b))
                    .map_or(bytes.len(), |end| index + end.max(1));
                let word = &bytes[index..end];
                let reserved = command_start
                    && matches!(
                        word,
                        b"if" | b"case" | b"while" | b"until" | b"for" | b"select" | b"{"
                    );
                if reserved {
                    depth += 1;
                } else if command_start && matches!(word, b"fi" | b"esac" | b"done" | b"}") {
                    depth = depth.saturating_sub(1);
                }
                // After these a command starts; after anything else, its arguments.
                command_start = command_start
                    && matches!(
                        word,
                        b"if"
                            | b"while"
                            | b"until"
                            | b"then"
                            | b"do"
                            | b"else"
                            | b"elif"
                            | b"{"
                            | b"!"
                            | b"time"
                    );
                index = end;
                deepest = deepest.max(depth);
                continue;
            }
        }
        deepest = deepest.max(depth);
        index += 1;
    }
    deepest
}

/// The deepest nesting of `$( )`, `${ }` and `$[ ]`, and of the brackets inside them, in text that
/// is not shell code, found in one pass without parsing: the word parser recurses once per level,
/// so text [`lexical_depth`] did not see (a here-document body) is checked before it is parsed.
/// In a `word`, single-quoted text is skipped; in a here-document body a quote is a character.
fn expansion_depth(text: &str, word: bool) -> usize {
    let bytes = text.as_bytes();
    // The closing bracket each open level waits for, and `"` for double quotes, which quote but
    // do not nest.
    let mut open: Vec<u8> = Vec::new();
    let mut depth = 0;
    let mut deepest = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 1,
            b'$' if matches!(bytes.get(index + 1), Some(b'(' | b'{' | b'[')) => {
                index += 1;
                open.push(closing_bracket(bytes[index]));
                depth += 1;
            }
            b'"' if word && open.last() == Some(&b'"') => {
                open.pop();
            }
            b'"' if word => open.push(b'"'),
            b'\'' if word && open.last() != Some(&b'"') => {
                // `$'...'` holds escapes; `'...'` does not.
                let ansi_c = index > 0 && bytes[index - 1] == b'$';
                index += 1;
                while index < bytes.len() && bytes[index] != b'\'' {
                    index += if ansi_c && bytes[index] == b'\\' {
                        2
                    } else {
                        1
                    };
                }
            }
            byte @ (b'(' | b'{' | b'[') if depth > 0 && open.last() != Some(&b'"') => {
                open.push(closing_bracket(byte));
                depth += 1;
            }
            byte if byte != b'"' && open.last() == Some(&byte) => {
                open.pop();
                depth -= 1;
            }
            _ => {}
        }
        deepest = deepest.max(depth);
        index += 1;
    }
    deepest
}

const fn closing_bracket(open: u8) -> u8 {
    match open {
        b'(' => b')',
        b'{' => b'}',
        _ => b']',
    }
}

fn list_has_background(list: &CompoundList, depth: usize) -> Result<(), Refusal> {
    if depth > 64 {
        return Err("shell code is nested too deeply for bash-tool".into());
    }
    for item in &list.0 {
        for (_, pipeline) in &item.0 {
            for command in &pipeline.seq {
                match command {
                    Command::Compound(command, redirects) => {
                        compound_has_background(command, depth + 1)?;
                        for redirect in redirects.iter().flat_map(|list| &list.0) {
                            redirect_at_depth(redirect, depth + 1)?;
                        }
                    }
                    Command::Function(function) => {
                        compound_has_background(&function.body.0, depth + 1)?;
                        for redirect in function.body.1.iter().flat_map(|list| &list.0) {
                            redirect_at_depth(redirect, depth + 1)?;
                        }
                    }
                    Command::Simple(command) => {
                        if let Some(name) = &command.word_or_name {
                            word_at_depth(&name.value, depth + 1)?;
                            // A refused command named literally refuses the whole script.
                            let literal = |word: &str| !word.contains(['$', '`', '\'', '"', '\\']);
                            let words: Vec<String> =
                                command
                                    .suffix
                                    .iter()
                                    .flat_map(|suffix| &suffix.0)
                                    .filter_map(|item| match item {
                                        brush_parser::ast::CommandPrefixOrSuffixItem::Word(
                                            word,
                                        ) if literal(&word.value) => Some(word.value.clone()),
                                        _ => None,
                                    })
                                    .collect();
                            if literal(&name.value)
                                && let Some(refused) = refusal(&name.value, &words)
                            {
                                return Err(refused.into());
                            }
                        }
                        for item in command
                            .prefix
                            .iter()
                            .flat_map(|p| &p.0)
                            .chain(command.suffix.iter().flat_map(|s| &s.0))
                        {
                            use brush_parser::ast::CommandPrefixOrSuffixItem as Item;
                            match item {
                                // Buffered: the list runs before, or after, the command.
                                Item::ProcessSubstitution(_, subshell) => {
                                    list_has_background(&subshell.list, depth + 1)?;
                                }
                                Item::IoRedirect(redirect) => {
                                    redirect_at_depth(redirect, depth + 1)?
                                }
                                Item::Word(word) | Item::AssignmentWord(_, word) => {
                                    word_at_depth(&word.value, depth + 1)?
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn redirect_at_depth(
    redirect: &brush_parser::ast::IoRedirect,
    depth: usize,
) -> Result<(), Refusal> {
    use brush_parser::ast::{IoFileRedirectTarget, IoRedirect};
    match redirect {
        IoRedirect::File(_, _, IoFileRedirectTarget::ProcessSubstitution(_, subshell))
        | IoRedirect::NamedFd(_, _, IoFileRedirectTarget::ProcessSubstitution(_, subshell)) => {
            list_has_background(&subshell.list, depth + 1)
        }
        IoRedirect::File(
            _,
            _,
            IoFileRedirectTarget::Filename(word) | IoFileRedirectTarget::Duplicate(word),
        )
        | IoRedirect::NamedFd(
            _,
            _,
            IoFileRedirectTarget::Filename(word) | IoFileRedirectTarget::Duplicate(word),
        )
        | IoRedirect::HereString(_, word)
        | IoRedirect::OutputAndError(word, _) => word_at_depth(&word.value, depth),
        IoRedirect::HereDocument(_, document) if document.requires_expansion => {
            if expansion_depth(&document.doc.value, false) > MAX_LEXICAL_DEPTH {
                return Err("shell code is nested too deeply for bash-tool".into());
            }
            let parts = brush_parser::word::parse_heredoc(
                &document.doc.value,
                &brush_parser::ParserOptions::default(),
            )
            .map_err(|error| error.to_string())?;
            parts
                .iter()
                .try_for_each(|part| piece_at_depth(&part.piece, depth + 1))
        }
        _ => Ok(()),
    }
}

fn compound_has_background(command: &CompoundCommand, depth: usize) -> Result<(), Refusal> {
    match command {
        CompoundCommand::Coprocess(_) => {
            Err("background coprocesses are unsupported in bash-tool".into())
        }
        CompoundCommand::BraceGroup(group) => list_has_background(&group.list, depth),
        CompoundCommand::Subshell(group) => list_has_background(&group.list, depth),
        CompoundCommand::ForClause(clause) => {
            for word in clause.values.iter().flatten() {
                word_at_depth(&word.value, depth)?;
            }
            list_has_background(&clause.body.list, depth)
        }
        CompoundCommand::SelectClause(clause) => {
            for word in clause.values.iter().flatten() {
                word_at_depth(&word.value, depth)?;
            }
            list_has_background(&clause.body.list, depth)
        }
        CompoundCommand::ArithmeticForClause(clause) => {
            for expression in [&clause.initializer, &clause.condition, &clause.updater]
                .into_iter()
                .flatten()
            {
                word_at_depth(&expression.value, depth)?;
            }
            list_has_background(&clause.body.list, depth)
        }
        CompoundCommand::CaseClause(clause) => {
            word_at_depth(&clause.value.value, depth)?;
            for case in &clause.cases {
                for pattern in &case.patterns {
                    word_at_depth(&pattern.value, depth)?;
                }
                if let Some(list) = &case.cmd {
                    list_has_background(list, depth)?;
                }
            }
            Ok(())
        }
        CompoundCommand::IfClause(clause) => {
            list_has_background(&clause.condition, depth)?;
            list_has_background(&clause.then, depth)?;
            for clause in clause.elses.iter().flatten() {
                if let Some(condition) = &clause.condition {
                    list_has_background(condition, depth)?;
                }
                list_has_background(&clause.body, depth)?;
            }
            Ok(())
        }
        CompoundCommand::WhileClause(clause) | CompoundCommand::UntilClause(clause) => {
            list_has_background(&clause.0, depth)?;
            list_has_background(&clause.1.list, depth)
        }
        CompoundCommand::Arithmetic(command) => word_at_depth(&command.expr.value, depth),
        CompoundCommand::ExtendedTest(expression) => test_at_depth(&expression.expr, depth),
    }
}

fn test_at_depth(
    expression: &brush_parser::ast::ExtendedTestExpr,
    depth: usize,
) -> Result<(), Refusal> {
    use brush_parser::ast::ExtendedTestExpr as Test;
    if depth > 64 {
        return Err("shell test is nested too deeply for bash-tool".into());
    }
    match expression {
        Test::And(left, right) | Test::Or(left, right) => {
            test_at_depth(left, depth + 1)?;
            test_at_depth(right, depth + 1)
        }
        Test::Not(inner) | Test::Parenthesized(inner) => test_at_depth(inner, depth + 1),
        // `-N` (modified since last read) is one of Brush's "not yet implemented" placeholders:
        // it would otherwise abort the whole script with Brush's own status 99 wherever it's
        // reached, instead of a canonical, documented refusal.
        Test::UnaryTest(
            brush_parser::ast::UnaryPredicate::FileExistsAndModifiedSinceLastRead,
            _,
        ) => Err("[[ -N ]] is unsupported in bash-tool".into()),
        Test::UnaryTest(_, word) => word_at_depth(&word.value, depth),
        Test::BinaryTest(_, left, right) => {
            word_at_depth(&left.value, depth)?;
            word_at_depth(&right.value, depth)
        }
    }
}

fn word_at_depth(word: &str, depth: usize) -> Result<(), Refusal> {
    if depth > 64 {
        return Err("shell expansions are nested too deeply for bash-tool".into());
    }
    if expansion_depth(word, true) > MAX_LEXICAL_DEPTH {
        return Err("shell code is nested too deeply for bash-tool".into());
    }
    let parts = brush_parser::word::parse(word, &brush_parser::ParserOptions::default())
        .map_err(|e| e.to_string())?;
    for part in parts {
        piece_at_depth(&part.piece, depth + 1)?;
    }
    Ok(())
}

fn piece_at_depth(piece: &WordPiece, depth: usize) -> Result<(), Refusal> {
    match piece {
        WordPiece::CommandSubstitution(script) => {
            script_at_depth(script, depth, Code::Substitution("$("))
        }
        WordPiece::ProcessSubstitution(kind, script) => {
            let open = match kind {
                brush_parser::ast::ProcessSubstitutionKind::Read => "<(",
                brush_parser::ast::ProcessSubstitutionKind::Write => ">(",
            };
            script_at_depth(script, depth, Code::Substitution(open))
        }
        WordPiece::BackquotedCommandSubstitution(script) => {
            script_at_depth(script, depth, Code::Backquoted)
        }
        WordPiece::DoubleQuotedSequence(parts) | WordPiece::GettextDoubleQuotedSequence(parts) => {
            parts
                .iter()
                .try_for_each(|part| piece_at_depth(&part.piece, depth))
        }
        WordPiece::ArithmeticExpression(expression) => word_at_depth(&expression.value, depth),
        WordPiece::ParameterExpansion(expression) => parameter_at_depth(expression, depth),
        _ => Ok(()),
    }
}

fn parameter_at_depth(expression: &ParameterExpr, depth: usize) -> Result<(), Refusal> {
    use ParameterExpr as P;
    let (parameter, values): (&Parameter, Vec<&str>) = match expression {
        P::UseDefaultValues {
            parameter,
            default_value,
            ..
        }
        | P::AssignDefaultValues {
            parameter,
            default_value,
            ..
        } => (parameter, default_value.as_deref().into_iter().collect()),
        P::IndicateErrorIfNullOrUnset {
            parameter,
            error_message,
            ..
        } => (parameter, error_message.as_deref().into_iter().collect()),
        P::UseAlternativeValue {
            parameter,
            alternative_value,
            ..
        } => (
            parameter,
            alternative_value.as_deref().into_iter().collect(),
        ),
        P::RemoveSmallestSuffixPattern {
            parameter, pattern, ..
        }
        | P::RemoveLargestSuffixPattern {
            parameter, pattern, ..
        }
        | P::RemoveSmallestPrefixPattern {
            parameter, pattern, ..
        }
        | P::RemoveLargestPrefixPattern {
            parameter, pattern, ..
        }
        | P::UppercaseFirstChar {
            parameter, pattern, ..
        }
        | P::UppercasePattern {
            parameter, pattern, ..
        }
        | P::LowercaseFirstChar {
            parameter, pattern, ..
        }
        | P::LowercasePattern {
            parameter, pattern, ..
        }
        | P::ToggleCaseFirstChar {
            parameter, pattern, ..
        }
        | P::ToggleCasePattern {
            parameter, pattern, ..
        } => (parameter, pattern.as_deref().into_iter().collect()),
        P::ReplaceSubstring {
            parameter,
            pattern,
            replacement,
            ..
        } => (
            parameter,
            std::iter::once(pattern.as_str())
                .chain(replacement.as_deref())
                .collect(),
        ),
        P::Substring {
            parameter,
            offset,
            length,
            ..
        } => (
            parameter,
            std::iter::once(offset.value.as_str())
                .chain(length.as_ref().map(|e| e.value.as_str()))
                .collect(),
        ),
        P::Parameter { parameter, .. }
        | P::ParameterLength { parameter, .. }
        | P::Transform { parameter, .. } => (parameter, vec![]),
        P::VariableNames { .. } | P::MemberKeys { .. } | P::BadSubstitution { .. } => {
            return Ok(());
        }
        P::FunctionSubstitution { command, .. } => {
            return script_at_depth(command, depth, Code::Script("-c"));
        }
    };
    if let Parameter::NamedWithIndex { index, .. } = parameter {
        word_at_depth(index, depth)?;
    }
    values
        .into_iter()
        .try_for_each(|word| word_at_depth(word, depth))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn compgen_command_is_found_in_every_form() {
        for (args, command) in [
            (&["-C", "cmd", "x"][..], Some("cmd")),
            (&["-Ccmd"][..], Some("cmd")),
            (&["-aC", "cmd"][..], Some("cmd")),
            (&["-W", "-C", "-C", "cmd"][..], Some("cmd")),
            (&["-W", "-C x"][..], None),
            (&["-A", "file", "--", "-C"][..], None),
            (&["x", "-C", "cmd"][..], None),
        ] {
            assert_eq!(compgen_command(&words(args)), command, "{args:?}");
        }
    }

    #[test]
    fn a_mapfile_callback_is_found_in_every_form() {
        for (args, callback) in [
            (&["-C", "cb", "a"][..], Some("cb")),
            (&["-tC", "echo cb", "-c", "2", "a"][..], Some("echo cb")),
            (&["-Ccb"][..], Some("cb")),
            (&["-d", "C", "a"][..], None),
            (&["a", "-C", "cb"][..], None),
        ] {
            assert_eq!(
                option_argument(&words(args), "dnOsuCc", 'C'),
                callback,
                "{args:?}"
            );
        }
    }

    #[test]
    fn prompts_are_checked_as_eval_text() {
        assert!(validate_prompt("$(echo ok) \\u").is_ok());
        assert_eq!(
            validate_prompt("x$(coproc cat)"),
            Err("bash: background coprocesses are unsupported in bash-tool\n".to_owned())
        );
        assert!(
            validate_prompt("${x@P").is_ok(),
            "the shell reports a malformed prompt"
        );
        assert!(
            validate_prompt("a$(fi)b").is_ok(),
            "the shell reports a substitution that does not parse"
        );
        assert_eq!(
            validate_prompt("$(fi)$(coproc cat)"),
            Err("bash: background coprocesses are unsupported in bash-tool\n".to_owned()),
            "the rest is checked"
        );
        assert!(
            script_at_depth("fi", 1, Code::Substitution("$(")).is_err(),
            "only a prompt's substitutions are let through"
        );
    }

    #[test]
    fn lexical_depth_counts_nesting_without_parsing() {
        assert_eq!(lexical_depth("echo hi"), 0);
        assert_eq!(lexical_depth("{ { echo hi; }; }"), 2);
        assert_eq!(lexical_depth("( ( echo hi ) )"), 2);
        assert_eq!(lexical_depth("echo $(echo $(echo hi))"), 2);
        assert_eq!(lexical_depth("echo \"$(echo \"$(echo hi)\")\""), 2);
        assert_eq!(lexical_depth("if true; then if true; then :; fi; fi"), 2);
        assert_eq!(lexical_depth("while :; do for x in a; do :; done; done"), 2);
        assert_eq!(
            lexical_depth("case a in (a) case b in b) :;; esac;; esac"),
            2
        );
        assert_eq!(lexical_depth(&"{ ".repeat(400)), 400);
        // Parameter expansions nest too, quoted or not, and each one ends at its own `}`.
        assert_eq!(lexical_depth("echo ${a:-${b:-${c}}}"), 3);
        assert_eq!(lexical_depth("echo \"${a:-\"${b:-x}\"}\""), 2);
        assert_eq!(lexical_depth(&"echo ${a} ${b}; ".repeat(300)), 1);
        assert_eq!(lexical_depth("{ echo ${a}; }"), 2);
        assert_eq!(
            lexical_depth(&format!("echo {}x{}", "${a:-".repeat(300), "}".repeat(300))),
            300
        );
    }

    #[test]
    fn numbered_from_counts_lines_from_the_given_line() {
        assert_eq!(
            numbered_from(
                "eval: line 1: syntax error near unexpected token `fi'\neval: line 2: `fi'",
                5
            ),
            "eval: line 5: syntax error near unexpected token `fi'\neval: line 6: `fi'"
        );
        assert_eq!(
            numbered_from("no line number here", 3),
            "no line number here"
        );
        assert_eq!(numbered_from("eval: line 1: x", 1), "eval: line 1: x");
        assert_eq!(
            numbered_from(
                "eval: line 3: syntax error: unexpected end of file from `if' command on line 2",
                4
            ),
            "eval: line 6: syntax error: unexpected end of file from `if' command on line 5"
        );
        assert_eq!(
            numbered_from(
                "line 2: warning: here-document at line 1 delimited by end-of-file (wanted `E')",
                3
            ),
            "line 4: warning: here-document at line 3 delimited by end-of-file (wanted `E')"
        );
    }

    #[test]
    fn substitution_errors_are_the_scripts() {
        let refused = |script: &str| validate_with_status(script, "-c").unwrap_err();
        assert_eq!(
            refused(":\nf() {\n  echo \"a $(echo b; fi) c\"\n}"),
            (
                "-c: line 3: syntax error near unexpected token `fi' while looking for matching \
                 `)'\n-c: line 3: `  echo \"a $(echo b; fi) c\"'"
                    .to_owned(),
                SUBSTITUTION_STATUS
            )
        );
        assert_eq!(
            refused("y=$(echo a\nfi)").0,
            "-c: line 2: syntax error near unexpected token `fi' while looking for matching `)'\n\
             -c: line 2: `fi)'"
        );
        assert_eq!(
            refused("echo $(if)").0,
            "-c: line 1: syntax error near unexpected token `)'\n-c: line 1: `echo $(if)'"
        );
        // Backquotes are the shell's to report as they run.
        assert_eq!(validate_with_status("echo `fi`", "-c"), Ok(()));
    }

    #[test]
    fn expansion_depth_counts_substitutions_in_text() {
        assert_eq!(expansion_depth("plain (text) {with} [brackets]", true), 0);
        assert_eq!(expansion_depth("a $(b (c)) ${d:-${e}} $[f[1]]", true), 2);
        assert_eq!(expansion_depth(&"${a} ".repeat(500), true), 1);
        assert_eq!(expansion_depth("\\$(a", true), 0);
        let deep = format!("{}y{}", "${a:-".repeat(1000), "}".repeat(1000));
        assert_eq!(expansion_depth(&deep, true), 1000);
        assert_eq!(expansion_depth(&format!("\"{deep}\""), true), 1000);
        // In a word, single-quoted text is not code; in a here-document body a quote is text.
        assert_eq!(expansion_depth(&format!("'{deep}'"), true), 0);
        assert_eq!(expansion_depth(&format!("$'\\'{deep}'"), true), 0);
        assert_eq!(expansion_depth(&format!("\"it's {deep}\""), true), 1000);
        assert_eq!(expansion_depth(&format!("'{deep}'"), false), 1000);
    }

    #[test]
    fn lexical_depth_skips_what_is_not_code() {
        // Words that are not at a command's start, quoted text and comments do not nest.
        assert_eq!(
            lexical_depth("echo if for while { ( '(((' \"{{\" # ((((\necho"),
            1
        );
        assert_eq!(lexical_depth(&"echo if; ".repeat(300)), 0);
        // Nor do here-document bodies, quoted delimiters or not.
        let prose = "if this\nfor that\n{ ( (\n".repeat(200);
        assert_eq!(
            lexical_depth(&format!("cat <<EOF\n{prose}EOF\necho done")),
            0
        );
        assert_eq!(
            lexical_depth(&format!("cat <<-'X' >f\n\t{prose}\tX\n{{ :; }}")),
            1
        );
        assert_eq!(lexical_depth("cat <<<'(((('; echo \\("), 0);
    }
}
