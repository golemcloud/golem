//! Finite shell evaluations with separate output streams. A session is one call's shell: nothing
//! it declares outlives it.
use std::{path::Path, sync::Arc};

use brush_builtins::{BuiltinSet, ShellBuilderExt};
use brush_core::{
    Shell, SourceInfo,
    openfiles::{OpenFile, OpenFiles},
};

use crate::commands::{CommandDescriptor, CommandInvoker, InvocationCommands};

mod baseline;
mod stateless;
#[cfg(target_arch = "wasm32")]
mod turns;

pub(crate) use baseline::fresh_child;

pub(crate) use stateless::{diagnostic, validate as validate_script};

/// Whether a call may start in `dir`: an absolute path to an existing directory.
///
/// # Errors
/// The reason it cannot, worded like `cd`'s diagnostics and naming `dir`.
pub fn check_working_dir(dir: &str) -> Result<(), String> {
    if dir.contains('\0') {
        return Err("cwd must not contain NUL bytes".into());
    }
    if !Path::new(dir).is_absolute() {
        return Err(format!("cwd {dir}: not an absolute path"));
    }
    match std::fs::metadata(dir) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(format!("cwd {dir}: Not a directory")),
        Err(error) => Err(format!("cwd {dir}: {}", crate::tools::io_message(&error))),
    }
}

/// Where a step of a stop acts on a script (see [`Session::set_turn_recorder`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    /// The turns the tasks of the script have taken.
    pub turn: u64,
    /// Whether the script waited there: no task of it was woken.
    pub waited: bool,
}

/// The result of an evaluated script.
#[derive(Debug, Default)]
pub struct LineResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: u8,
    pub cwd: String,
}

/// One execution-only shell. All jobs must finish before an invocation returns.
pub struct Session {
    shell: Shell,
    commands: Arc<InvocationCommands>,
    /// Whether a script runs as a `bash -c` string (see [`Self::read_scripts_as_input`]).
    command_string: bool,
    /// How long a call's script may run (see [`Session::set_time_limit`]).
    #[cfg_attr(
        not(target_arch = "wasm32"),
        allow(
            dead_code,
            reason = "only wasm32 runs a call's script as a process to stop"
        )
    )]
    time_limit: Option<std::time::Duration>,
    /// The caller's request to stop the script (see [`Session::set_stop_request`]).
    #[cfg_attr(
        not(target_arch = "wasm32"),
        allow(
            dead_code,
            reason = "only wasm32 runs a call's script as a process to stop"
        )
    )]
    stop_request: Option<futures::future::LocalBoxFuture<'static, ()>>,
    /// Keeps the place of each step of a stop for a second run of the call (see
    /// [`Session::set_turn_recorder`]).
    #[cfg_attr(
        not(target_arch = "wasm32"),
        allow(
            dead_code,
            reason = "only wasm32 runs a call's script as a process to stop"
        )
    )]
    turn_recorder: Option<TurnRecorder>,
}

/// Records a place of the script, or answers the place recorded in the first run of the call.
type TurnRecorder = Box<dyn FnMut(Place) -> Place>;

impl Session {
    /// Construct a native shell using the caller's Tokio runtime.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new() -> Result<Self, brush_core::Error> {
        Self::build().await
    }

    /// Install cooperative execution services before any startup code can execute.
    #[cfg(target_arch = "wasm32")]
    pub async fn new_with_execution_services(
        services: crate::ExecutionServices,
    ) -> Result<Self, brush_core::Error> {
        Self::build(services).await
    }

    async fn build(
        #[cfg(target_arch = "wasm32")] services: crate::ExecutionServices,
    ) -> Result<Self, brush_core::Error> {
        // A crashing embedded utility's own panic message must survive the trap that follows it
        // (wasm32-wasip2's panic strategy is `abort`, so nothing after the panic hook runs); see
        // `tools::coreutils::install_panic_hook`.
        #[cfg(target_arch = "wasm32")]
        crate::tools::coreutils::install_panic_hook();
        // `> /dev/stdout` onto a file opens that file again, as on Linux (see `devices`).
        #[cfg(target_arch = "wasm32")]
        brush_core::openfiles::set_reopener(crate::tools::devices::reopen_as);
        let builder = Shell::builder();
        #[cfg(target_arch = "wasm32")]
        let builder = builder.execution_services(turns::counted(services));
        let shell = builder
            // `$0` and `$BASH`: a name `bash -c` and `"$BASH" -c` both resolve to this shell.
            .shell_name("bash".to_owned())
            // Every script is a command string, as with `bash -c`: `$-` includes `c`.
            .command_string_mode(true)
            .default_builtins(BuiltinSet::BashMode)
            .builtins(crate::tools::coreutils::builtins())
            .builtins(crate::tools::texttools::builtins())
            .builtins(crate::tools::which::builtins())
            .builtins(crate::tools::man::builtins())
            .builtins(crate::tools::stat::builtins())
            .builtins(crate::tools::install::builtins())
            .builtins(crate::tools::find::builtins())
            .builtins(crate::tools::xargs::builtins())
            .builtins(crate::tools::timeout::builtins())
            .build()
            .await?;
        let mut session = Self {
            shell,
            commands: Arc::new(InvocationCommands::default()),
            command_string: true,
            time_limit: None,
            stop_request: None,
            turn_recorder: None,
        };
        session.set_identity()?;
        session.enable_stateless_mode();
        baseline::record(&session.shell);
        for name in ["sh", "bash"] {
            session
                .shell
                .register_builtin(name, crate::tools::sh::registration());
        }
        session
            .shell
            .register_builtin("curl", crate::commands::http_registration());
        session
            .shell
            .register_builtin("wget", crate::commands::http_registration());
        session.declare_programs();
        Ok(session)
    }

    /// The commands that stand for programs a Linux system keeps in `/bin` (see
    /// `tools::programs`): `/bin/cat f` runs `cat` as it would run the file there,
    /// `[ -x /bin/cat ]` holds, and `which cat`, `type cat` and `command -v cat` report
    /// `/bin/cat`.
    fn declare_programs(&mut self) {
        let programs: Vec<_> = self
            .shell
            .builtins()
            .keys()
            .filter_map(|name| Some((name.clone(), crate::tools::programs::kind(name)?)))
            .collect();
        self.shell.set_programs(programs);
    }

    /// Give the shell the identity bash would have for a user with no passwd entry. An agent has
    /// no user or parent process, so UID, EUID and PPID are fixed, unprivileged numbers; SHELL is
    /// bash's fallback for such a user. OSTYPE and MACHTYPE name Linux.
    fn set_identity(&mut self) -> Result<(), brush_core::Error> {
        use brush_core::{ShellValue, ShellVariable};
        for (name, value) in [("UID", "1000"), ("EUID", "1000"), ("PPID", "1")] {
            let mut var = ShellVariable::new(ShellValue::String(value.to_owned()));
            var.treat_as_integer().set_readonly();
            self.shell.env_mut().set_global(name, var)?;
        }
        if !self.shell.env().is_set("SHELL") {
            let var = ShellVariable::new(ShellValue::String("/bin/sh".to_owned()));
            self.shell.env_mut().set_global("SHELL", var)?;
        }
        // The embedded coreutils (`cut -c`, `expr length`/`substr`/`index`, `paste`, `fold -b`,
        // `ls`'s quoting, `md5sum -c`) each decide, once per invocation, whether to treat text
        // as UTF-8 or as single-byte "C" data, by reading `LC_ALL`/`LC_CTYPE`/`LANG` from their
        // own process environment (`uucore::i18n`). `coreutils::ProcessEnv` rebuilds that
        // environment from scratch before every embedded utility call, from this shell's own
        // *exported* variables (`exported_env`) — so a raw `std::env::set_var` here, before
        // `ProcessEnv` exists, is wiped before the first utility ever runs (confirmed: it
        // worked until the environment-per-call rework landed, then `expr length`/`md5sum -c`
        // silently went back to single-byte "C"). Setting it as an exported shell variable
        // instead flows through that same rebuild, and a script's own `export LC_ALL=...`
        // naturally overrides it (the two ways of setting a shell variable of the same name).
        if !self.shell.env().is_set("LC_ALL")
            && !self.shell.env().is_set("LC_CTYPE")
            && !self.shell.env().is_set("LANG")
        {
            let mut var = ShellVariable::new(ShellValue::String("C.UTF-8".to_owned()));
            var.export();
            self.shell.env_mut().set_global("LC_ALL", var)?;
        }
        // The shell and its commands behave as bash and its tools do on Linux, so a script that
        // branches on $OSTYPE takes its Linux path. The machine is honestly wasm32 (HOSTTYPE).
        const MACHTYPE: &str = "wasm32-unknown-linux-musl";
        for (name, value) in [("OSTYPE", "linux-musl"), ("MACHTYPE", MACHTYPE)] {
            let var = ShellVariable::new(ShellValue::String(value.to_owned()));
            self.shell.env_mut().set_global(name, var)?;
        }
        // BASH_VERSINFO[5] is the machine type too, and stays readonly.
        if let Some((_, var)) = self.shell.env().get("BASH_VERSINFO") {
            let mut values = var.value().element_values(&self.shell);
            if let Some(machine) = values.get_mut(5) {
                MACHTYPE.clone_into(machine);
                let values: Vec<&str> = values.iter().map(String::as_str).collect();
                let mut var = ShellVariable::new(ShellValue::indexed_array_from_strs(&values));
                var.set_readonly();
                self.shell.env_mut().set_global("BASH_VERSINFO", var)?;
            }
        }
        Ok(())
    }

    /// Register available commands; local builtins and existing names take precedence.
    /// Returns the names that were shadowed. Invalid names reject registration atomically.
    pub fn register_commands(
        &mut self,
        descriptors: Vec<CommandDescriptor>,
        invoker: Arc<dyn CommandInvoker>,
    ) -> Result<Vec<String>, String> {
        for descriptor in &descriptors {
            if descriptor.name.is_empty()
                || descriptor
                    .name
                    .chars()
                    .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
            {
                return Err(format!("invalid command name {:?}", descriptor.name));
            }
        }
        let mut commands = (*self.commands).clone();
        let mut shadowed = Vec::new();
        for descriptor in descriptors {
            if matches!(
                descriptor.name.as_str(),
                "if" | "then"
                    | "else"
                    | "elif"
                    | "fi"
                    | "case"
                    | "esac"
                    | "for"
                    | "select"
                    | "while"
                    | "until"
                    | "do"
                    | "done"
                    | "in"
                    | "function"
                    | "time"
                    | "coproc"
            ) || commands.contains(&descriptor.name)
            {
                shadowed.push(descriptor.name);
                continue;
            }
            // `bash` stays the local shell command; it hands `bash run …` to the bound tool.
            if crate::tools::sh::FORWARDS_TO_TOOL.contains(&descriptor.name.as_str()) {
                commands.insert(descriptor, invoker.clone());
                continue;
            }
            if self.shell.builtins().contains_key(&descriptor.name) {
                shadowed.push(descriptor.name);
                continue;
            }
            self.shell
                .register_builtin(descriptor.name.clone(), crate::commands::registration());
            commands.insert(descriptor, invoker.clone());
        }
        self.commands = Arc::new(commands);
        self.declare_programs();
        Ok(shadowed)
    }

    /// Register a raw local builtin for acceptance fixtures.
    #[cfg(feature = "test-support")]
    pub fn register_test_builtin(
        &mut self,
        name: &str,
        registration: brush_core::builtins::Registration<
            brush_core::extensions::DefaultShellExtensions,
        >,
    ) {
        self.shell.register_builtin(name, registration);
        self.declare_programs();
    }

    /// Stops the script once `limit` has passed, as `timeout(1)` stops a command: TERM to the
    /// script and everything it started, KILL a second later; the call then returns with status
    /// 124 and `bash: the call exceeded its N s time limit` on stderr. The script's leftover jobs
    /// are stopped as at any other end. Without a limit a call runs until its script ends.
    pub fn set_time_limit(&mut self, limit: std::time::Duration) {
        self.time_limit = Some(limit);
    }

    /// Stops the script when `stop` completes: INT to the script and all it started, KILL a
    /// second later, and the run returns exit code 130. Without it nothing but the time limit
    /// stops a script.
    pub fn set_stop_request(&mut self, stop: impl Future<Output = ()> + 'static) {
        self.stop_request = Some(Box::pin(stop));
    }

    /// Makes a stop act on the script at the same place in every run of a call. A caller that
    /// runs a call again from its records gives the script the time limit or the stop request
    /// earlier than in the first run, at a place that depends on timing. The script's place is
    /// the number of turns its tasks have taken (a task takes a turn each time it is woken, and
    /// runs until it waits or gives way), and whether it waits. At each step of a stop (TERM or
    /// INT, KILL, and the end of the wait for a script that outlives KILL) `recorder` gets that
    /// [`Place`]. It keeps the place in the first run and returns it; in a later run it returns
    /// the place it kept, and the step waits until the tasks have taken that many turns. A
    /// script that is one task and waits earlier is stopped where it waits, if the first run
    /// waited at the step too. A later run fails the call when its script is woken from outside
    /// on its way to the recorded turn, or is not there as the record says. Without a recorder
    /// each step acts where the script is.
    pub fn set_turn_recorder(&mut self, recorder: impl FnMut(Place) -> Place + 'static) {
        self.turn_recorder = Some(Box::new(recorder));
    }

    /// Sets `$0`, as `bash -c SCRIPT NAME` does.
    pub fn set_shell_name(&mut self, name: &str) {
        self.shell.set_shell_name(name);
    }

    /// Runs scripts as bash runs one it reads from standard input or a file, rather than as a
    /// `bash -c` string: its last command naming a program then runs as a child of the shell,
    /// not in its place, so the program sees the shell's own `SHLVL`.
    pub const fn read_scripts_as_input(&mut self) {
        self.command_string = false;
    }

    pub fn cwd(&self) -> &Path {
        self.shell.working_dir()
    }

    /// Continues a session's process numbering (`$$` and the next job number).
    pub fn seed_process_ids(&mut self, shell_pid: i32, next_pid: i32) {
        self.shell.processes().seed(shell_pid, next_pid);
    }

    /// Start the shell in `dir`, as if it had been launched there: `PWD` names it, and `OLDPWD`
    /// keeps what a new shell has, since no `cd` happened.
    ///
    /// # Errors
    /// The reason `dir` cannot be a starting directory, as [`check_working_dir`] words it.
    pub fn set_working_dir(&mut self, dir: &str) -> Result<(), String> {
        check_working_dir(dir)?;
        let oldpwd = self.shell.env().get("OLDPWD").map(|(_, var)| var.clone());
        self.shell
            .set_working_dir(dir)
            .map_err(|error| format!("cwd {dir}: {error}"))?;
        let restored = match oldpwd {
            Some(var) => self.shell.env_mut().set_global("OLDPWD", var),
            None => self.shell.env_mut().unset_raw("OLDPWD").map(|_| ()),
        };
        restored.map_err(|error| format!("cwd {dir}: {error}"))
    }

    /// Validate finite execution before running any part of a script.
    pub async fn run(&mut self, script: &str) -> LineResult {
        // Here-documents left open run to the end of the input, with bash's warning.
        let end_line = script.lines().count() + 1;
        let closed = stateless::close_here_documents(script);
        let (script, warnings) = closed.as_ref().map_or((script, ""), |(script, warnings)| {
            (script.as_str(), warnings.as_str())
        });
        if let Err((error, status)) = stateless::validate_with_status(script, "-c") {
            log::warn!("refused: {error}");
            self.shell.set_last_exit_status(status);
            // Bash warns as it reads the here-document, before it reaches a syntax error.
            let mut stderr = stateless::diagnostic("bash", warnings);
            if closed.is_some() {
                stderr.push_str(&stateless::diagnostic(
                    "bash",
                    &stateless::within_script(&error, end_line),
                ));
            } else {
                stderr.push_str(&stateless::diagnostic("bash", &error));
            }
            return LineResult {
                stderr: stderr.into_bytes(),
                exit_code: status,
                cwd: self.cwd().display().to_string(),
                ..Default::default()
            };
        }
        self.execute_program(script, warnings).await
    }

    async fn evaluate(
        &mut self,
        script: &str,
        here_document_warnings: &str,
        params: &brush_core::ExecutionParameters,
    ) -> Result<brush_core::ExecutionResult, brush_core::Error> {
        // The script runs as `bash -c` would: in a command-string frame, which is where LINENO,
        // the call stack and `$-`'s `c` come from.
        self.shell.start_command_string_mode();
        let result = if here_document_warnings.is_empty() {
            // As in `bash -c`, a last command naming a program runs in place of the shell.
            if self.command_string {
                self.shell.exec_last_command();
            }
            self.shell
                .run_string(script.to_owned(), &SourceInfo::from("bash-tool"), params)
                .await
        } else {
            self.run_warning_of_here_documents(script, here_document_warnings, params)
                .await
        };
        let _ = self.shell.end_command_string_mode();
        // The script's end, or its `exit`, runs the EXIT trap, as `bash -c` does; an `exit`
        // inside the trap sets the final status.
        self.shell.exit_with_trap(result).await
    }

    /// Run a script whose last command holds here-documents that run to its end, printing
    /// bash's `warnings` for them where bash does. Bash reads a `-c` script a command at a time and
    /// warns as it reads the here-documents, so the commands before theirs have run and printed
    /// by then, and an `exit` among them means no warning.
    async fn run_warning_of_here_documents(
        &mut self,
        script: &str,
        warnings: &str,
        params: &brush_core::ExecutionParameters,
    ) -> Result<brush_core::ExecutionResult, brush_core::Error> {
        let Ok(mut program) = self.shell.parse_string(script) else {
            // A script that does not parse was refused before it got here.
            self.write_stderr(params, warnings);
            return self
                .shell
                .run_string(script.to_owned(), &SourceInfo::from("bash-tool"), params)
                .await;
        };
        let last = program
            .complete_commands
            .split_off(program.complete_commands.len().saturating_sub(1));
        if !program.complete_commands.is_empty() {
            let result = self.run_parsed_program(program, params).await;
            if !result.is_normal_flow() {
                return Ok(result);
            }
        }
        self.write_stderr(params, warnings);
        Ok(self
            .run_parsed_program(
                brush_parser::ast::Program {
                    complete_commands: last,
                },
                params,
            )
            .await)
    }

    /// Run a parsed program as `run_string` runs one: an error is reported and ends it.
    async fn run_parsed_program(
        &mut self,
        program: brush_parser::ast::Program,
        params: &brush_core::ExecutionParameters,
    ) -> brush_core::ExecutionResult {
        match self.shell.run_program(program, params).await {
            Ok(result) => result,
            Err(error) => {
                let _ = self
                    .shell
                    .display_error(&mut params.stderr(&self.shell), &error);
                let result = error.into_result(&self.shell);
                self.shell.set_last_exit_status(result.exit_code.into());
                result
            }
        }
    }

    /// Write bash's diagnostic `lines` to the run's standard error.
    fn write_stderr(&self, params: &brush_core::ExecutionParameters, lines: &str) {
        use std::io::Write as _;
        let _ = params
            .stderr(&self.shell)
            .write_all(stateless::diagnostic("bash", lines).as_bytes());
    }

    /// Point the shell's own descriptors 0–2 at this run's streams.
    ///
    /// They go in the shell's open files, not in the run's `ExecutionParameters`: a command
    /// looks up a descriptor in its parameters before the shell's table, and `exec 2>&1`
    /// updates only the shell's table. Descriptors set on the parameters would hide every
    /// `exec` redirection from the commands that follow it.
    fn install_stdio(&mut self, stdin: OpenFile, stdout: OpenFile, stderr: OpenFile) {
        self.shell.replace_open_files(
            [
                (OpenFiles::STDIN_FD, stdin),
                (OpenFiles::STDOUT_FD, stdout),
                (OpenFiles::STDERR_FD, stderr),
            ]
            .into_iter(),
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn execute_program(&mut self, script: &str, here_document_warnings: &str) -> LineResult {
        use std::io::{Read, Seek, SeekFrom};
        let captured = async {
            let mut stdout = tempfile::tempfile()?;
            let mut stderr = tempfile::tempfile()?;
            let stdin = tempfile::tempfile()?;
            let mut params = self.shell.default_exec_params();
            params.set_context(self.commands.clone());
            self.install_stdio(
                OpenFile::from(stdin),
                OpenFile::from(stdout.try_clone()?),
                OpenFile::from(stderr.try_clone()?),
            );
            let result = self.evaluate(script, here_document_warnings, &params).await;
            drop(params);
            stdout.seek(SeekFrom::Start(0))?;
            stderr.seek(SeekFrom::Start(0))?;
            let mut out = Vec::new();
            let mut err = Vec::new();
            stdout.read_to_end(&mut out)?;
            stderr.read_to_end(&mut err)?;
            Ok::<_, std::io::Error>((result, out, err))
        }
        .await;
        match captured {
            Ok((result, out, err)) => self.finish(result, out, err),
            Err(error) => self.finish(Err(error.into()), Vec::new(), Vec::new()),
        }
    }

    #[cfg(target_arch = "wasm32")]
    async fn execute_program(&mut self, script: &str, here_document_warnings: &str) -> LineResult {
        let stdout = Buffer::default();
        let stderr = Buffer::default();
        let mut params = self.shell.default_exec_params();
        params.set_context(self.commands.clone());
        self.install_stdio(
            brush_core::openfiles::from_bytes(Vec::new()),
            OpenFile::Stream(Box::new(stdout.clone())),
            OpenFile::Stream(Box::new(stderr.clone())),
        );
        // The script runs as process `$$`, so `kill $$` reaches it. Its background jobs run in
        // the call's job scope, apart from it: a job outlives the subshell, stage or child shell
        // that started it, and a signal that ends the script leaves its jobs to be stopped below.
        let table = self.shell.processes().clone();
        let services = self.shell.execution_services();
        let jobs = brush_core::execution::process::JobScope::new(&table);
        let dispositions =
            brush_core::execution::process::Dispositions::from_traps(self.shell.traps());
        let completed = std::cell::Cell::new(false);
        let time_limit = self.time_limit;
        // Taken before the script borrows the session for as long as it runs.
        let stop_request = self.stop_request.take();
        let turn_recorder = self.turn_recorder.take();
        turns::start();
        // Registered before its first turn, so a stop request that is already there ends it.
        let process = brush_core::execution::process::NumberedProcess::register(
            &table,
            table.shell_pid(),
            dispositions,
        );
        let script = turns::task(Box::pin(async {
            let result = process
                .run(async {
                    let result = self.evaluate(script, here_document_warnings, &params).await;
                    completed.set(true);
                    result
                })
                .await;
            // A signal ended the script before it finished: its EXIT trap still runs, as in bash.
            if !completed.get() {
                self.shell.exit_trap_after_signal(&result, &params).await;
            }
            result
        }));
        let stopped = std::cell::Cell::new(None);
        let missed = std::cell::Cell::new(None);
        let result = match (time_limit, stop_request) {
            (None, None) => script.await,
            (limit, request) => {
                let watchdog = stop_when_due(
                    &table,
                    services,
                    limit,
                    request,
                    turn_recorder,
                    &stopped,
                    &missed,
                );
                // The watchdog is polled first, so its waits start before anything the script
                // does. Once one is due, the watchdog acts before the script runs further. A
                // run made again from the call's record learns of it earlier in the script, and
                // lets the script run on to the turn the first run recorded.
                match futures::future::select(std::pin::pin!(watchdog), std::pin::pin!(script))
                    .await
                {
                    // The script outlived its KILL and is abandoned; its status is set below.
                    futures::future::Either::Left(((), _)) => {
                        Ok(brush_core::ExecutionResult::new(TIMED_OUT))
                    }
                    futures::future::Either::Right((result, _)) => {
                        // The watchdog still waits for the script at a recorded turn: this is
                        // not the script that was stopped there.
                        if let Some(recorded) = turns::due() {
                            stopped.set(None);
                            missed.set(Some(Missed {
                                recorded,
                                turn: turns::now(),
                                script: "ended at",
                            }));
                        }
                        result
                    }
                }
            }
        };
        // The script is over: what gives a turn from here on is not held at one.
        turns::release();
        let result = match stopped.get() {
            Some(Stop::TimeLimit) => {
                let seconds = time_limit.unwrap_or_default().as_secs_f64();
                log::warn!("the call exceeded its {seconds} s time limit");
                Ok(brush_core::ExecutionResult::new(TIMED_OUT))
            }
            Some(Stop::Request) => {
                log::info!("the call was stopped on its caller's request");
                Ok(brush_core::ExecutionResult::new(INTERRUPTED))
            }
            None => result,
        };
        // A stop that this run could not repeat fails the call, whatever the script returned.
        let missed = missed.get().map(|missed| missed.to_string());
        let result = match &missed {
            Some(missed) => {
                log::error!("{missed}");
                Ok(brush_core::ExecutionResult::new(MISSED))
            }
            None => result,
        };
        // The shell's descriptors close as it exits, so an output process substitution it made
        // one of them (`exec > >(tee log)`) sees the end of its input and finishes.
        self.shell.replace_open_files(std::iter::empty());
        let notes = self.stop_leftover_jobs().await;
        jobs.cancel_and_join().await;
        drop(params);
        let (out, out_truncated) = stdout.take_text();
        let (mut err, err_truncated) = stderr.take_text();
        if stopped.get() == Some(Stop::TimeLimit) {
            let seconds = time_limit.unwrap_or_default().as_secs_f64();
            err.extend_from_slice(
                format!("bash: the call exceeded its {seconds} s time limit\n").as_bytes(),
            );
        }
        if let Some(missed) = missed {
            err.extend_from_slice(format!("bash: {missed}\n").as_bytes());
        }
        for note in notes {
            err.extend_from_slice(note.as_bytes());
        }
        for (truncated, name) in [(out_truncated, "stdout"), (err_truncated, "stderr")] {
            if truncated {
                log::warn!("{name} exceeded the output limit");
                err.extend_from_slice(
                    format!(
                        "bash: {name} exceeds the {} MiB output limit of bash-tool; the rest was \
                         discarded\n",
                        MAX_OUTPUT_BYTES >> 20
                    )
                    .as_bytes(),
                );
            }
        }
        self.finish(result, out, err)
    }

    /// HUP every job still running, give handlers up to one second on the durable clock, then
    /// KILL the rest. That is every job of the call, wherever it started: in this shell (listed
    /// or disowned), or in a subshell or child shell that has since ended. Each is reported on
    /// one line: `hangup` if it ended during the grace period, `killed` if it needed KILL. A job
    /// this shell started keeps its job number.
    ///
    /// The grace is one timer, raced against the jobs ending.
    #[cfg(target_arch = "wasm32")]
    async fn stop_leftover_jobs(&mut self) -> Vec<String> {
        use brush_core::execution::process::{self, signals};
        let table = self.shell.processes().clone();
        let services = self.shell.execution_services();
        // Let the jobs settle before anything is stopped, so only jobs still running are
        // reported: an output process substitution ends once its input has closed; a job not
        // yet started begins, as a process bash starts runs at once (and installs its traps); a
        // job a signal has already ended (by `kill 0`, say) exits, as bash's does at once.
        for _ in 0..SETTLE_TURNS {
            if !table.running_job_entries().iter().any(|job| {
                job.substitution
                    || !process::process_started(&table, job.pid)
                    || process::process_terminating(&table, job.pid)
            }) {
                break;
            }
            (services.yield_now)().await;
        }
        let running = table.running_job_entries();
        for job in &running {
            process::signal_process_group(&table, job.pid, signals::HUP);
        }
        let mut remaining: Vec<i32> = running.iter().map(|job| job.pid).collect();
        let mut grace = None;
        loop {
            remaining.retain(|pid| process::process_exists(&table, *pid));
            if remaining.is_empty() {
                break;
            }
            let grace =
                grace.get_or_insert_with(|| (services.sleep)(std::time::Duration::from_secs(1)));
            let next_exit = futures::future::select_all(
                remaining
                    .iter()
                    .map(|pid| Box::pin(process::process_exited(&table, *pid))),
            );
            if let futures::future::Either::Right(_) =
                futures::future::select(next_exit, grace.as_mut()).await
            {
                break;
            }
        }
        let mut notes = Vec::new();
        for job in running {
            let how = if process::process_exists(&table, job.pid) {
                process::signal_process_group(&table, job.pid, signals::KILL);
                "killed"
            } else {
                "hangup"
            };
            let number = self
                .shell
                .jobs()
                .jobs
                .iter()
                .chain(self.shell.jobs().disowned())
                .find(|listed| listed.leader() == Some(job.pid))
                .map(|listed| format!("[{}] ", listed.id))
                .unwrap_or_default();
            let note = format!(
                "stopped job {number}(pid {}, {how}): {}",
                job.shown,
                one_line(&job.command)
            );
            log::warn!("{note}");
            notes.push(format!("bash: {note}\n"));
        }
        for job in &mut self.shell.jobs_mut().jobs {
            let _ = job.wait().await;
        }
        notes
    }

    fn finish(
        &mut self,
        result: Result<brush_core::ExecutionResult, brush_core::Error>,
        stdout: Vec<u8>,
        mut stderr: Vec<u8>,
    ) -> LineResult {
        let exit_code = match result {
            Ok(result) => result.exit_code.into(),
            Err(error) => {
                log::error!("{error}");
                stderr.extend_from_slice(format!("bash: {error}\n").as_bytes());
                brush_core::ExecutionExitCode::from(&error).into()
            }
        };
        self.shell.set_last_exit_status(exit_code);
        LineResult {
            stdout,
            stderr,
            exit_code,
            cwd: self.cwd().display().to_string(),
        }
    }
}

/// A call stopped at its time limit ends with this status, as a command `timeout(1)` stopped
/// does.
#[cfg(target_arch = "wasm32")]
const TIMED_OUT: u8 = 124;

/// The exit code of a script stopped on its caller's request, as a shell reports SIGINT.
#[cfg(target_arch = "wasm32")]
const INTERRUPTED: u8 = 130;

/// The exit code of a call whose stop could not be repeated at its recorded turn.
#[cfg(target_arch = "wasm32")]
const MISSED: u8 = 1;

/// Why the watchdog stopped the script.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    TimeLimit,
    Request,
}

/// A step of a stop that this run of a call cannot repeat where an earlier run recorded it:
/// what the `script` does at `turn` instead (see [`hold_at_recorded_turn`]), or that it "ended
/// at" a turn before the recorded one.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy)]
struct Missed {
    recorded: u64,
    turn: u64,
    script: &'static str,
}

#[cfg(target_arch = "wasm32")]
impl std::fmt::Display for Missed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            recorded,
            turn,
            script,
        } = self;
        write!(
            formatter,
            "the stop was recorded at turn {recorded}, and the script {script} turn {turn}"
        )
    }
}

/// Waits for the time limit or the caller's stop request, then stops the script (process `$$`)
/// and every process it started: TERM for the limit as `timeout(1)` does, INT for a request as
/// a terminal does, and KILL a second later. A script that still runs a second after that (its
/// EXIT trap, which runs after the signal) is abandoned when this ends. The limit's timer is
/// created first, then the request is polled, in every run alike; each second of grace is a
/// timer created only once the wait before it has ended.
///
/// With a `recorder`, each of the three steps (the signal, the KILL, the end) acts at a recorded
/// turn of the script. A step that cannot be repeated at its turn is `missed`, and this ends.
#[cfg(target_arch = "wasm32")]
async fn stop_when_due(
    table: &brush_core::process_table::ProcessTable,
    services: crate::ExecutionServices,
    limit: Option<std::time::Duration>,
    request: Option<futures::future::LocalBoxFuture<'static, ()>>,
    mut recorder: Option<TurnRecorder>,
    stopped: &std::cell::Cell<Option<Stop>>,
    missed: &std::cell::Cell<Option<Missed>>,
) {
    use brush_core::execution::process::{self, signals};
    use futures::future::Either;
    let timer = async {
        match limit {
            Some(limit) => (services.sleep)(limit).await,
            None => std::future::pending().await,
        }
    };
    let request = async {
        match request {
            Some(request) => request.await,
            None => std::future::pending().await,
        }
    };
    let cause = match futures::future::select(std::pin::pin!(timer), std::pin::pin!(request)).await
    {
        Either::Left(_) => Stop::TimeLimit,
        Either::Right(_) => Stop::Request,
    };
    stopped.set(Some(cause));
    let signal = match cause {
        Stop::TimeLimit => signals::TERM,
        Stop::Request => signals::INT,
    };
    let steps = async {
        hold_at_recorded_turn(&mut recorder).await?;
        process::signal_process_group(table, table.shell_pid(), signal);
        turns::release();
        (services.sleep)(std::time::Duration::from_secs(1)).await;
        hold_at_recorded_turn(&mut recorder).await?;
        process::signal_process_group(table, table.shell_pid(), signals::KILL);
        turns::release();
        (services.sleep)(std::time::Duration::from_secs(1)).await;
        hold_at_recorded_turn(&mut recorder).await
    };
    if let Err(step) = steps.await {
        stopped.set(None);
        missed.set(Some(step));
    }
}

/// Holds the script at the place where a step of its stop acts, until [`turns::release`].
/// Without a recorder that is where the script is. With one it is the place the recorder
/// answers: this place in the first run of a call, and in a later run the place of the first
/// run. A later run can learn of the step after fewer turns, so it lets the script run on to
/// the recorded turn. The caller acts and releases without waiting in between, so the script
/// meets the step exactly there.
///
/// The script must get to the recorded turn by itself, and be there as the record says:
/// waiting, or with a task woken. If the record says that it waited, a script that waits
/// earlier with only its own task alive meets the step where it waits. It took fewer turns to
/// get there, where a completion of its own cut a stretch of computing short, and the count
/// goes on from the recorded number.
///
/// # Errors
/// A later run cannot meet the step where the first run did when its script
/// - is already past the recorded turn ("is at");
/// - is woken from outside between the question to the recorder and the step ("was woken
///   at"): a completion that the first run got only after the step would change what the
///   script does before it;
/// - waits where the first run's did not, at the recorded turn or before it, or waits before
///   the recorded turn with several tasks alive, where the place of each task in the first
///   run cannot be told ("waits at");
/// - still runs at the recorded turn, where the first run's waited ("runs at").
#[cfg(target_arch = "wasm32")]
async fn hold_at_recorded_turn(recorder: &mut Option<TurnRecorder>) -> Result<(), Missed> {
    let Some(recorder) = recorder else {
        return Ok(());
    };
    let place = Place {
        turn: turns::now(),
        waited: turns::waits(),
    };
    let wakes = turns::wakes_from_outside();
    let recorded = recorder(place);
    if recorded.turn < place.turn {
        return Err(Missed {
            recorded: recorded.turn,
            turn: place.turn,
            script: "is at",
        });
    }
    let script = match turns::reach(recorded.turn, recorded.waited, wakes).await {
        turns::Reached::There if recorded.waited == turns::waits() => return Ok(()),
        turns::Reached::There if recorded.waited => "runs at",
        turns::Reached::There | turns::Reached::Waits => "waits at",
        turns::Reached::Woken => "was woken at",
    };
    Err(Missed {
        recorded: recorded.turn,
        turn: turns::now(),
        script,
    })
}

/// Turns the jobs get to settle once the script has ended, before the rest are stopped.
#[cfg(target_arch = "wasm32")]
const SETTLE_TURNS: usize = 64;

/// A command's text on one line: each line trimmed, joined by spaces.
#[cfg(target_arch = "wasm32")]
fn one_line(command: &str) -> String {
    command
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The most of each output stream a call returns, counted as the text the caller receives. Golem
/// carries a result, and each page of `golem agent oplog`, in one gRPC message of at most 32 MiB.
/// The owner's oplog records a result twice, and a page of 50 entries can hold two calls, so it
/// carries up to four results; at this size that stays at half the limit.
#[cfg(target_arch = "wasm32")]
const MAX_OUTPUT_BYTES: usize = 2 << 20;

/// One of the call's output streams. Past [`MAX_OUTPUT_BYTES`] it refuses writes as a pipe whose
/// reader has gone does, so the writer gets SIGPIPE, as `bash -c` does when its caller stops
/// reading; the call then says what was cut.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Default)]
struct Buffer(Arc<std::sync::Mutex<(Vec<u8>, bool)>>);
#[cfg(target_arch = "wasm32")]
impl Buffer {
    /// The bytes kept, and whether any were refused.
    fn take(&self) -> (Vec<u8>, bool) {
        std::mem::take(
            &mut *self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// The bytes kept, cut so that their text is at most [`MAX_OUTPUT_BYTES`], and whether any
    /// were refused or cut. The call's result converts them as `String::from_utf8_lossy` does,
    /// so a sequence that is not UTF-8 becomes the three bytes of U+FFFD.
    fn take_text(&self) -> (Vec<u8>, bool) {
        let (mut bytes, refused) = self.take();
        let mut text = 0;
        let mut kept = 0;
        for chunk in bytes.utf8_chunks() {
            let valid = chunk.valid();
            let replacement = if chunk.invalid().is_empty() {
                0
            } else {
                char::REPLACEMENT_CHARACTER.len_utf8()
            };
            if text + valid.len() + replacement > MAX_OUTPUT_BYTES {
                let end = kept + valid.floor_char_boundary(MAX_OUTPUT_BYTES - text);
                bytes.truncate(end);
                return (bytes, true);
            }
            text += valid.len() + replacement;
            kept += valid.len() + chunk.invalid().len();
        }
        (bytes, refused)
    }
}
#[cfg(target_arch = "wasm32")]
impl std::io::Read for Buffer {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Ok(0)
    }
}
#[cfg(target_arch = "wasm32")]
impl std::io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut buffer = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let room = MAX_OUTPUT_BYTES.saturating_sub(buffer.0.len());
        if room == 0 && !bytes.is_empty() {
            buffer.1 = true;
            drop(buffer);
            brush_core::execution::process::record_broken_pipe();
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        let count = bytes.len().min(room);
        buffer.0.extend_from_slice(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[cfg(target_arch = "wasm32")]
impl brush_core::openfiles::Stream for Buffer {
    fn clone_box(&self) -> Box<dyn brush_core::openfiles::Stream> {
        Box::new(self.clone())
    }
    fn target_id(&self) -> Option<usize> {
        Some(Arc::as_ptr(&self.0).addr())
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod end_of_run_tests {
    use std::{cell::RefCell, rc::Rc, time::Duration};

    use futures::future::LocalBoxFuture;

    use super::{Place, Session};

    thread_local! {
        /// Every durable sleep the shell asked for.
        static SLEEPS: RefCell<Vec<Duration>> = const { RefCell::new(Vec::new()) };
    }

    fn counting_sleep(duration: Duration) -> LocalBoxFuture<'static, ()> {
        SLEEPS.with_borrow_mut(|sleeps| sleeps.push(duration));
        Box::pin(tokio::time::sleep(duration))
    }

    /// Runs `script` in a fresh session; returns its stderr and the sleeps it asked for.
    fn run(script: &str) -> (String, Vec<Duration>) {
        SLEEPS.with_borrow_mut(Vec::clear);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let stderr = runtime.block_on(tokio::task::LocalSet::new().run_until(async {
            let services = crate::ExecutionServices {
                sleep: counting_sleep,
                ..crate::ExecutionServices::default()
            };
            let mut session = Session::new_with_execution_services(services)
                .await
                .unwrap();
            let result = session.run(script).await;
            String::from_utf8(result.stderr).unwrap()
        }));
        (stderr, SLEEPS.with_borrow(Clone::clone))
    }

    /// The grace for leftover jobs is one timer, however long their HUP handlers compute.
    #[test]
    fn leftover_jobs_get_one_grace_timer_whatever_their_handlers_do() {
        let (stderr, sleeps) = run(
            "(trap 'for i in $(seq 300); do :; done; exit 0' HUP; while :; do :; done) & echo main",
        );
        assert!(stderr.contains(", hangup): "), "{stderr}");
        assert_eq!(sleeps, [Duration::from_secs(1)]);

        let (stderr, sleeps) = run("trap '' HUP; (while :; do :; done) & echo main");
        assert!(stderr.contains(", killed): "), "{stderr}");
        assert_eq!(sleeps, [Duration::from_secs(1)]);

        // One job ends on HUP after computing while another ignores it.
        let (stderr, sleeps) = run(
            "(trap 'for i in $(seq 300); do :; done; exit 0' HUP; while :; do :; done) & \
             trap '' HUP; (while :; do :; done) & echo main",
        );
        assert!(stderr.contains("[1] (pid 2, hangup): "), "{stderr}");
        assert!(stderr.contains("[2] (pid 3, killed): "), "{stderr}");
        assert_eq!(sleeps, [Duration::from_secs(1)]);

        // No leftover job, no timer.
        let (stderr, sleeps) = run("true & wait; echo main");
        assert_eq!(stderr, "");
        assert!(sleeps.is_empty());
    }

    /// Runs `script` with a time limit; returns its status, stdout, stderr and sleeps.
    fn run_limited(script: &str, limit: Duration) -> (u8, String, String, Vec<Duration>) {
        SLEEPS.with_borrow_mut(Vec::clear);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (code, stdout, stderr) =
            runtime.block_on(tokio::task::LocalSet::new().run_until(async {
                let services = crate::ExecutionServices {
                    sleep: counting_sleep,
                    ..crate::ExecutionServices::default()
                };
                let mut session = Session::new_with_execution_services(services)
                    .await
                    .unwrap();
                session.set_time_limit(limit);
                let result = session.run(script).await;
                (
                    result.exit_code,
                    String::from_utf8(result.stdout).unwrap(),
                    String::from_utf8(result.stderr).unwrap(),
                )
            }));
        (code, stdout, stderr, SLEEPS.with_borrow(Clone::clone))
    }

    const LIMIT: Duration = Duration::from_millis(200);

    /// A script still running at the limit is stopped with TERM, even a loop of commands that
    /// never wait; the call says so and ends with status 124, as `timeout(1)` does.
    #[test]
    fn a_script_past_its_time_limit_is_stopped() {
        for script in [
            "echo start; while :; do x=1; done",
            "echo start; while [[ 1 ]]; do x=$((x+1)); done",
            "echo start; sleep 30",
            "echo start; for ((;;)); do :; done",
        ] {
            let (code, stdout, stderr, sleeps) = run_limited(script, LIMIT);
            assert_eq!(code, 124, "{script}");
            assert_eq!(stdout, "start\n", "{script}");
            assert_eq!(
                stderr, "bash: the call exceeded its 0.2 s time limit\n",
                "{script}"
            );
            // The limit's timer comes first; the second of grace follows the TERM.
            assert_eq!(sleeps.first(), Some(&LIMIT), "{script}");
            assert_eq!(sleeps.last(), Some(&Duration::from_secs(1)), "{script}");
        }
    }

    /// TERM runs the script's handlers; one that ignores TERM is killed a second later, and one
    /// whose EXIT trap never ends is abandoned a second after that.
    #[test]
    fn a_script_that_resists_its_time_limit_is_killed() {
        let (code, stdout, stderr, _) = run_limited(
            "trap 'echo term; exit 3' TERM; trap 'echo exit' EXIT; while :; do :; done",
            LIMIT,
        );
        assert_eq!((code, stdout.as_str()), (124, "term\nexit\n"));
        assert_eq!(stderr, "bash: the call exceeded its 0.2 s time limit\n");

        let (code, _, stderr, sleeps) = run_limited("trap '' TERM; while :; do x=1; done", LIMIT);
        assert_eq!(code, 124);
        assert_eq!(stderr, "bash: the call exceeded its 0.2 s time limit\n");
        assert_eq!(sleeps[..2], [LIMIT, Duration::from_secs(1)]);

        let (code, _, stderr, sleeps) = run_limited(
            "trap 'while :; do x=1; done' EXIT; while :; do :; done",
            LIMIT,
        );
        assert_eq!(code, 124);
        assert_eq!(stderr, "bash: the call exceeded its 0.2 s time limit\n");
        assert_eq!(
            sleeps,
            [LIMIT, Duration::from_secs(1), Duration::from_secs(1)]
        );
    }

    /// A script that ends in time is untouched: its own status, no note, only the limit's timer.
    #[test]
    fn a_script_within_its_time_limit_ends_as_usual() {
        let (code, stdout, stderr, sleeps) = run_limited("echo done; exit 7", LIMIT);
        assert_eq!((code, stdout.as_str(), stderr.as_str()), (7, "done\n", ""));
        assert_eq!(sleeps, [LIMIT]);
    }

    /// What a run with a stop request returned, the sleeps it asked for, and the turn it ended at.
    struct Outcome {
        code: u8,
        stdout: String,
        stderr: String,
        cwd: String,
        sleeps: Vec<Duration>,
        turn: u64,
    }

    /// Runs `script` with a stop request, and with a time limit if one is given.
    fn run_stopped(
        script: &str,
        stop: impl Future<Output = ()> + 'static,
        limit: Option<Duration>,
    ) -> Outcome {
        run_session(script, counting_sleep, |session| {
            if let Some(limit) = limit {
                session.set_time_limit(limit);
            }
            session.set_stop_request(stop);
        })
    }

    /// Runs `script` in a fresh session that `prepare` has set up and that waits with `sleep`.
    fn run_session(
        script: &str,
        sleep: fn(Duration) -> LocalBoxFuture<'static, ()>,
        prepare: impl FnOnce(&mut Session),
    ) -> Outcome {
        run_session_in(Tasks::Tokio, script, sleep, prepare)
    }

    /// What runs the tasks of a call in a test.
    #[derive(Clone, Copy, Debug)]
    enum Tasks {
        /// Tokio, which polls the call's own task before the tasks it spawned, on every pass.
        Tokio,
        /// One queue for the call's own task and the tasks it spawned, polled in the order they
        /// were woken, as the component's runtime polls them.
        OneQueue,
    }

    /// The component's runtime in small, as the conformance example has it.
    mod one_queue {
        use std::{cell::RefCell, rc::Rc, task::Poll};

        use futures::{
            future::LocalBoxFuture,
            stream::{FuturesUnordered, StreamExt},
        };

        thread_local! {
            static SPAWNED: RefCell<Vec<LocalBoxFuture<'static, ()>>> =
                const { RefCell::new(Vec::new()) };
        }

        pub(super) fn spawn(task: LocalBoxFuture<'static, ()>) {
            SPAWNED.with_borrow_mut(|spawned| spawned.push(task));
        }

        /// A completion that waits to be given, and when it is due.
        type Completion = (Box<dyn Fn() -> bool>, futures::channel::oneshot::Sender<()>);

        thread_local! {
            static COMPLETIONS: RefCell<Vec<Completion>> = const { RefCell::new(Vec::new()) };
        }

        /// A completion that comes from outside the script, as the end of a host call does.
        /// Once `due` holds, the queue gives it where a component's runtime gives one: when a
        /// pass over the woken tasks is over, one completion at a time, the one that was asked
        /// for first before the others.
        pub(super) fn completion(due: impl Fn() -> bool + 'static) -> LocalBoxFuture<'static, ()> {
            let (give, given) = futures::channel::oneshot::channel();
            COMPLETIONS.with_borrow_mut(|completions| completions.push((Box::new(due), give)));
            Box::pin(async move {
                let _ = given.await;
            })
        }

        fn give_a_completion() -> bool {
            let due = COMPLETIONS.with_borrow_mut(|completions| {
                let first = completions.iter().position(|(due, _)| due())?;
                Some(completions.remove(first).1)
            });
            due.is_some_and(|give| {
                let _ = give.send(());
                true
            })
        }

        /// Gives way once: the task is woken at once and runs on at its next poll.
        pub(super) fn give_way() -> LocalBoxFuture<'static, ()> {
            let mut gave = false;
            Box::pin(std::future::poll_fn(move |context| {
                if gave {
                    Poll::Ready(())
                } else {
                    gave = true;
                    context.waker().wake_by_ref();
                    Poll::Pending
                }
            }))
        }

        /// Runs `main` to its end among the tasks spawned meanwhile, all in one queue.
        pub(super) async fn run<'a, T: 'a>(main: impl Future<Output = T> + 'a) -> T {
            SPAWNED.with_borrow_mut(Vec::clear);
            COMPLETIONS.with_borrow_mut(Vec::clear);
            let result = Rc::new(RefCell::new(None));
            let slot = Rc::clone(&result);
            let mut tasks: FuturesUnordered<LocalBoxFuture<'a, ()>> = FuturesUnordered::new();
            tasks.push(Box::pin(async move {
                let value = main.await;
                *slot.borrow_mut() = Some(value);
            }));
            std::future::poll_fn(move |context| {
                loop {
                    let poll = tasks.poll_next_unpin(context);
                    let spawned = SPAWNED.with_borrow_mut(std::mem::take);
                    let any_spawned = !spawned.is_empty();
                    tasks.extend(spawned);
                    if let Some(value) = result.borrow_mut().take() {
                        return Poll::Ready(value);
                    }
                    if !(poll.is_ready() || any_spawned) {
                        if give_a_completion() {
                            continue;
                        }
                        return Poll::Pending;
                    }
                }
            })
            .await
        }
    }

    /// Runs `script` as [`run_session`] does, with its tasks run by `tasks`.
    fn run_session_in(
        tasks: Tasks,
        script: &str,
        sleep: fn(Duration) -> LocalBoxFuture<'static, ()>,
        prepare: impl FnOnce(&mut Session),
    ) -> Outcome {
        SLEEPS.with_borrow_mut(Vec::clear);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let call = |services| async move {
            let mut session = Session::new_with_execution_services(services)
                .await
                .unwrap();
            prepare(&mut session);
            let result = session.run(script).await;
            (result, super::turns::now())
        };
        let (result, turn) = match tasks {
            Tasks::Tokio => runtime.block_on(tokio::task::LocalSet::new().run_until(call(
                crate::ExecutionServices {
                    sleep,
                    ..crate::ExecutionServices::default()
                },
            ))),
            Tasks::OneQueue => runtime.block_on(one_queue::run(call(crate::ExecutionServices {
                spawn_local: one_queue::spawn,
                yield_now: one_queue::give_way,
                sleep,
            }))),
        };
        Outcome {
            code: result.exit_code,
            stdout: String::from_utf8(result.stdout).unwrap(),
            stderr: String::from_utf8(result.stderr).unwrap(),
            cwd: result.cwd,
            sleeps: SLEEPS.with_borrow(Clone::clone),
            turn,
        }
    }

    /// A stop request that comes `delay` after the script starts.
    async fn stop_after(delay: Duration) {
        tokio::time::sleep(delay).await;
    }

    /// A stop request that comes once `due` holds. It is tied to what the script has done, not
    /// to the clock, so it comes at the same place however slowly the test runs.
    fn once(due: impl Fn() -> bool) -> impl Future<Output = ()> {
        std::future::poll_fn(move |context| {
            if due() {
                std::task::Poll::Ready(())
            } else {
                context.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        })
    }

    const SECOND: Duration = Duration::from_secs(1);

    /// A stop request ends a script that waits, with INT: status 130, as a shell reports SIGINT,
    /// what the script wrote so far, and no note on stderr.
    #[test]
    fn a_stop_request_ends_a_waiting_script() {
        let run = run_stopped(
            "echo start; sleep 300; echo end",
            once_it_waits(ASLEEP),
            None,
        );
        assert_eq!(
            (run.code, run.stdout.as_str(), run.stderr.as_str()),
            (130, "start\n", "")
        );
    }

    /// It reaches a loop of commands that never wait too, as the time limit does.
    #[test]
    fn a_stop_request_ends_a_script_that_computes() {
        for script in [
            "echo start; while :; do x=1; done",
            "echo start; while [[ 1 ]]; do x=$((x+1)); done",
            "echo start; for ((;;)); do :; done",
        ] {
            // The script has printed in its first turn and is in its loop by the third.
            let run = run_stopped(script, at_turn(3), None);
            assert_eq!(run.code, 130, "{script}");
            assert_eq!(run.stdout, "start\n", "{script}");
            assert_eq!(run.stderr, "", "{script}");
            // No time limit, so the only timer is the second of grace that follows the INT.
            assert_eq!(run.sleeps, [SECOND], "{script}");
        }
    }

    /// A script that ignores INT is killed a second later, and the status is still 130.
    #[test]
    fn a_script_that_ignores_the_stop_is_killed() {
        let run = run_stopped(
            "trap '' INT; echo start; while :; do :; done",
            at_turn(3),
            None,
        );
        assert_eq!((run.code, run.stdout.as_str()), (130, "start\n"));
        assert_eq!(run.sleeps, [SECOND, SECOND]);
    }

    /// A script that catches INT runs its handler at once, also while a `sleep` is in front: the
    /// INT goes to everything the script started, as Ctrl+C at a terminal does, so the `sleep`
    /// ends and the handler follows it. The script ends by itself, before the KILL, and the
    /// status is 130 whatever the handler does.
    #[test]
    fn a_script_that_catches_the_stop() {
        let run = run_stopped(
            "trap 'echo caught' INT; echo start; sleep 300",
            once_it_waits(ASLEEP),
            None,
        );
        assert_eq!((run.code, run.stdout.as_str()), (130, "start\ncaught\n"));
        assert_eq!(run.sleeps, [Duration::from_secs(300), SECOND]);

        let run = run_stopped(
            "trap 'echo caught; exit 3' INT; echo start; sleep 300",
            once_it_waits(ASLEEP),
            None,
        );
        assert_eq!((run.code, run.stdout.as_str()), (130, "start\ncaught\n"));
    }

    /// A caller that never asks for a stop gets the script's own result.
    #[test]
    fn a_stop_request_that_never_comes_changes_nothing() {
        let run = run_stopped("echo a; echo b", std::future::pending(), None);
        assert_eq!(
            (run.code, run.stdout.as_str(), run.stderr.as_str()),
            (0, "a\nb\n", "")
        );
        assert!(run.sleeps.is_empty());
    }

    /// A stop request that comes after the script has ended changes nothing. The script leaves
    /// a job that ignores HUP, so the end of the run asks for its second of grace; the request
    /// comes once that second has been asked for. The run returns the script's own status and
    /// stops the job as after any other script: no INT, and no second of grace for one.
    #[test]
    fn a_stop_after_the_script_ended_changes_nothing() {
        let run = run_stopped(
            "(trap '' HUP; sleep 300) & echo done",
            once(|| asked_for(SECOND) >= 1),
            None,
        );
        assert_eq!((run.code, run.stdout.as_str()), (0, "done\n"));
        assert!(
            run.stderr
                .starts_with("bash: stopped job [1] (pid 2, killed): "),
            "{}",
            run.stderr
        );
        assert_eq!(run.stderr.lines().count(), 1, "{}", run.stderr);
        // The request was due before the run returned.
        assert_eq!(run.sleeps, [ASLEEP, SECOND]);
    }

    /// A stop request that is already there when the script starts ends it before it runs
    /// anything, as a signal ends a process that has not had its first turn.
    #[test]
    fn a_stop_before_the_script_starts_ends_it_at_once() {
        let run = run_stopped("echo start; sleep 300", std::future::ready(()), None);
        assert_eq!(
            (run.code, run.stdout.as_str(), run.stderr.as_str()),
            (130, "", "")
        );
        assert_eq!(run.sleeps, [SECOND]);
    }

    /// A stopped run still says where its script was, so the caller can continue there.
    #[test]
    fn a_stopped_script_reports_its_directory() {
        let run = run_stopped("cd /tmp; sleep 300", once_it_waits(ASLEEP), None);
        assert_eq!((run.code, run.cwd.as_str()), (130, "/tmp"));
    }

    /// A time limit and a stop request due at the same moment end as the time limit does. The
    /// limit's timer is created before the request is first polled, so it is due no later than a
    /// request with the same delay, and the limit is checked first.
    #[test]
    fn the_time_limit_wins_when_both_are_due() {
        let run = run_stopped("echo start; sleep 30", stop_after(LIMIT), Some(LIMIT));
        assert_eq!((run.code, run.stdout.as_str()), (124, "start\n"));
        assert_eq!(run.stderr, "bash: the call exceeded its 0.2 s time limit\n");
    }

    /// A background job ignores INT, as in bash, so the stop leaves it to the end of the run,
    /// which stops and reports it as after any other script.
    #[test]
    fn a_stop_leaves_background_jobs_to_the_end_of_run() {
        // The stop comes once the job and the script have each asked for their sleep and wait.
        let run = run_stopped(
            "sleep 300 & echo start; sleep 300",
            once(|| waits_in(ASLEEP, 2)),
            None,
        );
        assert_eq!((run.code, run.stdout.as_str()), (130, "start\n"));
        assert_eq!(
            run.stderr,
            "bash: stopped job [1] (pid 2, hangup): sleep 300\n"
        );
    }

    /// The turns a script has taken when the first run of a call in these tests is stopped.
    const STOPPED_AT: u64 = 60;
    /// The turns a second of grace lasts in such a first run.
    const GRACE_TURNS: u64 = 20;

    /// The watchdog's waits in the first run of a call in these tests. They are measured in
    /// turns of the script, not by the clock, so that the tests do not depend on the speed of
    /// the machine: the time limit is over when the script has taken [`STOPPED_AT`] turns, and
    /// a second of grace [`GRACE_TURNS`] turns after it began.
    fn first_run(duration: Duration) -> LocalBoxFuture<'static, ()> {
        let turn = if duration == LIMIT {
            STOPPED_AT
        } else if duration == SECOND {
            super::turns::now() + GRACE_TURNS
        } else {
            return counting_sleep(duration);
        };
        SLEEPS.with_borrow_mut(|sleeps| sleeps.push(duration));
        Box::pin(at_turn(turn))
    }

    thread_local! {
        /// The seconds of grace that ended in the first run of the call being replayed.
        static ENDED_SECONDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// The watchdog's waits in a replay. Golem gives a call it runs again every completion the
    /// first run got, as early as the call can take it, and no other. So the time limit is over
    /// at the watchdog's next poll, and so is each second of grace that ended in the first run:
    /// a call gets a completion when it next returns to the host, not in the poll that asked
    /// for it. A second that did not end in the first run does not end now. A `sleep` of the
    /// script lasts as long as it says.
    fn replayed(duration: Duration) -> LocalBoxFuture<'static, ()> {
        let ended = if duration == SECOND {
            let left = ENDED_SECONDS.get();
            ENDED_SECONDS.set(left.saturating_sub(1));
            left > 0
        } else {
            duration == LIMIT
        };
        if ended {
            SLEEPS.with_borrow_mut(|sleeps| sleeps.push(duration));
            // Pending once, woken at once: over at the next poll.
            one_queue::give_way()
        } else if duration == SECOND {
            SLEEPS.with_borrow_mut(|sleeps| sleeps.push(duration));
            Box::pin(std::future::pending())
        } else {
            counting_sleep(duration)
        }
    }

    /// Lets a loop run to its first turn. The interpreter counts commands between turns in one
    /// place for the whole process, and the count is zero there, as it is when Golem starts a
    /// call. Runs that follow this in one test process then give their turns at the same
    /// commands.
    fn start_as_a_new_call() {
        let run = run_stopped("while :; do :; done", at_turn(1), None);
        // One turn to where the loop first gives way, and one in which it ends on the INT.
        assert_eq!((run.code, run.turn), (130, 2));
    }

    /// A stop request that comes once the script has asked for a wait of `duration` and waits:
    /// a task that was woken in the turn in which it asked takes one more turn first.
    fn once_it_waits(duration: Duration) -> impl Future<Output = ()> {
        std::future::poll_fn(move |context| {
            if SLEEPS.with_borrow(|sleeps| sleeps.contains(&duration)) && super::turns::waits() {
                std::task::Poll::Ready(())
            } else {
                context.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        })
    }

    /// A stop request that comes when the tasks of the script have taken `turn` turns.
    fn at_turn(turn: u64) -> impl Future<Output = ()> {
        std::future::poll_fn(move |context| {
            if super::turns::now() < turn {
                context.waker().wake_by_ref();
                std::task::Poll::Pending
            } else {
                std::task::Poll::Ready(())
            }
        })
    }

    /// How a call in these tests is stopped.
    #[derive(Clone, Copy)]
    enum By {
        Request,
        TimeLimit,
    }

    /// A stopped call and its replay, and the turns the first run recorded for its stop.
    ///
    /// The first run is stopped after [`STOPPED_AT`] turns, and records the turn of each step of
    /// the stop. The replay learns of every step as early as it can, long before the script is
    /// at that turn, and gets the turns of the first run from its recorder.
    fn stop_and_replay(script: &str, by: By) -> (Outcome, Outcome, Vec<Place>) {
        stop_and_replay_in(Tasks::OneQueue, script, by)
    }

    /// [`stop_and_replay`] with the tasks of both runs run by `tasks`.
    fn stop_and_replay_in(tasks: Tasks, script: &str, by: By) -> (Outcome, Outcome, Vec<Place>) {
        let recorded = Rc::new(RefCell::new(Vec::new()));
        start_as_a_new_call();
        let first = run_session_in(tasks, script, first_run, |session| {
            match by {
                By::Request => session.set_stop_request(at_turn(STOPPED_AT)),
                By::TimeLimit => session.set_time_limit(LIMIT),
            }
            let recorded = recorded.clone();
            session.set_turn_recorder(move |place| {
                recorded.borrow_mut().push(place);
                place
            });
        });
        let recorded = recorded.take();
        start_as_a_new_call();
        // Each step after the first followed a second of grace that ended.
        ENDED_SECONDS.set(recorded.len().saturating_sub(1));
        let replay = run_session_in(tasks, script, replayed, |session| {
            match by {
                By::Request => session.set_stop_request(std::future::ready(())),
                By::TimeLimit => session.set_time_limit(LIMIT),
            }
            let mut answers = recorded.clone().into_iter();
            session.set_turn_recorder(move |_| answers.next().unwrap());
        });
        (first, replay, recorded)
    }

    /// The place of a step that acted on a script that computed.
    fn computing_at(turn: u64) -> Place {
        Place {
            turn,
            waited: false,
        }
    }

    /// The place of a step that acted on a script that waited.
    fn waiting_at(turn: u64) -> Place {
        Place { turn, waited: true }
    }

    /// The two runs of a call gave the same result and ended at the same turn.
    fn assert_same_run(first: &Outcome, replay: &Outcome, script: &str) {
        assert_eq!(first.stdout, replay.stdout, "{script}");
        assert_eq!(first.stderr, replay.stderr, "{script}");
        assert_eq!(
            (first.code, first.turn),
            (replay.code, replay.turn),
            "{script}"
        );
    }

    /// A loop that prints as it counts: where it is stopped shows in what it printed.
    const PRINTING_LOOP: &str =
        "n=0; while :; do n=$((n+1)); if [ $((n % 10)) -eq 0 ]; then echo $n; fi; done";

    /// A replay learns of a stop request long before the script is where the first run was
    /// stopped. It stops the script at the recorded turn, so it prints the same lines.
    #[test]
    fn a_stop_request_is_repeated_at_its_recorded_turn() {
        let (first, replay, recorded) = stop_and_replay(PRINTING_LOOP, By::Request);
        assert_same_run(&first, &replay, PRINTING_LOOP);
        assert_eq!(first.code, 130);
        assert!(first.stdout.lines().count() > 1, "{}", first.stdout);
        // One step, taken while the script computed: the script ended on the INT.
        assert_eq!(recorded, [computing_at(STOPPED_AT)]);

        // The same for a replay that learns of the stop once the script has begun to run.
        start_as_a_new_call();
        let later = run_session(PRINTING_LOOP, replayed, |session| {
            session.set_stop_request(at_turn(1));
            session.set_turn_recorder(move |_| recorded[0]);
        });
        assert_same_run(&first, &later, PRINTING_LOOP);
    }

    /// The time limit is repeated at its recorded turn as a stop request is.
    #[test]
    fn the_time_limit_is_repeated_at_its_recorded_turn() {
        let (first, replay, recorded) = stop_and_replay(PRINTING_LOOP, By::TimeLimit);
        assert_same_run(&first, &replay, PRINTING_LOOP);
        assert_eq!(first.code, 124);
        assert_eq!(
            first.stderr,
            "bash: the call exceeded its 0.2 s time limit\n"
        );
        assert!(first.stdout.lines().count() > 1, "{}", first.stdout);
        assert_eq!(recorded, [computing_at(STOPPED_AT)]);
    }

    /// A script that ignores the first signal runs on to the KILL, which a replay repeats at
    /// its own recorded turn.
    #[test]
    fn a_kill_is_repeated_at_its_recorded_turn() {
        for (by, code) in [(By::Request, 130), (By::TimeLimit, 124)] {
            let script = format!("trap '' INT TERM; {PRINTING_LOOP}");
            let (first, replay, recorded) = stop_and_replay(&script, by);
            assert_same_run(&first, &replay, &script);
            assert_eq!(first.code, code);
            // Two steps: the signal the script ignores, and later the KILL that ends it.
            assert_eq!(
                recorded,
                [
                    computing_at(STOPPED_AT),
                    computing_at(STOPPED_AT + GRACE_TURNS)
                ]
            );
        }
    }

    /// A handler runs where the signal arrives, so what it prints shows the turn: the same in a
    /// replay, also when the handler itself computes. The script runs on after it, to the KILL.
    #[test]
    fn a_handler_runs_at_the_same_turn_in_a_replay() {
        for handler in [
            "echo caught at $n",
            "for i in $(seq 200); do :; done; echo caught at $n",
        ] {
            let script = format!("trap '{handler}' INT; {PRINTING_LOOP}");
            let (first, replay, recorded) = stop_and_replay(&script, By::Request);
            assert_same_run(&first, &replay, &script);
            assert_eq!(first.code, 130);
            assert_eq!(first.stdout.matches("caught at ").count(), 1, "{script}");
            assert_eq!(recorded.len(), 2, "{recorded:?}");
        }
    }

    /// The tasks of a script take the same turns in a replay for scripts of other shapes: a
    /// subshell, a command substitution, a pipeline, a job in the background that computes, a
    /// child shell, a function, and an EXIT trap that never ends, which the watchdog abandons at
    /// a third recorded turn. This holds with either way to run the tasks. With Tokio the
    /// watchdog always runs before the next task, so only the next test shows that a replay
    /// lets no task take a turn more.
    #[test]
    fn scripts_of_several_shapes_are_stopped_at_their_recorded_turn() {
        for tasks in [Tasks::Tokio, Tasks::OneQueue] {
            for (script, steps) in several_shapes() {
                let (first, replay, recorded) = stop_and_replay_in(tasks, &script, By::Request);
                assert_same_run(&first, &replay, &script);
                assert_eq!(first.code, 130, "{tasks:?} {script}");
                assert!(
                    first.stdout.lines().count() > 1,
                    "{tasks:?} {script}: {}",
                    first.stdout
                );
                assert_eq!(recorded.len(), steps, "{tasks:?} {script}: {recorded:?}");
            }
        }
    }

    /// Scripts of several shapes, and the steps a stop of each takes.
    fn several_shapes() -> Vec<(String, usize)> {
        vec![
            (format!("({PRINTING_LOOP})"), 1),
            (
                "n=0; while :; do n=$((n+1)); x=$(echo $n); if [ $((n % 10)) -eq 0 ]; then echo $x; \
                 fi; done"
                    .to_owned(),
                1,
            ),
            (
                "n=0; while :; do n=$((n+1)); echo $n; done | while read -r x; do \
                 if [ $((x % 10)) -eq 0 ]; then echo got $x; fi; done"
                    .to_owned(),
                1,
            ),
            (
                "seq 1000000 | while read -r x; do if [ $((x % 10)) -eq 0 ]; then echo $x; fi; done"
                    .to_owned(),
                1,
            ),
            (format!("(while :; do :; done) & {PRINTING_LOOP}"), 1),
            (format!("bash -c '{PRINTING_LOOP}'"), 1),
            (format!("count() {{ {PRINTING_LOOP}; }}; count"), 1),
            (format!("trap '{PRINTING_LOOP}' EXIT; while :; do :; done"), 3),
        ]
    }

    /// Tasks that compute at the same time are stopped where the first run stopped them, also
    /// when the call's own task waits in one queue with them, as in the component: no task
    /// takes one more turn in a replay than it took in the first run.
    #[test]
    fn tasks_in_one_queue_are_stopped_at_their_recorded_turn() {
        // Two stages that both compute and print.
        let two_stages = "n=0; while :; do n=$((n+1)); echo $n; done | while read -r x; do \
                          if [ $((x % 10)) -eq 0 ]; then echo got $x; fi; done";
        for (script, by, steps) in [
            (two_stages.to_owned(), By::Request, 1),
            (two_stages.to_owned(), By::TimeLimit, 1),
            // The stages ignore the first signal and run on to the KILL.
            (format!("trap '' INT; {two_stages}"), By::Request, 2),
            // Three stages.
            (
                "n=0; while :; do n=$((n+1)); echo $n; done | while read -r x; do echo $((x * 2)); \
                 done | while read -r y; do if [ $((y % 20)) -eq 0 ]; then echo got $y; fi; done"
                    .to_owned(),
                By::Request,
                1,
            ),
            // A stage that waits for its input more often than it gives way.
            (
                "n=0; while :; do n=$((n+1)); for i in 1 2 3 4 5 6 7 8 9; do :; done; echo $n; \
                 done | while read -r x; do echo got $x; done"
                    .to_owned(),
                By::Request,
                1,
            ),
            // A loop with a pipeline in each round.
            (
                "n=0; while :; do n=$((n+1)); echo $n | while read -r x; do echo got $x; done; done"
                    .to_owned(),
                By::Request,
                1,
            ),
            // A job in the background that computes and prints next to the script.
            (
                format!(
                    "(m=0; while :; do m=$((m+1)); if [ $((m % 10)) -eq 0 ]; then echo job $m; \
                     fi; done) & {PRINTING_LOOP}"
                ),
                By::Request,
                1,
            ),
            // The script alone, with a handler that prints where it runs.
            (
                format!("trap 'echo caught at $n' INT; {PRINTING_LOOP}"),
                By::Request,
                2,
            ),
        ] {
            let (first, replay, recorded) = stop_and_replay_in(Tasks::OneQueue, &script, by);
            assert_same_run(&first, &replay, &script);
            assert!(first.stdout.lines().count() > 1, "{script}: {}", first.stdout);
            assert_eq!(recorded.len(), steps, "{script}: {recorded:?}");
        }
    }

    /// A script that is one task and waits takes no turn, so a replay cannot bring it to a
    /// recorded turn that is ahead. It needs none when the record says that the first run's
    /// script waited at the step: the stop acts where the script waits, and the count goes on
    /// from the recorded number.
    #[test]
    fn a_lone_task_that_waits_below_its_recorded_turn_is_stopped_where_it_waits() {
        // The stop comes while the script waits.
        let run = run_session("echo start; sleep 300", counting_sleep, |session| {
            session.set_stop_request(once_it_waits(ASLEEP));
            session.set_turn_recorder(|place| waiting_at(place.turn + 1_000));
        });
        assert_eq!(
            (run.code, run.stdout.as_str(), run.stderr.as_str()),
            (130, "start\n", "")
        );
        assert_eq!(run.sleeps, [ASLEEP, SECOND]);
        // One turn to the `sleep`, so 1001 was recorded; one more turn to end on the INT.
        assert_eq!(run.turn, 1_002);

        // The stop comes while the script computes, and the script then begins to wait.
        let run = run_session(
            "for i in $(seq 300); do :; done; echo start; sleep 300",
            counting_sleep,
            |session| {
                session.set_stop_request(std::future::ready(()));
                session.set_turn_recorder(|_| waiting_at(1_000_000));
            },
        );
        assert_eq!(
            (run.code, run.stdout.as_str(), run.stderr.as_str()),
            (130, "start\n", "")
        );
        assert_eq!(run.turn, 1_000_001);
    }

    /// The same lone task is not stopped where it waits when the record says that the first run
    /// did not wait at the step: the first run was stopped while it computed, and this wait is
    /// one it was not in. The call fails and says so, before the recorded turn and at it.
    #[test]
    fn a_lone_task_that_waits_where_the_first_run_computed_fails_the_call() {
        for ahead in [1_000, 0] {
            let asked = Rc::new(std::cell::Cell::new(0));
            let run = run_session("echo start; sleep 300", counting_sleep, |session| {
                session.set_stop_request(once_it_waits(ASLEEP));
                let asked = asked.clone();
                session.set_turn_recorder(move |place| {
                    asked.set(place.turn);
                    computing_at(place.turn + ahead)
                });
            });
            let turn = asked.get();
            assert_eq!((run.code, run.stdout.as_str()), (1, "start\n"));
            assert_eq!(
                run.stderr,
                format!(
                    "bash: the stop was recorded at turn {}, and the script waits at turn {turn}\n",
                    turn + ahead
                )
            );
            // No signal was sent, so no second of grace was started.
            assert_eq!(run.sleeps, [ASLEEP]);
        }
    }

    /// With more tasks than its own alive, a script that waits below the recorded turn fails the
    /// call: where each task was in the first run cannot be told from where they wait now. The
    /// call fails at once and says so. It does not wait with the script.
    #[test]
    fn tasks_that_wait_below_their_recorded_turn_fail_the_call() {
        let asked = Rc::new(std::cell::Cell::new(0));
        let asked_at = Rc::new(std::cell::Cell::new(None));
        let run = run_session(
            "sleep 200 & echo start; sleep 300",
            counting_sleep,
            |session| {
                session.set_stop_request(once_it_waits(ASLEEP));
                let asked = asked.clone();
                let asked_at = asked_at.clone();
                session.set_turn_recorder(move |place| {
                    asked.set(place.turn);
                    asked_at.set(Some(std::time::Instant::now()));
                    waiting_at(place.turn + 1_000)
                });
            },
        );
        // From the question for the record to the end of the run, the call took far less time
        // than the shorter of the script's two sleeps: it did not wait with the script.
        let took = asked_at.get().unwrap().elapsed();
        assert!(took < Duration::from_secs(200) / 2, "{took:?}");
        assert_eq!((run.code, run.stdout.as_str()), (1, "start\n"));
        let turn = asked.get();
        // The line of the failure, then the note of the job the end of the run stopped.
        assert_eq!(
            run.stderr,
            format!(
                "bash: the stop was recorded at turn {}, and the script waits at turn {turn}\n\
                 bash: stopped job [1] (pid 2, hangup): sleep 200\n",
                turn + 1_000
            )
        );
        // No signal was sent to the script, so it got no second of grace. The one second is the
        // grace of the job at the end of the run.
        assert_eq!(run.sleeps, [ASLEEP, Duration::from_secs(200), SECOND]);
    }

    /// The time `timeout` gives its command in the tests below.
    const TIMEOUT: Duration = Duration::from_secs(7);
    /// How long the scripts of those tests sleep after it.
    const ASLEEP: Duration = Duration::from_secs(300);

    /// A stopped call and its replay, for a script whose own `timeout` ends a loop and that is
    /// stopped later, in its `sleep 300`. In the first run the timeout ends after 20 turns. In
    /// the replay it is over as early as it can be, as every completion the first run got, so
    /// the script gets to its sleep in fewer turns. With the two runs come the turns the first
    /// run recorded and the turns the replay was at when it asked for them.
    fn stop_at_a_wait_and_replay(script: &str) -> (Outcome, Outcome, Vec<Place>, Vec<Place>) {
        stop_at_a_wait_and_replay_with(script, first_run)
    }

    /// [`stop_at_a_wait_and_replay`], where the first run waits with `first` for all but the
    /// `timeout`.
    fn stop_at_a_wait_and_replay_with(
        script: &str,
        first: fn(Duration) -> LocalBoxFuture<'static, ()>,
    ) -> (Outcome, Outcome, Vec<Place>, Vec<Place>) {
        thread_local! {
            static FIRST: std::cell::Cell<fn(Duration) -> LocalBoxFuture<'static, ()>> =
                const { std::cell::Cell::new(counting_sleep) };
        }
        fn first_timeout(duration: Duration) -> LocalBoxFuture<'static, ()> {
            if duration == TIMEOUT {
                Box::pin(at_turn(super::turns::now() + 20))
            } else {
                FIRST.get()(duration)
            }
        }
        // Over once the command has taken a turn: a `timeout` whose time is over before its
        // command first runs does not reach the command with its signal.
        fn replayed_timeout(duration: Duration) -> LocalBoxFuture<'static, ()> {
            if duration == TIMEOUT {
                Box::pin(at_turn(super::turns::now() + 1))
            } else {
                replayed(duration)
            }
        }
        FIRST.set(first);
        let recorded = Rc::new(RefCell::new(Vec::new()));
        start_as_a_new_call();
        let first = run_session_in(Tasks::OneQueue, script, first_timeout, |session| {
            session.set_stop_request(once_it_waits(ASLEEP));
            let recorded = recorded.clone();
            session.set_turn_recorder(move |place| {
                recorded.borrow_mut().push(place);
                place
            });
        });
        let recorded = recorded.take();
        let asked = Rc::new(RefCell::new(Vec::new()));
        start_as_a_new_call();
        ENDED_SECONDS.set(recorded.len().saturating_sub(1));
        let replay = run_session_in(Tasks::OneQueue, script, replayed_timeout, |session| {
            session.set_stop_request(once_it_waits(ASLEEP));
            let mut answers = recorded.clone().into_iter();
            let asked = asked.clone();
            session.set_turn_recorder(move |place| {
                asked.borrow_mut().push(place);
                answers.next().unwrap()
            });
        });
        (first, replay, recorded, asked.take())
    }

    /// A script whose own timer ends a loop takes fewer turns in a replay, where that timer is
    /// over at once. A stop that came later, while the script waited, was recorded at a turn the
    /// replay does not get to. The script is one task and waits in the same `sleep` as in the
    /// first run, so the replay stops it there and gives the same result.
    #[test]
    fn a_script_that_gets_to_its_wait_in_fewer_turns_is_stopped_at_the_same_wait() {
        for script in [
            "timeout 7 bash -c 'while :; do :; done'; echo after; sleep 300",
            // A pipeline and a job that ended before: their tasks are not alive any more.
            "echo a | cat >/dev/null; true & wait; timeout 7 bash -c 'while :; do :; done'; \
             echo after; sleep 300",
        ] {
            let (first, replay, recorded, asked) = stop_at_a_wait_and_replay(script);
            assert_same_run(&first, &replay, script);
            assert_eq!(
                (first.code, first.stdout.as_str(), first.stderr.as_str()),
                (130, "after\n", ""),
                "{script}"
            );
            assert_eq!((recorded.len(), asked.len()), (1, 1), "{script}");
            assert!(
                recorded[0].turn > 20 && asked[0].turn < recorded[0].turn,
                "{script}: {recorded:?} {asked:?}"
            );
            assert!(
                recorded[0].waited && asked[0].waited,
                "{script}: {recorded:?} {asked:?}"
            );
        }
    }

    /// Such a script that catches the stop in its wait and then computes is killed where the
    /// first run was: the count of turns goes on from the recorded number of the first step, so
    /// the recorded number of the KILL is the same place in both runs.
    #[test]
    fn a_script_that_computes_after_such_a_wait_is_killed_at_the_same_turn() {
        let script = format!(
            "trap 'echo caught' INT; timeout 7 bash -c 'while :; do :; done'; sleep 300; \
             {PRINTING_LOOP}"
        );
        let (first, replay, recorded, asked) = stop_at_a_wait_and_replay(&script);
        assert_same_run(&first, &replay, &script);
        assert_eq!(first.code, 130);
        assert!(
            first.stdout.starts_with("caught\n10\n20\n"),
            "{}",
            first.stdout
        );
        assert_eq!((recorded.len(), asked.len()), (2, 2));
        assert!(asked[0].turn < recorded[0].turn, "{recorded:?} {asked:?}");
    }

    /// Such a script that ignores the stop is still in its wait a second later. The KILL acts
    /// there too, in the first run and in the replay.
    #[test]
    fn a_script_that_ignores_the_stop_in_such_a_wait_is_killed_there() {
        // The script takes no turn while it waits, so this second of grace is 30 ms by the clock.
        fn short_second(duration: Duration) -> LocalBoxFuture<'static, ()> {
            counting_sleep(if duration == SECOND {
                Duration::from_millis(30)
            } else {
                duration
            })
        }
        let script = "trap '' INT; timeout 7 bash -c 'while :; do :; done'; echo after; sleep 300";
        let (first, replay, recorded, asked) = stop_at_a_wait_and_replay_with(script, short_second);
        assert_same_run(&first, &replay, script);
        assert_eq!((first.code, first.stdout.as_str()), (130, "after\n"));
        assert_eq!((recorded.len(), asked.len()), (2, 2));
        assert!(asked[0].turn < recorded[0].turn, "{recorded:?} {asked:?}");
        // The count went on from the recorded number of the INT: the replay is at the recorded
        // place of the KILL when it learns of it, one turn after the INT.
        assert_eq!(asked[1], recorded[1]);
        assert_eq!(recorded[1], waiting_at(recorded[0].turn + 1));
    }

    /// A stop that came while the script computed is not repeated in a wait the script gets to
    /// later. Here the first run is stopped in its `for` loop; it catches the stop, runs on into
    /// its `sleep` and is killed there. The replay gets to that `sleep` before the recorded
    /// turn. A stop there would end the `sleep`, and the script would go on to make a file the
    /// first run never made, with the same output and status. The replay fails and says so.
    #[test]
    fn a_stop_that_came_while_the_script_computed_is_not_repeated_in_a_later_wait() {
        const MARKER: &str = "/tmp/bash-shell-made-after-the-sleep";
        // The timeout ends after 20 turns. The second of grace after the INT ends once the
        // script sleeps; the one after the KILL does not end.
        fn first(duration: Duration) -> LocalBoxFuture<'static, ()> {
            if duration == TIMEOUT {
                Box::pin(at_turn(super::turns::now() + 20))
            } else if duration == SECOND {
                let after_the_kill = SLEEPS.with_borrow(|sleeps| sleeps.contains(&SECOND));
                SLEEPS.with_borrow_mut(|sleeps| sleeps.push(duration));
                if after_the_kill {
                    Box::pin(std::future::pending())
                } else {
                    Box::pin(once_it_waits(ASLEEP))
                }
            } else {
                counting_sleep(duration)
            }
        }
        fn replay(duration: Duration) -> LocalBoxFuture<'static, ()> {
            if duration == TIMEOUT {
                Box::pin(at_turn(super::turns::now() + 1))
            } else {
                replayed(duration)
            }
        }
        let script = format!(
            "trap 'echo caught' INT; timeout 7 bash -c 'while :; do :; done'; \
             for i in $(seq 3000); do :; done; sleep 300; touch {MARKER}"
        );
        let made = || std::path::Path::new(MARKER).exists();
        // The replay of a first run that is stopped after `stop` turns, in its loop.
        let replay_of = |stop: u64| {
            let _ = std::fs::remove_file(MARKER);
            let recorded = Rc::new(RefCell::new(Vec::new()));
            start_as_a_new_call();
            let first = run_session_in(Tasks::OneQueue, &script, first, |session| {
                session.set_stop_request(at_turn(stop));
                let recorded = recorded.clone();
                session.set_turn_recorder(move |place| {
                    recorded.borrow_mut().push(place);
                    place
                });
            });
            assert_eq!(
                (first.code, first.stdout.as_str(), first.stderr.as_str()),
                (130, "caught\n", ""),
                "{stop}"
            );
            assert!(!made(), "{stop}");
            let recorded = recorded.take();
            // The INT acted while the script computed, the KILL while it waited.
            assert_eq!(recorded.len(), 2, "{recorded:?}");
            assert_eq!(recorded[0], computing_at(stop));
            assert!(recorded[1].waited, "{recorded:?}");

            start_as_a_new_call();
            ENDED_SECONDS.set(1);
            let later = run_session_in(Tasks::OneQueue, &script, replay, |session| {
                session.set_stop_request(at_turn(3));
                let mut answers = recorded.clone().into_iter();
                session.set_turn_recorder(move |_| answers.next().unwrap());
            });
            let made_in_the_replay = made();
            let _ = std::fs::remove_file(MARKER);
            assert!(
                !made_in_the_replay,
                "{stop}: {} {:?}",
                later.code, later.stderr
            );
            assert_eq!((later.code, later.stdout.as_str()), (1, ""), "{stop}");
            later
        };
        let waits_at = |later: &Outcome, stop: u64| -> u64 {
            let line = format!(
                "bash: the stop was recorded at turn {stop}, and the script waits at turn "
            );
            let turn = later.stderr.strip_prefix(&line).map(str::trim_end);
            turn.and_then(|turn| turn.parse().ok())
                .unwrap_or_else(|| panic!("{stop}: {}", later.stderr))
        };
        // The replay's script begins to wait before these turns.
        let begins_to_wait = waits_at(&replay_of(50), 50);
        assert!(begins_to_wait < 50, "{begins_to_wait}");
        assert_eq!(waits_at(&replay_of(60), 60), begins_to_wait);
        // And at this very turn: the same number of turns, but not the same place.
        assert_eq!(
            waits_at(&replay_of(begins_to_wait), begins_to_wait),
            begins_to_wait
        );
    }

    thread_local! {
        /// The steps of a stop that the call that runs now has asked its recorder for.
        static STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        /// The waits of [`ASLEEP`] the script of a first run has asked for when its first second
        /// of grace ends.
        static GRACE_ENDS_IN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    /// How a call in the tests below waits.
    type Waits = fn(Duration) -> LocalBoxFuture<'static, ()>;
    /// The stop request of such a call.
    type StopRequest<'a> = &'a dyn Fn() -> LocalBoxFuture<'static, ()>;

    /// A first run and its replay in one queue, each with its own waits and its own stop
    /// request, and `between` run between the two. With the runs come the places the first run
    /// recorded and the places the replay was at when it asked for them.
    fn run_and_replay(
        script: &str,
        first: (Waits, StopRequest<'_>),
        between: &dyn Fn(),
        replay: (Waits, StopRequest<'_>),
    ) -> (Outcome, Outcome, Vec<Place>, Vec<Place>) {
        let recorded = Rc::new(RefCell::new(Vec::new()));
        start_as_a_new_call();
        STEPS.set(0);
        let first_run = run_session_in(Tasks::OneQueue, script, first.0, |session| {
            session.set_stop_request(first.1());
            let recorded = recorded.clone();
            session.set_turn_recorder(move |place| {
                STEPS.set(STEPS.get() + 1);
                recorded.borrow_mut().push(place);
                place
            });
        });
        between();
        let recorded = recorded.take();
        let asked = Rc::new(RefCell::new(Vec::new()));
        start_as_a_new_call();
        STEPS.set(0);
        let replay_run = run_session_in(Tasks::OneQueue, script, replay.0, |session| {
            session.set_stop_request(replay.1());
            let mut answers = recorded.clone().into_iter();
            let asked = asked.clone();
            session.set_turn_recorder(move |place| {
                STEPS.set(STEPS.get() + 1);
                asked.borrow_mut().push(place);
                answers.next().unwrap_or(place)
            });
        });
        (first_run, replay_run, recorded, asked.take())
    }

    /// How many waits of `duration` the script has asked for.
    fn asked_for(duration: Duration) -> usize {
        SLEEPS.with_borrow(|sleeps| sleeps.iter().filter(|asked| **asked == duration).count())
    }

    /// Whether the script has asked for `count` waits of `duration` and waits.
    fn waits_in(duration: Duration, count: usize) -> bool {
        asked_for(duration) >= count && super::turns::waits()
    }

    /// The waits of a first run whose script has one wait of [`TIMEOUT`] of its own, which
    /// ends after the first step of the stop. The first second of grace ends once the script
    /// waits in the [`ASLEEP`] that [`GRACE_ENDS_IN`] names. No other wait ends.
    fn own_wait_ends_after_the_step(duration: Duration) -> LocalBoxFuture<'static, ()> {
        let seconds = asked_for(SECOND);
        SLEEPS.with_borrow_mut(|sleeps| sleeps.push(duration));
        if duration == TIMEOUT {
            one_queue::completion(|| STEPS.get() >= 1)
        } else if duration == SECOND && seconds == 0 {
            one_queue::completion(|| waits_in(ASLEEP, GRACE_ENDS_IN.get()))
        } else {
            Box::pin(std::future::pending())
        }
    }

    /// The same waits in a replay. Golem gives a replay each completion of the first run once
    /// the replay has had every completion that came before it: the end of the script's own
    /// wait once the replay has asked for the record of the first step, wherever its script is
    /// then, and the end of the first second of grace after that.
    fn own_wait_ends_after_the_record(duration: Duration) -> LocalBoxFuture<'static, ()> {
        let seconds = asked_for(SECOND);
        SLEEPS.with_borrow_mut(|sleeps| sleeps.push(duration));
        if duration == TIMEOUT {
            one_queue::completion(|| STEPS.get() >= 1)
        } else if duration == SECOND && seconds == 0 {
            one_queue::completion(|| true)
        } else {
            Box::pin(std::future::pending())
        }
    }

    /// A script whose own timer ends after the first step of its stop. The first run is stopped
    /// while the command under `timeout` sleeps. The command ignores INT, so the timer ends
    /// between the INT and the KILL, and `timeout` ends the command then. A replay that learns
    /// of the stop while its script still computes gets the timer's end before the script is at
    /// the recorded turn. Its script would leave the loop early and be stopped in another
    /// sleep, with the same result, and go on to make a file that the first run did not make.
    /// So the replay fails, and the file is not made.
    #[test]
    fn a_completion_between_the_record_and_the_step_fails_the_replay() {
        const MADE: &str = "/tmp/bash-shell-made-after-the-outer-sleep";
        let made = || std::path::Path::new(MADE).exists();
        let script = format!(
            "trap 'echo caught' INT; timeout 7 bash -c 'trap \"\" INT; \
             for i in $(seq 3000); do :; done; sleep 300'; sleep 300; touch {MADE}; sleep 300"
        );
        let in_the_wait: StopRequest<'_> = &|| one_queue::completion(|| waits_in(ASLEEP, 1));
        for learn in [1, 3, 20] {
            let _ = std::fs::remove_file(MADE);
            GRACE_ENDS_IN.set(2);
            let (first, replay, recorded, asked) = run_and_replay(
                &script,
                (own_wait_ends_after_the_step, in_the_wait),
                &|| assert!(!made()),
                (own_wait_ends_after_the_record, &move || {
                    one_queue::completion(move || super::turns::now() >= learn)
                }),
            );
            assert_eq!(
                (first.code, first.stdout.as_str(), first.stderr.as_str()),
                (130, "caught\n", "")
            );
            assert_eq!(first.sleeps, [TIMEOUT, ASLEEP, SECOND, ASLEEP, SECOND]);
            assert_eq!(recorded.len(), 2);
            assert!(recorded.iter().all(|place| place.waited), "{recorded:?}");
            assert!(asked[0].turn < recorded[0].turn, "{recorded:?} {asked:?}");
            assert_eq!(
                (replay.code, replay.stdout.as_str()),
                (1, ""),
                "learns at turn {learn}: {}",
                replay.stderr
            );
            assert!(
                replay.stderr.starts_with(&format!(
                    "bash: the stop was recorded at turn {}, and the script was woken at turn ",
                    recorded[0].turn
                )),
                "learns at turn {learn}: {}",
                replay.stderr
            );
            // No signal was sent, so no second of grace was started.
            assert_eq!(asked_for(SECOND), 0, "{:?}", replay.sleeps);
            assert!(!made(), "learns at turn {learn}");
        }
        // A replay that learns of the stop where the first run did, in the wait, repeats it:
        // the timer's end comes after the step, as in the first run.
        GRACE_ENDS_IN.set(2);
        let (first, replay, recorded, asked) = run_and_replay(
            &script,
            (own_wait_ends_after_the_step, in_the_wait),
            &|| assert!(!made()),
            (own_wait_ends_after_the_record, in_the_wait),
        );
        assert_same_run(&first, &replay, &script);
        assert_eq!((first.code, first.stdout.as_str()), (130, "caught\n"));
        assert_eq!(asked[0], recorded[0]);
        assert_eq!(first.sleeps, replay.sleeps);
        assert!(!made());
    }

    /// The same with two tasks: a job that sleeps ends after the first step of the stop. The
    /// first run is stopped in its `sleep 200`, catches the INT, makes the file and is killed
    /// in its next sleep. In a replay that learns of the stop early the job's end comes before
    /// the script is at the recorded turn and adds turns, so the script would still compute at
    /// that turn, catch the INT there and be killed before it makes the file. The replay fails.
    #[test]
    fn a_job_that_ends_between_the_record_and_the_step_fails_the_replay() {
        const MADE: &str = "/tmp/bash-shell-made-after-the-first-sleep";
        const FIRST_SLEEP: Duration = Duration::from_secs(200);
        let made = || std::path::Path::new(MADE).exists();
        let script = format!(
            "trap 'echo caught' INT; sleep 7 & for i in $(seq 3000); do :; done; sleep 200; \
             touch {MADE}; sleep 300"
        );
        for learn in [1, 3] {
            let _ = std::fs::remove_file(MADE);
            GRACE_ENDS_IN.set(1);
            let (first, replay, recorded, asked) = run_and_replay(
                &script,
                (own_wait_ends_after_the_step, &|| {
                    one_queue::completion(|| waits_in(FIRST_SLEEP, 1))
                }),
                &|| {
                    assert!(made());
                    let _ = std::fs::remove_file(MADE);
                },
                (own_wait_ends_after_the_record, &move || {
                    one_queue::completion(move || super::turns::now() >= learn)
                }),
            );
            assert_eq!(
                (first.code, first.stdout.as_str(), first.stderr.as_str()),
                (130, "caught\n", "")
            );
            assert_eq!(recorded.len(), 2);
            assert!(recorded.iter().all(|place| place.waited), "{recorded:?}");
            assert!(asked[0].turn < recorded[0].turn, "{recorded:?} {asked:?}");
            assert_eq!(
                (replay.code, replay.stdout.as_str()),
                (1, ""),
                "learns at turn {learn}: {}",
                replay.stderr
            );
            assert!(
                replay.stderr.starts_with(&format!(
                    "bash: the stop was recorded at turn {}, and the script ",
                    recorded[0].turn
                )),
                "learns at turn {learn}: {}",
                replay.stderr
            );
            assert!(!made(), "learns at turn {learn}");
        }
    }

    /// A script that still runs at the recorded turn, where the record says that the first
    /// run's script waited, is at another place. The call fails and says so, and sends no
    /// signal.
    #[test]
    fn a_script_that_runs_where_the_first_run_waited_fails_the_call() {
        let asked = Rc::new(std::cell::Cell::new(0));
        let run = run_session("while :; do :; done", counting_sleep, |session| {
            session.set_stop_request(at_turn(3));
            let asked = asked.clone();
            session.set_turn_recorder(move |place| {
                asked.set(place.turn);
                waiting_at(place.turn + 10)
            });
        });
        let turn = asked.get() + 10;
        assert_eq!((run.code, run.stdout.as_str()), (1, ""));
        assert_eq!(
            run.stderr,
            format!(
                "bash: the stop was recorded at turn {turn}, and the script runs at turn {turn}\n"
            )
        );
        assert_eq!(run.sleeps, []);
    }

    /// The waits of a call in which no wait ends: the script's own are pending host calls.
    fn no_wait_ends(duration: Duration) -> LocalBoxFuture<'static, ()> {
        SLEEPS.with_borrow_mut(|sleeps| sleeps.push(duration));
        one_queue::completion(|| false)
    }

    /// A stop that comes while a script computes under `timeout` is repeated at its recorded
    /// turn. The timer of `timeout` is a host call that is pending for the whole call, in both
    /// runs, and a pending host call wakes no task.
    #[test]
    fn a_stop_under_a_pending_timeout_is_repeated_at_its_recorded_turn() {
        let script = format!("timeout 7 bash -c '{PRINTING_LOOP}'; echo after");
        let (first, replay, recorded, asked) = run_and_replay(
            &script,
            (no_wait_ends, &|| {
                one_queue::completion(|| super::turns::now() >= STOPPED_AT)
            }),
            &|| (),
            (no_wait_ends, &|| {
                one_queue::completion(|| super::turns::now() >= 2)
            }),
        );
        assert_eq!(first.code, 130);
        assert!(first.stdout.lines().count() > 1, "{}", first.stdout);
        assert_eq!(recorded.len(), 1);
        assert!(
            !recorded[0].waited && recorded[0].turn >= STOPPED_AT,
            "{recorded:?}"
        );
        assert!(asked[0].turn < recorded[0].turn, "{recorded:?} {asked:?}");
        assert_same_run(&first, &replay, &script);
    }

    /// The waits of a first run whose own timer ends a loop after 20 turns, from outside.
    fn timer_ends_a_loop(duration: Duration) -> LocalBoxFuture<'static, ()> {
        SLEEPS.with_borrow_mut(|sleeps| sleeps.push(duration));
        if duration == TIMEOUT {
            one_queue::completion(|| super::turns::now() >= 20)
        } else {
            Box::pin(std::future::pending())
        }
    }

    /// The same waits in a replay: the timer is over as early as its command can take it.
    fn timer_ends_a_loop_at_once(duration: Duration) -> LocalBoxFuture<'static, ()> {
        SLEEPS.with_borrow_mut(|sleeps| sleeps.push(duration));
        if duration == TIMEOUT {
            one_queue::completion(|| super::turns::now() >= 1)
        } else {
            Box::pin(std::future::pending())
        }
    }

    /// A completion that came to the script before the stop does not stand in the way. The
    /// script's own timer ends a loop, from outside, and the script is stopped later in its
    /// `sleep`. The replay gets the timer's end at once and the stop after it, when its script
    /// waits in the same `sleep` after fewer turns, and stops it there.
    #[test]
    fn a_wait_reached_after_a_completion_from_outside_is_repeated() {
        let script = "timeout 7 bash -c 'while :; do :; done'; echo after; sleep 300";
        let in_the_wait: StopRequest<'_> = &|| one_queue::completion(|| waits_in(ASLEEP, 1));
        let (first, replay, recorded, asked) = run_and_replay(
            script,
            (timer_ends_a_loop, in_the_wait),
            &|| (),
            (timer_ends_a_loop_at_once, in_the_wait),
        );
        assert_eq!(
            (first.code, first.stdout.as_str(), first.stderr.as_str()),
            (130, "after\n", "")
        );
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0].waited && recorded[0].turn >= 20, "{recorded:?}");
        assert!(asked[0].turn < recorded[0].turn, "{recorded:?} {asked:?}");
        assert_same_run(&first, &replay, script);
        assert_eq!(first.sleeps, replay.sleeps);
    }

    /// With a second task alive, the same script fails its replay: the job waits too, and the
    /// replay cannot tell where each task was.
    #[test]
    fn tasks_that_get_to_their_wait_in_fewer_turns_fail_the_replay() {
        for script in [
            "sleep 200 & timeout 7 bash -c 'while :; do :; done'; echo after; sleep 300",
            // A job and a pipeline that ended before leave the count of live tasks once each:
            // the job that sleeps is still counted.
            "true & wait; echo a | cat >/dev/null; sleep 200 & \
             timeout 7 bash -c 'while :; do :; done'; echo after; sleep 300",
        ] {
            let (first, replay, recorded, asked) = stop_at_a_wait_and_replay(script);
            assert_eq!(
                (first.code, first.stdout.as_str()),
                (130, "after\n"),
                "{script}"
            );
            assert_eq!(
                (replay.code, replay.stdout.as_str()),
                (1, "after\n"),
                "{script}"
            );
            assert_eq!((recorded.len(), asked.len()), (1, 1), "{script}");
            assert!(
                asked[0].turn < recorded[0].turn,
                "{script}: {recorded:?} {asked:?}"
            );
            assert!(
                replay.stderr.starts_with(&format!(
                    "bash: the stop was recorded at turn {}, and the script waits at turn ",
                    recorded[0].turn
                )),
                "{script}: {}",
                replay.stderr
            );
        }
    }

    /// A script that waits when the stop comes is stopped at once, with a recorder as without:
    /// it gives no turn while it waits, so it is at the recorded turn already.
    #[test]
    fn a_waiting_script_is_stopped_at_once_at_its_recorded_turn() {
        let recorded = Rc::new(RefCell::new(Vec::new()));
        let run = run_session(
            "echo start; sleep 300; echo end",
            counting_sleep,
            |session| {
                session.set_stop_request(once_it_waits(ASLEEP));
                let recorded = recorded.clone();
                session.set_turn_recorder(move |place| {
                    recorded.borrow_mut().push(place);
                    place
                });
            },
        );
        assert_eq!(
            (run.code, run.stdout.as_str(), run.stderr.as_str()),
            (130, "start\n", "")
        );
        // The script ended on the INT: no KILL followed the second of grace.
        assert_eq!(run.sleeps, [Duration::from_secs(300), SECOND]);
        // One turn to the `sleep`; the script takes one more to end on the INT.
        assert_eq!((recorded.take(), run.turn), (vec![waiting_at(1)], 2));
    }

    /// A replay that learns of a stop when the script is already past the recorded turn cannot
    /// repeat it. The call fails and says so: it does not stop the script somewhere else.
    #[test]
    fn a_stop_recorded_behind_the_script_fails_the_call() {
        start_as_a_new_call();
        let run = run_session(PRINTING_LOOP, counting_sleep, |session| {
            session.set_stop_request(at_turn(3));
            session.set_turn_recorder(|place| computing_at(place.turn - 1));
        });
        assert_eq!((run.code, run.turn), (1, 3));
        assert_eq!(
            run.stderr,
            "bash: the stop was recorded at turn 2, and the script is at turn 3\n"
        );
        // Nothing follows the failure: no signal, so no second of grace.
        assert!(run.sleeps.is_empty(), "{:?}", run.sleeps);
    }

    /// A script that ends before the turn recorded for its stop is not the script of the first
    /// run. The call fails and says so.
    #[test]
    fn a_script_that_ends_before_its_recorded_stop_fails_the_call() {
        let run = run_session("echo done", counting_sleep, |session| {
            session.set_stop_request(std::future::ready(()));
            session.set_turn_recorder(|_| computing_at(1_000));
        });
        assert_eq!(run.code, 1);
        assert_eq!(
            run.stderr,
            "bash: the stop was recorded at turn 1000, and the script ended at turn 1\n"
        );
    }

    /// Without a stop nothing is recorded, so a call that ends by itself records nothing.
    #[test]
    fn a_call_that_is_not_stopped_records_no_turn() {
        let run = run_session(
            "for i in $(seq 500); do :; done; echo done",
            counting_sleep,
            |session| {
                session.set_time_limit(LIMIT);
                session.set_stop_request(std::future::pending());
                session.set_turn_recorder(|_| panic!("no turn is recorded without a stop"));
            },
        );
        assert_eq!((run.code, run.stdout.as_str()), (0, "done\n"));
        assert!(run.turn > 1, "{}", run.turn);
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::{Buffer, MAX_OUTPUT_BYTES};
    use std::io::Write;

    #[test]
    fn output_past_the_limit_is_refused_as_a_closed_pipe() {
        let mut buffer = Buffer::default();
        buffer.write_all(&vec![b'y'; MAX_OUTPUT_BYTES - 1]).unwrap();
        let error = buffer.write_all(b"yy").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        let (bytes, truncated) = buffer.take();
        assert_eq!(bytes.len(), MAX_OUTPUT_BYTES);
        assert!(truncated);
    }

    #[test]
    fn output_within_the_limit_is_kept_whole() {
        let mut buffer = Buffer::default();
        buffer.write_all(&vec![b'y'; MAX_OUTPUT_BYTES]).unwrap();
        let (bytes, truncated) = buffer.take_text();
        assert_eq!(bytes.len(), MAX_OUTPUT_BYTES);
        assert!(!truncated);
    }

    /// The limit counts the text the caller receives: a byte that is not UTF-8 becomes three.
    #[test]
    fn output_that_is_not_utf8_is_cut_to_the_limit_as_text() {
        let mut buffer = Buffer::default();
        buffer.write_all(&vec![0xff; MAX_OUTPUT_BYTES / 2]).unwrap();
        let (bytes, truncated) = buffer.take_text();
        assert!(truncated);
        assert_eq!(bytes.len(), MAX_OUTPUT_BYTES / 3);
        assert!(String::from_utf8_lossy(&bytes).len() <= MAX_OUTPUT_BYTES);
    }

    /// A character the limit splits is dropped whole rather than returned as U+FFFD.
    #[test]
    fn output_cut_inside_a_character_drops_the_character() {
        let mut buffer = Buffer::default();
        buffer.write_all(&vec![b'y'; MAX_OUTPUT_BYTES - 1]).unwrap();
        assert!(buffer.write_all("é".as_bytes()).is_err());
        let (bytes, truncated) = buffer.take_text();
        assert!(truncated);
        assert_eq!(bytes, vec![b'y'; MAX_OUTPUT_BYTES - 1]);
    }
}
