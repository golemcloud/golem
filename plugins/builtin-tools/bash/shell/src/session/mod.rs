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
}

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
        let builder = builder.execution_services(services);
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
        let script = async {
            let result = brush_core::execution::process::run_numbered_process(
                &table,
                table.shell_pid(),
                dispositions,
                async {
                    let result = self.evaluate(script, here_document_warnings, &params).await;
                    completed.set(true);
                    result
                },
            )
            .await;
            // A signal ended the script before it finished: its EXIT trap still runs, as in bash.
            if !completed.get() {
                self.shell.exit_trap_after_signal(&result, &params).await;
            }
            result
        };
        let timed_out = std::cell::Cell::new(false);
        let result = match time_limit {
            None => script.await,
            Some(limit) => {
                let mut watchdog =
                    std::pin::pin!(stop_at_time_limit(&table, services, limit, &timed_out));
                // Arm the timer before the script runs: its durable clock calls then come before
                // anything the script does in the oplog. Golem's recovery of a POST interrupted
                // by a crash discards the oplog after the request's start by position, and a
                // timer call still being delivered in that range makes the replay fail.
                for _ in 0..ARM_TURNS {
                    if futures::poll!(watchdog.as_mut()).is_ready() {
                        break;
                    }
                    (services.yield_now)().await;
                }
                // The watchdog is polled first: once its timer is due, the script is stopped
                // before it runs further, live and on replay alike.
                match futures::future::select(watchdog, std::pin::pin!(script)).await {
                    futures::future::Either::Left(((), _)) => {
                        Ok(brush_core::ExecutionResult::new(TIMED_OUT))
                    }
                    futures::future::Either::Right((result, _)) => result,
                }
            }
        };
        let result = if timed_out.get() {
            let seconds = time_limit.unwrap_or_default().as_secs_f64();
            log::warn!("the call exceeded its {seconds} s time limit");
            Ok(brush_core::ExecutionResult::new(TIMED_OUT))
        } else {
            result
        };
        // The shell's descriptors close as it exits, so an output process substitution it made
        // one of them (`exec > >(tee log)`) sees the end of its input and finishes.
        self.shell.replace_open_files(std::iter::empty());
        let notes = self.stop_leftover_jobs().await;
        jobs.cancel_and_join().await;
        drop(params);
        let (out, out_truncated) = stdout.take();
        let (mut err, err_truncated) = stderr.take();
        if timed_out.get() {
            let seconds = time_limit.unwrap_or_default().as_secs_f64();
            err.extend_from_slice(
                format!("bash: the call exceeded its {seconds} s time limit\n").as_bytes(),
            );
        }
        for note in notes {
            err.extend_from_slice(note.as_bytes());
        }
        for (truncated, name) in [(out_truncated, "stdout"), (err_truncated, "stderr")] {
            if truncated {
                log::warn!("{name} exceeded the output limit");
                err.extend_from_slice(
                    format!(
                        "bash: {name} exceeds the {} output limit of bash-tool; the rest was \
                         discarded\n",
                        crate::tools::buffer_limit()
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
    /// The grace is one timer, raced against the jobs ending, never a loop of short sleeps: how
    /// many durable timers a run creates must not depend on how far local compute got, or replay
    /// would ask for timers the recorded run never created. A job that ends while others still
    /// run is marked in the durable record with a zero-length sleep: replay releases the timer
    /// only once it has passed that mark, so a job that ended before the timer live also ends
    /// before it on replay, whatever its handler computes.
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
            match futures::future::select(next_exit, grace.as_mut()).await {
                futures::future::Either::Left(_) => {
                    if remaining
                        .iter()
                        .any(|pid| process::process_exists(&table, *pid))
                    {
                        (services.sleep)(std::time::Duration::ZERO).await;
                    }
                }
                futures::future::Either::Right(_) => break,
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

/// Turns the call's timer gets to be armed before its script starts (see `execute_program`).
#[cfg(target_arch = "wasm32")]
const ARM_TURNS: usize = 4;

/// A call stopped at its time limit ends with this status, as a command `timeout(1)` stopped
/// does.
#[cfg(target_arch = "wasm32")]
const TIMED_OUT: u8 = 124;

/// Waits out `limit`, then stops the script (process `$$`) and every process it started as
/// `timeout(1)` stops a command: TERM, and KILL a second later. A script that still runs a
/// second after that (its EXIT trap, which runs after the signal) is abandoned when this ends.
/// The timers are the same in every run that reaches the limit: one for the limit, then one per
/// second of grace, each created only once the one before it has fired.
#[cfg(target_arch = "wasm32")]
async fn stop_at_time_limit(
    table: &brush_core::process_table::ProcessTable,
    services: crate::ExecutionServices,
    limit: std::time::Duration,
    timed_out: &std::cell::Cell<bool>,
) {
    use brush_core::execution::process::{self, signals};
    (services.sleep)(limit).await;
    timed_out.set(true);
    process::signal_process_group(table, table.shell_pid(), signals::TERM);
    (services.sleep)(std::time::Duration::from_secs(1)).await;
    process::signal_process_group(table, table.shell_pid(), signals::KILL);
    (services.sleep)(std::time::Duration::from_secs(1)).await;
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

/// The most of each output stream a call returns (see `tools::MAX_BUFFER_BYTES`).
#[cfg(target_arch = "wasm32")]
const MAX_OUTPUT_BYTES: usize = crate::tools::MAX_BUFFER_BYTES;

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
    use std::{cell::RefCell, time::Duration};

    use futures::future::LocalBoxFuture;

    use super::Session;

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

    /// The grace for leftover jobs is one durable timer, however long their HUP handlers
    /// compute: replay then asks for exactly the timers the recorded run created.
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

        // One job ends on HUP after computing while another ignores it: its end is marked with
        // a zero-length sleep before the timer fires.
        let (stderr, sleeps) = run(
            "(trap 'for i in $(seq 300); do :; done; exit 0' HUP; while :; do :; done) & \
             trap '' HUP; (while :; do :; done) & echo main",
        );
        assert!(stderr.contains("[1] (pid 2, hangup): "), "{stderr}");
        assert!(stderr.contains("[2] (pid 3, killed): "), "{stderr}");
        assert_eq!(sleeps, [Duration::from_secs(1), Duration::ZERO]);

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
        let (bytes, truncated) = buffer.take();
        assert_eq!(bytes.len(), MAX_OUTPUT_BYTES);
        assert!(!truncated);
    }
}
