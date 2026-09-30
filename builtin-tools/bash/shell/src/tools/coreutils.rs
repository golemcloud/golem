//! Embedded uutils with native descriptor capture and cooperative WASM adapters.
//! Synchronous descriptor and working-directory guards never cross an await.

#![allow(clippy::similar_names)] // argv/args/arg-style locals are inherent to arg parsing here

use std::io::Write;

use brush_core::builtins::{ContentOptions, ContentType, Registration, SimpleCommand};
use brush_core::commands::ExecutionContext;
use brush_core::extensions::ShellExtensions;
use brush_core::{Error, ExecutionResult};

/// Serializes the native fd-1/2 swap below. The `dup2` redirect targets the **process-global**
/// stdout/stderr, so two threads running uu_* builtins at once would clobber each other's capture.
/// the shell executes one line at a time in production, but parallel tests (many `Session`s on the
/// multi-thread runtime) do run uu_* concurrently — this guard makes that safe. Held only around
/// the swap+uumain+restore, never across an `.await`.
#[cfg(not(target_arch = "wasm32"))]
static FD_SWAP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Bookkeeping for [`ShellCwd`]: how many builtin calls are currently inside a cwd window, and where
/// to return when the last of them leaves.
///
/// Locked **only** for that bookkeeping — never across a builtin's run — so it cannot deadlock a
/// pipeline (see [`ShellCwd`]).
struct CwdState {
    /// Number of live [`ShellCwd`] guards.
    depth: usize,
    /// Where the first guard in found the process; restored when the last one leaves.
    restore: Option<std::path::PathBuf>,
}

static CWD_STATE: std::sync::Mutex<CwdState> = std::sync::Mutex::new(CwdState {
    depth: 0,
    restore: None,
});

/// Points the **process's** working directory at the **shell's** for the duration of in-process
/// builtin calls, restoring it when the last one finishes.
///
/// Brush keeps `cd` in its own `Shell::working_dir` and deliberately never calls `set_current_dir` —
/// the right call for a shell *library*, which hands the working directory to each spawned child
/// rather than mutating its host process. But the shell's builtins are never spawned (wasip2 has no
/// process spawn at all): they run *in this process* and resolve relative paths through
/// `std::env::current_dir()`. Without this bridge they ignore `cd` outright — `cd sub; ls` lists the
/// process's directory and `cd sub; cat f` reads the wrong `f`, while `pwd` and Brush's own redirects
/// (which consult `working_dir`) look perfectly correct. That split is the bug this closes.
///
/// **Restoring is load-bearing**, not tidiness: the cwd is process-global and `Shell::new` seeds a new
/// session's `working_dir` from `std::env::current_dir()`, so a leaked `cd` would silently become the
/// starting directory of the next `Session` built in the same process.
///
/// **Why it is refcounted rather than a plain save/restore.** Brush runs pipeline stages concurrently
/// on native, so `cd sub; a | b` has two guards live at once. A per-guard restore would have the
/// *first* stage to finish put the directory back while the *second* is still running — yanking the
/// cwd out from under it, so a path it opens late resolves against the wrong place. Instead the first
/// guard in records the return path and only the last one out restores it. (Serializing the guards
/// instead is not an option: `run_tool` cannot hold a lock across a stage's run, or a stage blocked
/// writing into a pipe would deadlock the stage draining it.)
///
/// Stages of one line share a `working_dir`, so a nested guard finds the process already where it
/// wants it and simply leaves it. What refcounting cannot fix is two *different* sessions with
/// *different* working dirs running builtins at the same time: there is one process directory and they
/// disagree about it, so whoever is between calls loses. That cannot arise in production — one agent,
/// one line at a time — and only parallel tests get near it. Closing it properly means teaching every
/// tool to resolve against `working_dir` explicitly, which is impossible for the `uu_*` crates: they
/// call `current_dir()` deep inside code we don't own.
pub(super) struct ShellCwd;

impl ShellCwd {
    pub(super) fn enter<SE: ShellExtensions>(context: &ExecutionContext<'_, SE>) -> Self {
        // Poisoning is harmless — the state is two plain fields — so recover the guard either way.
        let mut state = CWD_STATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let target = context.shell.working_dir();
        let current = std::env::current_dir().ok();

        // Already there is the common case (no `cd` yet, or a sibling stage of this line moved us to
        // the same place) — then there is nothing to move and nothing to put back.
        if current.as_deref() != Some(target) {
            // A directory the process cannot enter (the virtual `/dev`, or one deleted since the
            // `cd`) is still where relative paths resolve, as in bash: `cd /dev; rm -rf tmp`
            // removes nothing, and `touch f` in a deleted directory fails. Never `/` instead.
            let moved = std::env::set_current_dir(target).is_ok() || force_cwd(target);
            if moved && state.depth == 0 {
                // Only the first guard in records the return path. A later one would record a
                // directory a sibling stage had already moved us to, and restoring *that* at the end
                // would leave the process somewhere the shell never was.
                state.restore = current;
            }
        }

        state.depth += 1;
        Self
    }
}

impl Drop for ShellCwd {
    fn drop(&mut self) {
        let mut state = CWD_STATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.depth -= 1;
        // Last one out returns the process, so a still-running stage never has it moved underneath it.
        if state.depth == 0
            && let Some(dir) = state.restore.take()
            && std::env::set_current_dir(&dir).is_err()
        {
            force_cwd(&dir);
        }
    }
}

/// Makes `dir` the process's working directory even though it cannot be entered: WASI libc
/// resolves relative paths against its own record of the directory, which is set here without
/// the check `chdir` makes, so they resolve under `dir` and fail as they would there.
#[cfg(target_arch = "wasm32")]
#[allow(
    unsafe_code,
    reason = "sets WASI libc's working-directory record, as its chdir does"
)]
fn force_cwd(dir: &std::path::Path) -> bool {
    unsafe extern "C" {
        /// WASI libc's working directory, an absolute path it reads for every relative one.
        static mut __wasilibc_cwd: *mut std::ffi::c_char;
    }
    let bytes = dir.as_os_str().as_encoded_bytes();
    if !dir.is_absolute() || bytes.contains(&0) {
        return false;
    }
    // A successful `chdir` first leaves libc owning its record, allocated with `malloc` and
    // freed by its next `chdir`; the record put in its place is allocated the same way.
    if std::env::set_current_dir("/").is_err() {
        return false;
    }
    // SAFETY: `malloc` returns null or `bytes.len() + 1` writable bytes, all written below
    // before libc reads them. The old record is libc's own `malloc`ed copy (the `chdir` above),
    // which it would free on its next `chdir`, and nothing else points at it. The WASM component
    // is single-threaded, so nothing reads the record while it changes.
    unsafe {
        let record = libc::malloc(bytes.len() + 1).cast::<u8>();
        if record.is_null() {
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), record, bytes.len());
        record.add(bytes.len()).write(0);
        let old = __wasilibc_cwd;
        __wasilibc_cwd = record.cast();
        libc::free(old.cast());
    }
    true
}

#[cfg(not(target_arch = "wasm32"))]
fn force_cwd(_dir: &std::path::Path) -> bool {
    false
}

/// Stage pipeline or redirected input before taking the process descriptor lock.
/// Native uutils read the process-wide stdin, so staging must finish before another
/// pipeline stage needs that lock. WASM uses cooperative drivers instead.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::items_after_statements)] // the SEQ counter lives beside its only use
fn stage_piped_stdin<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
) -> Option<std::fs::File> {
    use brush_core::openfiles::{OpenFile, OpenFiles};
    use std::io::Seek;

    use std::os::fd::AsRawFd;
    let mut source = context.try_fd(OpenFiles::STDIN_FD)?;
    // Stage a pipeline stage's `PipeReader` (always safe), OR a genuine `< file` / here-doc redirect
    // — an `OpenFile::File` the redirect opened on a FRESH fd. The shell's own inherited stdin must
    // NEVER be staged: it arrives as `OpenFile::Stdin` when the shell is pipe-fed, or as an
    // `OpenFile::File` on **fd 0** when the shell is `< script`-fed; draining either swallows the rest of
    // the session. A redirect-opened file is the only `File` whose fd is not 0, which is exactly what
    // separates it from the inherited stdin.
    let stageable = match &source {
        OpenFile::PipeReader(_) => true,
        OpenFile::File(_) => source
            .try_borrow_as_fd()
            .is_ok_and(|fd| fd.as_raw_fd() != 0),
        _ => false,
    };
    if !stageable {
        return None;
    }

    // Unique per process AND per call: two pipeline stages stage their stdin concurrently.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(".bash-uu-stdin-{}-{seq}", std::process::id()));

    let mut staged = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .ok()?;
    let _ = std::fs::remove_file(&path);
    // Stream straight to the file — never `read_to_end` into a `Vec` first. Materializing the whole
    // upstream in memory made `cat /dev/zero | head -1` climb past 16 GB RSS before the watchdog
    // killed it; copying in fixed-size chunks keeps this flat, which is what the temp file is for.
    std::io::copy(&mut source, &mut staged).ok()?;
    staged.seek(std::io::SeekFrom::Start(0)).ok()?;
    Some(staged)
}

/// Run a uutils `uumain` closure with the process's stdin/stdout/stderr pointed at the `OpenFile`s
/// brush assigned for this command, so its input and output land wherever brush wants them.
///
/// `util` names the utility for uucore (`set_embedded_util`): in-process, argv[0] is the host
/// program, and the translations a utility's own `main` loads never are.
#[cfg(not(target_arch = "wasm32"))]
#[allow(unsafe_code, clippy::similar_names)] // libc dup/dup2/signal/close FFI over raw fds (see per-call SAFETY); saved_in/out/err intentional
pub(crate) fn run_uu<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    util: &'static str,
    uumain: impl FnOnce() -> i32,
) -> i32 {
    use brush_core::openfiles::OpenFiles;
    use std::io::Write;
    use std::os::fd::AsRawFd;

    // Drain a pipeline stage's piped stdin BEFORE taking the lock — the ordering keeps concurrent
    // stages from deadlocking. See `stage_piped_stdin`.
    let staged_stdin = stage_piped_stdin(context);

    // Serialize the process-global fd swap (see `FD_SWAP_LOCK`). Poisoning is harmless here — the
    // guarded region restores fds even on panic paths — so recover the guard either way.
    let _fd_guard = FD_SWAP_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // Relative operands (`cat f`, `ls`, `touch f`) resolve against the shell's `cd`, not the process's
    // directory. See `ShellCwd`.
    let _cwd = ShellCwd::enter(context);

    // A broken pipe (e.g. `cat | head`) must not kill the embedding process; make writes to
    // a closed pipe return EPIPE instead of raising SIGPIPE.
    // SAFETY: `libc::signal` is an FFI call with no memory-safety precondition — `SIGPIPE`/`SIG_IGN`
    // are valid constants and it returns the previous handler (or `SIG_ERR`), never exhibiting UB.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };

    // Flush anything already buffered before swapping the underlying fds.
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    // Bind the assigned output streams and staged stdin only for the synchronous call.
    let redirect = |shell_fd, real_fd| -> i32 {
        // SAFETY: `libc::dup`/`dup2` are FFI calls over raw fds with no memory-safety precondition;
        // they return -1 on error (handled by the `saved >= 0` guard in the restore loop) rather than
        // exhibiting UB. `real_fd` is a standard fd (0/1/2); `fd` is borrowed from a live `OpenFile`.
        let saved = unsafe { libc::dup(real_fd) };
        if let Some(target) = context.try_fd(shell_fd)
            && let Ok(fd) = target.try_borrow_as_fd()
        {
            // SAFETY: as above — `dup2` over valid raw fds, -1 on error, never UB.
            unsafe { libc::dup2(fd.as_raw_fd(), real_fd) };
        }
        saved
    };
    let saved_in = match &staged_stdin {
        Some(staged) => {
            // SAFETY: `dup`/`dup2` over valid raw fds (fd 0 and the staged file's live fd); they
            // return -1 on error and never exhibit UB.
            let saved = unsafe { libc::dup(0) };
            unsafe { libc::dup2(staged.as_raw_fd(), 0) };
            saved
        }
        None => -1,
    };
    let saved_out = redirect(OpenFiles::STDOUT_FD, 1);
    let saved_err = redirect(OpenFiles::STDERR_FD, 2);

    // uucore's exit code is a process-global (`AtomicI32`) that upstream resets by process
    // exit — which never happens in-process here. Without this, one failed command poisons
    // every later success's exit code (ls-of-missing made a subsequent touch report 2).
    uucore::error::set_exit_code(0);
    uucore::set_embedded_util(util);
    let code = uumain();

    // Restore SIGPIPE to SIG_IGN. A uu tool may flip it to SIG_DFL for GNU broken-pipe semantics and
    // not put it back; in the shell's single-process model that leftover would let a LATER in-process
    // pipeline (or a later test in the same binary) be killed by a broken pipe instead of getting
    // EPIPE. Re-asserting here keeps SIGPIPE ignored process-wide across successive uu calls.
    // SAFETY: as at the pre-call set on line 231 — a plain `libc::signal` FFI over valid constants,
    // returning the previous handler (or `SIG_ERR`), never exhibiting UB.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };

    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    for (saved, real_fd) in [(saved_in, 0), (saved_out, 1), (saved_err, 2)] {
        if saved >= 0 {
            // SAFETY: `saved` is a live fd from a successful `dup` above (the `saved >= 0` guard);
            // `dup2` restores it onto the standard fd and `close` releases it. FFI, -1 on error, no UB.
            unsafe {
                libc::dup2(saved, real_fd);
                libc::close(saved);
            }
        }
    }
    code
}

/// Install the panic hook that keeps a crashing embedded utility's own panic message from being
/// lost inside a served call's `OpenFile` (see `devices::write_to_real_stderr`). Call once,
/// before any utility runs.
///
/// `wasm32-wasip2`'s panic strategy is `abort`: a panic here never unwinds back to a caller that
/// could restore process state, so the default hook's own write -- which, while a utility's
/// descriptors are served, lands in that utility's own (unread, about-to-be-discarded) `OpenFile`
/// rather than the real stream -- is lost the instant the trap tears the whole component down. A
/// panic hook still runs under `panic = "abort"` (only unwinding/catching don't), so writing
/// straight to the real, unwrapped fd 2 from inside it survives even though nothing after it ever
/// runs.
#[cfg(target_arch = "wasm32")]
pub(crate) fn install_panic_hook() {
    // Each call (`Session::build`) would otherwise stack another wrapper on top of the last over
    // a long-lived instance's many invocations; install it only the first time.
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            previous(info);
            // Outside a served call, the default hook's own write already reaches the real fd 2
            // (nothing intercepts it), so writing again here would just duplicate the message.
            if super::devices::serving() {
                super::devices::write_to_real_stderr(format!("{info}\n").as_bytes());
            }
        }));
    });
}

/// Run a uutils `uumain` closure on the streams Brush assigned for this command: its descriptors
/// 0–2 are served from them for the synchronous call (see `devices::serve`), so no file is
/// created and no descriptor moved. Commands run through the finite drivers in `streaming`, which
/// serve the command's streams already; a call outside them serves its own.
#[cfg(target_arch = "wasm32")]
pub(crate) fn run_uu<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    util: &'static str,
    uumain: impl FnOnce() -> i32,
) -> i32 {
    use brush_core::openfiles::OpenFiles;
    let _cwd = ShellCwd::enter(context);
    // The utility reads `TZ`, `TMPDIR` and the like from the process environment.
    let _env = ProcessEnv::enter(exported_env(context));
    // Nothing written before the call may land in the command's streams.
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    let _served = (!super::devices::serving()).then(|| {
        let files = [
            (OpenFiles::STDIN_FD, effective_stdin_file(context)),
            (OpenFiles::STDOUT_FD, context.stdout()),
            (OpenFiles::STDERR_FD, context.stderr()),
        ];
        super::devices::serve(files.into_iter().collect(), false)
    });
    uucore::error::set_exit_code(0);
    uucore::set_embedded_util(util);
    let code = uumain();
    // Whatever the utility left buffered belongs to its streams. If its stdout refused it (the
    // output limit), it is dropped: Rust's stdout buffer would otherwise carry it into the next
    // command's output.
    if std::io::stdout().flush().is_err() {
        super::devices::discard_served(1);
        let _ = std::io::stdout().flush();
    }
    let _ = std::io::stderr().flush();
    code
}

/// Give code we don't own, that reads the process's real stdin directly instead of going
/// through `run_uu` (the sed fork's `-f -`, which reads `io::stdin()` while *compiling*
/// the script — before there is an `Engine` to hand records to, so it can't go through the
/// record-at-a-time driver the way the main input does), a chance to see the shell's
/// actual piped or redirected input for the duration of `f`.
///
/// Serves `input`'s bytes on descriptor 0 exactly as `run_uu` serves a command's own streams
/// (see `devices::serve`), with a discard sink standing in for stdout/stderr — `f` here only
/// compiles a script, so there is nothing to capture from them. Takes the bytes already read,
/// rather than reading them itself: the live pipe/redirect behind stdin needs an async read to
/// fill (see [`crate::tools::streaming::input`]), and this must stay callable from `f`'s
/// synchronous caller. Unlike the old real-fd-swap this replaced, `devices::serve` is a pure
/// in-memory redirect table with nothing that can fail, so this is infallible too.
#[cfg(target_arch = "wasm32")]
pub(crate) fn with_shell_stdin_staged<R>(input: &[u8], f: impl FnOnce() -> R) -> R {
    use brush_core::openfiles::OpenFiles;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    let files = [
        (
            OpenFiles::STDIN_FD,
            brush_core::openfiles::from_bytes(input.to_vec()),
        ),
        (OpenFiles::STDOUT_FD, brush_core::openfiles::null_sink()),
        (OpenFiles::STDERR_FD, brush_core::openfiles::null_sink()),
    ];
    let _served = super::devices::serve(files.into_iter().collect(), false);
    f()
}

/// Gives the process the shell's exported variables as its environment for the duration of an
/// in-process utility call, then puts back what it had: a utility reads `TZ`, `TMPDIR` or
/// `SIMPLE_BACKUP_SUFFIX` from the process environment, which a script's `export` never reaches.
#[cfg(target_arch = "wasm32")]
pub(crate) struct ProcessEnv {
    saved: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

#[cfg(target_arch = "wasm32")]
#[allow(
    unsafe_code,
    reason = "the WASM component is single-threaded, so nothing reads the environment while it changes"
)]
impl ProcessEnv {
    pub(crate) fn enter(exported: Vec<(String, String)>) -> Self {
        let saved: Vec<_> = std::env::vars_os().collect();
        Self::replace(
            saved.iter().map(|(name, _)| name.clone()),
            exported.into_iter().map(|(name, value)| {
                (
                    super::shell_bytes::to_os_string(&name),
                    super::shell_bytes::to_os_string(&value),
                )
            }),
        );
        Self { saved }
    }

    fn replace(
        old: impl Iterator<Item = std::ffi::OsString>,
        new: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    ) {
        for name in old {
            // SAFETY: single-threaded; see the impl's reason.
            unsafe { std::env::remove_var(name) };
        }
        for (name, value) in new {
            if !name.is_empty() && !name.to_string_lossy().contains('=') {
                // SAFETY: single-threaded; see the impl's reason.
                unsafe { std::env::set_var(name, value) };
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for ProcessEnv {
    fn drop(&mut self) {
        let current: Vec<_> = std::env::vars_os().map(|(name, _)| name).collect();
        Self::replace(
            current.into_iter(),
            std::mem::take(&mut self.saved).into_iter(),
        );
    }
}

/// Render a uutils `error` as the utility's own `uumain` would: `util: message`, then the usage
/// hint when the error calls for one.
///
/// uutils translates when an error is displayed, and pipeline stages interleave on one thread, so
/// this names `util` for uucore immediately before formatting rather than once per command.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn uu_error_text(util: &'static str, error: &dyn uucore::error::UError) -> String {
    uucore::set_embedded_util(util);
    let message = error.to_string();
    let mut text = String::new();
    if !message.is_empty() {
        text = format!("{util}: {message}\n");
    }
    if error.usage() {
        text.push_str(&format!("Try '{util} --help' for more information.\n"));
    }
    text
}

/// The `clap::Command` a `finite`-driven tool's own binary would build, for [`gnu_clap_error`] to
/// look up a short flag from -- the one piece GNU's own wording needs that a bare `clap::Error`
/// never carries (`Arg`'s own rendering always prefers a long name when one exists, so an error
/// naming `--lines` has no way back to `-n`; see `uucore::error::ClapErrorWrapper::error`). Also
/// carries the exit status GNU gives a usage error for this tool: 1 for most, but 2 for a tool
/// that asks its own `handle_clap_result_with_exit_code`/`ClapErrorWrapper::with_exit_code` for a
/// different one, since a `clap::Error` has no memory of a caller-chosen exit code (clap's own
/// default is `2` for any usage error, so this can't just read it off the error).
///
/// Most tools that once needed this are covered by `uucore::clap_localization`'s own
/// `print_gnu_invalid_value` now (it has the `Command` already, from inside the tool's own
/// `handle_clap_result_with_exit_code` call, and gets the exit code right there directly) --
/// `tail` doesn't route through that module at all, and `shuf`'s `ArgumentConflict` (`-e`/`-i`
/// together) isn't a kind either translator covers, so both stay listed here. `None` for any
/// other tool: [`super::streaming::finite`] then runs it exactly as before, relying on
/// `ClapErrorWrapper`'s own (Command-independent) translation instead.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn uu_command(util: &str) -> Option<(clap::Command, u8)> {
    Some(match util {
        "shuf" => (uu_shuf::uu_app(), 1),
        "tail" => (uu_tail::uu_app(), 1),
        _ => return None,
    })
}

/// A GNU-worded message for a `clap::Error` from one of the [`uu_command`] tools, checked
/// *before* that tool's own call runs (see `finite` in `streaming.rs`). `None` when this doesn't recognize the error, or
/// can't resolve a name it needs back to a short flag: the caller's normal path then runs
/// unchanged, which for most of these tools still means `ClapErrorWrapper`'s own generic
/// translation once their real call reaches one (a plain `?`, not `clap_localization`).
/// `spelled` is `Arg`'s own rendering: "--flag <PLACEHOLDER>" for a value-taking arg (or
/// "--flag[=<PLACEHOLDER>]" for one whose value is optional -- no space before the bracket
/// there), "-f <PLACEHOLDER>" when the arg has no long name, or a bare flag/subcommand name with
/// nothing after it. The name itself is whatever comes before the first space, `=` or `[`,
/// whichever comes first.
#[cfg(any(target_arch = "wasm32", test))]
fn flag_name(spelled: &str) -> &str {
    spelled
        .find([' ', '=', '['])
        .map_or(spelled, |end| &spelled[..end])
}

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn gnu_clap_error(
    util: &str,
    command: &clap::Command,
    error: &clap::Error,
    status: u8,
) -> Option<(String, u8)> {
    use clap::error::{ContextKind, ContextValue};

    let short_flag = |spelled: &str| -> Option<char> {
        let name = flag_name(spelled);
        command
            .get_arguments()
            .find(|arg| {
                name.strip_prefix("--")
                    .is_some_and(|long| arg.get_long() == Some(long))
                    || arg
                        .get_short()
                        .is_some_and(|short| name == format!("-{short}"))
            })
            .and_then(clap::Arg::get_short)
    };
    let usage =
        |message: String| format!("{util}: {message}\nTry '{util} --help' for more information.\n");

    match error.kind() {
        clap::error::ErrorKind::UnknownArgument => {
            let ContextValue::String(invalid_arg) = error.get(ContextKind::InvalidArg)? else {
                return None;
            };
            Some((
                usage(if invalid_arg.starts_with("--") {
                    format!("unrecognized option '{invalid_arg}'")
                } else if let Some(short) = invalid_arg.strip_prefix('-') {
                    format!("invalid option -- '{short}'")
                } else {
                    format!("extra operand '{invalid_arg}'")
                }),
                status,
            ))
        }
        clap::error::ErrorKind::TooManyValues => {
            let ContextValue::String(invalid_value) = error.get(ContextKind::InvalidValue)? else {
                return None;
            };
            Some((usage(format!("extra operand '{invalid_value}'")), status))
        }
        clap::error::ErrorKind::InvalidValue => {
            let ContextValue::String(invalid_arg) = error.get(ContextKind::InvalidArg)? else {
                return None;
            };
            let ContextValue::String(invalid_value) = error.get(ContextKind::InvalidValue)? else {
                return None;
            };
            if invalid_value.is_empty() {
                let short = short_flag(invalid_arg)?;
                return Some((
                    usage(format!("option requires an argument -- '{short}'")),
                    status,
                ));
            }
            let flag = flag_name(invalid_arg);
            let mut message = format!("invalid argument '{invalid_value}' for '{flag}'");
            if let Some(ContextValue::Strings(valid)) = error.get(ContextKind::ValidValue) {
                // Declaration order, not sorted: GNU's own order here is each tool's own (e.g.
                // `sort --sort`'s is alphabetical, `uniq --group`'s is not), and this crate's
                // `Arg::value_parser` choices are declared in the same order as the upstream
                // GNU enum they port, so what clap's context already gives back is GNU's own
                // order too -- confirmed against the oracle for both of those.
                message.push_str("\nValid arguments are:");
                for value in valid {
                    message.push_str(&format!("\n  - '{value}'"));
                }
            }
            // GNU gives a bad *value* for an otherwise-recognized option exit code 1
            // regardless of what the tool asked `clap_localization` for on any other usage
            // error (confirmed against the oracle: `sort -k missing argument` wants `2`, but
            // `sort --sort bogus` wants `1`) -- matching a comment on the very code path this
            // sidesteps (`handle_invalid_value` in `clap_localization.rs`): "InvalidValue
            // errors traditionally use exit code 1 for backward compatibility".
            Some((usage(message), 1))
        }
        clap::error::ErrorKind::ArgumentConflict => {
            let ContextValue::String(invalid_arg) = error.get(ContextKind::InvalidArg)? else {
                return None;
            };
            let Some(ContextValue::String(prior_arg)) = error.get(ContextKind::PriorArg) else {
                return None;
            };
            let a = short_flag(invalid_arg)?;
            let b = short_flag(prior_arg)?;
            Some((
                usage(format!("cannot combine -{a} and -{b} options")),
                status,
            ))
        }
        _ => None,
    }
}

/// This command's *effective* stdin on the wasm agent: the source Brush assigned when it is a pipe
/// stage (`OpenFile::Stream`, the in-memory pipe reader) or a redirect (`OpenFile::File`) — and
/// **empty input** when it is the default `OpenFile::Stdin`. The real wasip2 stdin resource must
/// never be read: a durable agent has no interactive stdin and `input-stream.blocking-read` on it
/// TRAPS the whole component (wedging the agent instance).
#[cfg(target_arch = "wasm32")]
fn effective_stdin<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
) -> Box<dyn std::io::Read> {
    Box::new(effective_stdin_file(context))
}

/// [`effective_stdin`] as the open file itself.
#[cfg(target_arch = "wasm32")]
fn effective_stdin_file<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
) -> brush_core::openfiles::OpenFile {
    use brush_core::openfiles::{OpenFile, OpenFiles};
    match context.try_fd(OpenFiles::STDIN_FD) {
        Some(f @ (OpenFile::File(_) | OpenFile::PipeReader(_) | OpenFile::Stream(_))) => f,
        _ => brush_core::openfiles::from_bytes(Vec::new()),
    }
}

/// Run a tool closure over Brush's assigned streams — used by the text/data builtins
/// (grep/jq/sed/…) whose Rust-library implementations we control. Unlike [`run_uu`], this does NOT
/// swap process fds: it hands the closure `context.stdin()`, `context.stdout()`, and
/// `context.stderr()`, which are Brush's `OpenFile`s. On wasm those are the in-memory capture/pipe
/// streams, so output is captured and piped input (`cmd | grep …`) reaches the tool — writing to the
/// process-global `io::stdout()` / reading process-global `io::stdin()` do neither on wasm. The
/// `stdin` reader lets a tool consume an upstream pipeline stage's output when given no file operands.
/// No `/tmp` capture file, no fd games; on wasm the stdin handed over is [`effective_stdin`] (never
/// the trapping real stdin resource).
pub(crate) fn run_tool<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    run: impl FnOnce(&mut dyn std::io::Read, &mut dyn std::io::Write, &mut dyn std::io::Write) -> i32,
) -> i32 {
    // File operands (`grep pat f`, `find .`, `stat f`) resolve against the shell's `cd`. Note this
    // takes no lock across the run — brush runs pipeline stages concurrently on native, so blocking a
    // stage another is draining through a pipe would deadlock; `ShellCwd` is refcounted instead.
    let _cwd = ShellCwd::enter(context);
    let mut stdin = tool_stdin(context);
    let mut out = context.stdout();
    let mut err = context.stderr();
    let code = run(&mut stdin, &mut out, &mut err);
    let _ = out.flush();
    let _ = err.flush();
    code
}

/// This command's stdin as a reader — Brush's assigned `OpenFile` on native, and on wasm the
/// [`effective_stdin`] guard (piped/redirected input, or empty for the default stdin: the real
/// wasip2 stdin resource must never be read — its blocking read traps the agent). For builtins
/// that read stdin outside the [`run_tool`] closure shape (e.g. `xargs`).
pub(crate) fn tool_stdin<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
) -> Box<dyn std::io::Read> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Box::new(context.stdin())
    }
    #[cfg(target_arch = "wasm32")]
    {
        effective_stdin(context)
    }
}

/// Shared `get_content` body: derive real help content from a synopsis rather than a stub. Used by
/// every embedded coreutils builtin.
#[allow(clippy::needless_pass_by_value)] // ContentType is a fieldless brush enum matched here; by-ref would ripple to every caller
fn uu_get_content(name: &str, synopsis: &str, content_type: ContentType) -> Result<String, Error> {
    match content_type {
        ContentType::ShortDescription => Ok(format!("{name} - {synopsis}\n")),
        ContentType::ShortUsage => Ok(format!("{name}: {name} [args...]\n")),
        ContentType::DetailedHelp => Ok(format!(
            "{name} - {synopsis}\n\n(uutils coreutils builtin)\n"
        )),
        ContentType::ManPage => brush_core::error::unimp("man page not yet implemented"),
    }
}

/// Define a brush `SimpleCommand` that dispatches to a uutils `uumain`, prepending `argv[0]`.
///
/// The `$synopsis` is the command's one-line description: it feeds both `get_content` (so Brush's
/// `help`/`type` surface real content instead of a stub) and the command [`Manifest`] built in
/// [`manifests`] — defined once, so the two can't drift.
macro_rules! uu_builtin {
    ($ty:ident, $name:literal, $synopsis:literal, $uumain:path) => {
        pub(crate) struct $ty;

        impl $ty {
            const NAME: &'static str = $name;
            const SYNOPSIS: &'static str = $synopsis;
        }

        impl SimpleCommand for $ty {
            fn get_content(
                name: &str,
                content_type: ContentType,
                _options: &ContentOptions,
            ) -> Result<String, Error> {
                uu_get_content(name, $ty::SYNOPSIS, content_type)
            }

            #[allow(clippy::cast_sign_loss)] // code is clamped to 0..=255 before the u8 cast
            fn execute<SE, I, S>(
                context: ExecutionContext<'_, SE>,
                args: I,
            ) -> Result<ExecutionResult, Error>
            where
                SE: ShellExtensions,
                I: Iterator<Item = S>,
                S: AsRef<str>,
            {
                // brush already passes the command name as args[0], which is what uutils'
                // `uumain` expects for argv[0].
                // Each argument as the bytes it stands for, including any that are not UTF-8.
                let argv = args.map(|s| super::shell_bytes::to_os_string(s.as_ref()));
                let code = run_uu(&context, $name, move || $uumain(argv));
                Ok(ExecutionResult::new(code.clamp(0, 255) as u8))
            }
        }
    };
}

uu_builtin!(Wc, "wc", "count lines, words, and bytes", uu_wc::uumain);
uu_builtin!(
    Head,
    "head",
    "print the first lines of a file",
    uu_head::uumain
);
uu_builtin!(Sort, "sort", "sort lines of text", uu_sort::uumain);
uu_builtin!(Rm, "rm", "remove files and directories", uu_rm::uumain);
uu_builtin!(Mv, "mv", "move or rename files", uu_mv::uumain);
uu_builtin!(Cp, "cp", "copy files and directories", uu_cp::uumain);
// mkdir uses the same convention as the others: brush passes the command name as argv[0], which is
// what uumain expects — do NOT skip it, or flags like `-p` get dropped (dropping `-p` turned
// `mkdir -p /tmp/a/b` into a non-recursive mkdir that fails when an intermediate dir is missing).
uu_builtin!(Mkdir, "mkdir", "create directories", uu_mkdir::uumain);
uu_builtin!(Cut, "cut", "select fields from each line", uu_cut::uumain);
uu_builtin!(Tr, "tr", "translate or delete characters", uu_tr::uumain);
uu_builtin!(
    Uniq,
    "uniq",
    "report or omit repeated lines",
    uu_uniq::uumain
);
uu_builtin!(
    Tail,
    "tail",
    "print the last lines of a file",
    uu_tail::uumain
);
uu_builtin!(Tee, "tee", "copy stdin to stdout and files", uu_tee::uumain);
uu_builtin!(
    Touch,
    "touch",
    "create files or update timestamps",
    uu_touch::uumain
);
uu_builtin!(Sleep, "sleep", "pause for a duration", uu_sleep::uumain);
// printf shadows Brush's builtin (registered after `default_builtins`; last write wins). Brush's
// printf is gated to `any(unix, windows)` upstream, so without this the wasm agent has no printf
// at all and the word falls through to (unsupported) external exec. Trade-off: bash's `printf -v
// VAR` (assign to a shell variable) is not supported by uu_printf on either target.
uu_builtin!(Printf, "printf", "format and print data", uu_printf::uumain);

uu_builtin!(
    Cat,
    "cat",
    "concatenate files and print to stdout",
    uu_cat::uumain
);
uu_builtin!(Ls, "ls", "list directory contents", uu_ls::uumain);

// Further coreutils (all finite — captured synchronously like the rest of this macro's
// output, then forwarded via `streaming::finite_utility_builtin` on wasm; see `builtins()` below).
uu_builtin!(
    Basename,
    "basename",
    "strip directory and suffix from a path",
    uu_basename::uumain
);
uu_builtin!(
    Dirname,
    "dirname",
    "strip the last path component",
    uu_dirname::uumain
);
uu_builtin!(
    Realpath,
    "realpath",
    "resolve a path to its absolute, canonical form",
    uu_realpath::uumain
);
uu_builtin!(
    Readlink,
    "readlink",
    "print the target of a symbolic link",
    uu_readlink::uumain
);
uu_builtin!(Ln, "ln", "create links between files", ln_main);

/// `ln`, refusing symbolic links to absolute paths up front: WASI (and so an agent's filesystem)
/// cannot create them, and the failure would otherwise read only "Permission denied".
fn ln_main(args: impl uucore::Args) -> i32 {
    let args: Vec<std::ffi::OsString> = args.collect();
    if let Some(target) = absolute_symlink_target(&args) {
        eprintln!(
            "ln: failed to create symbolic link to '{target}': links to absolute paths are \
             unsupported in bash-tool; use a relative target"
        );
        return 1;
    }
    uu_ln::uumain(args.into_iter())
}

/// The first absolute target of a symbolic `ln`, if any. With `-r` the link holds a relative
/// path to it, which WASI can create.
fn absolute_symlink_target(args: &[std::ffi::OsString]) -> Option<String> {
    let (mut symbolic, mut into_directory, mut options) = (false, false, true);
    let mut relative = false;
    let mut operands = Vec::new();
    let mut args = args.iter().skip(1).map(|arg| arg.to_string_lossy());
    while let Some(arg) = args.next() {
        if options && arg == "--" {
            options = false;
        } else if options && arg.starts_with("--") {
            symbolic |= arg == "--symbolic";
            relative |= arg == "--relative";
            if arg.starts_with("--target-directory") {
                into_directory = true;
                if !arg.contains('=') {
                    args.next();
                }
            } else if arg == "--suffix" {
                args.next();
            }
        } else if options && arg.len() > 1 && arg.starts_with('-') {
            symbolic |= arg.contains('s');
            relative |= arg.contains('r');
            into_directory |= arg.contains('t');
            // `-t DIR` and `-S SUFFIX` take the next argument when they end the cluster.
            if arg.ends_with('t') || arg.ends_with('S') {
                args.next();
            }
        } else {
            operands.push(arg.into_owned());
        }
    }
    let targets = match operands.len() {
        _ if into_directory => &operands[..],
        0 | 1 => &operands[..],
        count => &operands[..count - 1],
    };
    targets
        .iter()
        .find(|target| symbolic && !relative && target.starts_with('/'))
        .cloned()
}
uu_builtin!(
    Link,
    "link",
    "create a hard link to a file",
    uu_link::uumain
);
uu_builtin!(
    Unlink,
    "unlink",
    "remove a single file via the unlink syscall",
    uu_unlink::uumain
);
uu_builtin!(Rmdir, "rmdir", "remove empty directories", uu_rmdir::uumain);
uu_builtin!(
    Mktemp,
    "mktemp",
    "create a temporary file or directory",
    uu_mktemp::uumain
);
uu_builtin!(
    Truncate,
    "truncate",
    "shrink or extend a file to a given size",
    uu_truncate::uumain
);
uu_builtin!(Nl, "nl", "number lines of a file", uu_nl::uumain);
uu_builtin!(Paste, "paste", "merge lines of files", uu_paste::uumain);
uu_builtin!(
    Join,
    "join",
    "join lines of two sorted files on a common field",
    uu_join::uumain
);
uu_builtin!(
    Comm,
    "comm",
    "compare two sorted files line by line",
    uu_comm::uumain
);
uu_builtin!(
    Fold,
    "fold",
    "wrap each line to fit a given width",
    uu_fold::uumain
);
uu_builtin!(
    Fmt,
    "fmt",
    "simple text formatter/paragraph filler",
    uu_fmt::uumain
);
uu_builtin!(
    Expand,
    "expand",
    "convert tabs to spaces",
    uu_expand::uumain
);
uu_builtin!(
    Unexpand,
    "unexpand",
    "convert spaces to tabs",
    uu_unexpand::uumain
);
uu_builtin!(
    Tsort,
    "tsort",
    "topological sort of a partial ordering",
    uu_tsort::uumain
);
uu_builtin!(Split, "split", "split a file into pieces", uu_split::uumain);
uu_builtin!(
    Csplit,
    "csplit",
    "split a file into sections by context",
    uu_csplit::uumain
);
uu_builtin!(
    Base64,
    "base64",
    "base64 encode/decode data",
    uu_base64::uumain
);
uu_builtin!(
    Base32,
    "base32",
    "base32 encode/decode data",
    uu_base32::uumain
);
uu_builtin!(
    Basenc,
    "basenc",
    "encode/decode data with a chosen base",
    uu_basenc::uumain
);
uu_builtin!(
    Md5sum,
    "md5sum",
    "compute or check MD5 checksums",
    uu_md5sum::uumain
);
uu_builtin!(
    Sha1sum,
    "sha1sum",
    "compute or check SHA1 checksums",
    uu_sha1sum::uumain
);
uu_builtin!(
    Sha256sum,
    "sha256sum",
    "compute or check SHA256 checksums",
    uu_sha256sum::uumain
);
uu_builtin!(
    Sha512sum,
    "sha512sum",
    "compute or check SHA512 checksums",
    uu_sha512sum::uumain
);
uu_builtin!(
    B2sum,
    "b2sum",
    "compute or check BLAKE2 checksums",
    uu_b2sum::uumain
);
uu_builtin!(
    Cksum,
    "cksum",
    "checksum and count the bytes of a file",
    uu_cksum::uumain
);
uu_builtin!(
    Od,
    "od",
    "dump files in octal and other formats",
    uu_od::uumain
);
uu_builtin!(
    Date,
    "date",
    "print or format the current date and time",
    uu_date::uumain
);
uu_builtin!(Expr, "expr", "evaluate an expression", uu_expr::uumain);
uu_builtin!(
    Factor,
    "factor",
    "print the prime factors of numbers",
    uu_factor::uumain
);
uu_builtin!(
    Numfmt,
    "numfmt",
    "reformat numbers (e.g. --to=iec)",
    uu_numfmt::uumain
);
uu_builtin!(
    Shuf,
    "shuf",
    "generate random permutations of input lines",
    uu_shuf::uumain
);
uu_builtin!(
    Nproc,
    "nproc",
    "print the number of processing units available",
    uu_nproc::uumain
);
uu_builtin!(Uname, "uname", "print system information", uu_uname::uumain);
/// `hostname`'s own shape of `uumain`. `uu_hostname` gates its whole implementation to
/// `cfg(unix, windows)` (it calls into the platform `hostname` crate, which has no WASI
/// support), so this is a small hand-rolled subset instead of the real uutils command:
/// the bare case (print it) and `-s`/`--short` (the same, since this fixed name has no
/// domain part to strip). Every other option, and setting a new hostname, is refused, rather
/// than silently ignored or answered with a made-up value.
fn hostname_uumain(args: impl uucore::Args) -> i32 {
    // GOLEM's own answer to "what is this agent" is its worker name, which is a much more
    // useful `hostname` for a script than a constant would be, and matches the spirit of the
    // OSTYPE/UID identity choices already made for this sandbox.
    let name = std::env::var("GOLEM_WORKER_NAME").unwrap_or_else(|_| "localhost".to_owned());
    let mut argv = args.skip(1);
    match argv.next() {
        None => println!("{name}"),
        Some(arg) if arg == "-s" || arg == "--short" => {
            println!("{}", name.split('.').next().unwrap_or(&name));
        }
        Some(arg) => {
            eprintln!(
                "hostname: {}: unsupported in bash-tool",
                arg.to_string_lossy()
            );
            return 2;
        }
    }
    0
}
uu_builtin!(
    Hostname,
    "hostname",
    "print or set the system's host name",
    hostname_uumain
);
uu_builtin!(Dd, "dd", "convert and copy a file", uu_dd::uumain);
uu_builtin!(Du, "du", "estimate file space usage", uu_du::uumain);
/// `chmod`'s own shape of `uumain`, refusing unconditionally: WASI has no permission bits, so
/// there is nothing a real `chmod` could change here (`ls`/`stat` already report this sandbox's
/// one fixed mode by file type -- see the README's Limits section). This canonical refusal
/// (status 2) is better than the "command not found" (127) a script would otherwise see, which
/// reads as though the utility were simply unbuilt rather than a deliberate limitation.
fn chmod_uumain(_args: impl uucore::Args) -> i32 {
    eprintln!("chmod: changing file modes is unsupported in bash-tool");
    2
}
uu_builtin!(Chmod, "chmod", "change file mode bits", chmod_uumain);
/// `printenv [-0] [NAME]...`: the exported variables, or the values of the named ones.
pub(crate) struct Printenv;

impl Printenv {
    const NAME: &'static str = "printenv";
    const SYNOPSIS: &'static str = "print all or part of the environment";
}

impl SimpleCommand for Printenv {
    fn get_content(
        name: &str,
        content_type: ContentType,
        _options: &ContentOptions,
    ) -> Result<String, Error> {
        uu_get_content(name, Printenv::SYNOPSIS, content_type)
    }

    fn execute<SE, I, S>(
        context: ExecutionContext<'_, SE>,
        args: I,
    ) -> Result<ExecutionResult, Error>
    where
        SE: ShellExtensions,
        I: Iterator<Item = S>,
        S: AsRef<str>,
    {
        let argv: Vec<String> = args.skip(1).map(|s| s.as_ref().to_string()).collect();
        let usage = |message: String| {
            let _ = write!(
                context.stderr(),
                "printenv: {message}\nTry 'printenv --help' for more information.\n"
            );
            Ok(ExecutionResult::new(2))
        };
        let mut null = false;
        let mut names = Vec::new();
        let mut options = true;
        for arg in &argv {
            match arg.as_str() {
                "--" if options => options = false,
                "-0" | "--null" if options => null = true,
                "--help" if options => {
                    let _ = write!(
                        context.stdout(),
                        "Usage: printenv [OPTION] [VARIABLE]...\nPrint the values of the \
                         specified environment VARIABLE(s).\nIf no VARIABLE is specified, print \
                         name and value pairs for them all.\n\n  -0, --null     end each output \
                         line with NUL, not newline\n      --help     display this help and \
                         exit\n      --version  output version information and exit\n"
                    );
                    return Ok(ExecutionResult::new(0));
                }
                "--version" if options => {
                    let _ = writeln!(
                        context.stdout(),
                        "printenv (bash-tool, GNU coreutils compatible)"
                    );
                    return Ok(ExecutionResult::new(0));
                }
                long if options && long.starts_with("--") => {
                    return usage(format!("unrecognized option '{long}'"));
                }
                short if options && short.len() > 1 && short.starts_with('-') => {
                    let letter = short
                        .chars()
                        .find(|c| *c != '-' && *c != '0')
                        .unwrap_or('-');
                    if short[1..].chars().all(|c| c == '0') {
                        null = true;
                    } else {
                        return usage(format!("invalid option -- '{letter}'"));
                    }
                }
                name => names.push(name.to_owned()),
            }
        }
        let end = if null { '\0' } else { '\n' };
        let mut env = exported_env(&context);
        env.sort();
        let mut out = context.stdout();
        if names.is_empty() {
            for (name, value) in env {
                let _ = out.write_all(&super::shell_bytes::encode(&format!("{name}={value}{end}")));
            }
            return Ok(ExecutionResult::new(0));
        }
        let mut missing = false;
        for name in &names {
            match env
                .iter()
                .find(|(key, _)| key == name && !name.contains('='))
            {
                Some((_, value)) => {
                    let _ = out.write_all(&super::shell_bytes::encode(&format!("{value}{end}")));
                }
                None => missing = true,
            }
        }
        Ok(ExecutionResult::new(u8::from(missing)))
    }
}

pub(crate) struct Env;

impl Env {
    const NAME: &'static str = "env";
    const SYNOPSIS: &'static str = "print the environment";
}

impl SimpleCommand for Env {
    fn get_content(
        name: &str,
        content_type: ContentType,
        _options: &ContentOptions,
    ) -> Result<String, Error> {
        uu_get_content(name, Env::SYNOPSIS, content_type)
    }

    #[allow(clippy::cast_sign_loss)] // code is clamped to 0..=255 before the u8 cast
    fn execute<SE, I, S>(
        context: ExecutionContext<'_, SE>,
        args: I,
    ) -> Result<ExecutionResult, Error>
    where
        SE: ShellExtensions,
        I: Iterator<Item = S>,
        S: AsRef<str>,
    {
        let argv: Vec<String> = args.map(|s| s.as_ref().to_string()).collect();

        let has_command = argv
            .iter()
            .skip(1)
            .any(|a| !a.starts_with('-') && !a.contains('='));
        if has_command {
            let os_argv = argv.iter().map(|arg| super::shell_bytes::to_os_string(arg));
            let code = run_uu(&context, "env", move || uu_env::uumain(os_argv));
            return Ok(ExecutionResult::new(code.clamp(0, 255) as u8));
        }

        let mut null_sep = false;
        let mut ignore_env = false;
        let mut unset: Vec<String> = Vec::new();
        let mut adds: Vec<(String, String)> = Vec::new();
        // GNU env's usage errors: the message, then the --help hint, exit 125.
        let usage = |message: String| {
            let _ = write!(
                context.stderr(),
                "env: {message}\nTry 'env --help' for more information.\n"
            );
            Ok(ExecutionResult::new(125))
        };
        let mut it = argv.iter().skip(1).peekable();
        while let Some(a) = it.next() {
            match a.as_str() {
                "-0" | "--null" => null_sep = true,
                "-i" | "--ignore-environment" | "-" => ignore_env = true,
                "-u" | "--unset" => match it.next() {
                    Some(name) => unset.push(name.clone()),
                    None if a == "-u" => {
                        return usage("option requires an argument -- 'u'".into());
                    }
                    None => return usage("option '--unset' requires an argument".into()),
                },
                _ if a.starts_with("--unset=") => unset.push(a["--unset=".len()..].to_string()),
                _ if a.starts_with("-u") => unset.push(a[2..].to_string()),
                long if long.starts_with("--") => {
                    return usage(format!("unrecognized option '{long}'"));
                }
                short if short.starts_with('-') => {
                    let letter = short.chars().nth(1).unwrap_or('-');
                    return usage(format!("invalid option -- '{letter}'"));
                }
                _ => {
                    if let Some((k, v)) = a.split_once('=') {
                        adds.push((k.to_string(), v.to_string()));
                    }
                }
            }
        }

        let mut env: std::collections::BTreeMap<String, String> = if ignore_env {
            std::collections::BTreeMap::new()
        } else {
            exported_env(&context).into_iter().collect()
        };
        for name in &unset {
            env.remove(name);
        }
        for (k, v) in adds {
            env.insert(k, v);
        }

        let mut out = context.stdout();
        for (k, v) in env {
            let line = format!("{k}={v}{}", if null_sep { '\0' } else { '\n' });
            let _ = out.write_all(&super::shell_bytes::encode(&line));
        }
        Ok(ExecutionResult::new(0))
    }
}

/// `env [OPTION]... [NAME=VALUE]... COMMAND [ARG]...`: runs COMMAND as a child of this shell
/// (every command here runs in-process) with the environment changed: `-i` empties it, `-u` unsets
/// a name, `NAME=VALUE` sets one, `-C` changes directory, `-S` splits a string into arguments,
/// `-v` traces each step, and the signal options ignore, restore or block signals for it (a
/// blocked signal is held off as an ignored one is: nothing in a call delivers it later). `-a`
/// gives `sh -c` and `bash -c` their `$0`; no other command here has an argv[0] to set. Without a
/// command it prints the environment, as the finite `Env` does.
#[cfg(target_arch = "wasm32")]
pub(crate) fn env_driver<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<brush_core::CommandArg>,
) -> brush_core::builtins::BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        use super::env_options::quote;
        use futures::io::AsyncWriteExt;
        let argv: Vec<String> = args.iter().map(ToString::to_string).collect();
        let exported = exported_env(&context);
        let mut plan = match env_plan(&argv, &exported) {
            Ok(Some(plan)) => plan,
            Ok(None) => return super::streaming::finite::<Env, SE>(context, args).await,
            Err((message, code)) => {
                context
                    .stderr()
                    .async_io()
                    .write_all(message.as_bytes())
                    .await?;
                return Ok(ExecutionResult::new(code));
            }
        };
        let mut trace = std::mem::take(&mut plan.trace);
        if plan.debug {
            if plan.clear {
                trace.push_str("cleaning environ\n");
            } else {
                for name in &plan.unset {
                    trace.push_str(&format!("unset:    {name}\n"));
                }
            }
            for (name, value) in &plan.assign {
                trace.push_str(&format!("setenv:   {name}={value}\n"));
            }
        }
        if plan.command.is_empty() {
            // Print the environment the options leave, as the finite `Env` prints one.
            if !trace.is_empty() {
                context
                    .stderr()
                    .async_io()
                    .write_all(trace.as_bytes())
                    .await?;
            }
            let mut words = vec!["env".to_owned()];
            if plan.clear {
                words.push("-i".to_owned());
            }
            if plan.null {
                words.push("-0".to_owned());
            }
            for name in &plan.unset {
                words.extend(["-u".to_owned(), name.clone()]);
            }
            words.extend(
                plan.assign
                    .iter()
                    .map(|(name, value)| format!("{name}={value}")),
            );
            let args = words
                .into_iter()
                .map(brush_core::CommandArg::String)
                .collect();
            return super::streaming::finite::<Env, SE>(context, args).await;
        }
        if let Some((&number, _)) = plan
            .signals
            .iter()
            .find(|(number, handling)| matches!(number, 9 | 19) && handling.ignore.is_some())
        {
            let message =
                format!("env: failed to set signal action for signal {number}: Invalid argument\n");
            return env_fail(&context, format!("{trace}{message}"), 125).await;
        }
        if plan.list_signals {
            // What the command starts with: the signals this shell ignores, then the options.
            let mut state: std::collections::BTreeMap<u8, (bool, bool)> = context
                .shell
                .traps()
                .signal_dispositions()
                .filter(|(_, disposition)| {
                    matches!(disposition, brush_core::traps::PipeDisposition::Ignored)
                })
                .map(|(number, _)| (number, (true, false)))
                .collect();
            for (&number, handling) in &plan.signals {
                let entry = state.entry(number).or_default();
                if let Some(ignore) = handling.ignore {
                    entry.0 = ignore;
                }
                entry.1 |= handling.block;
            }
            for (number, (ignored, blocked)) in state {
                let what = match (blocked, ignored) {
                    (true, true) => "BLOCK,IGNORE",
                    (true, false) => "BLOCK",
                    (false, true) => "IGNORE",
                    (false, false) => continue,
                };
                let name = super::env_options::signal_name(number)
                    .unwrap_or_else(|| format!("SIG{number}"));
                trace.push_str(&format!("{name:<10} ({number:2}): {what}\n"));
            }
        }
        let cwd = match &plan.chdir {
            Some(dir) => {
                // GNU names the directory in shell quotes, always.
                let quoted = format!("'{}'", dir.replace('\'', r"'\''"));
                if plan.debug {
                    trace.push_str(&format!("chdir:    {quoted}\n"));
                }
                let path = context.shell.absolute_path(std::path::Path::new(dir));
                if !path.is_dir() {
                    let reason = std::fs::metadata(&path).map_or_else(
                        |error| super::io_message(&error),
                        |_| "Not a directory".to_owned(),
                    );
                    let message = format!("env: cannot change directory to {quoted}: {reason}\n");
                    return env_fail(&context, format!("{trace}{message}"), 125).await;
                }
                path
            }
            None => context.shell.working_dir().to_path_buf(),
        };
        if let Some(argv0) = &plan.argv0 {
            let program = plan.command[0].rsplit('/').next().unwrap_or_default();
            let with_argv0 = matches!(program, "sh" | "bash")
                .then(|| super::sh::with_argv0(program, &plan.command[1..], argv0))
                .flatten();
            let Some(rest) = with_argv0 else {
                let message = "env: -a (argv[0]) is unsupported in bash-tool except for `sh -c` \
                               and `bash -c`, whose $0 it sets\n";
                return env_fail(&context, format!("{trace}{message}"), 2).await;
            };
            plan.command.truncate(1);
            plan.command.extend(rest);
        }
        if plan.debug {
            trace.push_str(&format!("executing: {}\n", plan.command[0]));
            for (index, arg) in plan.command.iter().enumerate() {
                trace.push_str(&format!("   arg[{index}]= {}\n", quote(arg)));
            }
        }
        let name = &plan.command[0];
        match super::xargs::lookup(context.shell, name, &cwd) {
            super::xargs::Lookup::Found => {}
            lookup => {
                let (reason, code) = match lookup {
                    super::xargs::Lookup::Denied => ("Permission denied", 126),
                    _ => ("No such file or directory", 127),
                };
                let message = format!("env: {}: {reason}\n", quote(name));
                return env_fail(&context, format!("{trace}{message}"), code).await;
            }
        }
        if !trace.is_empty() {
            context
                .stderr()
                .async_io()
                .write_all(trace.as_bytes())
                .await?;
        }
        // GNU env runs the printf program, coreutils' own, not bash's builtin of the same name.
        if name == "printf" || name.ends_with("/printf") {
            let args = plan
                .command
                .iter()
                .map(|word| brush_core::CommandArg::String(word.clone()))
                .collect();
            return super::streaming::finite::<Printf, SE>(context, args).await;
        }
        // The child's environment and signals, changed before the command runs.
        let mut prologue = Vec::new();
        let exported: Vec<String> = context
            .shell
            .env()
            .iter()
            .filter(|(_, variable)| variable.is_exported() && !variable.is_readonly())
            .map(|(name, _)| name.clone())
            .collect();
        let unset: Vec<&String> = if plan.clear {
            exported.iter().collect()
        } else {
            plan.unset
                .iter()
                .filter(|name| exported.contains(name))
                .collect()
        };
        if !unset.is_empty() {
            let names: Vec<String> = unset
                .iter()
                .map(|name| super::xargs::shell_quote(name))
                .collect();
            prologue.push(format!("unset -v {}", names.join(" ")));
        }
        if plan.clear {
            prologue.push("unset -f $(compgen -A function) 2>/dev/null".to_owned());
        }
        for (name, value) in &plan.assign {
            if is_identifier(name) {
                prologue.push(format!(
                    "export {name}={}",
                    super::xargs::shell_quote(value)
                ));
            }
        }
        for (number, handling) in &plan.signals {
            let action = match (handling.ignore, handling.block) {
                (Some(true), _) | (_, true) => "''",
                (Some(false), false) => "-",
                (None, false) => continue,
            };
            // A signal this shell has no name for cannot reach the command anyway.
            prologue.push(format!("trap {action} {number} 2>/dev/null"));
        }
        let prologue = (!prologue.is_empty()).then(|| prologue.join("\n"));
        let line = super::xargs::command_line(&plan.command);
        let params = context.params.clone();
        super::xargs::run_child(
            context.shell,
            &params,
            prologue,
            line,
            plan.chdir.as_ref().map(|_| cwd.as_path()),
            None,
            false,
        )
        .await
    })
}

/// Ends `env` with `text` (its trace, then its diagnostic) on stderr and `code`.
#[cfg(target_arch = "wasm32")]
async fn env_fail<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
    text: String,
    code: u8,
) -> Result<ExecutionResult, Error> {
    use futures::io::AsyncWriteExt;
    context
        .stderr()
        .async_io()
        .write_all(text.as_bytes())
        .await?;
    Ok(ExecutionResult::new(code))
}

/// `split --filter=COMMAND`: GNU pipes each piece to COMMAND, run by the shell with `$FILE` set
/// to the piece's name, and writes no files. The pieces are made in a private directory by the
/// ordinary split, then each is handed to COMMAND, in order, and removed. Without `--filter` it is
/// the ordinary split.
#[cfg(target_arch = "wasm32")]
pub(crate) fn split_driver<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<brush_core::CommandArg>,
) -> brush_core::builtins::BoxFuture<'_, Result<ExecutionResult, Error>> {
    Box::pin(async move {
        use clap::parser::ValueSource;
        use futures::io::AsyncWriteExt;
        let argv: Vec<String> = args.iter().map(ToString::to_string).collect();
        let Ok(matches) = uu_split::uu_app().try_get_matches_from(&argv) else {
            return super::streaming::finite::<Split, SE>(context, args).await;
        };
        let Some(filter) = matches.get_one::<String>("filter").cloned() else {
            return super::streaming::finite::<Split, SE>(context, args).await;
        };
        // Where the operands are: they follow the options, so the last words equal to them.
        let given = |id: &str| {
            (matches.value_source(id) == Some(ValueSource::CommandLine))
                .then(|| matches.get_one::<std::ffi::OsString>(id))
                .flatten()
                .map(|value| value.to_string_lossy().into_owned())
        };
        let prefix_value = given("prefix");
        let input_value = given("input");
        let prefix_at = prefix_value
            .as_ref()
            .and_then(|prefix| argv.iter().rposition(|word| word == prefix));
        let input_at = input_value.as_ref().and_then(|input| {
            argv[..prefix_at.unwrap_or(argv.len())]
                .iter()
                .rposition(|word| word == input)
        });
        let prefix = prefix_value.unwrap_or_else(|| "x".to_owned());
        let Some(scratch) = split_scratch(&context) else {
            let message = "split: cannot create a directory for --filter pieces\n";
            context
                .stderr()
                .async_io()
                .write_all(message.as_bytes())
                .await?;
            return Ok(ExecutionResult::new(1));
        };
        // The same split, into the private directory, without the filter.
        let mut words: Vec<Option<String>> = argv.iter().cloned().map(Some).collect();
        let mut index = 1;
        while index < words.len() {
            let word = words[index].clone().unwrap_or_default();
            let long = word.strip_prefix("--").unwrap_or_default();
            let name = long.split_once('=').map_or(long, |(name, _)| name);
            if !name.is_empty()
                && "filter".starts_with(name)
                && Some(index) != input_at
                && Some(index) != prefix_at
            {
                words[index] = None;
                if !long.contains('=') && index + 1 < words.len() {
                    words[index + 1] = None;
                    index += 1;
                }
            }
            index += 1;
        }
        if let Some(at) = input_at
            && let Some(input) = words[at].as_mut()
            && input != "-"
        {
            *input = context
                .shell
                .absolute_path(std::path::Path::new(input.as_str()))
                .to_string_lossy()
                .into_owned();
        }
        let scratch_prefix = format!("{}/p", scratch.display());
        match prefix_at {
            Some(at) => words[at] = Some(scratch_prefix),
            None => {
                if input_at.is_none() {
                    words.push(Some("-".into()));
                }
                words.push(Some(scratch_prefix));
            }
        }
        let words: Vec<brush_core::CommandArg> = words
            .into_iter()
            .flatten()
            .map(brush_core::CommandArg::String)
            .collect();
        let child = ExecutionContext {
            shell: &mut *context.shell,
            params: context.params.clone(),
            command_name: context.command_name.clone(),
        };
        let split = super::streaming::finite::<Split, SE>(child, words).await;
        let mut pieces: Vec<std::path::PathBuf> = std::fs::read_dir(&scratch)
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        pieces.sort();
        let mut status = split
            .as_ref()
            .map_or(1, |result| u8::from(result.exit_code));
        if status == 0 {
            for piece in pieces.iter() {
                let suffix = piece
                    .file_name()
                    .map(|name| name.to_string_lossy()[1..].to_owned())
                    .unwrap_or_default();
                let file = format!("{prefix}{suffix}");
                let bytes = std::fs::read(piece).unwrap_or_default();
                let _ = std::fs::remove_file(piece);
                let mut params = context.params.clone();
                params.set_fd(
                    brush_core::openfiles::OpenFiles::STDIN_FD,
                    brush_core::openfiles::from_bytes(bytes),
                );
                let prologue = format!("export FILE={}", super::xargs::shell_quote(&file));
                let result = super::xargs::run_child(
                    context.shell,
                    &params,
                    Some(prologue),
                    filter.clone(),
                    None,
                    None,
                    false,
                )
                .await;
                let code = result.map_or(1, |result| u8::from(result.exit_code));
                if code != 0 {
                    let message =
                        format!("split: with FILE={file}, exit {code} from command: {filter}\n");
                    context
                        .stderr()
                        .async_io()
                        .write_all(message.as_bytes())
                        .await?;
                    status = code;
                    break;
                }
            }
        }
        let _ = std::fs::remove_dir_all(&scratch);
        match split {
            Err(error) => Err(error),
            Ok(_) => Ok(ExecutionResult::new(status)),
        }
    })
}

/// A new private directory for `split --filter`'s pieces: in `$TMPDIR`, `/tmp` or the working
/// directory, whichever takes it.
#[cfg(target_arch = "wasm32")]
fn split_scratch<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
) -> Option<std::path::PathBuf> {
    use std::hash::{BuildHasher, Hasher};
    let tmpdir = exported_env(context)
        .into_iter()
        .find(|(name, _)| name == "TMPDIR")
        .map(|(_, value)| std::path::PathBuf::from(value));
    let random = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    [
        tmpdir,
        Some("/tmp".into()),
        Some(context.shell.working_dir().to_path_buf()),
    ]
    .into_iter()
    .flatten()
    .map(|dir| dir.join(format!(".bash-tool-split-{random:016x}")))
    .find(|dir| std::fs::create_dir(dir).is_ok())
}

#[cfg(target_arch = "wasm32")]
fn split_utility<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<brush_core::CommandArg>,
) -> brush_core::builtins::BoxFuture<'_, Result<ExecutionResult, Error>> {
    super::streaming::utility(context, args, split_driver::<SE>)
}

#[cfg(target_arch = "wasm32")]
fn env_utility<SE: ShellExtensions>(
    context: ExecutionContext<'_, SE>,
    args: Vec<brush_core::CommandArg>,
) -> brush_core::builtins::BoxFuture<'_, Result<ExecutionResult, Error>> {
    super::streaming::utility(context, args, env_driver::<SE>)
}

#[cfg(target_arch = "wasm32")]
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// What `env` is asked to run.
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(dead_code, reason = "env runs its command only on wasm32")
)]
struct EnvPlan {
    clear: bool,
    unset: Vec<String>,
    assign: Vec<(String, String)>,
    chdir: Option<String>,
    command: Vec<String>,
    /// `-a`/`--argv0`: the command's argv[0].
    argv0: Option<String>,
    /// `-v`/`--debug`: trace each step to stderr.
    debug: bool,
    /// The trace of what parsing did (`-S`'s splits), printed before the rest.
    trace: String,
    /// The signal options' requests, by signal number.
    signals: std::collections::BTreeMap<u8, super::env_options::Handling>,
    /// `--list-signal-handling`.
    list_signals: bool,
    /// `-0`/`--null`, which only printing the environment takes.
    null: bool,
}

/// Parses `env`'s arguments as GNU does (options first, then assignments, then the command); a
/// `-S` string's words take its place and are parsed in turn. `env` is the environment `-S`
/// expands `${NAME}` from. `Ok(None)` for `--help` and `--version`, which the finite `Env`
/// answers; a plan with no command prints the environment; `Err` with the diagnostic and status
/// for an error.
#[cfg_attr(
    not(target_arch = "wasm32"),
    allow(dead_code, reason = "env runs its command only on wasm32")
)]
fn env_plan(argv: &[String], env: &[(String, String)]) -> Result<Option<EnvPlan>, (String, u8)> {
    use super::env_options::{Handling, quote, signal_list, split_string};
    let usage = |message: &str| {
        Err((
            format!("env: {message}\nTry 'env --help' for more information.\n"),
            125,
        ))
    };
    let mut plan = EnvPlan {
        clear: false,
        unset: Vec::new(),
        assign: Vec::new(),
        chdir: None,
        command: Vec::new(),
        argv0: None,
        debug: false,
        trace: String::new(),
        signals: std::collections::BTreeMap::new(),
        list_signals: false,
        null: false,
    };
    let mut words: std::collections::VecDeque<String> = argv.iter().skip(1).cloned().collect();
    // `-S STRING`: its words replace it, and parsing goes on through them.
    let split = |plan: &mut EnvPlan,
                 words: &mut std::collections::VecDeque<String>,
                 text: &str|
     -> Result<(), (String, u8)> {
        let parts = split_string(text, env)?;
        if plan.debug {
            plan.trace
                .push_str(&format!("split -S:  {}\n", quote(text)));
            for (index, part) in parts.iter().enumerate() {
                let lead = if index == 0 { " into:" } else { "     &" };
                plan.trace.push_str(&format!("{lead}    {}\n", quote(part)));
            }
        }
        for part in parts.into_iter().rev() {
            words.push_front(part);
        }
        Ok(())
    };
    let signals = |plan: &mut EnvPlan,
                   list: Option<&str>,
                   apply: &dyn Fn(&mut Handling)|
     -> Result<(), (String, u8)> {
        for number in signal_list(list)? {
            apply(plan.signals.entry(number).or_default());
        }
        Ok(())
    };
    while let Some(arg) = words.front().cloned() {
        if arg == "--" {
            words.pop_front();
            break;
        }
        if arg == "-" {
            plan.clear = true;
            words.pop_front();
            continue;
        }
        if !arg.starts_with('-') || arg.len() == 1 {
            break;
        }
        words.pop_front();
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = long
                .split_once('=')
                .map_or((long, None), |(n, v)| (n, Some(v.to_owned())));
            let value = |words: &mut std::collections::VecDeque<String>| {
                inline.clone().or_else(|| words.pop_front())
            };
            match name {
                "ignore-environment" => plan.clear = true,
                "null" => plan.null = true,
                "debug" => plan.debug = true,
                "unset" => match value(&mut words) {
                    Some(name) => plan.unset.push(name),
                    None => return usage("option '--unset' requires an argument"),
                },
                "chdir" => match value(&mut words) {
                    Some(dir) => plan.chdir = Some(dir),
                    None => return usage("option '--chdir' requires an argument"),
                },
                "argv0" => match value(&mut words) {
                    Some(name) => plan.argv0 = Some(name),
                    None => return usage("option '--argv0' requires an argument"),
                },
                "split-string" => match value(&mut words) {
                    Some(text) => split(&mut plan, &mut words, &text)?,
                    None => return usage("option '--split-string' requires an argument"),
                },
                "ignore-signal" => signals(&mut plan, inline.as_deref(), &|handling| {
                    handling.ignore = Some(true);
                })?,
                "default-signal" => signals(&mut plan, inline.as_deref(), &|handling| {
                    handling.ignore = Some(false);
                })?,
                "block-signal" => signals(&mut plan, inline.as_deref(), &|handling| {
                    handling.block = true;
                })?,
                "list-signal-handling" => plan.list_signals = true,
                // Help and version: the finite `Env` answers.
                "help" | "version" => return Ok(None),
                _ => return usage(&format!("unrecognized option '{arg}'")),
            }
            continue;
        }
        for (index, letter) in arg[1..].char_indices() {
            let rest = &arg[1 + index + letter.len_utf8()..];
            match letter {
                'i' => plan.clear = true,
                '0' => plan.null = true,
                'v' => plan.debug = true,
                'u' | 'C' | 'a' | 'S' => {
                    let value = if rest.is_empty() {
                        match words.pop_front() {
                            Some(value) => value,
                            None => {
                                return usage(&format!(
                                    "option requires an argument -- '{letter}'"
                                ));
                            }
                        }
                    } else {
                        rest.to_owned()
                    };
                    match letter {
                        'u' => plan.unset.push(value),
                        'C' => plan.chdir = Some(value),
                        'a' => plan.argv0 = Some(value),
                        _ => split(&mut plan, &mut words, &value)?,
                    }
                    break;
                }
                _ => return usage(&format!("invalid option -- '{letter}'")),
            }
        }
    }
    while let Some(arg) = words.front().filter(|arg| arg.contains('=')).cloned() {
        words.pop_front();
        let (name, value) = arg.split_once('=').unwrap_or((&arg, ""));
        if name.is_empty() {
            return Err((
                format!("env: cannot set {}: Invalid argument\n", quote(name)),
                125,
            ));
        }
        plan.assign.push((name.to_owned(), value.to_owned()));
    }
    plan.command = words.into_iter().collect();
    if plan.command.is_empty() {
        if plan.chdir.is_some() {
            return usage("must specify command with --chdir (-C)");
        }
        return Ok(Some(plan));
    }
    if plan.null {
        return usage("cannot specify --null (-0) with command");
    }
    Ok(Some(plan))
}

/// The shell's exported variables as jq sees them: text, each byte that is not UTF-8 replaced by
/// U+FFFD, as jq replaces them in `$ENV`.
pub(crate) fn exported_env_utf8<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
) -> Vec<(String, String)> {
    exported_env(context)
        .into_iter()
        .map(|(name, value)| (name, super::shell_bytes::to_utf8_lossy(&value)))
        .collect()
}

/// The shell's exported variables, which commands see as their environment. As in bash, one
/// exported before it has a value, and an array, are left out, and each exported function is
/// there as `BASH_FUNC_NAME%%=() { BODY }`.
pub(crate) fn exported_env<SE: ShellExtensions>(
    context: &ExecutionContext<'_, SE>,
) -> Vec<(String, String)> {
    let variables = context
        .shell
        .env()
        .iter()
        .filter(|(_, variable)| {
            variable.is_exported() && variable.value().is_set() && !variable.value().is_array()
        })
        .map(|(name, variable)| {
            (
                name.to_owned(),
                variable.value().to_cow_str(context.shell).into_owned(),
            )
        });
    let functions = context
        .shell
        .funcs()
        .iter()
        .filter(|(_, function)| function.is_exported())
        .map(|(name, function)| {
            (
                format!("BASH_FUNC_{name}%%"),
                exported_function_text(&function.definition().to_string()),
            )
        });
    variables.chain(functions).collect()
}

/// A function's definition (as `declare -f` prints it) as bash exports it: without its name, on
/// the line after `() `, and indented by one blank at every level: `() {  echo hi\n}`.
pub(crate) fn exported_function_text(definition: &str) -> String {
    let mut lines = definition.lines().skip(1);
    let mut text = String::from("() ");
    text.push_str(lines.next().unwrap_or_default());
    for (index, line) in lines.enumerate() {
        if index > 0 {
            text.push('\n');
        }
        let body = line.trim_start();
        if body.len() < line.len() {
            text.push(' ');
        }
        text.push_str(body);
    }
    text
}

/// The coreutils builtins to register on the shell, in addition to brush's bash set.
pub(crate) fn builtins<SE: ShellExtensions>() -> Vec<(String, Registration<SE>)> {
    use brush_core::builtins::simple_builtin;
    #[allow(unused_mut)] // The WASM drivers replace registrations below.
    let mut registrations: Vec<(String, Registration<SE>)> = vec![
        ("cat".into(), simple_builtin::<Cat, SE>()),
        ("ls".into(), simple_builtin::<Ls, SE>()),
        ("wc".into(), simple_builtin::<Wc, SE>()),
        ("head".into(), simple_builtin::<Head, SE>()),
        ("sort".into(), simple_builtin::<Sort, SE>()),
        ("mkdir".into(), simple_builtin::<Mkdir, SE>()),
        ("rm".into(), simple_builtin::<Rm, SE>()),
        ("mv".into(), simple_builtin::<Mv, SE>()),
        ("cp".into(), simple_builtin::<Cp, SE>()),
        ("env".into(), simple_builtin::<Env, SE>()),
        ("printenv".into(), simple_builtin::<Printenv, SE>()),
        ("cut".into(), simple_builtin::<Cut, SE>()),
        ("tr".into(), simple_builtin::<Tr, SE>()),
        ("uniq".into(), simple_builtin::<Uniq, SE>()),
        ("tail".into(), simple_builtin::<Tail, SE>()),
        ("tee".into(), simple_builtin::<Tee, SE>()),
        ("touch".into(), simple_builtin::<Touch, SE>()),
        ("sleep".into(), simple_builtin::<Sleep, SE>()),
        ("printf".into(), simple_builtin::<Printf, SE>()),
        ("basename".into(), simple_builtin::<Basename, SE>()),
        ("dirname".into(), simple_builtin::<Dirname, SE>()),
        ("realpath".into(), simple_builtin::<Realpath, SE>()),
        ("readlink".into(), simple_builtin::<Readlink, SE>()),
        ("ln".into(), simple_builtin::<Ln, SE>()),
        ("link".into(), simple_builtin::<Link, SE>()),
        ("unlink".into(), simple_builtin::<Unlink, SE>()),
        ("rmdir".into(), simple_builtin::<Rmdir, SE>()),
        ("mktemp".into(), simple_builtin::<Mktemp, SE>()),
        ("truncate".into(), simple_builtin::<Truncate, SE>()),
        ("nl".into(), simple_builtin::<Nl, SE>()),
        ("paste".into(), simple_builtin::<Paste, SE>()),
        ("join".into(), simple_builtin::<Join, SE>()),
        ("comm".into(), simple_builtin::<Comm, SE>()),
        ("fold".into(), simple_builtin::<Fold, SE>()),
        ("fmt".into(), simple_builtin::<Fmt, SE>()),
        ("expand".into(), simple_builtin::<Expand, SE>()),
        ("unexpand".into(), simple_builtin::<Unexpand, SE>()),
        ("tsort".into(), simple_builtin::<Tsort, SE>()),
        ("split".into(), simple_builtin::<Split, SE>()),
        ("csplit".into(), simple_builtin::<Csplit, SE>()),
        ("base64".into(), simple_builtin::<Base64, SE>()),
        ("base32".into(), simple_builtin::<Base32, SE>()),
        ("basenc".into(), simple_builtin::<Basenc, SE>()),
        ("md5sum".into(), simple_builtin::<Md5sum, SE>()),
        ("sha1sum".into(), simple_builtin::<Sha1sum, SE>()),
        ("sha256sum".into(), simple_builtin::<Sha256sum, SE>()),
        ("sha512sum".into(), simple_builtin::<Sha512sum, SE>()),
        ("b2sum".into(), simple_builtin::<B2sum, SE>()),
        ("cksum".into(), simple_builtin::<Cksum, SE>()),
        ("od".into(), simple_builtin::<Od, SE>()),
        ("date".into(), simple_builtin::<Date, SE>()),
        ("expr".into(), simple_builtin::<Expr, SE>()),
        ("factor".into(), simple_builtin::<Factor, SE>()),
        ("numfmt".into(), simple_builtin::<Numfmt, SE>()),
        ("shuf".into(), simple_builtin::<Shuf, SE>()),
        ("nproc".into(), simple_builtin::<Nproc, SE>()),
        ("uname".into(), simple_builtin::<Uname, SE>()),
        ("hostname".into(), simple_builtin::<Hostname, SE>()),
        ("dd".into(), simple_builtin::<Dd, SE>()),
        ("chmod".into(), simple_builtin::<Chmod, SE>()),
        ("du".into(), simple_builtin::<Du, SE>()),
    ];
    #[cfg(target_arch = "wasm32")]
    for (name, registration) in &mut registrations {
        use super::streaming;
        match name.as_str() {
            "cat" => registration.execute_func = streaming::cat,
            "head" => registration.execute_func = streaming::head,
            "cut" => registration.execute_func = streaming::cut,
            "tr" => registration.execute_func = streaming::tr,
            "uniq" => registration.execute_func = streaming::uniq,
            "tee" => registration.execute_func = streaming::tee,
            "sleep" => registration.execute_func = streaming::sleep,
            "sort" => registration.execute_func = streaming::sort,
            "wc" => registration.execute_func = streaming::wc,
            "tail" => registration.execute_func = streaming::tail,
            "printf" => registration.execute_func = streaming::printf,
            "ls" => *registration = streaming::finite_utility_builtin::<Ls, SE>(),
            "mkdir" => *registration = streaming::finite_utility_builtin::<Mkdir, SE>(),
            "rm" => *registration = streaming::finite_utility_builtin::<Rm, SE>(),
            "mv" => *registration = streaming::finite_utility_builtin::<Mv, SE>(),
            "cp" => *registration = streaming::finite_utility_builtin::<Cp, SE>(),
            "printenv" => *registration = streaming::finite_utility_builtin::<Printenv, SE>(),
            "env" => {
                *registration = streaming::finite_utility_builtin::<Env, SE>();
                registration.execute_func = env_utility::<SE>;
            }
            "touch" => *registration = streaming::finite_utility_builtin::<Touch, SE>(),
            "basename" => *registration = streaming::finite_utility_builtin::<Basename, SE>(),
            "dirname" => *registration = streaming::finite_utility_builtin::<Dirname, SE>(),
            "realpath" => *registration = streaming::finite_utility_builtin::<Realpath, SE>(),
            "readlink" => *registration = streaming::finite_utility_builtin::<Readlink, SE>(),
            "ln" => *registration = streaming::finite_utility_builtin::<Ln, SE>(),
            "link" => *registration = streaming::finite_utility_builtin::<Link, SE>(),
            "unlink" => *registration = streaming::finite_utility_builtin::<Unlink, SE>(),
            "rmdir" => *registration = streaming::finite_utility_builtin::<Rmdir, SE>(),
            "mktemp" => *registration = streaming::finite_utility_builtin::<Mktemp, SE>(),
            "truncate" => *registration = streaming::finite_utility_builtin::<Truncate, SE>(),
            "nl" => *registration = streaming::finite_stdin_utility_builtin::<Nl, SE>(),
            "paste" => *registration = streaming::finite_stdin_utility_builtin::<Paste, SE>(),
            "join" => *registration = streaming::finite_stdin_utility_builtin::<Join, SE>(),
            "comm" => *registration = streaming::finite_stdin_utility_builtin::<Comm, SE>(),
            "fold" => *registration = streaming::finite_stdin_utility_builtin::<Fold, SE>(),
            "fmt" => *registration = streaming::finite_stdin_utility_builtin::<Fmt, SE>(),
            "expand" => *registration = streaming::finite_stdin_utility_builtin::<Expand, SE>(),
            "unexpand" => *registration = streaming::finite_stdin_utility_builtin::<Unexpand, SE>(),
            "tsort" => *registration = streaming::finite_stdin_utility_builtin::<Tsort, SE>(),
            "split" => {
                *registration = streaming::finite_utility_builtin::<Split, SE>();
                registration.execute_func = split_utility::<SE>;
            }
            "csplit" => *registration = streaming::finite_utility_builtin::<Csplit, SE>(),
            "base64" => *registration = streaming::finite_stdin_utility_builtin::<Base64, SE>(),
            "base32" => *registration = streaming::finite_stdin_utility_builtin::<Base32, SE>(),
            "basenc" => *registration = streaming::finite_stdin_utility_builtin::<Basenc, SE>(),
            "md5sum" => *registration = streaming::finite_stdin_utility_builtin::<Md5sum, SE>(),
            "sha1sum" => *registration = streaming::finite_stdin_utility_builtin::<Sha1sum, SE>(),
            "sha256sum" => {
                *registration = streaming::finite_stdin_utility_builtin::<Sha256sum, SE>()
            }
            "sha512sum" => {
                *registration = streaming::finite_stdin_utility_builtin::<Sha512sum, SE>()
            }
            "b2sum" => *registration = streaming::finite_stdin_utility_builtin::<B2sum, SE>(),
            "cksum" => *registration = streaming::finite_stdin_utility_builtin::<Cksum, SE>(),
            "od" => *registration = streaming::finite_stdin_utility_builtin::<Od, SE>(),
            "date" => *registration = streaming::finite_utility_builtin::<Date, SE>(),
            "expr" => *registration = streaming::finite_utility_builtin::<Expr, SE>(),
            "factor" => *registration = streaming::finite_utility_builtin::<Factor, SE>(),
            "numfmt" => *registration = streaming::finite_stdin_utility_builtin::<Numfmt, SE>(),
            "shuf" => *registration = streaming::finite_stdin_utility_builtin::<Shuf, SE>(),
            "nproc" => *registration = streaming::finite_utility_builtin::<Nproc, SE>(),
            "uname" => *registration = streaming::finite_utility_builtin::<Uname, SE>(),
            "hostname" => *registration = streaming::finite_utility_builtin::<Hostname, SE>(),
            "dd" => *registration = streaming::finite_stdin_utility_builtin::<Dd, SE>(),
            "du" => *registration = streaming::finite_utility_builtin::<Du, SE>(),
            _ => (),
        }
        // These are logical utility processes even though their code is embedded. printf
        // retains Bash builtin semantics and therefore runs in its caller's context.
        if name != "printf" {
            registration.execution_boundary = brush_core::builtins::ExecutionBoundary::Command;
        }
    }
    registrations
}
/// Documentation for the embedded coreutils.
pub(crate) fn manifests() -> Vec<crate::manifest::Manifest> {
    use crate::manifest::Manifest;
    vec![
        Manifest::builtin(Cat::NAME, Cat::SYNOPSIS),
        Manifest::builtin(Ls::NAME, Ls::SYNOPSIS),
        Manifest::builtin(Wc::NAME, Wc::SYNOPSIS),
        Manifest::builtin(Head::NAME, Head::SYNOPSIS),
        Manifest::builtin(Sort::NAME, Sort::SYNOPSIS),
        Manifest::builtin(Mkdir::NAME, Mkdir::SYNOPSIS),
        Manifest::builtin(Rm::NAME, Rm::SYNOPSIS),
        Manifest::builtin(Mv::NAME, Mv::SYNOPSIS),
        Manifest::builtin(Cp::NAME, Cp::SYNOPSIS),
        Manifest::builtin(Env::NAME, Env::SYNOPSIS),
        Manifest::builtin(Printenv::NAME, Printenv::SYNOPSIS),
        Manifest::builtin(Cut::NAME, Cut::SYNOPSIS),
        Manifest::builtin(Tr::NAME, Tr::SYNOPSIS),
        Manifest::builtin(Uniq::NAME, Uniq::SYNOPSIS),
        Manifest::builtin(Tail::NAME, Tail::SYNOPSIS),
        Manifest::builtin(Tee::NAME, Tee::SYNOPSIS),
        Manifest::builtin(Touch::NAME, Touch::SYNOPSIS),
        Manifest::builtin(Sleep::NAME, Sleep::SYNOPSIS),
        Manifest::builtin(Printf::NAME, Printf::SYNOPSIS).with_help(
            "printf FORMAT [ARG...] — format and print data (uutils printf). Supports %s %d %x \
             %f etc. and \\n escapes. bash's `printf -v VAR` (assign to a shell variable) is not \
             supported.",
        ),
        Manifest::builtin(Basename::NAME, Basename::SYNOPSIS),
        Manifest::builtin(Dirname::NAME, Dirname::SYNOPSIS),
        Manifest::builtin(Realpath::NAME, Realpath::SYNOPSIS),
        Manifest::builtin(Readlink::NAME, Readlink::SYNOPSIS),
        Manifest::builtin(Ln::NAME, Ln::SYNOPSIS),
        Manifest::builtin(Link::NAME, Link::SYNOPSIS),
        Manifest::builtin(Unlink::NAME, Unlink::SYNOPSIS),
        Manifest::builtin(Rmdir::NAME, Rmdir::SYNOPSIS),
        Manifest::builtin(Mktemp::NAME, Mktemp::SYNOPSIS),
        Manifest::builtin(Truncate::NAME, Truncate::SYNOPSIS),
        Manifest::builtin(Nl::NAME, Nl::SYNOPSIS),
        Manifest::builtin(Paste::NAME, Paste::SYNOPSIS),
        Manifest::builtin(Join::NAME, Join::SYNOPSIS),
        Manifest::builtin(Comm::NAME, Comm::SYNOPSIS),
        Manifest::builtin(Fold::NAME, Fold::SYNOPSIS),
        Manifest::builtin(Fmt::NAME, Fmt::SYNOPSIS),
        Manifest::builtin(Expand::NAME, Expand::SYNOPSIS),
        Manifest::builtin(Unexpand::NAME, Unexpand::SYNOPSIS),
        Manifest::builtin(Tsort::NAME, Tsort::SYNOPSIS),
        Manifest::builtin(Split::NAME, Split::SYNOPSIS),
        Manifest::builtin(Csplit::NAME, Csplit::SYNOPSIS),
        Manifest::builtin(Base64::NAME, Base64::SYNOPSIS),
        Manifest::builtin(Base32::NAME, Base32::SYNOPSIS),
        Manifest::builtin(Basenc::NAME, Basenc::SYNOPSIS),
        Manifest::builtin(Md5sum::NAME, Md5sum::SYNOPSIS),
        Manifest::builtin(Sha1sum::NAME, Sha1sum::SYNOPSIS),
        Manifest::builtin(Sha256sum::NAME, Sha256sum::SYNOPSIS),
        Manifest::builtin(Sha512sum::NAME, Sha512sum::SYNOPSIS),
        Manifest::builtin(B2sum::NAME, B2sum::SYNOPSIS),
        Manifest::builtin(Cksum::NAME, Cksum::SYNOPSIS),
        Manifest::builtin(Od::NAME, Od::SYNOPSIS),
        Manifest::builtin(Date::NAME, Date::SYNOPSIS),
        Manifest::builtin(Expr::NAME, Expr::SYNOPSIS),
        Manifest::builtin(Factor::NAME, Factor::SYNOPSIS),
        Manifest::builtin(Numfmt::NAME, Numfmt::SYNOPSIS),
        Manifest::builtin(Shuf::NAME, Shuf::SYNOPSIS),
        Manifest::builtin(Nproc::NAME, Nproc::SYNOPSIS),
        Manifest::builtin(Uname::NAME, Uname::SYNOPSIS),
        Manifest::builtin(Hostname::NAME, Hostname::SYNOPSIS),
        Manifest::builtin(Dd::NAME, Dd::SYNOPSIS),
        Manifest::builtin(Chmod::NAME, Chmod::SYNOPSIS),
        Manifest::builtin(Du::NAME, Du::SYNOPSIS),
    ]
}

#[cfg(test)]
mod tests {
    use super::absolute_symlink_target;

    #[test]
    fn exported_functions_are_indented_one_blank_per_level() {
        assert_eq!(
            super::exported_function_text(
                "f () \n{ \n    echo hi;\n    if true; then\n        echo y;\n    fi\n}"
            ),
            "() {  echo hi;\n if true; then\n echo y;\n fi\n}"
        );
        assert_eq!(
            super::exported_function_text("g () \n{ \n    :\n}"),
            "() {  :\n}"
        );
    }

    fn target(args: &[&str]) -> Option<String> {
        let args: Vec<std::ffi::OsString> = std::iter::once("ln")
            .chain(args.iter().copied())
            .map(Into::into)
            .collect();
        absolute_symlink_target(&args)
    }

    #[test]
    fn finds_absolute_symbolic_targets() {
        assert_eq!(
            target(&["-sf", "/tmp/d", "link"]).as_deref(),
            Some("/tmp/d")
        );
        assert_eq!(
            target(&["--symbolic", "/a", "/b", "dir"]).as_deref(),
            Some("/a")
        );
        assert_eq!(target(&["-s", "-t", "/dir", "/a"]).as_deref(), Some("/a"));
        assert_eq!(target(&["-s", "/only"]).as_deref(), Some("/only"));
        assert_eq!(target(&["-s", "rel", "/tmp/link"]), None);
        assert_eq!(target(&["/tmp/hard", "/tmp/link"]), None);
        assert_eq!(target(&["-S", "s", "/a", "b"]), None);
    }
}
