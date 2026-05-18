// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

mod control;
mod pty;

use anyhow::{Context, anyhow};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use portable_pty::PtySize;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc as tokio_mpsc;
use uuid::Uuid;

pub const REPL_CONTROL_ADDR_ENV: &str = "GOLEM_REPL_CONTROL_ADDR";
pub const REPL_CONTROL_TOKEN_ENV: &str = "GOLEM_REPL_CONTROL_TOKEN";
const GOLEM_REPL_PTY_DEBUG_ENV: &str = "GOLEM_REPL_PTY_DEBUG";
const GOLEM_REPL_PTY_DEBUG_LOG_ENV: &str = "GOLEM_REPL_PTY_DEBUG_LOG";

#[derive(Clone, Debug)]
pub struct ReplCommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
}

#[derive(Debug)]
pub struct ReplSessionResult {
    pub exit: CommandExit,
}

#[derive(Clone, Copy, Debug)]
pub struct CommandExit {
    pub code: Option<i32>,
    pub success: bool,
}

pub struct ReloadCoordinator {
    request_tx: tokio_mpsc::UnboundedSender<ReloadRequest>,
}

pub struct ReloadDriver {
    pub request_rx: tokio_mpsc::UnboundedReceiver<ReloadRequest>,
}

pub struct ReloadRequest {
    response_tx: Sender<Result<ReplCommandSpec, String>>,
}

impl ReloadRequest {
    pub fn respond(self, response: Result<ReplCommandSpec, String>) {
        let _ = self.response_tx.send(response);
    }
}

pub fn reload_channel() -> (ReloadCoordinator, ReloadDriver) {
    let (request_tx, request_rx) = tokio_mpsc::unbounded_channel();
    (
        ReloadCoordinator { request_tx },
        ReloadDriver { request_rx },
    )
}

fn add_control_env(spec: &mut ReplCommandSpec, addr: &str, token: &str) {
    spec.env
        .insert(REPL_CONTROL_ADDR_ENV.to_string(), addr.to_string());
    spec.env
        .insert(REPL_CONTROL_TOKEN_ENV.to_string(), token.to_string());
}

pub fn run_repl_session(
    mut node_spec: ReplCommandSpec,
    reload: Option<ReloadCoordinator>,
) -> anyhow::Result<ReplSessionResult> {
    let token = Uuid::new_v4().to_string();
    let control = control::ControlServer::start(token.clone())?;
    let control_addr = control.addr().to_string();

    add_control_env(&mut node_spec, &control_addr, &token);

    let debug_log = DebugLog::from_env();
    debug_log.log(format!(
        "starting REPL supervisor for {:?}",
        node_spec.program
    ));

    let terminal_guard = RawTerminalGuard::enter()?;
    let mut supervisor = Supervisor::new(control, control_addr, token, debug_log, reload);
    let result = supervisor.run(node_spec);
    drop(terminal_guard);
    result
}

struct Supervisor {
    control: Option<control::ControlServer>,
    control_addr: String,
    control_token: String,
    debug_log: DebugLog,
    state: SupervisorState,
    terminal_state: TerminalState,
    node: Option<SessionRuntime>,
    cli: Option<SessionRuntime>,
    pending_cli_response: Option<Sender<control::RunCliResponse>>,
    reload: Option<ReloadCoordinator>,
    ctrl_c_debounced: bool,
}

impl Supervisor {
    fn new(
        control: control::ControlServer,
        control_addr: String,
        control_token: String,
        debug_log: DebugLog,
        reload: Option<ReloadCoordinator>,
    ) -> Self {
        Self {
            control: Some(control),
            control_addr,
            control_token,
            debug_log,
            state: SupervisorState::NodeStarting,
            terminal_state: TerminalState::Raw,
            node: None,
            cli: None,
            pending_cli_response: None,
            reload,
            ctrl_c_debounced: false,
        }
    }

    fn run(&mut self, node_spec: ReplCommandSpec) -> anyhow::Result<ReplSessionResult> {
        let (event_tx, event_rx) = mpsc::channel::<SupervisorEvent>();
        spawn_input_reader(event_tx.clone());
        spawn_resize_watcher(event_tx.clone());
        self.control
            .take()
            .expect("missing REPL control server")
            .spawn_request_reader(event_tx.clone());

        self.node = Some(self.spawn_session(SessionId::Node, node_spec, &event_tx)?);
        self.set_state(SupervisorState::NodeStarting);

        loop {
            match event_rx.recv() {
                Ok(event) => {
                    if let Some(result) = self.handle_event(event, &event_tx)? {
                        self.cleanup_all();
                        return Ok(result);
                    }
                }
                Err(_) => {
                    self.cleanup_all();
                    return Err(anyhow!("REPL supervisor event loop stopped unexpectedly"));
                }
            }
        }
    }

    fn handle_event(
        &mut self,
        event: SupervisorEvent,
        event_tx: &Sender<SupervisorEvent>,
    ) -> anyhow::Result<Option<ReplSessionResult>> {
        match event {
            SupervisorEvent::Input(bytes) => {
                self.handle_input(bytes)?;
                Ok(None)
            }
            SupervisorEvent::CtrlC => {
                self.handle_ctrl_c()?;
                Ok(None)
            }
            SupervisorEvent::Output { session, bytes } => {
                self.handle_output(session, bytes);
                Ok(None)
            }
            SupervisorEvent::OutputClosed { session, error } => {
                self.handle_output_closed(session, error, event_tx)
            }
            SupervisorEvent::Resize(size) => {
                self.handle_resize(size);
                Ok(None)
            }
            SupervisorEvent::Exited { session, exit } => {
                self.handle_process_exit(session, exit, event_tx)
            }
            SupervisorEvent::RunCli(request) => {
                self.handle_run_cli(request, event_tx)?;
                Ok(None)
            }
        }
    }

    fn handle_input(&mut self, bytes: Vec<u8>) -> anyhow::Result<()> {
        let target = match self.state {
            SupervisorState::ReplActive | SupervisorState::NodeStarting => self.node.as_mut(),
            SupervisorState::CliStarting
            | SupervisorState::CliActive
            | SupervisorState::CliCancelling
            | SupervisorState::CliExiting => self.cli.as_mut(),
            SupervisorState::NodeExiting
            | SupervisorState::Reloading
            | SupervisorState::ShuttingDown => None,
        };

        if let Some(target) = target {
            target.writer.write_all(&bytes)?;
            target.writer.flush()?;
        }

        Ok(())
    }

    fn handle_output(&mut self, session: SessionId, bytes: Vec<u8>) {
        let should_write = matches!(
            (self.state, session),
            (SupervisorState::ReplActive, SessionId::Node)
                | (SupervisorState::NodeStarting, SessionId::Node)
                | (SupervisorState::NodeExiting, SessionId::Node)
                | (SupervisorState::CliStarting, SessionId::Cli)
                | (SupervisorState::CliActive, SessionId::Cli)
                | (SupervisorState::CliCancelling, SessionId::Cli)
                | (SupervisorState::CliExiting, SessionId::Cli)
        );

        if should_write {
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(&bytes);
            let _ = stdout.flush();
        }

        if matches!(self.state, SupervisorState::NodeStarting) && session == SessionId::Node {
            self.set_state(SupervisorState::ReplActive);
        }

        if matches!(self.state, SupervisorState::CliStarting) && session == SessionId::Cli {
            self.set_state(SupervisorState::CliActive);
        }
    }

    fn handle_run_cli(
        &mut self,
        request: control::RunCliSupervisorRequest,
        event_tx: &Sender<SupervisorEvent>,
    ) -> anyhow::Result<()> {
        if self.pending_cli_response.is_some() || !matches!(self.state, SupervisorState::ReplActive)
        {
            let _ = request.response.send(control::RunCliResponse {
                ok: false,
                code: None,
                stdout: None,
                stderr: Some("another CLI command is already running".to_string()),
            });
            return Ok(());
        }

        self.debug_log
            .log(format!("runCli request: {:?}", request.args));
        let cli = self.spawn_session(
            SessionId::Cli,
            ReplCommandSpec {
                program: PathBuf::from(crate::binary_path_to_string()?),
                args: request.args,
                cwd: crate::fs::current_dir_lexical()?,
                env: HashMap::new(),
            },
            event_tx,
        )?;

        self.cli = Some(cli);
        self.pending_cli_response = Some(request.response);
        self.ctrl_c_debounced = false;
        self.set_state(SupervisorState::CliStarting);
        Ok(())
    }

    fn handle_resize(&mut self, size: PtySize) {
        if let Some(node) = self.node.as_mut()
            && let Err(err) = node.resize(size)
        {
            self.debug_log
                .log(format!("failed to resize Node PTY: {err:#}"));
        }
        if let Some(cli) = self.cli.as_mut()
            && let Err(err) = cli.resize(size)
        {
            self.debug_log
                .log(format!("failed to resize CLI PTY: {err:#}"));
        }
    }

    fn handle_ctrl_c(&mut self) -> anyhow::Result<()> {
        self.debug_log
            .log(format!("ctrl-c in state {:?}", self.state));

        match self.state {
            SupervisorState::ReplActive | SupervisorState::NodeStarting => {
                if let Some(node) = self.node.as_mut() {
                    node.writer.write_all(&[3])?;
                    node.writer.flush()?;
                }
            }
            SupervisorState::CliStarting | SupervisorState::CliActive => {
                if !self.ctrl_c_debounced {
                    self.request_cli_cancel()?;
                }
            }
            SupervisorState::CliCancelling => {
                if let Some(cli) = self.cli.as_mut() {
                    self.debug_log
                        .log("force-killing CLI after repeated ctrl-c");
                    let _ = cli.kill();
                }
                self.ctrl_c_debounced = true;
            }
            SupervisorState::CliExiting => {
                self.ctrl_c_debounced = true;
            }
            SupervisorState::NodeExiting
            | SupervisorState::Reloading
            | SupervisorState::ShuttingDown => {
                self.ctrl_c_debounced = true;
            }
        }

        Ok(())
    }

    fn request_cli_cancel(&mut self) -> anyhow::Result<()> {
        self.ctrl_c_debounced = true;
        self.set_state(SupervisorState::CliCancelling);
        if let Some(cli) = self.cli.as_mut() {
            self.debug_log.log("forwarding ctrl-c to CLI PTY");
            cli.writer.write_all(&[3])?;
            cli.writer.flush()?;
        }
        Ok(())
    }

    fn handle_process_exit(
        &mut self,
        session: SessionId,
        exit: CommandExit,
        event_tx: &Sender<SupervisorEvent>,
    ) -> anyhow::Result<Option<ReplSessionResult>> {
        self.debug_log.log(format!(
            "session {:?} exited with {:?}",
            session,
            format_exit_code(exit.code)
        ));

        match session {
            SessionId::Node => {
                if let Some(node) = self.node.as_mut() {
                    node.exit = Some(exit);
                }
                self.set_state(SupervisorState::NodeExiting);
                self.try_finish_node(event_tx)
            }
            SessionId::Cli => {
                if let Some(cli) = self.cli.as_mut() {
                    cli.exit = Some(exit);
                }
                self.set_state(SupervisorState::CliExiting);
                self.try_finish_cli()
            }
        }
    }

    fn handle_output_closed(
        &mut self,
        session: SessionId,
        error: Option<String>,
        event_tx: &Sender<SupervisorEvent>,
    ) -> anyhow::Result<Option<ReplSessionResult>> {
        if let Some(error) = error {
            self.debug_log.log(format!(
                "PTY output reader for {session:?} stopped: {error}"
            ));
        } else {
            self.debug_log
                .log(format!("PTY output reader for {session:?} reached EOF"));
        }

        match session {
            SessionId::Node => {
                if let Some(node) = self.node.as_mut() {
                    node.output_closed = true;
                }
                self.try_finish_node(event_tx)
            }
            SessionId::Cli => {
                if let Some(cli) = self.cli.as_mut() {
                    cli.output_closed = true;
                }
                self.try_finish_cli()
            }
        }
    }

    fn try_finish_node(
        &mut self,
        event_tx: &Sender<SupervisorEvent>,
    ) -> anyhow::Result<Option<ReplSessionResult>> {
        let Some(node) = self.node.as_ref() else {
            return Ok(None);
        };
        if !node.output_closed || node.exit.is_none() {
            return Ok(None);
        }

        let exit = node.exit.expect("checked above");
        self.node = None;

        if exit.code == Some(75)
            && let Some(node) = self.reload_node(event_tx)?
        {
            self.node = Some(node);
            self.set_state(SupervisorState::NodeStarting);
            return Ok(None);
        }

        self.set_state(SupervisorState::ShuttingDown);
        Ok(Some(ReplSessionResult { exit }))
    }

    fn try_finish_cli(&mut self) -> anyhow::Result<Option<ReplSessionResult>> {
        let Some(cli) = self.cli.as_ref() else {
            return Ok(None);
        };
        if !cli.output_closed || cli.exit.is_none() {
            return Ok(None);
        }

        let exit = cli.exit.expect("checked above");
        self.cli = None;
        self.ctrl_c_debounced = false;

        self.refresh_terminal_mode()?;
        self.set_state(SupervisorState::ReplActive);

        if let Some(response) = self.pending_cli_response.take() {
            let _ = response.send(control::RunCliResponse {
                ok: exit.success,
                code: exit.code,
                stdout: None,
                stderr: None,
            });
        }

        Ok(None)
    }

    fn reload_node(
        &mut self,
        event_tx: &Sender<SupervisorEvent>,
    ) -> anyhow::Result<Option<SessionRuntime>> {
        let Some(request_tx) = self.reload.as_ref().map(|reload| reload.request_tx.clone()) else {
            return Ok(None);
        };

        self.set_state(SupervisorState::Reloading);
        self.leave_raw_mode()?;
        self.debug_log
            .log("requesting REPL reload from async runner");

        let (response_tx, response_rx) = mpsc::channel();
        request_tx
            .send(ReloadRequest { response_tx })
            .map_err(|_| anyhow!("Failed to request REPL reload"))?;

        let mut node_spec = response_rx
            .recv()
            .map_err(|_| anyhow!("Failed to receive REPL reload result"))?
            .map_err(|err| anyhow!(err))?;
        add_control_env(&mut node_spec, &self.control_addr, &self.control_token);

        self.debug_log.log("starting reloaded Node PTY backend");
        self.enter_raw_mode()?;
        self.spawn_session(SessionId::Node, node_spec, event_tx)
            .map(Some)
    }

    fn spawn_session(
        &mut self,
        session: SessionId,
        spec: ReplCommandSpec,
        event_tx: &Sender<SupervisorEvent>,
    ) -> anyhow::Result<SessionRuntime> {
        self.debug_log.log(format!(
            "spawning {:?}: program={:?} args={:?}",
            session, spec.program, spec.args
        ));
        let child = pty::spawn_pty_command(spec)?;
        spawn_pty_output_reader(session, child.reader, event_tx.clone());
        spawn_waiter(session, child.exit_receiver, event_tx.clone());
        Ok(SessionRuntime {
            writer: child.writer,
            killer: child.killer,
            pty_pair: child.pair,
            exit: None,
            output_closed: false,
        })
    }

    fn enter_raw_mode(&mut self) -> anyhow::Result<()> {
        if matches!(self.terminal_state, TerminalState::Cooked) {
            enable_raw_terminal_mode().context("Failed to enable terminal raw mode")?;
            self.terminal_state = TerminalState::Raw;
        }
        Ok(())
    }

    fn leave_raw_mode(&mut self) -> anyhow::Result<()> {
        if matches!(self.terminal_state, TerminalState::Raw) {
            disable_raw_mode().context("Failed to disable terminal raw mode")?;
            self.terminal_state = TerminalState::Cooked;
            normalize_terminal_line();
        }
        Ok(())
    }

    fn refresh_terminal_mode(&mut self) -> anyhow::Result<()> {
        if matches!(self.terminal_state, TerminalState::Raw) {
            self.leave_raw_mode()?;
            self.enter_raw_mode()?;
        }
        Ok(())
    }

    fn cleanup_all(&mut self) {
        self.debug_log.log("cleaning up PTY sessions");
        self.set_state(SupervisorState::ShuttingDown);
        if let Some(cli) = self.cli.as_mut() {
            let _ = cli.kill();
        }
        if let Some(node) = self.node.as_mut() {
            let _ = node.kill();
        }
    }

    fn set_state(&mut self, state: SupervisorState) {
        if self.state != state {
            self.debug_log
                .log(format!("state {:?} -> {:?}", self.state, state));
            self.state = state;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SupervisorState {
    NodeStarting,
    ReplActive,
    NodeExiting,
    CliStarting,
    CliActive,
    CliCancelling,
    CliExiting,
    Reloading,
    ShuttingDown,
}

#[derive(Clone, Copy, Debug)]
enum TerminalState {
    Raw,
    Cooked,
}

struct SessionRuntime {
    writer: Box<dyn Write + Send>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    pty_pair: portable_pty::PtyPair,
    exit: Option<CommandExit>,
    output_closed: bool,
}

impl SessionRuntime {
    fn kill(&mut self) -> anyhow::Result<()> {
        self.killer.kill().context("Failed to kill PTY child")
    }

    fn resize(&mut self, size: PtySize) -> anyhow::Result<()> {
        self.pty_pair.master.resize(size)
    }
}

fn spawn_input_reader(event_tx: Sender<SupervisorEvent>) {
    thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0_u8; 4096];
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) => return,
                Ok(n) => {
                    if buffer[..n].contains(&3) && event_tx.send(SupervisorEvent::CtrlC).is_err() {
                        return;
                    }

                    let filtered = buffer[..n]
                        .iter()
                        .copied()
                        .filter(|byte| *byte != 3)
                        .collect::<Vec<_>>();

                    if !filtered.is_empty()
                        && event_tx.send(SupervisorEvent::Input(filtered)).is_err()
                    {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });
}

fn normalize_terminal_line() {
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(b"\x1b[?25h\x1b[0m\r");
    let _ = stdout.flush();
}

fn current_pty_size() -> PtySize {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn spawn_resize_watcher(event_tx: Sender<SupervisorEvent>) {
    thread::spawn(move || {
        let mut last_size = current_pty_size();
        loop {
            thread::sleep(Duration::from_millis(200));
            let size = current_pty_size();
            if size != last_size {
                last_size = size;
                if event_tx.send(SupervisorEvent::Resize(size)).is_err() {
                    return;
                }
            }
        }
    });
}

fn spawn_pty_output_reader(
    session: SessionId,
    mut reader: Box<dyn Read + Send>,
    event_tx: Sender<SupervisorEvent>,
) {
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = event_tx.send(SupervisorEvent::OutputClosed {
                        session,
                        error: None,
                    });
                    return;
                }
                Ok(n) => {
                    if event_tx
                        .send(SupervisorEvent::Output {
                            session,
                            bytes: buffer[..n].to_vec(),
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                Err(err) => {
                    let _ = event_tx.send(SupervisorEvent::OutputClosed {
                        session,
                        error: Some(err.to_string()),
                    });
                    return;
                }
            }
        }
    });
}

fn spawn_waiter(
    session: SessionId,
    exit_rx: Receiver<CommandExit>,
    event_tx: Sender<SupervisorEvent>,
) {
    thread::spawn(move || {
        if let Ok(exit) = exit_rx.recv() {
            let _ = event_tx.send(SupervisorEvent::Exited { session, exit });
        }
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionId {
    Node,
    Cli,
}

enum SupervisorEvent {
    Input(Vec<u8>),
    CtrlC,
    Output {
        session: SessionId,
        bytes: Vec<u8>,
    },
    OutputClosed {
        session: SessionId,
        error: Option<String>,
    },
    Resize(PtySize),
    Exited {
        session: SessionId,
        exit: CommandExit,
    },
    RunCli(control::RunCliSupervisorRequest),
}

struct RawTerminalGuard {
    #[cfg(windows)]
    windows_input_mode: Option<WindowsConsoleInputModeGuard>,
}

impl RawTerminalGuard {
    fn enter() -> anyhow::Result<Self> {
        #[cfg(windows)]
        let windows_input_mode = WindowsConsoleInputModeGuard::capture();
        enable_raw_terminal_mode().context("Failed to enable terminal raw mode")?;
        Ok(Self {
            #[cfg(windows)]
            windows_input_mode,
        })
    }
}

impl Drop for RawTerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        #[cfg(windows)]
        if let Some(guard) = &self.windows_input_mode {
            guard.restore();
        }
    }
}

fn enable_raw_terminal_mode() -> anyhow::Result<()> {
    enable_raw_mode().context("Failed to enable terminal raw mode")?;
    #[cfg(windows)]
    enable_windows_virtual_terminal_input();
    Ok(())
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct WindowsConsoleInputModeGuard {
    handle: windows_sys::Win32::Foundation::HANDLE,
    original_mode: windows_sys::Win32::System::Console::CONSOLE_MODE,
}

#[cfg(windows)]
impl WindowsConsoleInputModeGuard {
    fn capture() -> Option<Self> {
        use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE};

        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        let mut original_mode = 0;
        if unsafe { GetConsoleMode(handle, &mut original_mode) } == 0 {
            return None;
        }

        Some(Self {
            handle,
            original_mode,
        })
    }

    fn restore(&self) {
        use windows_sys::Win32::System::Console::SetConsoleMode;
        let _ = unsafe { SetConsoleMode(self.handle, self.original_mode) };
    }
}

#[cfg(windows)]
fn enable_windows_virtual_terminal_input() {
    // crossterm raw mode disables line/echo/processed input on Windows, but it
    // does not enable VT input. The Node REPL running inside the child PTY can
    // emit terminal queries such as ESC[6n; the terminal's response must reach
    // this supervisor as raw stdin bytes so it can be forwarded to the child.
    use windows_sys::Win32::System::Console::{
        ENABLE_VIRTUAL_TERMINAL_INPUT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE,
        SetConsoleMode,
    };

    let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let mut mode = 0;
    if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
        return;
    }

    let updated_mode = mode | ENABLE_VIRTUAL_TERMINAL_INPUT;
    if updated_mode != mode {
        let _ = unsafe { SetConsoleMode(handle, updated_mode) };
    }
}

#[derive(Clone, Default)]
struct DebugLog {
    enabled: bool,
    path: Option<PathBuf>,
}

impl DebugLog {
    fn from_env() -> Self {
        let enabled = std::env::var_os(GOLEM_REPL_PTY_DEBUG_ENV).is_some()
            || std::env::var_os(GOLEM_REPL_PTY_DEBUG_LOG_ENV).is_some();
        let path = std::env::var_os(GOLEM_REPL_PTY_DEBUG_LOG_ENV).map(PathBuf::from);
        Self { enabled, path }
    }

    fn log(&self, message: impl AsRef<str>) {
        if !self.enabled {
            return;
        }

        let line = format!("[golem-repl-pty] {}\n", message.as_ref());
        if let Some(path) = &self.path {
            let _ = append_log_line(path, &line);
        } else {
            let _ = std::io::stderr().write_all(line.as_bytes());
        }
    }
}

fn append_log_line(path: &Path, line: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    file.flush()?;
    Ok(())
}

fn format_exit_code(code: Option<i32>) -> String {
    match code {
        Some(code) if code < 0 => format!("{code} ({:#010X})", code as u32),
        Some(code) => code.to_string(),
        None => "<unknown>".to_string(),
    }
}
