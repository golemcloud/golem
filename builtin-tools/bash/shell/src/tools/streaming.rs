//! Command-owned WASM stream drivers. Synchronous helper guards end before forwarding awaits.

#![cfg_attr(
    not(target_arch = "wasm32"),
    allow(
        dead_code,
        reason = "WASM drivers are compiled on the host for processor tests"
    )
)]

use brush_core::{
    CommandArg, Error, ExecutionResult,
    builtins::{BoxFuture, Registration, SimpleCommand},
    commands::ExecutionContext,
    extensions::ShellExtensions,
    openfiles::{OpenFile, OpenFiles, Stream},
};
use futures::io::{AsyncReadExt, AsyncWriteExt};
use std::{
    collections::VecDeque,
    ffi::OsString,
    io,
    path::Path,
    sync::{Arc, Mutex},
};

type EventLog = Vec<(bool, Vec<u8>)>;
type Events = Arc<Mutex<EventLog>>;

/// Keep utility exit codes distinct from the shell's synthetic SIGPIPE status. The
/// command boundary supplies inherited ignored/default disposition for each poll.
pub(crate) fn utility<SE: ShellExtensions>(
    mut context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
    execute: brush_core::builtins::CommandExecuteFunc<SE>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    let name = context.command_name.clone();
    // Diagnostics written to a closed stderr (`2>&-`) are lost, as GNU's `error` loses them,
    // and the utility goes on with its own work and status.
    if context.try_fd(OpenFiles::STDERR_FD).is_none() {
        context
            .params
            .set_fd(OpenFiles::STDERR_FD, brush_core::openfiles::null_sink());
    }
    let mut stderr = context.stderr();
    Box::pin(async move {
        let result = execute(context, args).await;
        utility_result(result, &name, &mut stderr).await
    })
}

#[cfg(target_arch = "wasm32")]
fn pipe_ignored() -> bool {
    matches!(
        brush_core::execution::process::pipe_disposition(),
        brush_core::traps::PipeDisposition::Ignored
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn pipe_ignored() -> bool {
    false
}

async fn utility_result(
    result: Result<ExecutionResult, Error>,
    name: &str,
    stderr: &mut OpenFile,
) -> Result<ExecutionResult, Error> {
    // These are command outcomes, not a single generic signal-to-status conversion.
    let status = match name {
        "grep" | "jq" | "sort" | "ls" => 2,
        "sed" => 4,
        "awk" if pipe_ignored() => return Ok(ExecutionResult::new(141)),
        _ => 1,
    };
    // Any other I/O failure is the utility's to report, in its own name, as GNU's do: never the
    // shell's `bash: line N: …`.
    if let Err(error) = &result
        && let Some(error) = error.as_io_error()
        && error.kind() != io::ErrorKind::BrokenPipe
    {
        let message = super::io_message(error);
        // GNU `yes` names its destination for any failed write, as for a broken pipe below.
        let diagnostic = match message.strip_prefix("write error: ") {
            Some(reason) if name == "yes" => format!("yes: standard output: {reason}\n"),
            _ => format!("{name}: {message}\n"),
        };
        stderr.async_io().write_all(diagnostic.as_bytes()).await?;
        return Ok(ExecutionResult::new(status));
    }
    if !pipe_ignored()
        || !result.as_ref().is_err_and(|error| {
            error
                .as_io_error()
                .is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
        })
    {
        return result;
    }
    // GNU `yes` names its own destination rather than reporting a generic "write error"; every
    // other command here (including our own `seq`/`rev`) uses the generic wording.
    let diagnostic = if name == "yes" {
        "yes: standard output: Broken pipe\n".to_string()
    } else {
        format!("{name}: write error: Broken pipe\n")
    };
    if let Err(error) = stderr.async_io().write_all(diagnostic.as_bytes()).await {
        // An ignored SIGPIPE on the diagnostic destination cannot replace the utility's
        // result or recursively attempt to diagnose its own failed diagnostic.
        if error.kind() != io::ErrorKind::BrokenPipe {
            return Err(error.into());
        }
    }
    Ok(ExecutionResult::new(status))
}

macro_rules! utility_drivers {
    ($($name:ident => $implementation:ident),+ $(,)?) => {
        $(pub(crate) fn $name<SE: ShellExtensions>(
            context: ExecutionContext<'_, SE>,
            args: Vec<CommandArg>,
        ) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
            utility(context, args, $implementation)
        })+
    };
}

utility_drivers! {
    cat => cat_impl,
    head => head_impl,
    cut => cut_impl,
    tr => tr_impl,
    uniq => uniq_impl,
    tee => tee_impl,
    sort => sort_impl,
    wc => wc_impl,
    tail => tail_impl,
    yes => yes_impl,
    seq => seq_impl,
    rev => rev_impl,
}

/// The most of a finite command's piped input, and of each of its output streams, that is held
/// in memory (see [`super::MAX_BUFFER_BYTES`]). A command whose output goes to a file or
/// `/dev/null` writes it there directly, and one reading a file reads it directly, without this
/// limit.
pub(crate) const MAX_BUFFERED_BYTES: usize = super::MAX_BUFFER_BYTES;

/// One output stream of a finite command, kept until the command finishes and then forwarded.
/// Past [`MAX_BUFFERED_BYTES`] a write fails as one to a closed pipe does.
#[derive(Clone)]
struct Capture {
    events: Events,
    stderr: bool,
    /// Whether the command's stdout and stderr go to one place (`2>&1`): then both captures
    /// are one target, as the descriptors they stand for are.
    merged: bool,
    /// Bytes kept on this stream, and, once a write was refused at the limit, how many events
    /// had been kept then.
    kept: Arc<Mutex<(usize, Option<usize>)>>,
}
impl Capture {
    fn new(events: &Events, stderr: bool, merged: bool) -> Self {
        Self {
            events: events.clone(),
            stderr,
            merged,
            kept: Arc::default(),
        }
    }
    /// How many events were kept when this stream refused a write at the limit.
    fn cut(&self) -> Option<usize> {
        self.kept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .1
    }
}
impl io::Read for Capture {
    fn read(&mut self, _data: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}
impl io::Write for Capture {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let count = {
            let mut kept = self
                .kept
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let room = MAX_BUFFERED_BYTES.saturating_sub(kept.0);
            if room == 0 {
                if kept.1.is_none() {
                    kept.1 = Some(
                        self.events
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .len(),
                    );
                }
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            let count = data.len().min(room);
            kept.0 += count;
            count
        };
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Consecutive writes to one stream are one event.
        match events.last_mut() {
            Some((stderr, bytes)) if *stderr == self.stderr => {
                bytes.extend_from_slice(&data[..count]);
            }
            _ => events.push((self.stderr, data[..count].to_vec())),
        }
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Stream for Capture {
    fn clone_box(&self) -> Box<dyn Stream> {
        Box::new(self.clone())
    }
    fn target_id(&self) -> Option<usize> {
        self.merged.then(|| Arc::as_ptr(&self.events).addr())
    }
    #[cfg(unix)]
    fn try_clone_to_owned(&self) -> Result<std::os::fd::OwnedFd, Error> {
        Err(brush_core::ErrorKind::CannotConvertToNativeFd.into())
    }
    #[cfg(unix)]
    fn try_borrow_as_fd(&self) -> Result<std::os::fd::BorrowedFd<'_>, Error> {
        Err(brush_core::ErrorKind::CannotConvertToNativeFd.into())
    }
}

/// Preserve existing synchronous finite helper semantics and await its output destinations.
pub(crate) fn finite_builtin<C: SimpleCommand + Send + Sync, SE: ShellExtensions>()
-> Registration<SE> {
    let mut registration = brush_core::builtins::simple_builtin::<C, SE>();
    registration.execute_func = finite::<C, SE>;
    registration
}

/// A finite external utility still needs a command boundary and pipe-error policy when
/// its captured output is forwarded asynchronously.
pub(crate) fn finite_utility_builtin<C: SimpleCommand + Send + Sync, SE: ShellExtensions>()
-> Registration<SE> {
    let mut registration = finite_builtin::<C, SE>();
    registration.execute_func = finite_utility::<C, SE>;
    #[cfg(target_arch = "wasm32")]
    {
        registration.execution_boundary = brush_core::builtins::ExecutionBoundary::Command;
    }
    registration
}

fn finite_utility<C: SimpleCommand + Send + Sync, SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    utility(context, args, finite::<C, SE>)
}

/// Like [`finite_utility_builtin`], for a command whose `-` operand means standard input
/// (`file -`). Input is read only when an operand asks for it, so `yes | file f` still ends.
pub(crate) fn finite_dash_utility_builtin<C: SimpleCommand + Send + Sync, SE: ShellExtensions>()
-> Registration<SE> {
    finite_utility_builtin::<C, SE>()
}

/// Like [`finite_utility_builtin`], but for a command that may also read piped/redirected stdin
/// (not just file operands) — e.g. `nl`, `base64`, `md5sum` with no `FILE` argument. Which
/// arguments make a command read it is [`command_reads_stdin`]'s to say.
pub(crate) fn finite_stdin_utility_builtin<C: SimpleCommand + Send + Sync, SE: ShellExtensions>()
-> Registration<SE> {
    finite_utility_builtin::<C, SE>()
}

/// What a finite command gets besides its arguments: its standard input, and the descriptors
/// above 2 its operands name as `/dev/fd/N`.
struct Input {
    stdin: OpenFile,
    /// `stdin` holds only the first [`MAX_BUFFERED_BYTES`] of what was piped.
    cut: bool,
    /// The script closed its own standard input (`<&-`) before this call. A *direct* read of
    /// stdin still has to see `EBADF` (`stdin` is [`closed`] for that), but a command that
    /// names `/dev/stdin` as an operand instead resolves that path -- and on a real system,
    /// resolving a symlink to a closed descriptor (`/dev/stdin` is `/proc/self/fd/0`) fails with
    /// `ENOENT`, not `EBADF`. So this fd is deliberately left out of what gets served in
    /// [`capture_operation`], and `devices::open_device`'s existing "nothing served" branch
    /// already answers `ENOENT` for that case -- this only has to make sure it's reached.
    stdin_closed: bool,
    descriptors: std::collections::HashMap<i32, OpenFile>,
}

impl Input {
    fn empty() -> Self {
        Self {
            stdin: brush_core::openfiles::from_bytes(Vec::new()),
            cut: false,
            stdin_closed: false,
            descriptors: std::collections::HashMap::new(),
        }
    }
}

/// What a finite command left for the shell: its buffered output, in order, and whether a limit
/// cut its output, or cut its input where it read it.
struct Captured {
    events: EventLog,
    output_cut: bool,
    input_cut_reached: bool,
}

/// Runs a synchronous operation with `input` as its standard input and descriptors, and its
/// output kept for [`forward`]. Output bound for a file or `/dev/null` goes there directly when
/// `direct` allows it (a command that rewrites its own diagnostics does not). On WASM the
/// operation's libc descriptors are served from these streams (see `devices::serve`).
fn capture_operation<T, SE: ShellExtensions>(
    context: &mut ExecutionContext<'_, SE>,
    input: Input,
    direct: bool,
    operation: impl FnOnce(ExecutionContext<'_, SE>) -> T,
) -> (T, Captured) {
    let events = Events::default();
    let merged = context.stdout().same_target(&context.stderr());
    let mut params = context.params.clone();
    let mut captures = Vec::new();
    for (fd, stderr) in [(OpenFiles::STDOUT_FD, false), (OpenFiles::STDERR_FD, true)] {
        let file = match context.try_fd(fd) {
            Some(file)
                if direct
                    && (matches!(file, OpenFile::File(_))
                        || brush_core::openfiles::is_null_sink(&file)) =>
            {
                file
            }
            _ => {
                let capture = Capture::new(&events, stderr, merged);
                captures.push(capture.clone());
                OpenFile::Stream(Box::new(capture))
            }
        };
        params.set_fd(fd, file);
    }
    params.set_fd(OpenFiles::STDIN_FD, input.stdin.clone());
    #[cfg(target_arch = "wasm32")]
    let serving = {
        let mut files = input.descriptors;
        for fd in [
            OpenFiles::STDIN_FD,
            OpenFiles::STDOUT_FD,
            OpenFiles::STDERR_FD,
        ] {
            // A closed stdin is deliberately left unserved: opening it by path (`/dev/stdin`)
            // must reach `devices::open_device`'s own "nothing served here" branch, which
            // already answers `ENOENT` -- serving the `closed()` placeholder here instead would
            // let that open succeed and only fail (with the wrong errno) once actually read.
            if fd == OpenFiles::STDIN_FD && input.stdin_closed {
                continue;
            }
            if let Some(file) = params.try_fd(context.shell, fd) {
                files.insert(fd, file);
            }
        }
        super::devices::serve(files, input.cut)
    };
    #[cfg(not(target_arch = "wasm32"))]
    drop(input.descriptors);
    let child = ExecutionContext {
        shell: context.shell,
        params,
        command_name: context.command_name.clone(),
    };
    // All process-global cwd/stdio guards end inside this synchronous call.
    let result = operation(child);
    #[cfg(target_arch = "wasm32")]
    let input_cut_reached = serving.input_cut_reached();
    #[cfg(not(target_arch = "wasm32"))]
    let input_cut_reached = false;
    let mut events = std::mem::take(
        &mut *events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    let cut = captures.iter().filter_map(Capture::cut).min();
    if let Some(cut) = cut {
        // What the command said after its output was refused is about the refusal, which
        // `finish` reports in its own words.
        let mut index = 0;
        events.retain(|(stderr, _)| {
            index += 1;
            index <= cut || !stderr
        });
    }
    let captured = Captured {
        events,
        output_cut: cut.is_some(),
        input_cut_reached,
    };
    (result, captured)
}

async fn forward<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    events: EventLog,
) -> Result<(), Error> {
    let mut stdout = context.stdout();
    let mut stderr = context.stderr();
    let mut diagnostics = true;
    for (is_stderr, bytes) in events {
        if is_stderr {
            // Diagnostics that cannot be written (`2>&-`) are lost, as GNU's `error` loses them;
            // the command's status stands.
            if diagnostics {
                match stderr.async_io().write_all(&bytes).await {
                    Err(error) if error.kind() != io::ErrorKind::BrokenPipe => diagnostics = false,
                    result => result?,
                }
            }
        } else {
            stdout.async_io().write_all(&bytes).await?;
        }
    }
    Ok(())
}

/// Forwards a finite command's output, then reports a limit that cut what it read or wrote.
/// A reader that closed before the end ends the command as SIGPIPE does, with nothing said: the
/// cut was never seen, as `yes | nl | head -1` shows nothing in bash either.
async fn finish<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    result: Result<ExecutionResult, Error>,
    captured: Captured,
) -> Result<ExecutionResult, Error> {
    forward(context, captured.events).await?;
    let name = &context.command_name;
    let mut notes = String::new();
    if captured.input_cut_reached {
        notes.push_str(&format!(
            "{name}: standard input over {} is unsupported in bash-tool\n",
            super::buffer_limit()
        ));
    }
    if captured.output_cut {
        notes.push_str(&format!(
            "{name}: output over {} is unsupported in bash-tool\n",
            super::buffer_limit()
        ));
    }
    if notes.is_empty() {
        return result;
    }
    log::warn!("{}", notes.trim_end());
    let code = u8::from(result?.exit_code);
    context
        .stderr()
        .async_io()
        .write_all(notes.as_bytes())
        .await?;
    Ok(ExecutionResult::new(code.max(1)))
}

pub(crate) fn finite<C: SimpleCommand + Send + Sync, SE: ShellExtensions>(
    mut context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let strings = argv(args.clone());
        let reads_stdin = command_reads_stdin(&context, &strings);
        // An operand that spells out a path to stdin (`/dev/stdin`, `/dev/fd/0`, ...) is not
        // this: resolving that path when the descriptor it names is closed is a real, per-tool
        // `open()` failure (`ENOENT` -- ultimately routed through `devices::open_device`'s own
        // "nothing served" branch, once `command_input` leaves this fd unserved for exactly
        // that reason), not the synthetic message below. Only the bare `-`/no-operand
        // convention needs it, since that never resolves a path at all -- it just reads
        // whatever `stdin()` was hooked to, which Rust reports as a silent empty read on a
        // closed descriptor instead of an error.
        let names_stdin_path = strings.iter().skip(1).any(|arg| {
            arg != "-" && super::devices::operand(arg) == Some(super::devices::Device::Stream(0))
        });
        if reads_stdin && !names_stdin_path && context.try_fd(OpenFiles::STDIN_FD).is_none() {
            // A closed standard input. Rust's standard input reads a closed descriptor as empty,
            // so the utility would never see the error; report it as GNU's utilities do. A name
            // for it (`/dev/stdin`) names nothing then, as `/proc/self/fd/0` does on Linux.
            let named = strings
                .iter()
                .skip(1)
                .find(|arg| arg.as_str() != "-" && names_stdin(&context, arg));
            let (message, status) = if let Some(name) = named {
                (format!("{name}: No such file or directory"), 1)
            } else if context.command_name == "sort" {
                ("stat failed: -".to_owned(), 2)
            } else {
                ("-".to_owned(), 1)
            };
            let text = if named.is_some() {
                format!("{}: {message}\n", context.command_name)
            } else {
                format!("{}: {message}: Bad file descriptor\n", context.command_name)
            };
            context
                .stderr()
                .async_io()
                .write_all(text.as_bytes())
                .await?;
            return Ok(ExecutionResult::new(status));
        }
        // A short flag isn't in a `clap::Error` alone (see `gnu_clap_error`), so the two kinds
        // that need one for GNU's own wording are checked here, ahead of the real call, against
        // this tool's own `Command` -- when there is one; most callers of `finite` (e.g. the text
        // tools) have no entry in `uu_command` and this is skipped entirely. A parse that
        // succeeds, or fails some other way, changes nothing: the real call below still runs and
        // reports it, now through `ClapErrorWrapper`'s own translation for every other kind.
        if let Some((command, status)) = super::coreutils::uu_command(&context.command_name)
            && let Err(error) = command
                .clone()
                .try_get_matches_from(strings.iter().cloned())
            && let Some((message, status)) =
                super::coreutils::gnu_clap_error(&context.command_name, &command, &error, status)
        {
            context
                .stderr()
                .async_io()
                .write_all(message.as_bytes())
                .await?;
            return Ok(ExecutionResult::new(status));
        }
        let input = command_input(&context, &strings, reads_stdin).await?;
        let (result, captured) = capture_operation(&mut context, input, true, |child| {
            C::execute(child, strings.into_iter())
        });
        finish(&context, result, captured).await
    })
}

/// A finite command's input, read before it runs: its standard input when it reads it (a file
/// is handed over as it is, since the command can read that as it goes), and the descriptors its
/// operands name.
async fn command_input<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    args: &[String],
    reads_stdin: bool,
) -> Result<Input, Error> {
    let (stdin, cut, stdin_closed) = match context.try_fd(OpenFiles::STDIN_FD) {
        Some(file @ OpenFile::File(_)) => (file, false, false),
        None => (closed(), false, true),
        _ if reads_stdin => {
            let (bytes, cut) = read_limited(input(context)).await?;
            (brush_core::openfiles::from_bytes(bytes), cut, false)
        }
        _ => (brush_core::openfiles::from_bytes(Vec::new()), false, false),
    };
    Ok(Input {
        stdin,
        cut,
        stdin_closed,
        descriptors: named_descriptors(context, args).await,
    })
}

/// Reads `source` up to [`MAX_BUFFERED_BYTES`]; whether more was left unread.
pub(crate) async fn read_limited(mut source: OpenFile) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut chunk = vec![0; 64 * 1024];
    loop {
        let count = source.async_io().read(&mut chunk).await?;
        if count == 0 {
            return Ok((bytes, false));
        }
        if bytes.len() + count > MAX_BUFFERED_BYTES {
            let room = MAX_BUFFERED_BYTES - bytes.len();
            bytes.extend_from_slice(&chunk[..room]);
            return Ok((bytes, true));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
}

/// Whether the finite command running now reads its standard input with these arguments. It
/// runs synchronously, so its input is read before it starts, and a command that will not read
/// it must not consume it: `{ nl f; cat; } <<< x` leaves `x` for `cat`, as in bash.
fn command_reads_stdin<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    args: &[String],
) -> bool {
    let name = context.command_name.as_str();
    let is_stdin = |operand: &str| operand == "-" || names_stdin(context, operand);
    let names_it = || args.iter().skip(1).any(|arg| names_stdin(context, arg));
    // Positional operands as the utility's own parser sees them; `None` when it rejects the
    // arguments (it then reads nothing).
    let operands = |app: clap::Command| positional_operands(app, args);
    let reads_files = |app: clap::Command| {
        operands(app).is_some_and(|ops| ops.is_empty() || ops.iter().any(|op| is_stdin(op)))
    };
    match name {
        // Answers to `-i` prompts come from standard input.
        "rm" | "mv" | "cp" | "ln" if interactive(args, name == "rm") => true,
        "cp" | "sed" => names_it(),
        "file" => args.iter().skip(1).any(|arg| is_stdin(arg)),
        "date" => uu_date::uu_app()
            .try_get_matches_from(args)
            .ok()
            .and_then(|matches| matches.get_one::<OsString>("file").cloned())
            .is_some_and(|file| is_stdin(&file.to_string_lossy())),
        "split" => operands(uu_split::uu_app())
            .is_some_and(|ops| ops.first().is_none_or(|op| is_stdin(op))),
        "csplit" => operands(uu_csplit::uu_app())
            .is_some_and(|ops| ops.first().is_some_and(|op| is_stdin(op))),
        "factor" => operands(uu_factor::uu_app()).is_some_and(|ops| ops.is_empty()),
        "shuf" => {
            uu_shuf::uu_app()
                .try_get_matches_from(args)
                .is_ok_and(|matches| {
                    !matches.get_flag("echo") && !matches.contains_id("input-range")
                })
                && reads_files(uu_shuf::uu_app())
        }
        "nl" => reads_files(uu_nl::uu_app()),
        "paste" => reads_files(uu_paste::uu_app()),
        "join" => reads_files(uu_join::uu_app()),
        "comm" => reads_files(uu_comm::uu_app()),
        "fold" => reads_files(uu_fold::uu_app()),
        "fmt" => reads_files(uu_fmt::uu_app()),
        "expand" => reads_files(uu_expand::uu_app()),
        "unexpand" => reads_files(uu_unexpand::uu_app()),
        "tsort" => reads_files(uu_tsort::uu_app()),
        "base64" => reads_files(uu_base64::uu_app()),
        "base32" => reads_files(uu_base32::uu_app()),
        "basenc" => reads_files(uu_basenc::uu_app()),
        "md5sum" => reads_files(uu_md5sum::uu_app()),
        "sha1sum" => reads_files(uu_sha1sum::uu_app()),
        "sha256sum" => reads_files(uu_sha256sum::uu_app()),
        "sha512sum" => reads_files(uu_sha512sum::uu_app()),
        "b2sum" => reads_files(uu_b2sum::uu_app()),
        "cksum" => reads_files(uu_cksum::uu_app()),
        "od" => reads_files(uu_od::uu_app()),
        "numfmt" => reads_files(uu_numfmt::uu_app()),
        "sort" => reads_files(uu_sort::uu_app()),
        // `dd`'s operands are `KEY=VALUE`, not positional files, so it needs its own check:
        // it reads standard input when `if=` is absent (its default) or names it explicitly.
        "dd" => match args.iter().skip(1).find_map(|arg| arg.strip_prefix("if=")) {
            Some(value) => is_stdin(value),
            None => true,
        },
        // Their own parsers; they may read standard input whatever their arguments.
        "diff" | "cmp" | "patch" | "tac" => true,
        _ => false,
    }
}

/// The values of every positional argument of `app`, as it parses `args`.
fn positional_operands(app: clap::Command, args: &[String]) -> Option<Vec<String>> {
    let ids: Vec<clap::Id> = app
        .get_positionals()
        .map(|arg| arg.get_id().clone())
        .collect();
    let matches = app.try_get_matches_from(args).ok()?;
    Some(
        ids.iter()
            .filter_map(|id| matches.get_raw(id.as_str()))
            .flatten()
            .map(|value| value.to_string_lossy().into_owned())
            .collect(),
    )
}

/// Whether `rm`, `mv`, `cp` or `ln` prompts (`-i`, `--interactive`, and `rm -I`).
fn interactive(args: &[String], once_too: bool) -> bool {
    args.iter()
        .skip(1)
        .take_while(|arg| *arg != "--")
        .any(|arg| match arg.strip_prefix("--") {
            Some(long) => long.starts_with("interactive") && long != "interactive=never",
            None => {
                arg.len() > 1
                    && arg.starts_with('-')
                    && arg
                        .chars()
                        .skip(1)
                        .any(|c| c == 'i' || (once_too && c == 'I'))
            }
        })
}

/// The descriptors above 2 that a finite command's arguments name as `/dev/fd/N`, so it can open
/// them (see `devices`). A regular file is handed over as it is; a pipe or substitution is read
/// first, as the command runs synchronously, up to [`MAX_BUFFERED_BYTES`]; one that cannot be
/// read, such as an output substitution, is handed over for writing.
async fn named_descriptors<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    args: &[String],
) -> std::collections::HashMap<i32, OpenFile> {
    let mut descriptors = std::collections::HashMap::new();
    let mut named = Vec::new();
    for arg in args.iter().skip(1) {
        let path = context.shell.absolute_path(Path::new(arg));
        if let Some(super::devices::Device::Descriptor(fd)) = super::devices::classify(&path) {
            named.push(fd);
        }
        // Options that take a file: `--file=/dev/fd/3`, `-o/dev/fd/4`.
        for prefix in ["/dev/fd/", "/proc/self/fd/"] {
            for (at, _) in arg.match_indices(prefix) {
                let digits: String = arg[at + prefix.len()..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                if let Ok(fd) = digits.parse::<i32>()
                    && fd > 2
                {
                    named.push(fd);
                }
            }
        }
    }
    for fd in named {
        if descriptors.contains_key(&fd) {
            continue;
        }
        let Some(file) = context.try_fd(fd) else {
            continue;
        };
        if matches!(file, OpenFile::File(_)) || brush_core::openfiles::is_null_sink(&file) {
            descriptors.insert(fd, file);
            continue;
        }
        let file = match read_limited(file.clone()).await {
            Ok((bytes, cut)) => brush_core::openfiles::from_bytes_then(
                bytes,
                cut.then_some(brush_core::openfiles::TRUNCATED_SUBSTITUTION),
            ),
            Err(_) => file,
        };
        descriptors.insert(fd, file);
    }
    descriptors
}

/// Never access the real WASI stdin resource in a tool component. A standard input the script
/// closed (`<&-`) fails to read, as a closed descriptor does.
pub(crate) fn input<SE: ShellExtensions>(context: &ExecutionContext<'_, SE>) -> OpenFile {
    match context.try_fd(OpenFiles::STDIN_FD) {
        Some(file @ (OpenFile::File(_) | OpenFile::PipeReader(_) | OpenFile::Stream(_))) => file,
        Some(_) => brush_core::openfiles::from_bytes(Vec::new()),
        None => closed(),
    }
}

/// A descriptor the script closed: every read fails with `EBADF`.
fn closed() -> OpenFile {
    #[derive(Clone)]
    struct Closed;
    impl io::Read for Closed {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from_raw_os_error(libc::EBADF))
        }
    }
    impl io::Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from_raw_os_error(libc::EBADF))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Stream for Closed {
        fn clone_box(&self) -> Box<dyn Stream> {
            Box::new(Self)
        }
        #[cfg(unix)]
        fn try_clone_to_owned(&self) -> Result<std::os::fd::OwnedFd, Error> {
            Err(brush_core::ErrorKind::CannotConvertToNativeFd.into())
        }
        #[cfg(unix)]
        fn try_borrow_as_fd(&self) -> Result<std::os::fd::BorrowedFd<'_>, Error> {
            Err(brush_core::ErrorKind::CannotConvertToNativeFd.into())
        }
    }
    OpenFile::Stream(Box::new(Closed))
}

/// The command's own stream when `path` names a standard stream (`/dev/stdin`, `/dev/fd/1`, …).
/// The process descriptors behind those names are not this command's streams; see `devices`.
pub(crate) fn standard_stream<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    path: &str,
) -> Option<OpenFile> {
    match super::devices::classify(&context.shell.absolute_path(Path::new(path)))? {
        super::devices::Device::Stream(0) => Some(input(context)),
        super::devices::Device::Stream(1) => Some(context.stdout()),
        super::devices::Device::Stream(2) => Some(context.stderr()),
        super::devices::Device::Descriptor(fd) => context.try_fd(fd),
        _ => None,
    }
}

/// The command's own descriptor when `path` names one above 2, as a process substitution's
/// `/dev/fd/63` does.
fn named_descriptor<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    path: &str,
) -> Option<Result<OpenFile, String>> {
    match super::devices::classify(&context.shell.absolute_path(Path::new(path)))? {
        super::devices::Device::Descriptor(fd) => Some(
            context
                .try_fd(fd)
                .ok_or_else(|| "No such file or directory".to_owned()),
        ),
        _ => None,
    }
}

/// What a command reads when it opens `/dev/stdin` or `/dev/fd/N` onto `file`, as on Linux,
/// where those paths link to `/proc/self/fd/N`: a pipe or buffer is the same stream, position
/// and all, but a regular file is opened again from its start (see `devices::reopen`). `-` is no
/// such path: it reads the descriptor itself.
pub(crate) fn opened_again(file: OpenFile) -> OpenFile {
    #[cfg(target_arch = "wasm32")]
    if let OpenFile::File(file) = file {
        return OpenFile::from(super::devices::reopen(file));
    }
    file
}

/// Whether `path` names the command's standard input, as `/dev/stdin` does.
fn names_stdin<SE: ShellExtensions>(context: &ExecutionContext<'_, SE>, path: &str) -> bool {
    super::devices::classify(&context.shell.absolute_path(Path::new(path)))
        == Some(super::devices::Device::Stream(0))
}

/// Whether operand `path` reads the command's standard input: `-` or a name for it.
pub(crate) fn reads_stdin<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    path: &str,
) -> bool {
    path == "-" || names_stdin(context, path)
}

/// A path operand the way GNU's own "no such operand" diagnostics show it: bare for an ordinary
/// name (`/tmp/nosuch: No such file or directory`), but wrapped in ASCII quotes when it is the
/// empty string, since an unquoted empty name would leave nothing between the two colons for a
/// reader to see (`'': No such file or directory`, not `: No such file or directory`).
fn display_path(path: &str) -> std::borrow::Cow<'_, str> {
    if path.is_empty() {
        std::borrow::Cow::Borrowed("''")
    } else {
        std::borrow::Cow::Borrowed(path)
    }
}

pub(crate) fn operand<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    path: &str,
) -> Result<OpenFile, String> {
    if path == "-" {
        return Ok(input(context));
    }
    if names_stdin(context, path) {
        // A closed standard input has no `/proc/self/fd/0` for the name to open, as on Linux.
        if context.try_fd(OpenFiles::STDIN_FD).is_none() {
            return Err("No such file or directory".to_owned());
        }
        return Ok(opened_again(input(context)));
    }
    if let Some(descriptor) = named_descriptor(context, path) {
        return descriptor.map(opened_again);
    }
    let path = context.shell.absolute_path(Path::new(path));
    let name = path.to_string_lossy();
    if name == "/dev/null" {
        return Ok(brush_core::openfiles::from_bytes(Vec::new()));
    }
    std::fs::File::open(path)
        .map(OpenFile::from)
        .map_err(|error| super::io_message(&error))
}

/// Report a uutils error as the utility's own `uumain` would. A clap usage error prints itself,
/// straight to the process streams, when it is displayed; displaying it inside a capture keeps
/// that text in this command's streams, so redirections such as `2>/dev/null` still apply.
async fn report_uu_error<SE: ShellExtensions>(
    context: &mut ExecutionContext<'_, SE>,
    util: &'static str,
    error: Box<dyn uucore::error::UError>,
) -> Result<ExecutionResult, Error> {
    let ((text, code), captured) = capture_operation(context, Input::empty(), true, |child| {
        let mut text = String::new();
        super::coreutils::run_uu(&child, util, || {
            text = super::coreutils::uu_error_text(util, &*error);
            0
        });
        // Displaying a help or version "error" decides its status, so read it afterwards.
        (text, u8::try_from(error.code().clamp(0, 255)).unwrap_or(1))
    });
    forward(context, captured.events).await?;
    let mut destination = if code == 0 {
        context.stdout()
    } else {
        context.stderr()
    };
    destination.async_io().write_all(text.as_bytes()).await?;
    Ok(ExecutionResult::new(code))
}

fn argv(args: Vec<CommandArg>) -> Vec<String> {
    args.into_iter().map(|arg| arg.to_string()).collect()
}

fn files(matches: &clap::ArgMatches, key: &str) -> Vec<String> {
    matches.get_many::<OsString>(key).map_or_else(
        || vec!["-".into()],
        |values| {
            values
                .map(|value| value.to_string_lossy().into_owned())
                .collect()
        },
    )
}

#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one switch per existing cat CLI option"
)]
struct CatState {
    number: u64,
    at_start: bool,
    blank_kept: bool,
    pending_cr: bool,
    number_all: bool,
    number_nonblank: bool,
    squeeze: bool,
    ends: bool,
    tabs: bool,
    nonprint: bool,
}

impl CatState {
    fn new(matches: &clap::ArgMatches) -> Self {
        Self {
            number: 1,
            at_start: true,
            number_all: matches.get_flag("number") && !matches.get_flag("number-nonblank"),
            number_nonblank: matches.get_flag("number-nonblank"),
            squeeze: matches.get_flag("squeeze-blank"),
            ends: matches.get_flag("show-ends")
                || matches.get_flag("show-all")
                || matches.get_flag("e"),
            tabs: matches.get_flag("show-tabs")
                || matches.get_flag("show-all")
                || matches.get_flag("t"),
            nonprint: matches.get_flag("show-nonprinting")
                || matches.get_flag("show-all")
                || matches.get_flag("t")
                || matches.get_flag("e"),
            ..Self::default()
        }
    }
    fn prefix(&mut self, out: &mut Vec<u8>) {
        out.extend_from_slice(format!("{:>6}\t", self.number).as_bytes());
        self.number += 1;
    }
    fn byte(&self, mut byte: u8, out: &mut Vec<u8>) {
        if byte == b'\t' {
            out.extend_from_slice(if self.tabs { b"^I" } else { b"\t" });
        } else if self.nonprint {
            if byte >= 128 {
                out.extend_from_slice(b"M-");
                byte -= 128;
            }
            if byte < 32 {
                out.extend_from_slice(&[b'^', byte + 64]);
            } else if byte == 127 {
                out.extend_from_slice(b"^?");
            } else {
                out.push(byte);
            }
        } else {
            out.push(byte);
        }
    }
    fn push(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        for &byte in bytes {
            if self.pending_cr {
                self.pending_cr = false;
                if byte == b'\n' {
                    out.extend_from_slice(b"^M");
                } else {
                    self.byte(b'\r', out);
                }
            }
            if byte == b'\n' {
                if !(self.at_start && self.squeeze && self.blank_kept) {
                    if self.at_start && self.number_all {
                        self.prefix(out);
                    }
                    if self.ends {
                        out.push(b'$');
                    }
                    out.push(b'\n');
                }
                self.blank_kept = self.at_start;
                self.at_start = true;
            } else {
                if self.at_start && (self.number_all || self.number_nonblank) {
                    self.prefix(out);
                }
                self.at_start = false;
                self.blank_kept = false;
                if byte == b'\r' && self.ends {
                    self.pending_cr = true;
                } else {
                    self.byte(byte, out);
                }
            }
        }
    }
}

fn cat_impl<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let args = argv(args);
        let Ok(matches) = uu_cat::uu_app().try_get_matches_from(&args) else {
            return finite::<super::coreutils::Cat, SE>(
                context,
                args.into_iter().map(CommandArg::String).collect(),
            )
            .await;
        };
        // GNU looks at its output before reading anything: a closed one ends it at once.
        if context.try_fd(1).is_none() {
            context
                .stderr()
                .async_io()
                .write_all(b"cat: standard output: Bad file descriptor\n")
                .await?;
            return Ok(ExecutionResult::new(1));
        }
        let mut state = CatState::new(&matches);
        let mut stdout = context.stdout();
        let mut stderr = context.stderr();
        // GNU cat checks its output before reading anything.
        if context.try_fd(OpenFiles::STDOUT_FD).is_none() {
            stderr
                .async_io()
                .write_all(b"cat: standard output: Bad file descriptor\n")
                .await?;
            return Ok(ExecutionResult::new(1));
        }
        let mut failed = false;
        let mut buffer = vec![0; 16 * 1024];
        let mut rendered = Vec::new();
        let output = match &stdout {
            OpenFile::File(file) => file_identity(file).ok(),
            _ => None,
        };
        for path in files(&matches, "file") {
            let mut source = match operand(&context, &path) {
                Ok(source) => source,
                Err(error) => {
                    // GNU quotes a name that needs it; the empty one always does.
                    let name = if path.is_empty() { "''" } else { path.as_str() };
                    stderr
                        .async_io()
                        .write_all(format!("cat: {name}: {error}\n").as_bytes())
                        .await?;
                    failed = true;
                    continue;
                }
            };
            // As GNU: copying a nonempty file onto itself (`cat f >> f`) would never end.
            if let (Some(output), OpenFile::File(file)) = (output, &source)
                && file_identity(file).is_ok_and(|input| input == output)
                && let (Ok(position), Ok(metadata)) = (
                    std::io::Seek::stream_position(&mut &**file),
                    file.metadata(),
                )
                && position < metadata.len()
            {
                let name = if path == "-" { "-" } else { path.as_str() };
                stderr
                    .async_io()
                    .write_all(format!("cat: {name}: input file is output file\n").as_bytes())
                    .await?;
                failed = true;
                continue;
            }
            loop {
                match source.async_io().read(&mut buffer).await {
                    Ok(0) => break,
                    Ok(n) => {
                        rendered.clear();
                        state.push(&buffer[..n], &mut rendered);
                        stdout.async_io().write_all(&rendered).await?;
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
                    Err(error) => {
                        stderr
                            .async_io()
                            .write_all(
                                format!("cat: {path}: {}\n", super::io_message(&error)).as_bytes(),
                            )
                            .await?;
                        failed = true;
                        break;
                    }
                }
            }
        }
        if state.pending_cr {
            rendered.clear();
            state.byte(b'\r', &mut rendered);
            stdout.async_io().write_all(&rendered).await?;
        }
        Ok(ExecutionResult::new(u8::from(failed)))
    })
}

// GNU obsolete head spelling, following uutils head's MIT-licensed parser.
fn normalize_head(args: &[String]) -> Vec<String> {
    let mut normalized = vec![args[0].clone()];
    let mut options = true;
    for (index, arg) in args[1..].iter().enumerate() {
        if arg == "--" {
            options = false;
        }
        let body = arg.strip_prefix('-').unwrap_or_default();
        let digits = body.bytes().take_while(u8::is_ascii_digit).count();
        if index != 0 || !options || digits == 0 {
            normalized.push(arg.clone());
            continue;
        }
        let mut verbose = None;
        let mut zero = false;
        let mut multiplier = None;
        let mut valid = true;
        for byte in body[digits..].bytes() {
            match byte {
                b'q' => verbose = Some(false),
                b'v' => verbose = Some(true),
                b'z' => zero = true,
                b'c' => multiplier = Some(1_u64),
                b'b' => multiplier = Some(512),
                b'k' => multiplier = Some(1024),
                b'm' => multiplier = Some(1024 * 1024),
                _ => {
                    valid = false;
                    break;
                }
            }
        }
        if !valid {
            normalized.push(arg.clone());
            continue;
        }
        if let Some(verbose) = verbose {
            normalized.push(if verbose { "-v" } else { "-q" }.into());
        }
        if zero {
            normalized.push("-z".into());
        }
        let count = body[..digits].parse::<u64>().unwrap_or(u64::MAX);
        normalized.push(if multiplier.is_some() { "-c" } else { "-n" }.into());
        normalized.push(count.saturating_mul(multiplier.unwrap_or(1)).to_string());
    }
    normalized
}

fn head_impl<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let args = argv(args);
        let normalized = normalize_head(&args);
        let Ok(matches) = uu_head::uu_app().try_get_matches_from(normalized) else {
            return finite::<super::coreutils::Head, SE>(
                context,
                args.into_iter().map(CommandArg::String).collect(),
            )
            .await;
        };
        let bytes_mode = matches.get_one::<String>("BYTES").is_some();
        let count = matches
            .get_one::<String>(if bytes_mode { "BYTES" } else { "LINES" })
            .map_or("10", String::as_str);
        let Ok(count) = uucore::parser::parse_signed_num::parse_signed_num_max(count) else {
            return finite::<super::coreutils::Head, SE>(
                context,
                args.into_iter().map(CommandArg::String).collect(),
            )
            .await;
        };
        let all_but_last = count.sign == Some(uucore::parser::parse_signed_num::SignPrefix::Minus);
        let paths = files(&matches, "FILE");
        let headers =
            matches.get_flag("VERBOSE") || (paths.len() > 1 && !matches.get_flag("QUIET"));
        let delimiter = if matches.get_flag("ZERO") { 0 } else { b'\n' };
        let mut first = true;
        let mut failed = false;
        let mut stdout = context.stdout();
        let mut stderr = context.stderr();
        for path in paths {
            let mut source = match operand(&context, &path) {
                Ok(source) => source,
                Err(error) => {
                    stderr
                        .async_io()
                        .write_all(
                            format!("head: cannot open '{path}' for reading: {error}\n").as_bytes(),
                        )
                        .await?;
                    failed = true;
                    continue;
                }
            };
            if headers {
                let name = if path == "-" { "standard input" } else { &path };
                stdout
                    .async_io()
                    .write_all(
                        format!("{}==> {name} <==\n", if first { "" } else { "\n" }).as_bytes(),
                    )
                    .await?;
                first = false;
            }
            let result = head_stream(
                &mut source,
                &mut stdout,
                count.value,
                bytes_mode,
                all_but_last,
                delimiter,
            )
            .await;
            if let Err(error) = result {
                if error.kind() == io::ErrorKind::BrokenPipe {
                    return Err(error.into());
                }
                stderr
                    .async_io()
                    .write_all(
                        format!(
                            "head: error reading '{path}': {}\n",
                            super::io_message(&error)
                        )
                        .as_bytes(),
                    )
                    .await?;
                failed = true;
            }
        }
        Ok(ExecutionResult::new(u8::from(failed)))
    })
}

async fn head_stream(
    source: &mut OpenFile,
    stdout: &mut OpenFile,
    count: u64,
    bytes_mode: bool,
    all_but_last: bool,
    delimiter: u8,
) -> io::Result<()> {
    let mut buffer = vec![0; 16 * 1024];
    let mut remaining = count;
    let mut window = VecDeque::new();
    let mut byte_window: VecDeque<u8> = VecDeque::new();
    let mut record = Vec::new();
    while all_but_last || remaining > 0 {
        let limit = if bytes_mode && !all_but_last {
            usize::try_from(remaining)
                .unwrap_or(usize::MAX)
                .min(buffer.len())
        } else {
            buffer.len()
        };
        let n = source.async_io().read(&mut buffer[..limit]).await?;
        if n == 0 {
            break;
        }
        if all_but_last {
            if bytes_mode {
                byte_window.extend(buffer[..n].iter().copied());
                let emit = byte_window
                    .len()
                    .saturating_sub(usize::try_from(count).unwrap_or(usize::MAX));
                record.clear();
                record.extend(byte_window.drain(..emit));
                stdout.async_io().write_all(&record).await?;
            } else {
                for &byte in &buffer[..n] {
                    record.push(byte);
                    if byte == delimiter {
                        window.push_back(std::mem::take(&mut record));
                        if u64::try_from(window.len()).unwrap_or(u64::MAX) > count {
                            stdout
                                .async_io()
                                .write_all(&window.pop_front().unwrap_or_default())
                                .await?;
                        }
                    }
                }
            }
        } else {
            let mut used = n;
            if bytes_mode {
                remaining -= u64::try_from(n).unwrap_or(remaining);
            } else {
                for (index, &byte) in buffer[..n].iter().enumerate() {
                    if byte == delimiter {
                        remaining -= 1;
                        if remaining == 0 {
                            used = index + 1;
                            break;
                        }
                    }
                }
            }
            stdout.async_io().write_all(&buffer[..used]).await?;
            if used < n
                && let OpenFile::File(file) = source
            {
                use io::Seek;
                let _ = file.as_ref().seek(io::SeekFrom::Current(
                    -i64::try_from(n - used).unwrap_or(i64::MAX),
                ))?;
            }
        }
    }
    if all_but_last && !bytes_mode && !record.is_empty() {
        window.push_back(record);
        if u64::try_from(window.len()).unwrap_or(u64::MAX) > count {
            stdout
                .async_io()
                .write_all(&window.pop_front().unwrap_or_default())
                .await?;
        }
    }
    Ok(())
}

/// Framing retains only the unfinished record and unread bytes in the current chunk.
pub(crate) struct Records {
    pub(crate) source: OpenFile,
    chunk: Vec<u8>,
    offset: usize,
    delimiter: u8,
}
impl Records {
    pub(crate) fn new(source: OpenFile, delimiter: u8) -> Self {
        Self {
            source,
            chunk: Vec::new(),
            offset: 0,
            delimiter,
        }
    }
    pub(crate) async fn next(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut record = Vec::new();
        loop {
            let available = &self.chunk[self.offset..];
            if let Some(end) = available.iter().position(|&byte| byte == self.delimiter) {
                record.extend_from_slice(&available[..=end]);
                self.offset += end + 1;
                return Ok(Some(record));
            }
            record.extend_from_slice(available);
            self.chunk.resize(16 * 1024, 0);
            let count = self.source.async_io().read(&mut self.chunk).await?;
            self.chunk.truncate(count);
            self.offset = 0;
            if count == 0 {
                return Ok((!record.is_empty()).then_some(record));
            }
        }
    }
}

fn cut_impl<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let args = argv(args);
        let normalized: Vec<_> = args
            .iter()
            .map(|arg| {
                if arg == "-d=" {
                    "--delimiter=="
                } else {
                    arg.as_str()
                }
            })
            .collect();
        uucore::set_embedded_util("cut");
        let Ok(matches) = uu_cut::uu_app().try_get_matches_from(
            normalized
                .iter()
                .map(|arg| super::shell_bytes::to_os_string(arg)),
        ) else {
            return finite::<super::coreutils::Cut, SE>(
                context,
                args.into_iter().map(CommandArg::String).collect(),
            )
            .await;
        };
        let delimiter = if matches.get_flag("zero-terminated") {
            0
        } else {
            b'\n'
        };
        // `uu_cut::cut_record` calls straight into uucore's locale detection
        // (`cut -c` byte-vs-character splitting), which reads the process environment. Unlike
        // `run_uu`'s path, this fast path calls the fork's library function directly rather
        // than going through `run_tool`, so it never got the shell's exported `LC_ALL`/... into
        // the process environment on its own -- confirmed by the oracle: `cut -c1-4` on a
        // multi-byte character truncated mid-character instead of taking 4 whole characters.
        #[cfg(target_arch = "wasm32")]
        let _env = super::coreutils::ProcessEnv::enter(super::coreutils::exported_env(&context));
        let mut stdout = context.stdout();
        let mut stderr = context.stderr();
        let mut rendered = Vec::new();
        if let Err(error) = uu_cut::cut_record(&matches, &[], &mut rendered) {
            let message = super::coreutils::uu_error_text("cut", &*error);
            stderr.async_io().write_all(message.as_bytes()).await?;
            return Ok(ExecutionResult::new(1));
        }
        let mut failed = false;
        for path in files(&matches, "file") {
            let source = match operand(&context, &path) {
                Ok(source) => source,
                Err(error) => {
                    failed = true;
                    stderr
                        .async_io()
                        .write_all(format!("cut: {}: {error}\n", display_path(&path)).as_bytes())
                        .await?;
                    continue;
                }
            };
            let mut records = Records::new(source, delimiter);
            loop {
                let record = match records.next().await {
                    Ok(Some(record)) => record,
                    Ok(None) => break,
                    Err(error) => {
                        failed = true;
                        let message = format!(
                            "cut: {}: {}\n",
                            display_path(&path),
                            super::io_message(&error)
                        );
                        stderr.async_io().write_all(message.as_bytes()).await?;
                        break;
                    }
                };
                rendered.clear();
                if let Err(error) = uu_cut::cut_record(&matches, &record, &mut rendered) {
                    let message = super::coreutils::uu_error_text("cut", &*error);
                    stderr.async_io().write_all(message.as_bytes()).await?;
                    return Ok(ExecutionResult::new(1));
                }
                stdout.async_io().write_all(&rendered).await?;
            }
        }
        Ok(ExecutionResult::new(u8::from(failed)))
    })
}

fn tr_impl<SE: ShellExtensions>(
    mut context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let strings = argv(args);
        let (parsed, captured) = capture_operation(&mut context, Input::empty(), true, |child| {
            let mut operation = None;
            super::coreutils::run_uu(&child, "tr", || {
                operation = Some(uu_tr::translator(
                    strings
                        .iter()
                        .map(|arg| super::shell_bytes::to_os_string(arg)),
                ));
                0
            });
            operation
        });
        forward(&context, captured.events).await?;
        let mut operation = match parsed {
            Some(Ok(operation)) => operation,
            Some(Err(error)) => return report_uu_error(&mut context, "tr", error).await,
            None => return Err(io::Error::other("tr initialization did not run").into()),
        };
        let mut source = input(&context);
        let mut stdout = context.stdout();
        let mut chunk = vec![0; 16 * 1024];
        let mut rendered = Vec::new();
        loop {
            let count = match source.async_io().read(&mut chunk).await {
                Ok(count) => count,
                Err(error) => {
                    let message = format!("tr: read error: {}\n", super::io_message(&error));
                    context
                        .stderr()
                        .async_io()
                        .write_all(message.as_bytes())
                        .await?;
                    return Ok(ExecutionResult::new(1));
                }
            };
            if count == 0 {
                break;
            }
            rendered.clear();
            rendered.extend(
                chunk[..count]
                    .iter()
                    .filter_map(|&byte| operation.translate(byte)),
            );
            stdout.async_io().write_all(&rendered).await?;
        }
        Ok(ExecutionResult::success())
    })
}

fn uniq_impl<SE: ShellExtensions>(
    mut context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let args = argv(args);
        uucore::set_embedded_util("uniq");
        let (mut state, paths) =
            match uu_uniq::UniqStream::from_args(args.iter().map(OsString::from)) {
                Ok(parsed) => parsed,
                Err(error) => return report_uu_error(&mut context, "uniq", error).await,
            };
        let path = paths
            .first()
            .map_or("-".into(), |path| path.to_string_lossy().into_owned());
        // Unlike the other streaming tools, GNU uniq's real answer for a *named* path to a
        // closed stdin (`/dev/stdin`, `/dev/fd/0`, ...) is still `Bad file descriptor` -- the
        // same reason bare `-` gets -- reported with the same bare `tool: path: reason` wrapper
        // `operand`'s open-error branch already uses, not the "error reading '...'" wrapper the
        // read loop below uses for errors on an already-opened source (e.g. a directory). So this
        // one case is special-cased ahead of `operand`, which otherwise answers every other
        // caller's version of the same path with `No such file or directory` instead.
        if path != "-"
            && names_stdin(&context, &path)
            && context.try_fd(OpenFiles::STDIN_FD).is_none()
        {
            context
                .stderr()
                .async_io()
                .write_all(format!("uniq: {path}: Bad file descriptor\n").as_bytes())
                .await?;
            return Ok(ExecutionResult::new(1));
        }
        let source = match operand(&context, &path) {
            Ok(source) => source,
            Err(error) => {
                context
                    .stderr()
                    .async_io()
                    .write_all(format!("uniq: {}: {error}\n", display_path(&path)).as_bytes())
                    .await?;
                return Ok(ExecutionResult::new(1));
            }
        };
        let mut stdout = if let Some(output) = paths.get(1).filter(|path| path.as_os_str() != "-") {
            match std::fs::File::create(context.shell.absolute_path(Path::new(output))) {
                Ok(file) => OpenFile::from(file),
                Err(error) => {
                    let message = format!(
                        "uniq: {}: {}\n",
                        output.to_string_lossy(),
                        super::io_message(&error)
                    );
                    context
                        .stderr()
                        .async_io()
                        .write_all(message.as_bytes())
                        .await?;
                    return Ok(ExecutionResult::new(1));
                }
            }
        } else {
            context.stdout()
        };
        let delimiter = state.delimiter();
        let mut records = Records::new(source, delimiter);
        let mut rendered = Vec::new();
        loop {
            let mut record = match records.next().await {
                Ok(Some(record)) => record,
                Ok(None) => break,
                Err(error) => {
                    let message = format!(
                        "uniq: error reading '{path}': {}\n",
                        super::io_message(&error)
                    );
                    context
                        .stderr()
                        .async_io()
                        .write_all(message.as_bytes())
                        .await?;
                    return Ok(ExecutionResult::new(1));
                }
            };
            if record.last() == Some(&delimiter) {
                record.pop();
            }
            rendered.clear();
            state
                .push(record, &mut rendered)
                .map_err(|error| io::Error::other(error.to_string()))?;
            stdout.async_io().write_all(&rendered).await?;
        }
        rendered.clear();
        state
            .finish(&mut rendered)
            .map_err(|error| io::Error::other(error.to_string()))?;
        stdout.async_io().write_all(&rendered).await?;
        Ok(ExecutionResult::success())
    })
}

/// `yes [STRING]...`: an unbounded producer. Writes the same line forever, one `.await` at a
/// time, so a downstream `head` closing its end of the pipe surfaces as a `BrokenPipe` write
/// error on the very next write rather than after materializing anything. The line itself comes
/// from `texttools::yes_line`, shared with the native driver so both format identically. The
/// write error is propagated (not swallowed here) so `utility()`/`utility_result` can apply the
/// same ignored-SIGPIPE-vs-default-disposition policy every other utility driver gets — with
/// SIGPIPE ignored (`trap '' PIPE`) GNU's `yes` reports `yes: standard output: Broken pipe` and
/// exits 1; with the default disposition, the pipeline's synthetic-SIGPIPE machinery is what
/// turns this into status 141.
fn yes_impl<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let args = argv(args);
        let line = match super::texttools::yes_plan(&args[1..]) {
            super::texttools::YesPlan::Repeat(line) => line,
            super::texttools::YesPlan::Exit(text, code) => {
                let mut destination = if code == 0 {
                    context.stdout()
                } else {
                    context.stderr()
                };
                destination.async_io().write_all(text.as_bytes()).await?;
                return Ok(ExecutionResult::new(code));
            }
        };
        let mut stdout = context.stdout();
        loop {
            stdout.async_io().write_all(&line).await?;
        }
    })
}

/// `seq`: pushes one term at a time from `texttools::SeqPlan`, so `seq 1 1000000000 | head -1`
/// returns as soon as the first `.await`ed write comes back, never allocating the full range.
/// Like `yes_impl`, a write error is propagated rather than caught, so the shared pipe-error
/// policy in `utility_result` applies.
fn seq_impl<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let args = argv(args);
        let plan = match super::texttools::SeqPlan::parse(&args[1..]) {
            Ok(plan) => plan,
            Err(message) => {
                // `message` is already a complete GNU-formatted diagnostic (see
                // `texttools::seq_usage_error`), trailing newline(s) included.
                context
                    .stderr()
                    .async_io()
                    .write_all(message.as_bytes())
                    .await?;
                return Ok(ExecutionResult::new(1));
            }
        };
        let mut stdout = context.stdout();
        for index in 0..plan.count {
            stdout.async_io().write_all(&plan.render(index)).await?;
        }
        Ok(ExecutionResult::success())
    })
}

/// `rev [FILE]...`: reverses one record at a time via `Records`/`texttools::rev_line`, so
/// `while :; do echo x; done | rev | head -1` terminates instead of blocking on a full read.
/// Files are opened one at a time (not all up front via `sources`/`chain`) so a missing operand
/// among several — `rev good1 /nonexist good2` — reports it and still processes `good2`, matching
/// GNU (util-linux) `rev`; the missing-file diagnostic drops the raw `(os error N)` suffix via
/// `tools::io_message`, matching GNU's wording exactly.
fn rev_impl<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let args = argv(args);
        let paths = &args[1..];
        let paths: Vec<&str> = if paths.is_empty() {
            vec!["-"]
        } else {
            paths.iter().map(String::as_str).collect()
        };
        let mut stdout = context.stdout();
        let mut stderr = context.stderr();
        let mut failed = false;
        for path in paths {
            let source = match operand(&context, path) {
                Ok(source) => source,
                Err(message) => {
                    failed = true;
                    stderr
                        .async_io()
                        .write_all(format!("rev: {path}: {message}\n").as_bytes())
                        .await?;
                    continue;
                }
            };
            let mut records = Records::new(source, b'\n');
            // util-linux rev stops reading an operand it cannot read (a directory) and says
            // nothing.
            while let Ok(Some(record)) = records.next().await {
                stdout
                    .async_io()
                    .write_all(&super::texttools::rev_line(&record))
                    .await?;
            }
        }
        Ok(ExecutionResult::new(u8::from(failed)))
    })
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn sleep<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let args = argv(args);
        let Ok(matches) = uu_sleep::uu_app().try_get_matches_from(&args) else {
            return finite::<super::coreutils::Sleep, SE>(
                context,
                args.into_iter().map(CommandArg::String).collect(),
            )
            .await;
        };
        let Some(numbers) = matches.get_many::<String>("NUMBER") else {
            return finite::<super::coreutils::Sleep, SE>(
                context,
                args.into_iter().map(CommandArg::String).collect(),
            )
            .await;
        };
        let mut duration = std::time::Duration::ZERO;
        let mut failed = false;
        for number in numbers {
            match uucore::parser::parse_time::from_str(number, true) {
                Ok(time) => duration = duration.saturating_add(time),
                Err(error) => {
                    failed = true;
                    context
                        .stderr()
                        .async_io()
                        .write_all(format!("sleep: {error}\n").as_bytes())
                        .await?;
                }
            }
        }
        if failed {
            // As uutils' own `sleep`: a bad interval is a usage error.
            context
                .stderr()
                .async_io()
                .write_all(b"Try 'sleep --help' for more information.\n")
                .await?;
            return Ok(ExecutionResult::new(1));
        }
        (context.shell.execution_services().sleep)(duration).await;
        Ok(ExecutionResult::success())
    })
}

fn tee_impl<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let args = argv(args);
        let Ok(matches) = uu_tee::uu_app().try_get_matches_from(&args) else {
            return finite::<super::coreutils::Tee, SE>(
                context,
                args.into_iter().map(CommandArg::String).collect(),
            )
            .await;
        };
        let mode = matches
            .get_one::<String>("output-error")
            .map(String::as_str)
            .or(matches
                .get_flag("ignore-pipe-errors")
                .then_some("warn-nopipe"));
        let mut destinations = vec![("standard output".to_string(), context.stdout(), true)];
        if mode.is_some() || pipe_ignored() {
            destinations[0].1.set_broken_pipe_cancellation(false);
        }
        let mut failed = false;
        let mut stderr = context.stderr();
        if mode.is_some() {
            // tee's explicit error modes handle EPIPE themselves, including a closed
            // diagnostic destination. This applies only to this command-owned handle.
            stderr.set_broken_pipe_cancellation(false);
        }
        if let Some(paths) = matches.get_many::<OsString>("file") {
            for path in paths {
                if let Some(stream) = standard_stream(&context, &path.to_string_lossy()) {
                    destinations.push((path.to_string_lossy().into_owned(), stream, false));
                    continue;
                }
                let absolute = context.shell.absolute_path(Path::new(path));
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .append(matches.get_flag("append"))
                    .truncate(!matches.get_flag("append"))
                    .open(absolute)
                {
                    Ok(file) => destinations.push((
                        path.to_string_lossy().into_owned(),
                        OpenFile::from(file),
                        false,
                    )),
                    Err(error) => {
                        failed = true;
                        stderr
                            .async_io()
                            .write_all(
                                format!(
                                    "tee: {}: {}\n",
                                    path.to_string_lossy(),
                                    super::io_message(&error)
                                )
                                .as_bytes(),
                            )
                            .await?;
                    }
                }
            }
        }
        let mut source = input(&context);
        let mut chunk = vec![0; 16 * 1024];
        loop {
            let count = source.async_io().read(&mut chunk).await?;
            if count == 0 {
                break;
            }
            let mut index = 0;
            while index < destinations.len() {
                let (name, stream, _stdout) = &mut destinations[index];
                match stream.async_io().write_all(&chunk[..count]).await {
                    Ok(()) => index += 1,
                    Err(error) => {
                        let pipe = error.kind() == io::ErrorKind::BrokenPipe;
                        if pipe && mode.is_none() && !pipe_ignored() {
                            return Err(error.into());
                        }
                        let ignored = pipe
                            && (matches!(mode, Some("warn-nopipe" | "exit-nopipe"))
                                || (mode.is_none() && pipe_ignored()));
                        if !ignored {
                            failed = true;
                            let message = if pipe {
                                format!("tee: '{name}': Broken pipe\n")
                            } else {
                                format!("tee: {name}: {}\n", super::io_message(&error))
                            };
                            if let Err(error) =
                                stderr.async_io().write_all(message.as_bytes()).await
                                && error.kind() != io::ErrorKind::BrokenPipe
                            {
                                return Err(error.into());
                            }
                            if matches!(mode, Some("exit" | "exit-nopipe")) {
                                return Ok(ExecutionResult::new(1));
                            }
                        }
                        destinations.remove(index);
                    }
                }
            }
            if destinations.is_empty() {
                break;
            }
        }
        Ok(ExecutionResult::new(u8::from(failed)))
    })
}

/// Sort requires EOF: its input is collected asynchronously, then all sorting options apply.
fn sort_impl<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    finite::<super::coreutils::Sort, SE>(context, args)
}

/// Files are opened against the command's own directory before any stream await.
#[cfg_attr(
    target_arch = "wasm32",
    expect(dead_code, reason = "the text drivers now open operands one at a time")
)]
pub(crate) fn sources<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    paths: &[String],
) -> io::Result<OpenFile> {
    if paths.is_empty() {
        return Ok(input(context));
    }
    let sources = paths
        .iter()
        .map(|path| {
            operand(context, path)
                .map(|source| (path.clone(), source))
                .map_err(io::Error::other)
        })
        .collect::<io::Result<_>>()?;
    Ok(chain(sources))
}

/// Read already opened files one after another as a single stream. A read error names the file
/// it came from, as GNU sed words it: `read error on NAME: …`.
pub(crate) fn chain(sources: Vec<(String, OpenFile)>) -> OpenFile {
    #[derive(Clone)]
    struct Chain(VecDeque<(String, OpenFile)>);
    fn named(name: &str, error: io::Error) -> io::Error {
        if error.kind() == io::ErrorKind::BrokenPipe {
            return error;
        }
        io::Error::other(format!(
            "read error on {name}: {}",
            super::io_message(&error)
        ))
    }
    impl io::Read for Chain {
        fn read(&mut self, data: &mut [u8]) -> io::Result<usize> {
            loop {
                let Some((name, source)) = self.0.front_mut() else {
                    return Ok(0);
                };
                let count = io::Read::read(source, data).map_err(|error| named(name, error))?;
                if count != 0 || data.is_empty() {
                    return Ok(count);
                }
                self.0.pop_front();
            }
        }
    }
    impl io::Write for Chain {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::PermissionDenied.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Stream for Chain {
        fn poll_read(
            &mut self,
            cx: &mut std::task::Context<'_>,
            data: &mut [u8],
        ) -> std::task::Poll<io::Result<usize>> {
            loop {
                let Some((name, source)) = self.0.front_mut() else {
                    return std::task::Poll::Ready(Ok(0));
                };
                match futures::io::AsyncRead::poll_read(std::pin::Pin::new(source), cx, data) {
                    std::task::Poll::Ready(Ok(0)) if !data.is_empty() => {
                        self.0.pop_front();
                    }
                    std::task::Poll::Ready(Err(error)) => {
                        return std::task::Poll::Ready(Err(named(name, error)));
                    }
                    result => return result,
                }
            }
        }
        fn clone_box(&self) -> Box<dyn Stream> {
            Box::new(self.clone())
        }
        #[cfg(unix)]
        fn try_clone_to_owned(&self) -> Result<std::os::fd::OwnedFd, Error> {
            Err(brush_core::ErrorKind::CannotConvertToNativeFd.into())
        }
        #[cfg(unix)]
        fn try_borrow_as_fd(&self) -> Result<std::os::fd::BorrowedFd<'_>, Error> {
            Err(brush_core::ErrorKind::CannotConvertToNativeFd.into())
        }
    }
    OpenFile::Stream(Box::new(Chain(sources.into())))
}

#[derive(Default, Clone)]
struct Counts {
    bytes: u64,
    chars: u64,
    lines: u64,
    words: u64,
    longest: u64,
    column: u64,
    in_word: bool,
    encoded: Vec<u8>,
}
impl Counts {
    fn character(&mut self, character: char) {
        use unicode_width::UnicodeWidthChar;
        self.chars += 1;
        if character.is_whitespace() || character == '\u{2060}' {
            self.in_word = false;
        } else if !self.in_word {
            self.words += 1;
            self.in_word = true;
        }
        match character {
            '\n' => {
                self.lines += 1;
                self.column = 0;
            }
            '\r' | '\u{000c}' => self.column = 0,
            '\t' => self.column += 8 - self.column % 8,
            character => self.column += character.width().unwrap_or(0) as u64,
        }
        self.longest = self.longest.max(self.column);
    }
    fn consume(&mut self, bytes: &[u8], eof: bool) {
        self.bytes += bytes.len() as u64;
        self.encoded.extend_from_slice(bytes);
        let mut offset = 0;
        loop {
            let remaining = &self.encoded[offset..];
            let (valid, invalid) = match std::str::from_utf8(remaining) {
                Ok(_) => (remaining.len(), None),
                Err(error) => (error.valid_up_to(), error.error_len()),
            };
            let text = String::from_utf8_lossy(&remaining[..valid]).into_owned();
            for character in text.chars() {
                self.character(character);
            }
            offset += valid;
            if let Some(invalid) = invalid {
                // Invalid bytes are non-whitespace, but do not count as UTF-8 characters.
                if !self.in_word {
                    self.words += 1;
                    self.in_word = true;
                }
                offset += invalid;
            } else {
                break;
            }
        }
        self.encoded.drain(..offset);
        if eof {
            self.encoded.clear();
        }
    }
    fn add(&mut self, other: &Self) {
        self.bytes += other.bytes;
        self.chars += other.chars;
        self.lines += other.lines;
        self.words += other.words;
        self.longest = self.longest.max(other.longest);
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "keeps wc option, input-list and per-file count handling together"
)]
fn wc_impl<SE: ShellExtensions>(
    mut context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        let args = argv(args);
        let Ok(matches) = uu_wc::uu_app().try_get_matches_from(&args) else {
            return finite::<super::coreutils::Wc, SE>(
                context,
                args.into_iter().map(CommandArg::String).collect(),
            )
            .await;
        };
        if matches.get_flag("debug") {
            let (_, captured) = capture_operation(&mut context, Input::empty(), false, |child| {
                <super::coreutils::Wc as SimpleCommand>::execute(
                    child,
                    ["wc", "--debug", "-c"].into_iter(),
                )
            });
            forward(
                &context,
                captured
                    .events
                    .into_iter()
                    .filter(|(stderr, _)| *stderr)
                    .collect(),
            )
            .await?;
        }
        let mut paths = files(&matches, "files");
        let supplied = matches.contains_id("files");
        let list = matches.get_one::<OsString>("files0-from");
        if let Some(list) = list {
            if supplied {
                context
                    .stderr()
                    .async_io()
                    .write_all(b"wc: file operands cannot be combined with --files0-from\n")
                    .await?;
                return Ok(ExecutionResult::new(1));
            }
            let mut records = Records::new(
                operand(&context, &list.to_string_lossy()).map_err(io::Error::other)?,
                0,
            );
            paths.clear();
            while let Some(mut record) = records.next().await? {
                if record.last() == Some(&0) {
                    record.pop();
                }
                paths.push(String::from_utf8_lossy(&record).into_owned());
            }
        }
        let mut enabled: Vec<&str> = ["lines", "words", "chars", "bytes", "max-line-length"]
            .into_iter()
            .filter(|name| matches.get_flag(name))
            .collect();
        if enabled.is_empty() {
            enabled = vec!["lines", "words", "bytes"];
        }
        let total_mode = matches
            .get_one::<String>("total")
            .map_or("auto", String::as_str);
        // GNU's column-alignment rule is two different algorithms depending on whether a
        // `total` line can follow. With more than one operand, the width is decided up
        // front (before any bytes are read) from the summed *byte size* of the named
        // files, so the counts line up with the total; a pipe or other operand whose size
        // isn't known ahead of time widens every column to 7. With exactly one operand
        // there is only one line to print, so GNU doesn't bother pre-sizing it from a
        // stat: it sizes the line from the counts it actually produced (the digit width of
        // the largest enabled counter on that line) -- except a non-seekable single pipe
        // with more than one counter requested, which still guesses 7 rather than wait.
        let single_operand = paths.len() == 1;
        // As GNU: any *named* input that is not a regular file (a pipe, `/dev/null`) widens
        // the columns. Standard input is handled on its own below, since "-" can't be
        // `stat`ed by path.
        // A zero-length entry from `--files0-from` is reported on its own (see the loop
        // below) and must not resolve to the shell's cwd and count toward the width.
        let irregular = |path: &String| {
            path != "-"
                && !path.is_empty()
                && std::fs::metadata(context.shell.absolute_path(Path::new(path)))
                    .is_ok_and(|metadata| !metadata.is_file())
        };
        let stdin_is_sized = |context: &ExecutionContext<'_, SE>| {
            matches!(context.try_fd(OpenFiles::STDIN_FD), Some(OpenFile::File(_)))
        };
        let mut width = 1;
        let mut size_pending = !single_operand;
        if single_operand {
            let unsized_pipe = if paths[0] == "-" {
                !stdin_is_sized(&context)
            } else {
                irregular(&paths[0])
            };
            if enabled.len() > 1 && unsized_pipe {
                width = 7;
            } else {
                // Computed after the read below, from the actual counts.
                size_pending = false;
            }
        } else if (enabled.len() > 1 && paths.iter().any(irregular))
            || (paths.iter().any(|path| path == "-") && !stdin_is_sized(&context))
        {
            // An unsized pipe among several operands widens to 7 regardless of how many
            // counters are enabled (unlike a named irregular file, which only does that
            // alongside a multi-counter request) -- GNU has no byte count to size the
            // `total` line's column from ahead of time either way.
            width = 7;
        }
        if size_pending {
            let total_size: u64 = paths
                .iter()
                .filter(|path| path.as_str() != "-" && !path.is_empty())
                .filter_map(|path| {
                    std::fs::metadata(context.shell.absolute_path(Path::new(path))).ok()
                })
                .map(|metadata| metadata.len())
                .sum();
            width = width.max(total_size.to_string().len());
        }
        let show_total = match total_mode {
            "always" | "only" => true,
            "never" => false,
            _ => paths.len() > 1,
        };
        let mut totals = Counts::default();
        let mut failed = false;
        let mut stdout = context.stdout();
        let mut stderr = context.stderr();
        let mut chunk = vec![0; 16 * 1024];
        for (index, path) in paths.iter().enumerate() {
            if path.is_empty() || (list.is_some_and(|list| list == "-") && path == "-") {
                failed = true;
                // GNU names the offending record's position when it came from
                // --files0-from; a plain zero-length operand has no such position (verified
                // against the oracle: both say "invalid zero-length file name", not
                // "zero-terminated").
                let message = if let Some(list) = list {
                    format!(
                        "wc: {}:{}: invalid zero-length file name\n",
                        list.to_string_lossy(),
                        index + 1
                    )
                } else {
                    "wc: invalid zero-length file name\n".to_owned()
                };
                stderr.async_io().write_all(message.as_bytes()).await?;
                continue;
            }
            let mut source = match operand(&context, path) {
                Ok(source) => source,
                Err(error) => {
                    failed = true;
                    stderr
                        .async_io()
                        .write_all(format!("wc: {path}: {error}\n").as_bytes())
                        .await?;
                    continue;
                }
            };
            let mut counts = Counts::default();
            // GNU still prints this operand's (zero) counts line for a read failure mid-way
            // (a directory opens fine and only fails on the actual read, with `EISDIR`) --
            // and prints it *before* the error, since the counts line is this operand's normal
            // output and the error is reported once that's done. So the message is held here
            // rather than written immediately, and flushed after the stdout write below.
            let mut pending_error = None;
            loop {
                match source.async_io().read(&mut chunk).await {
                    Ok(count) => {
                        counts.consume(&chunk[..count], count == 0);
                        if count == 0 {
                            break;
                        }
                    }
                    Err(error) => {
                        failed = true;
                        pending_error =
                            Some(format!("wc: {path}: {}\n", super::io_message(&error)));
                        break;
                    }
                }
            }
            totals.add(&counts);
            if single_operand && width != 7 {
                // No `total` line is coming and the size wasn't known up front: size this,
                // the only line, from what it actually printed.
                width = enabled
                    .iter()
                    .map(|name| {
                        match *name {
                            "lines" => counts.lines,
                            "words" => counts.words,
                            "chars" => counts.chars,
                            "bytes" => counts.bytes,
                            _ => counts.longest,
                        }
                        .to_string()
                        .len()
                    })
                    .max()
                    .unwrap_or(1);
            }
            if total_mode != "only" {
                let label = if supplied || list.is_some() {
                    Some(path.as_str())
                } else {
                    None
                };
                stdout
                    .async_io()
                    .write_all(format_counts(&counts, &enabled, width, label).as_bytes())
                    .await?;
            }
            if let Some(message) = pending_error {
                stderr.async_io().write_all(message.as_bytes()).await?;
            }
        }
        if show_total {
            stdout
                .async_io()
                .write_all(
                    format_counts(
                        &totals,
                        &enabled,
                        if total_mode == "only" { 1 } else { width },
                        if total_mode == "only" {
                            None
                        } else {
                            Some("total")
                        },
                    )
                    .as_bytes(),
                )
                .await?;
        }
        Ok(ExecutionResult::new(u8::from(failed)))
    })
}
fn format_counts(counts: &Counts, enabled: &[&str], width: usize, label: Option<&str>) -> String {
    let numbers = enabled
        .iter()
        .map(|name| match *name {
            "lines" => counts.lines,
            "words" => counts.words,
            "chars" => counts.chars,
            "bytes" => counts.bytes,
            _ => counts.longest,
        })
        .map(|count| format!("{count:>width$}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{numbers}{}\n",
        label.map_or_else(String::new, |label| format!(" {label}"))
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "keeps byte and record window handling with tail options"
)]
fn tail_impl<SE: ShellExtensions>(
    mut context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        use uu_tail::args::{FilterMode, Signum};
        let args = argv(args);
        uucore::set_embedded_util("tail");
        let Ok(settings) = uu_tail::args::parse_args(args.iter().map(OsString::from)) else {
            return finite::<super::coreutils::Tail, SE>(
                context,
                args.into_iter().map(CommandArg::String).collect(),
            )
            .await;
        };
        let (checked, warnings) = capture_operation(&mut context, Input::empty(), true, |child| {
            let mut checked = Ok(());
            super::coreutils::run_uu(&child, "tail", || {
                checked = settings.check_warnings();
                0
            });
            checked
        });
        forward(&context, warnings.events).await?;
        if let Err(error) = checked {
            let message = format!("tail: {}\n", super::io_message(&error));
            context
                .stderr()
                .async_io()
                .write_all(message.as_bytes())
                .await?;
            return Ok(ExecutionResult::new(1));
        }
        match settings.verify() {
            uu_tail::args::VerificationResult::NoOutput => return Ok(ExecutionResult::success()),
            uu_tail::args::VerificationResult::CannotFollowStdinByName => {
                context
                    .stderr()
                    .async_io()
                    .write_all(b"tail: cannot follow '-' by name\n")
                    .await?;
                return Ok(ExecutionResult::new(1));
            }
            uu_tail::args::VerificationResult::Ok => {}
        }
        #[cfg(target_arch = "wasm32")]
        let mut followers = Vec::new();
        let (bytes_mode, sign, delimiter) = match settings.mode {
            FilterMode::Bytes(sign) => (true, sign, b'\n'),
            FilterMode::Lines(sign, delimiter) => (false, sign, delimiter),
        };
        let (from_start, count) = match sign {
            Signum::Positive(count) => (true, count.saturating_sub(1)),
            Signum::PlusZero => (true, 0),
            Signum::Negative(count) => (false, count),
            Signum::MinusZero => (false, 0),
        };
        let headers = settings.verbose;
        let mut first = true;
        let mut stdout = context.stdout();
        let mut stderr = context.stderr();
        let mut failed = false;
        for path in &settings.inputs {
            let name = if path.is_stdin() {
                "-"
            } else {
                &path.display_name
            };
            let mut source = match operand(&context, name) {
                Ok(source) => source,
                Err(error) => {
                    #[cfg(target_arch = "wasm32")]
                    if settings.follow.is_some() && settings.retry && !path.is_stdin() {
                        followers.push(FollowFile {
                            path: context.shell.absolute_path(Path::new(name)),
                            file: None,
                            identity: None,
                        });
                    }
                    failed = true;
                    stderr
                        .async_io()
                        .write_all(
                            format!("tail: cannot open '{name}' for reading: {error}\n").as_bytes(),
                        )
                        .await?;
                    continue;
                }
            };
            #[cfg(target_arch = "wasm32")]
            if settings.follow.is_some()
                && !path.is_stdin()
                && let OpenFile::File(file) = &source
            {
                followers.push(FollowFile {
                    path: context.shell.absolute_path(Path::new(name)),
                    file: Some(file.clone()),
                    identity: Some(file_identity(file)?),
                });
            }
            if headers {
                stdout
                    .async_io()
                    .write_all(
                        format!(
                            "{}==> {} <==\n",
                            if first { "" } else { "\n" },
                            path.display_name
                        )
                        .as_bytes(),
                    )
                    .await?;
                first = false;
            }
            if bytes_mode {
                let mut chunk = vec![0; 16 * 1024];
                let mut remaining = count;
                let mut window = VecDeque::new();
                loop {
                    let size = match source.async_io().read(&mut chunk).await {
                        Ok(size) => size,
                        Err(error) => {
                            failed = true;
                            let message = format!(
                                "tail: error reading '{name}': {}\n",
                                super::io_message(&error)
                            );
                            stderr.async_io().write_all(message.as_bytes()).await?;
                            break;
                        }
                    };
                    if size == 0 {
                        break;
                    }
                    if from_start {
                        let skip = usize::try_from(remaining).unwrap_or(usize::MAX).min(size);
                        remaining -= skip as u64;
                        stdout.async_io().write_all(&chunk[skip..size]).await?;
                    } else {
                        window.extend(chunk[..size].iter().copied());
                        let excess = window
                            .len()
                            .saturating_sub(usize::try_from(count).unwrap_or(usize::MAX));
                        window.drain(..excess);
                    }
                }
                if !from_start {
                    stdout
                        .async_io()
                        .write_all(&window.into_iter().collect::<Vec<u8>>())
                        .await?;
                }
            } else {
                let mut records = Records::new(source, delimiter);
                let mut remaining = count;
                let mut window = VecDeque::new();
                loop {
                    let record = match records.next().await {
                        Ok(Some(record)) => record,
                        Ok(None) => break,
                        Err(error) => {
                            failed = true;
                            let message = format!(
                                "tail: error reading '{name}': {}\n",
                                super::io_message(&error)
                            );
                            stderr.async_io().write_all(message.as_bytes()).await?;
                            break;
                        }
                    };
                    if from_start {
                        if remaining > 0 {
                            remaining -= 1;
                        } else {
                            stdout.async_io().write_all(&record).await?;
                        }
                    } else {
                        window.push_back(record);
                        if window.len() as u64 > count {
                            window.pop_front();
                        }
                    }
                }
                if !from_start {
                    for record in window {
                        stdout.async_io().write_all(&record).await?;
                    }
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        if settings.follow.is_some() && !followers.is_empty() {
            follow_files(&context, &settings, followers).await?;
        }
        Ok(ExecutionResult::new(u8::from(failed)))
    })
}

#[cfg(target_arch = "wasm32")]
/// `printf` as bash's builtin behaves: `-v VAR` assigns the output instead of printing it, `%q`
/// quotes as bash does, and diagnostics read `bash: line N: printf: …`. Each item is formatted
/// and written in turn, so an endless reader downstream sees output as it is produced.
pub(crate) fn printf<SE: ShellExtensions>(
    mut context: ExecutionContext<'_, SE>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        use uucore::format::ArgumentLocation;
        use uucore::format::{FormatArgument, FormatArguments, FormatItem, Spec};
        const USAGE: &str = "printf: usage: printf [-v var] format [arguments]\n";
        let strings = argv(args.clone());
        let (variable, rest) = match strings.get(1).map(String::as_str) {
            Some("-v") => match strings.get(2) {
                Some(name) => (Some(name.clone()), &strings[3..]),
                None => {
                    let message = format!(
                        "{}printf: -v: option requires an argument\n{USAGE}",
                        context.shell.diagnostic_prefix()
                    );
                    context
                        .stderr()
                        .async_io()
                        .write_all(message.as_bytes())
                        .await?;
                    return Ok(ExecutionResult::new(2));
                }
            },
            Some(option) if option.starts_with("-v") => {
                (Some(option[2..].to_owned()), &strings[2..])
            }
            _ => (None, &strings[1..]),
        };
        // Bash refuses a -v name that is not a variable or an element of one, before printing.
        if let Some(name) = &variable {
            let base = name
                .split_once('[')
                .filter(|(_, rest)| rest.ends_with(']'))
                .map_or(name.as_str(), |(base, _)| base);
            if !brush_core::env::valid_variable_name(base) {
                let message = format!(
                    "{}printf: `{name}': not a valid identifier\n",
                    context.shell.diagnostic_prefix()
                );
                context
                    .stderr()
                    .async_io()
                    .write_all(message.as_bytes())
                    .await?;
                return Ok(ExecutionResult::new(2));
            }
        }
        let saw_dash_dash = rest.first().is_some_and(|word| word == "--");
        let rest = if saw_dash_dash { &rest[1..] } else { rest };
        // Any other `-`-led word ahead of FORMAT is an unrecognized option (`-v` is the
        // builtin's only one), not the format string itself -- unless `--` already said
        // there are no more options, in which case FORMAT is taken as-is even if it happens
        // to start with `-` (`printf -- '-v\nx\n'`).
        if !saw_dash_dash
            && let Some(unknown) = rest
                .first()
                .filter(|word| word.starts_with('-') && *word != "-")
        {
            let message = format!(
                "{}printf: {unknown}: invalid option\n{USAGE}",
                context.shell.diagnostic_prefix()
            );
            context
                .stderr()
                .async_io()
                .write_all(message.as_bytes())
                .await?;
            return Ok(ExecutionResult::new(2));
        }
        let Some((format, values)) = rest.split_first() else {
            context
                .stderr()
                .async_io()
                .write_all(USAGE.as_bytes())
                .await?;
            return Ok(ExecutionResult::new(2));
        };
        // The format and arguments as the bytes they stand for, including any that are not
        // UTF-8 (see `shell_bytes`).
        let format = super::shell_bytes::encode(format);
        let items = bash_printf_items(&format);
        let consumes = items.iter().any(|item| {
            matches!(
                item,
                PrintfItem::Uu(Ok(FormatItem::Spec(_)))
                    | PrintfItem::Consume
                    | PrintfItem::Quoted { .. }
                    | PrintfItem::Time { .. }
            )
        });
        let values: Vec<_> = values
            .iter()
            .map(|value| FormatArgument::Unparsed(super::shell_bytes::to_os_string(value)))
            .collect();
        let mut values = FormatArguments::new(&values);
        let prefix = context.shell.diagnostic_prefix();
        let mut stdout = context.stdout();
        let mut assigned = variable.as_ref().map(|_| Vec::new());
        let mut code = 0;
        'batches: loop {
            for item in &items {
                let mut output = Vec::new();
                let item = match item {
                    PrintfItem::Uu(item) => item,
                    // Bash's own: `%n` takes an argument and prints nothing.
                    PrintfItem::Consume => {
                        values.next_string(ArgumentLocation::NextArgument);
                        continue;
                    }
                    // `%Q`: `%q` with the precision applied to the argument before quoting.
                    PrintfItem::Quoted {
                        left,
                        width,
                        precision,
                    } => {
                        let value = super::shell_bytes::decode(
                            values
                                .next_string(ArgumentLocation::NextArgument)
                                .as_encoded_bytes(),
                        );
                        let value: String = match precision {
                            Some(precision) => value.chars().take(*precision).collect(),
                            None => value.into_owned(),
                        };
                        output.extend_from_slice(&pad(
                            bash_quote(&value).into_bytes(),
                            *left,
                            *width,
                        ));
                        if let Err(error) = emit(&mut stdout, assigned.as_mut(), &output).await {
                            return printf_write_failure(&context, &prefix, error).await;
                        }
                        continue;
                    }
                    // `%(FORMAT)T`: a time as strftime writes it, from seconds since the epoch
                    // (-1, or no argument, for now), as `date` formats it here.
                    PrintfItem::Time {
                        format,
                        left,
                        width,
                        precision,
                    } => {
                        let argument = values
                            .next_string(ArgumentLocation::NextArgument)
                            .to_string_lossy()
                            .into_owned();
                        let mut text = strftime(&mut context, format, &argument);
                        if let Some(precision) = precision {
                            text.truncate(*precision);
                        }
                        output.extend_from_slice(&pad(text, *left, *width));
                        if let Err(error) = emit(&mut stdout, assigned.as_mut(), &output).await {
                            return printf_write_failure(&context, &prefix, error).await;
                        }
                        continue;
                    }
                    // A warning bash gives while it goes on (`missing hex digit for \x`).
                    PrintfItem::Warn(message) => {
                        let message = format!("{prefix}printf: {message}\n");
                        context
                            .stderr()
                            .async_io()
                            .write_all(message.as_bytes())
                            .await?;
                        continue;
                    }
                };
                let formatted = match item {
                    Ok(FormatItem::Spec(Spec::QuotedString { position })) => {
                        let value = super::shell_bytes::decode(
                            values.next_string(*position).as_encoded_bytes(),
                        );
                        output.extend_from_slice(bash_quote(&value).as_bytes());
                        Some(Ok(std::ops::ControlFlow::Continue(())))
                    }
                    // `\c` in the FORMAT text itself (as opposed to inside a `%b`-expanded
                    // argument, where it does stop output — see the dedicated case that
                    // handles that through `item.write` below) is not one of the bash
                    // builtin's own recognized escapes: bash leaves it exactly as written,
                    // backslash included, like any other escape it doesn't know. uucore's
                    // shared parser (also used for external `printf(1)`'s own FORMAT, which
                    // *does* treat `\c` as a stop) instead turns it into `EscapedChar::End`;
                    // reproduce bash's literal passthrough for it here rather than in the
                    // shared parser.
                    Ok(FormatItem::Char(uucore::format::EscapedChar::End)) => {
                        output.extend_from_slice(b"\\c");
                        Some(Ok(std::ops::ControlFlow::Continue(())))
                    }
                    item => {
                        uucore::set_embedded_util("printf");
                        let ((formatted, status), captured) =
                            capture_operation(&mut context, Input::empty(), false, |child| {
                                let mut formatted = None;
                                let status = super::coreutils::run_uu(&child, "printf", || {
                                    formatted = Some(match item {
                                        Ok(item) => item.write(&mut output, &mut values),
                                        Err(error) => Err(printf_error(error)),
                                    });
                                    0
                                });
                                (formatted, status.max(uucore::error::get_exit_code()))
                            });
                        code = code.max(status);
                        // uucore reports an argument that is not a number in its own words.
                        let events = captured
                            .events
                            .into_iter()
                            .map(|(is_stderr, bytes)| {
                                let bytes = if is_stderr {
                                    printf_diagnostic(&bytes, &prefix)
                                } else {
                                    bytes
                                };
                                (is_stderr, bytes)
                            })
                            .collect();
                        forward(&context, events).await?;
                        formatted
                    }
                };
                let flow = match formatted {
                    Some(Ok(flow)) => flow,
                    Some(Err(error)) => {
                        // Keep any prefix produced before a formatting error.
                        if let Err(error) = emit(&mut stdout, assigned.as_mut(), &output).await {
                            return printf_write_failure(&context, &prefix, error).await;
                        }
                        let message = format!("{prefix}printf: {}\n", spec_message(&error));
                        context
                            .stderr()
                            .async_io()
                            .write_all(message.as_bytes())
                            .await?;
                        code = 1;
                        break 'batches;
                    }
                    None => return Ok(ExecutionResult::new(1)),
                };
                if let Err(error) = emit(&mut stdout, assigned.as_mut(), &output).await {
                    return printf_write_failure(&context, &prefix, error).await;
                }
                if flow.is_break() {
                    break 'batches;
                }
            }
            values.start_next_batch();
            if !consumes || values.is_exhausted() {
                break;
            }
        }
        if let (Some(name), Some(bytes)) = (variable, assigned) {
            let value = super::shell_bytes::decode_vec(bytes);
            brush_core::expansion::assign_to_named_parameter(
                context.shell,
                &context.params,
                &name,
                value,
            )
            .await?;
        }
        Ok(ExecutionResult::new(
            u8::try_from(code.clamp(0, 255)).unwrap_or(1),
        ))
    })
}

/// printf's output that could not be written, as bash reports it: `printf: write error: …`,
/// status 1. A broken pipe is the pipeline's to handle.
async fn printf_write_failure<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    prefix: &str,
    error: io::Error,
) -> Result<ExecutionResult, Error> {
    if error.kind() == io::ErrorKind::BrokenPipe {
        return Err(error.into());
    }
    let message = super::io_message(&error);
    let reason = message.strip_prefix("write error: ").unwrap_or(&message);
    let diagnostic = format!("{prefix}printf: write error: {reason}\n");
    // Nowhere to say it is no reason to fail differently.
    let _ = context
        .stderr()
        .async_io()
        .write_all(diagnostic.as_bytes())
        .await;
    Ok(ExecutionResult::new(1))
}

/// A piece of a builtin printf FORMAT: uucore's, or one bash has and `printf(1)` does not.
#[cfg(target_arch = "wasm32")]
enum PrintfItem {
    Uu(
        Result<
            uucore::format::FormatItem<uucore::format::EscapedChar>,
            uucore::format::FormatError,
        >,
    ),
    /// `%n`.
    Consume,
    /// `%Q`.
    Quoted {
        left: bool,
        width: Option<usize>,
        precision: Option<usize>,
    },
    /// `%(FORMAT)T`.
    Time {
        format: Vec<u8>,
        left: bool,
        width: Option<usize>,
        precision: Option<usize>,
    },
    Warn(&'static str),
}

/// Splits a builtin printf FORMAT into uucore's items and bash's own: `%n`, `%Q`, `%(…)T`, and
/// bash's reading of escapes -- `\x` and `\u` with no digit print as written with a warning,
/// and `\u`/`\U` take one to four (eight) digits.
#[cfg(target_arch = "wasm32")]
fn bash_printf_items(format: &[u8]) -> Vec<PrintfItem> {
    let mut items = Vec::new();
    let mut plain: Vec<u8> = Vec::new();
    let flush = |plain: &mut Vec<u8>, items: &mut Vec<PrintfItem>| {
        items.extend(uucore::format::parse_spec_and_escape(plain).map(PrintfItem::Uu));
        plain.clear();
    };
    let digits = |bytes: &[u8], at: usize, max: usize| {
        bytes[at..]
            .iter()
            .take(max)
            .take_while(|byte| byte.is_ascii_hexdigit())
            .count()
    };
    let mut i = 0;
    while i < format.len() {
        match format[i] {
            b'\\' if i + 1 < format.len() => {
                let kind = format[i + 1];
                match kind {
                    b'x' | b'u' | b'U' => {
                        let max = match kind {
                            b'x' => 2,
                            b'u' => 4,
                            _ => 8,
                        };
                        let count = digits(format, i + 2, max);
                        if count == 0 {
                            // As written, with a warning.
                            plain.extend_from_slice(b"\\\\");
                            plain.push(kind);
                            flush(&mut plain, &mut items);
                            items.push(PrintfItem::Warn(if kind == b'x' {
                                "missing hex digit for \\x"
                            } else if kind == b'u' {
                                "missing unicode digit for \\u"
                            } else {
                                "missing unicode digit for \\U"
                            }));
                        } else if kind == b'x' {
                            plain.extend_from_slice(&format[i..i + 2 + count]);
                        } else {
                            // uucore wants every digit: pad the short ones.
                            plain.push(b'\\');
                            plain.push(kind);
                            plain.extend(std::iter::repeat_n(b'0', max - count));
                            plain.extend_from_slice(&format[i + 2..i + 2 + count]);
                        }
                        i += 2 + count;
                    }
                    _ => {
                        plain.extend_from_slice(&format[i..i + 2]);
                        i += 2;
                    }
                }
            }
            b'%' if format.get(i + 1) == Some(&b'%') => {
                plain.extend_from_slice(b"%%");
                i += 2;
            }
            b'%' => {
                // Flags, width and precision, then the conversion.
                let mut j = i + 1;
                let mut left = false;
                while let Some(&flag) = format.get(j).filter(|b| b"-+ #0'".contains(b)) {
                    left |= flag == b'-';
                    j += 1;
                }
                let number = |j: &mut usize| {
                    let start = *j;
                    while format.get(*j).is_some_and(u8::is_ascii_digit) {
                        *j += 1;
                    }
                    std::str::from_utf8(&format[start..*j])
                        .ok()?
                        .parse::<usize>()
                        .ok()
                };
                let width = number(&mut j);
                let precision = if format.get(j) == Some(&b'.') {
                    j += 1;
                    Some(number(&mut j).unwrap_or(0))
                } else {
                    None
                };
                let bash_item = match format.get(j) {
                    Some(b'n') => Some((PrintfItem::Consume, j + 1)),
                    Some(b'Q') => Some((
                        PrintfItem::Quoted {
                            left,
                            width,
                            precision,
                        },
                        j + 1,
                    )),
                    Some(b'(') => {
                        format[j..]
                            .windows(2)
                            .position(|pair| pair == b")T")
                            .map(|end| {
                                (
                                    PrintfItem::Time {
                                        format: format[j + 1..j + end].to_vec(),
                                        left,
                                        width,
                                        precision,
                                    },
                                    j + end + 2,
                                )
                            })
                    }
                    _ => None,
                };
                match bash_item {
                    Some((item, next)) => {
                        flush(&mut plain, &mut items);
                        items.push(item);
                        i = next;
                    }
                    None => {
                        plain.push(b'%');
                        i += 1;
                    }
                }
            }
            byte => {
                plain.push(byte);
                i += 1;
            }
        }
    }
    flush(&mut plain, &mut items);
    items
}

/// `text` padded with spaces to `width`, on the right when `left` asks for it.
#[cfg(target_arch = "wasm32")]
fn pad(mut text: Vec<u8>, left: bool, width: Option<usize>) -> Vec<u8> {
    let length = String::from_utf8_lossy(&text).chars().count();
    if let Some(padding) = width.and_then(|width| width.checked_sub(length)) {
        let spaces = std::iter::repeat_n(b' ', padding);
        if left {
            text.extend(spaces);
        } else {
            text.splice(0..0, spaces);
        }
    }
    text
}

/// A time for `%(FORMAT)T`: `date`'s formatting of seconds since the epoch (`-1` or nothing
/// for now), in the command's time zone, without `date`'s newline.
#[cfg(target_arch = "wasm32")]
fn strftime<SE: ShellExtensions>(
    context: &mut ExecutionContext<'_, SE>,
    format: &[u8],
    argument: &str,
) -> Vec<u8> {
    let seconds = argument
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|&seconds| seconds >= 0);
    let mut argv = vec![std::ffi::OsString::from("date")];
    if let Some(seconds) = seconds {
        argv.push(format!("-d@{seconds}").into());
    }
    let mut spec = b"+".to_vec();
    spec.extend_from_slice(format);
    argv.push(super::shell_bytes::to_os_string(
        &super::shell_bytes::decode(&spec),
    ));
    let (_, captured) = capture_operation(context, Input::empty(), false, |child| {
        super::coreutils::run_uu(&child, "date", || uu_date::uumain(argv.into_iter()))
    });
    let mut text: Vec<u8> = captured
        .events
        .into_iter()
        .filter(|(is_stderr, _)| !is_stderr)
        .flat_map(|(_, bytes)| bytes)
        .collect();
    if text.last() == Some(&b'\n') {
        text.pop();
    }
    text
}

/// Writes printf output to the command's stdout, or keeps it for `-v`.
async fn emit(
    stdout: &mut OpenFile,
    assigned: Option<&mut Vec<u8>>,
    bytes: &[u8],
) -> io::Result<()> {
    match assigned {
        Some(buffer) => {
            buffer.extend_from_slice(bytes);
            Ok(())
        }
        None => stdout.async_io().write_all(bytes).await,
    }
}

/// A parse error in the format string, as a [`uucore::format::FormatError`].
fn printf_error(error: &uucore::format::FormatError) -> uucore::format::FormatError {
    use uucore::format::FormatError;
    match error {
        FormatError::SpecError(spec, range) => FormatError::SpecError(spec.clone(), range.clone()),
        FormatError::EndsWithPercent(spec) => FormatError::EndsWithPercent(spec.clone()),
        other => FormatError::InvalidPrecision(other.to_string()),
    }
}

/// A format-string error as bash words it: a conversion that stops at a length modifier or the
/// end lacks its character; any other letter is not a conversion.
fn spec_message(error: &uucore::format::FormatError) -> String {
    use uucore::format::FormatError;
    match error {
        // The spec is what follows the `%`.
        FormatError::SpecError(spec, _) => {
            let spec = String::from_utf8_lossy(spec);
            match spec.chars().last() {
                Some(last) if !"hlLqjzt".contains(last) => {
                    format!("`{last}': invalid format character")
                }
                _ => format!("`%{spec}': missing format character"),
            }
        }
        FormatError::EndsWithPercent(_) => "`%': missing format character".to_owned(),
        FormatError::InvalidPrecision(message) => message.clone(),
        other => other.to_string(),
    }
}

/// uucore's argument diagnostics (`printf: 'x': expected a numeric value`) as bash words them
/// (`bash: line 1: printf: x: invalid number`).
fn printf_diagnostic(bytes: &[u8], prefix: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        let rewritten = body.strip_prefix("printf: ").and_then(|rest| {
            let unquote = |value: &'_ str| -> String {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
                    .or_else(|| {
                        value
                            .strip_prefix('\u{2018}')
                            .and_then(|value| value.strip_suffix('\u{2019}'))
                    })
                    .unwrap_or(value)
                    .to_owned()
            };
            // A number too large: bash names it without quotes.
            if let Some(value) = rest.strip_suffix(": Result not representable") {
                return Some(format!(
                    "{prefix}printf: {}: Result not representable",
                    unquote(value)
                ));
            }
            let value = rest
                .strip_suffix(": value not completely converted")
                .or_else(|| rest.strip_suffix(": expected a numeric value"))?;
            Some(format!(
                "{prefix}printf: {}: invalid number",
                unquote(value)
            ))
        });
        match rewritten {
            Some(message) => {
                out.push_str(&message);
                out.push('\n');
            }
            None => out.push_str(line),
        }
    }
    out.into_bytes()
}

/// `%q` as bash quotes: backslashes before special characters, `$'…'` for control characters.
fn bash_quote(value: &str) -> String {
    use brush_core::escape::{QuoteMode, quote_if_needed};
    if value.is_empty() {
        return "''".to_owned();
    }
    let quoted = quote_if_needed(value, QuoteMode::BackslashEscape);
    if quoted.starts_with("$'") {
        return quoted.into_owned();
    }
    let mut quoted = quoted.replace(":~", ":\\~").replace("=~", "=\\~");
    if matches!(quoted.as_bytes().first(), Some(b'~' | b'#')) {
        quoted.insert(0, '\\');
    }
    quoted
}

#[cfg(target_arch = "wasm32")]
struct FollowFile {
    path: std::path::PathBuf,
    file: Option<Arc<std::fs::File>>,
    identity: Option<(u64, u64)>,
}

#[cfg(target_arch = "wasm32")]
async fn follow_files<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    settings: &uu_tail::args::Settings,
    mut files: Vec<FollowFile>,
) -> Result<(), Error> {
    use std::io::{Seek, SeekFrom};
    let services = context.shell.execution_services();
    let mut chunk = vec![0; 16 * 1024];
    let mut stdout = context.stdout();
    let mut last_header = files.len().saturating_sub(1);
    loop {
        (services.sleep)(settings.sleep_sec).await;
        for (index, followed) in files.iter_mut().enumerate() {
            let FollowFile {
                path,
                file,
                identity,
            } = followed;
            if file.is_none() || settings.follow == Some(uu_tail::args::FollowMode::Name) {
                match std::fs::File::open(&path) {
                    Ok(replacement) if Some(file_identity(&replacement)?) != *identity => {
                        *identity = Some(file_identity(&replacement)?);
                        *file = Some(Arc::new(replacement));
                        context
                            .stderr()
                            .async_io()
                            .write_all(
                                format!(
                                    "tail: '{}' has been replaced; following new file\n",
                                    path.display()
                                )
                                .as_bytes(),
                            )
                            .await?;
                    }
                    Err(_) if settings.retry => continue,
                    Err(error) => return Err(error.into()),
                    _ => {}
                }
            }
            let Some(file) = file else {
                continue;
            };
            if file.metadata()?.len() < (&**file).stream_position()? {
                (&**file).seek(SeekFrom::Start(0))?;
                context
                    .stderr()
                    .async_io()
                    .write_all(format!("tail: {}: file truncated\n", path.display()).as_bytes())
                    .await?;
            }
            // A WASI input-stream is closed permanently at EOF. Seek before polling the
            // same descriptor again so libc opens a fresh stream at the current offset.
            let offset = (&**file).stream_position()?;
            if file.metadata()?.len() <= offset {
                continue;
            }
            (&**file).seek(SeekFrom::End(0))?;
            (&**file).seek(SeekFrom::Start(offset))?;
            loop {
                let count = std::io::Read::read(&mut &**file, &mut chunk)?;
                if count == 0 {
                    break;
                }
                if settings.verbose && last_header != index {
                    stdout
                        .async_io()
                        .write_all(format!("\n==> {} <==\n", path.display()).as_bytes())
                        .await?;
                    last_header = index;
                }
                stdout.async_io().write_all(&chunk[..count]).await?;
                (services.yield_now)().await;
            }
        }
    }
}

/// WASI libc provides stable fstat even though Rust's WASI `MetadataExt` is unstable.
#[cfg(target_arch = "wasm32")]
#[allow(
    unsafe_code,
    reason = "fstat initializes its output on success; the owned File stays open"
)]
fn file_identity(file: &std::fs::File) -> io::Result<(u64, u64)> {
    use std::os::fd::AsRawFd;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: a live descriptor and a correctly sized writable stat allocation. Read only
    // after fstat reports success, when libc has initialized the entire structure.
    if unsafe { libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: success above initialized the stat structure.
    let stat = unsafe { stat.assume_init() };
    Ok((stat.st_dev, stat.st_ino))
}

/// A file's device and inode.
#[cfg(not(target_arch = "wasm32"))]
fn file_identity(file: &std::fs::File) -> io::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cat_formatting_keeps_state_across_chunk_boundaries() {
        let matches = uu_cat::uu_app()
            .try_get_matches_from(["cat", "-A", "-b", "-s"])
            .unwrap();
        let mut state = CatState::new(&matches);
        let mut output = Vec::new();
        for byte in b"\n\nhello\t\r\n\n\n\xc3\xb8\n" {
            state.push(&[*byte], &mut output);
        }
        assert_eq!(output, b"$\n     1\thello^I^M$\n$\n     2\tM-CM-8$\n");
    }
    #[test]
    fn obsolete_head_flags_respect_the_option_separator() {
        assert_eq!(
            normalize_head(&["head".into(), "-2kqz".into(), "--".into(), "-5".into()]),
            ["head", "-q", "-z", "-c", "2048", "--", "-5"]
        );
    }
    #[test]
    fn oversized_and_unterminated_records_are_not_split_or_lost() {
        futures::executor::block_on(async {
            let mut bytes = "ø".repeat(40_000).into_bytes();
            bytes.push(b'\n');
            bytes.extend_from_slice(b"final");
            let mut records = Records::new(brush_core::openfiles::from_bytes(bytes.clone()), b'\n');
            assert_eq!(records.next().await.unwrap().unwrap(), bytes[..80_001]);
            assert_eq!(records.next().await.unwrap(), Some(b"final".to_vec()));
            assert!(records.next().await.unwrap().is_none());
        });
    }
    #[test]
    fn counts_keep_utf8_words_and_display_columns_across_reads() {
        let mut counts = Counts::default();
        for byte in "héllø\t世界\nend".as_bytes() {
            counts.consume(&[*byte], false);
        }
        counts.consume(&[], true);
        assert_eq!(
            (
                counts.lines,
                counts.words,
                counts.chars,
                counts.bytes,
                counts.longest
            ),
            (1, 3, 12, 18, 12)
        );
    }
}
