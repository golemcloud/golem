//! Incremental drivers over the existing text parsers and invocation-owned engines.
use super::{Error, ExecutionContext, ExecutionResult, Path, ShellExtensions, ToolResult};
use crate::tools::streaming::{Records, operand};
use brush_core::{CommandArg, builtins::BoxFuture, openfiles::OpenFile};
use futures::io::{AsyncReadExt, AsyncWriteExt};

macro_rules! utility_drivers {
    ($($name:ident => $implementation:ident),+ $(,)?) => {
        $(pub(super) fn $name<SE: ShellExtensions>(
            context: ExecutionContext<'_, SE>,
            args: Vec<CommandArg>,
        ) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
            crate::tools::streaming::utility(context, args, $implementation)
        })+
    };
}

utility_drivers! {
    grep => grep_impl,
    sed => sed_impl,
    jq => jq_impl,
}

async fn finish<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    name: &str,
    result: ToolResult<i32>,
) -> Result<ExecutionResult, Error> {
    match result {
        Ok(code) => Ok(ExecutionResult::new(
            u8::try_from(code.clamp(0, 255)).unwrap_or(1),
        )),
        Err(error) => {
            if let Some(error) = error.downcast_ref::<std::io::Error>()
                && error.kind() == std::io::ErrorKind::BrokenPipe
            {
                return Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe).into());
            }
            // An I/O failure in GNU's words and with the utility's error status.
            let (text, status) = match error.downcast_ref::<std::io::Error>() {
                Some(error) => (crate::tools::io_message(error), 0),
                None => (error.to_string(), 1),
            };
            let (text, status) = match (name, status) {
                ("jq", 0) => (format!("error: {text}"), 2),
                ("sed", 0) => (text, 4),
                ("grep", 0) => (text, 2),
                (_, 0) => (text, 1),
                (_, status) => (text, status),
            };
            context
                .stderr()
                .async_io()
                .write_all(format!("{name}: {text}\n").as_bytes())
                .await?;
            Ok(ExecutionResult::new(status))
        }
    }
}

fn grep_impl<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let args: Vec<String> = args.into_iter().map(|arg| arg.to_string()).collect();
        let result = grep_run(&context, &args).await;
        finish(&context, "grep", result).await
    })
}

/// Reads each target as its own record stream; `-q` and `-m` stop an endless producer.
async fn grep_run<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    argv: &[String],
) -> ToolResult<i32> {
    use crate::tools::grep::{self, Prepared, Search, Step, Target};
    let resolve = |path: &str| context.shell.absolute_path(Path::new(path));
    let utf8 = grep::utf8_locale(&crate::tools::coreutils::exported_env(context));
    let mut stdout = context.stdout();
    let mut stderr = context.stderr();
    // Pattern files are read while the arguments are parsed, synchronously; one that is standard
    // input (`-f -`, `-f /dev/stdin`) is read first and the arguments parsed again.
    let names_stdin = |path: &str| {
        path == "-"
            || crate::tools::devices::classify(&resolve(path))
                == Some(crate::tools::devices::Device::Stream(0))
    };
    let wants_stdin = std::cell::Cell::new(false);
    let _ = grep::prepare(argv, utf8, &|path: &str| {
        if names_stdin(path) {
            wants_stdin.set(true);
            Ok(Vec::new())
        } else {
            crate::tools::read_file(&resolve(path))
        }
    });
    let stdin_patterns = if wants_stdin.get() {
        let (bytes, cut) =
            crate::tools::streaming::read_limited(crate::tools::streaming::input(context)).await?;
        if cut {
            let message = format!(
                "grep: standard input over {} is unsupported in bash-tool\n",
                crate::tools::buffer_limit()
            );
            stderr.async_io().write_all(message.as_bytes()).await?;
            return Ok(2);
        }
        bytes
    } else {
        Vec::new()
    };
    let read = |path: &str| {
        if names_stdin(path) {
            Ok(stdin_patterns.clone())
        } else {
            crate::tools::read_file(&resolve(path))
        }
    };
    let (options, matcher) = match grep::prepare(argv, utf8, &read) {
        Ok(Prepared::Run(options, matcher)) => (options, matcher),
        Ok(Prepared::Done(text)) => {
            stdout.async_io().write_all(text.as_bytes()).await?;
            return Ok(0);
        }
        Err(refusal) => {
            stderr
                .async_io()
                .write_all(refusal.message.as_bytes())
                .await?;
            return Ok(refusal.code);
        }
    };
    let (targets, recursed) = grep::targets(&options, &resolve);
    let with_filename = grep::with_filename(&options, &targets, recursed);
    let delimiter = Search::delimiter(&options);
    let mut failed = false;
    let mut selected = false;
    // Whether an earlier target already printed something with context on — see
    // `Search::separator_owed`'s own doc comment.
    let mut separator_owed = false;
    for target in targets {
        let (name, source, pre_binary) = match target {
            Target::Stdin(None) => (
                grep::stdin_name(&options),
                crate::tools::streaming::input(context),
                false,
            ),
            // A name for it, such as `/dev/stdin`, opens a file again from its start.
            Target::Stdin(Some(name)) => (
                name,
                crate::tools::streaming::opened_again(crate::tools::streaming::input(context)),
                false,
            ),
            Target::File(name, path) => match if path == Path::new("/dev/null") {
                Ok((brush_core::openfiles::from_bytes(Vec::new()), false))
            } else if let Some(crate::tools::devices::Device::Descriptor(fd)) =
                crate::tools::devices::classify(&path)
            {
                context
                    .try_fd(fd)
                    .map(|source| (crate::tools::streaming::opened_again(source), false))
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
            } else {
                std::fs::File::open(&path).map(|mut file| {
                    // GNU decides "binary" from a read-ahead over the whole file, not line
                    // by line: a NUL found only partway through must still suppress a match
                    // on an earlier line (see `grep::file_starts_with_nul`'s doc comment).
                    let pre_binary = delimiter != 0 && grep::file_starts_with_nul(&mut file);
                    (OpenFile::from(file), pre_binary)
                })
            } {
                Ok((source, pre_binary)) => (name, source, pre_binary),
                Err(error) => {
                    failed = true;
                    if !options.no_messages() {
                        let message =
                            format!("grep: {name}: {}\n", crate::tools::io_message(&error));
                        stderr.async_io().write_all(message.as_bytes()).await?;
                    }
                    continue;
                }
            },
            Target::Error(message) => {
                failed = true;
                if !options.no_messages() {
                    stderr.async_io().write_all(message.as_bytes()).await?;
                }
                continue;
            }
        };
        let mut search = Search::new(
            &options,
            &matcher,
            name,
            with_filename,
            separator_owed,
            utf8,
        );
        if pre_binary {
            search.mark_binary();
        }
        let mut records = Records::new(source, delimiter);
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();
        while let Some(record) = records.next().await? {
            output.clear();
            diagnostics.clear();
            let step = search.record(&record, &mut output, &mut diagnostics);
            stdout.async_io().write_all(&output).await?;
            stderr.async_io().write_all(&diagnostics).await?;
            match step {
                Step::Continue => (),
                Step::NextFile => break,
                Step::Quit => return Ok(0),
            }
        }
        output.clear();
        search.finish(&mut output);
        stdout.async_io().write_all(&output).await?;
        selected |= search.selected() > 0;
        separator_owed |= search.printed();
    }
    Ok(grep::status(&options, selected, failed))
}

fn sed_impl<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let argv: Vec<String> = args.iter().map(ToString::to_string).collect();
        let locale = crate::tools::sed::locale(&crate::tools::coreutils::exported_env(&context));
        // `prepare` compiles the script, which for `w`, `r`, `R` and `-f` opens a file by
        // whatever relative path the script gives — through code (the sed fork) that
        // resolves it against the process's own directory, not the shell's `cd`. Point the
        // process there for just the compile, the same way `run_uu` does for `-i`/`-s`
        // (scoped narrowly, since the guard's cwd is process-global). `-f -` additionally
        // reads the script itself from the process's real stdin, bypassing the shell's
        // piped/redirected one entirely — stage that in too, but only when asked: draining
        // the shell's stdin here when nothing asked for it would starve the main input's
        // own `-` operand.
        let staged_stdin = if crate::tools::sed::wants_stdin_script(&argv) {
            let mut bytes = Vec::new();
            crate::tools::streaming::input(&context)
                .async_io()
                .read_to_end(&mut bytes)
                .await?;
            Some(bytes)
        } else {
            None
        };
        let prepared = {
            let _cwd = crate::tools::coreutils::ShellCwd::enter(&context);
            let compile = || crate::tools::sed::prepare(&argv, locale);
            match &staged_stdin {
                Some(bytes) => crate::tools::coreutils::with_shell_stdin_staged(bytes, compile),
                None => compile(),
            }
        };
        match prepared {
            // `-i` and `-s` depend on whole, separate files: uutils' own entry point.
            Ok((engine, _)) if engine.needs_files() => {
                crate::tools::streaming::finite::<super::Sed, SE>(context, args).await
            }
            Ok((mut engine, files)) => {
                // `r /dev/stdin` reads standard input while the script runs, synchronously; when
                // the input is files, standard input is read first and served to it.
                let stdin = if !files.is_empty()
                    && !files
                        .iter()
                        .any(|file| crate::tools::streaming::reads_stdin(&context, file))
                    && argv.iter().skip(1).any(|arg| {
                        ["/dev/stdin", "/dev/fd/0", "/proc/self/fd/0"]
                            .iter()
                            .any(|name| arg.contains(name))
                    }) {
                    let (bytes, _) = crate::tools::streaming::read_limited(
                        crate::tools::streaming::input(&context),
                    )
                    .await?;
                    Some(brush_core::openfiles::from_bytes(bytes))
                } else {
                    None
                };
                let result = sed_run(&context, &mut engine, &files, stdin).await;
                finish(&context, "sed", result).await
            }
            Err(refusal) => {
                // `--help`/`--version` (code 0 — see `sed::prepare`'s own note) belong
                // on stdout, like every other command's help/version text in this tool; every
                // other refusal belongs on stderr as usual.
                let mut stream = if refusal.code == 0 {
                    context.stdout()
                } else {
                    context.stderr()
                };
                stream
                    .async_io()
                    .write_all(refusal.message.as_bytes())
                    .await?;
                Ok(ExecutionResult::new(
                    u8::try_from(refusal.code.clamp(0, 255)).unwrap_or(1),
                ))
            }
        }
    })
}

/// Feeds the engine one record at a time, reading ahead one record only for scripts that ask
/// about the last line, so endless producers still reach `q`.
///
/// Addressing (line numbers, `$`) still spans every operand as one continuous stream, as GNU
/// sed does without `-s`: files are read one after another, not `-s`-separated. But each
/// operand keeps its own name for the `F` command, so operands are queued instead of joined
/// into one byte stream ahead of time.
async fn sed_run<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    engine: &mut uu_sed::sed::incremental::Engine,
    files: &[String],
    stdin: Option<OpenFile>,
) -> ToolResult<i32> {
    // Standard input for the script's own reads (`r /dev/stdin`), served around each synchronous
    // step: never across an await, where other commands run.
    let serve = || {
        stdin.as_ref().map(|stdin| {
            crate::tools::devices::serve(std::iter::once((0, stdin.clone())).collect(), false)
        })
    };
    use uu_sed::sed::incremental::Flow;
    let mut stdout = context.stdout();
    let mut stderr = context.stderr();
    let mut status = 0;
    let mut sources: std::collections::VecDeque<(String, Records)> =
        std::collections::VecDeque::new();
    for file in files {
        match operand(context, file) {
            Ok(source) => {
                sources.push_back((file.clone(), Records::new(source, engine.delimiter())));
            }
            Err(message) => {
                let diagnostic =
                    crate::tools::sed::unreadable(file, &std::io::Error::other(message));
                stderr.async_io().write_all(diagnostic.as_bytes()).await?;
                status = 2;
            }
        }
    }
    async fn next_record(
        sources: &mut std::collections::VecDeque<(String, Records)>,
    ) -> std::io::Result<Option<(String, Vec<u8>)>> {
        while let Some((name, records)) = sources.front_mut() {
            // GNU sed names the file a read error came from: `read error on NAME: …` (see
            // `streaming::chain`'s own identical wording, for the single-stream case this
            // per-operand queue replaces so `F` can still report each operand's own name).
            let named = |error: std::io::Error| {
                if error.kind() == std::io::ErrorKind::BrokenPipe {
                    return error;
                }
                std::io::Error::other(format!(
                    "read error on {name}: {}",
                    crate::tools::io_message(&error)
                ))
            };
            if let Some(record) = records.next().await.map_err(named)? {
                return Ok(Some((name.clone(), record)));
            }
            sources.pop_front();
        }
        Ok(None)
    }
    let lookahead = engine.needs_last();
    let mut current = next_record(&mut sources).await?;
    let mut output = Vec::new();
    let mut current_name: Option<String> = None;
    while let Some((name, record)) = current {
        let next = if lookahead {
            next_record(&mut sources).await?
        } else {
            None
        };
        if current_name.as_deref() != Some(name.as_str()) {
            engine.set_input_name(name.as_str());
            current_name = Some(name.clone());
        }
        output.clear();
        let flow = {
            let _served = serve();
            engine.record(&record, lookahead && next.is_none(), &mut output)
        };
        stdout.async_io().write_all(&output).await?;
        match flow {
            Ok(Flow::Continue) => (),
            Ok(Flow::Quit) => break,
            Err(error) => {
                stderr
                    .async_io()
                    .write_all(format!("sed: {error}\n").as_bytes())
                    .await?;
                return Ok(error.code());
            }
        }
        current = if lookahead {
            next
        } else {
            next_record(&mut sources).await?
        };
    }
    output.clear();
    let finished = {
        let _served = serve();
        engine.finish(&mut output)
    };
    stdout.async_io().write_all(&output).await?;
    if let Err(error) = finished {
        stderr
            .async_io()
            .write_all(format!("sed: {error}\n").as_bytes())
            .await?;
        return Ok(error.code());
    }
    Ok(match engine.exit_code() {
        0 => status,
        code => code,
    })
}

fn jq_impl<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        // jq reads its arguments as text: a byte that is not UTF-8 becomes U+FFFD, as in jq.
        let args: Vec<String> = args
            .iter()
            .map(|arg| crate::tools::shell_bytes::to_utf8_lossy(&arg.to_string()))
            .collect();
        let result = jq_run(&context, &args).await;
        finish(&context, "jq", result).await
    })
}

/// Streams one JSON value (or `-R` line) at a time, so endless producers work; filters that
/// need all input first (`-s`, `input`, `inputs`) read every operand, then run once. Either way
/// jq's own reader decides what is read when, as in jq.
async fn jq_run<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    argv: &[String],
) -> ToolResult<i32> {
    use crate::tools::jq::{self, Need, Preloaded};
    let env = crate::tools::coreutils::exported_env_utf8(context);
    let resolve = |path: &str| context.shell.absolute_path(Path::new(path));
    let mut stdout = context.stdout();
    let mut stderr = context.stderr();
    // A program or `--rawfile`/`--slurpfile` read from `/dev/stdin` is read while jq compiles:
    // standard input is staged in for that, as for `sed -f /dev/stdin`.
    let staged_stdin = if jq::reads_stdin_file(argv) {
        let mut bytes = Vec::new();
        crate::tools::streaming::input(context)
            .async_io()
            .read_to_end(&mut bytes)
            .await?;
        Some(bytes)
    } else {
        None
    };
    let prepared = match &staged_stdin {
        Some(bytes) => crate::tools::coreutils::with_shell_stdin_staged(bytes, || {
            jq::prepare(argv, env, &resolve)
        }),
        None => jq::prepare(argv, env, &resolve),
    };
    let (options, program) = match prepared {
        Ok(prepared) => prepared,
        Err(outcome) => {
            write_ordered(&mut stdout, &mut stderr, outcome.writes()).await?;
            return Ok(outcome.code);
        }
    };
    if !program.reads_input() || program.buffered() {
        let mut preloaded = Vec::new();
        if program.reads_input() {
            let stdin = ["-".to_owned()];
            let operands = if options.files.is_empty() {
                &stdin[..]
            } else {
                &options.files[..]
            };
            for path in operands {
                preloaded.push(match operand(context, path) {
                    Ok(mut file) => {
                        let mut bytes = Vec::new();
                        let error = file.async_io().read_to_end(&mut bytes).await.err();
                        Preloaded::Read(bytes, error.map(|error| crate::tools::io_message(&error)))
                    }
                    Err(message) => Preloaded::Unopened(message),
                });
            }
        }
        let outcome = jq::run_preloaded(&program, &options.files, preloaded);
        write_ordered(&mut stdout, &mut stderr, outcome.writes()).await?;
        return Ok(outcome.code);
    }
    // No filter pulls input itself here, so only this loop reads, one value at a time.
    let input = std::cell::RefCell::new(jq::Input::new(program.reader(&options.files), Vec::new()));
    let mut state = jq::RunState::default();
    let mut file: Option<OpenFile> = None;
    let mut buffer = vec![0; 16 * 1024];
    let mut streams = jq::Streams::default();
    // jq checks for unreadable operands before each value, not while reading one.
    while input.borrow().reader.failures == 0 {
        let next = loop {
            let step = input.borrow_mut().reader.next_input();
            let messages = std::mem::take(&mut input.borrow_mut().reader.messages);
            streams.err(&messages);
            write_ordered(&mut stdout, &mut stderr, borrowed(&streams.take())).await?;
            match step {
                Ok(next) => break next,
                Err(Need::Open(index)) => {
                    let path = input.borrow().reader.operand(index).to_owned();
                    let opened = operand(context, &path).map(|opened| file = Some(opened));
                    input.borrow_mut().reader.opened(opened);
                }
                Err(Need::Bytes) => {
                    let read = match &mut file {
                        Some(open) => open.async_io().read(&mut buffer).await,
                        None => Ok(0),
                    };
                    let mut input = input.borrow_mut();
                    match read {
                        Ok(count) => input.reader.feed(&buffer[..count], count == 0),
                        Err(error) => input.reader.failed(crate::tools::io_message(&error)),
                    }
                }
            }
        };
        let stop = match next {
            Some(Ok(value)) => {
                let ret = program.process(value, &input, &mut state, &mut streams);
                program.processed(ret, &mut state);
                state.halted()
            }
            Some(Err(error)) => state.parse_error(&error, program.seq(), &mut streams),
            None => true,
        };
        write_ordered(&mut stdout, &mut stderr, borrowed(&streams.take())).await?;
        if stop {
            break;
        }
    }
    write_ordered(&mut stdout, &mut stderr, borrowed(&streams.finish())).await?;
    let failures = input.borrow().reader.failures;
    Ok(program.exit_code(&state, failures))
}

fn borrowed(
    writes: &[(crate::tools::jq::Stream, Vec<u8>)],
) -> Vec<(crate::tools::jq::Stream, &[u8])> {
    writes
        .iter()
        .map(|(stream, bytes)| (*stream, &bytes[..]))
        .collect()
}

/// Make jq's writes in the order it makes them (see `jq::Streams`).
async fn write_ordered(
    stdout: &mut OpenFile,
    stderr: &mut OpenFile,
    writes: Vec<(crate::tools::jq::Stream, &[u8])>,
) -> std::io::Result<()> {
    for (stream, bytes) in writes {
        if bytes.is_empty() {
            continue;
        }
        match stream {
            crate::tools::jq::Stream::Out => stdout.async_io().write_all(bytes).await?,
            crate::tools::jq::Stream::Err => stderr.async_io().write_all(bytes).await?,
        }
    }
    Ok(())
}
