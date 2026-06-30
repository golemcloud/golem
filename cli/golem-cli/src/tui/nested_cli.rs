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

use crate::tui::TuiEvent;
use anyhow::Context;
use portable_pty::{ChildKiller, CommandBuilder, PtyPair, PtySize, native_pty_system};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use tokio::sync::mpsc::{self, Sender};

const PTY_CONTROL_CHANNEL_CAPACITY: usize = 128;

pub struct NestedCliSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
}

#[derive(Clone, Copy)]
pub enum NestedCliTarget {
    Command,
    Server,
    Repl,
    AgentOplog,
    AgentStream,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandExit {
    pub code: Option<i32>,
    pub success: bool,
}

pub struct NestedCliRuntime {
    control_tx: mpsc::Sender<NestedCliControl>,
}

impl NestedCliRuntime {
    pub fn write_all(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.send_control(NestedCliControl::Write(bytes.to_vec()))
    }

    pub fn send_ctrl_c(&mut self) -> anyhow::Result<()> {
        self.send_control(NestedCliControl::Write(vec![3]))
    }

    pub fn kill(&mut self) -> anyhow::Result<()> {
        self.send_control(NestedCliControl::Kill)
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.send_control(NestedCliControl::Resize { cols, rows })
    }

    fn send_control(&self, command: NestedCliControl) -> anyhow::Result<()> {
        self.control_tx
            .try_send(command)
            .context("Failed to queue nested CLI control command")
    }
}

enum NestedCliControl {
    Write(Vec<u8>),
    Kill,
    Resize { cols: u16, rows: u16 },
}

struct NestedCliControlRuntime {
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    pty_pair: PtyPair,
}

pub fn spawn_nested_cli(
    spec: NestedCliSpec,
    event_tx: Sender<TuiEvent>,
    target: NestedCliTarget,
) -> anyhow::Result<NestedCliRuntime> {
    let pty_system = native_pty_system();
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("Failed to open PTY for nested CLI")?;

    let mut command = CommandBuilder::new(spec.program);
    command.args(spec.args);
    command.cwd(spec.cwd);
    for (key, value) in spec.env {
        command.env(key, value);
    }

    let mut child = pair
        .slave
        .spawn_command(command)
        .context("Failed to spawn nested CLI")?;
    let killer = child.clone_killer();
    let mut reader = pair
        .master
        .try_clone_reader()
        .context("Failed to clone nested CLI PTY reader")?;
    let writer = pair
        .master
        .take_writer()
        .context("Failed to take nested CLI PTY writer")?;

    let (control_tx, control_rx) = mpsc::channel::<NestedCliControl>(PTY_CONTROL_CHANNEL_CAPACITY);

    spawn_blocking_pty_reader({
        let event_tx = event_tx.clone();
        move || {
            let mut buffer = [0_u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        let _ = event_tx.blocking_send(output_closed_event(target, None));
                        return;
                    }
                    Ok(n) => {
                        if event_tx
                            .blocking_send(output_event(target, buffer[..n].to_vec()))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = event_tx
                            .blocking_send(output_closed_event(target, Some(error.to_string())));
                        return;
                    }
                }
            }
        }
    });

    spawn_blocking_pty_waiter(move || {
        if let Ok(status) = child.wait() {
            let code = Some(status.exit_code() as i32);
            let exit = CommandExit {
                code,
                success: code == Some(0),
            };
            let _ = event_tx.blocking_send(exit_event(target, exit));
        }
    });

    spawn_blocking_pty_control(
        NestedCliControlRuntime {
            writer,
            killer,
            pty_pair: pair,
        },
        control_rx,
    );

    Ok(NestedCliRuntime { control_tx })
}

fn spawn_blocking_pty_reader(read_loop: impl FnOnce() + Send + 'static) {
    tokio::task::spawn_blocking(read_loop);
}

fn spawn_blocking_pty_waiter(wait_loop: impl FnOnce() + Send + 'static) {
    tokio::task::spawn_blocking(wait_loop);
}

fn spawn_blocking_pty_control(
    mut runtime: NestedCliControlRuntime,
    mut control_rx: mpsc::Receiver<NestedCliControl>,
) {
    tokio::task::spawn_blocking(move || {
        while let Some(command) = control_rx.blocking_recv() {
            match command {
                NestedCliControl::Write(bytes) => {
                    let _ = runtime.writer.write_all(&bytes);
                    let _ = runtime.writer.flush();
                }
                NestedCliControl::Kill => {
                    let _ = runtime.killer.kill();
                }
                NestedCliControl::Resize { cols, rows } => {
                    let _ = runtime.pty_pair.master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
            }
        }
    });
}

fn output_event(target: NestedCliTarget, bytes: Vec<u8>) -> TuiEvent {
    match target {
        NestedCliTarget::Command => TuiEvent::CommandOutput(bytes),
        NestedCliTarget::Server => TuiEvent::ServerOutput(bytes),
        NestedCliTarget::Repl => TuiEvent::ReplOutput(bytes),
        NestedCliTarget::AgentOplog => TuiEvent::AgentOplogOutput(bytes),
        NestedCliTarget::AgentStream => TuiEvent::AgentStreamOutput(bytes),
    }
}

fn output_closed_event(target: NestedCliTarget, error: Option<String>) -> TuiEvent {
    match target {
        NestedCliTarget::Command => TuiEvent::CommandOutputClosed(error),
        NestedCliTarget::Server => TuiEvent::ServerOutputClosed(error),
        NestedCliTarget::Repl => TuiEvent::ReplOutputClosed(error),
        NestedCliTarget::AgentOplog => TuiEvent::AgentOplogOutputClosed(error),
        NestedCliTarget::AgentStream => TuiEvent::AgentStreamOutputClosed(error),
    }
}

fn exit_event(target: NestedCliTarget, exit: CommandExit) -> TuiEvent {
    match target {
        NestedCliTarget::Command => TuiEvent::CommandExited(exit),
        NestedCliTarget::Server => TuiEvent::ServerExited(exit),
        NestedCliTarget::Repl => TuiEvent::ReplExited(exit),
        NestedCliTarget::AgentOplog => TuiEvent::AgentOplogExited(exit),
        NestedCliTarget::AgentStream => TuiEvent::AgentStreamExited(exit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn maps_output_events_by_target() {
        assert!(matches!(
            output_event(NestedCliTarget::Command, b"cmd".to_vec()),
            TuiEvent::CommandOutput(bytes) if bytes == b"cmd"
        ));
        assert!(matches!(
            output_event(NestedCliTarget::Server, b"server".to_vec()),
            TuiEvent::ServerOutput(bytes) if bytes == b"server"
        ));
        assert!(matches!(
            output_event(NestedCliTarget::Repl, b"repl".to_vec()),
            TuiEvent::ReplOutput(bytes) if bytes == b"repl"
        ));
        assert!(matches!(
            output_event(NestedCliTarget::AgentOplog, b"oplog".to_vec()),
            TuiEvent::AgentOplogOutput(bytes) if bytes == b"oplog"
        ));
        assert!(matches!(
            output_event(NestedCliTarget::AgentStream, b"stream".to_vec()),
            TuiEvent::AgentStreamOutput(bytes) if bytes == b"stream"
        ));
    }

    #[test]
    fn maps_closed_events_by_target() {
        assert!(matches!(
            output_closed_event(NestedCliTarget::Command, Some("closed".to_string())),
            TuiEvent::CommandOutputClosed(Some(error)) if error == "closed"
        ));
        assert!(matches!(
            output_closed_event(NestedCliTarget::Server, None),
            TuiEvent::ServerOutputClosed(None)
        ));
        assert!(matches!(
            output_closed_event(NestedCliTarget::Repl, None),
            TuiEvent::ReplOutputClosed(None)
        ));
        assert!(matches!(
            output_closed_event(NestedCliTarget::AgentOplog, None),
            TuiEvent::AgentOplogOutputClosed(None)
        ));
        assert!(matches!(
            output_closed_event(NestedCliTarget::AgentStream, None),
            TuiEvent::AgentStreamOutputClosed(None)
        ));
    }

    #[test]
    fn maps_exit_events_by_target() {
        let exit = CommandExit {
            code: Some(0),
            success: true,
        };

        assert!(matches!(
            exit_event(NestedCliTarget::Command, exit),
            TuiEvent::CommandExited(value) if value == exit
        ));
        assert!(matches!(
            exit_event(NestedCliTarget::Server, exit),
            TuiEvent::ServerExited(value) if value == exit
        ));
        assert!(matches!(
            exit_event(NestedCliTarget::Repl, exit),
            TuiEvent::ReplExited(value) if value == exit
        ));
        assert!(matches!(
            exit_event(NestedCliTarget::AgentOplog, exit),
            TuiEvent::AgentOplogExited(value) if value == exit
        ));
        assert!(matches!(
            exit_event(NestedCliTarget::AgentStream, exit),
            TuiEvent::AgentStreamExited(value) if value == exit
        ));
    }

    #[test]
    fn runtime_queues_write_control_commands() {
        let (control_tx, mut control_rx) = mpsc::channel(1);
        let mut runtime = NestedCliRuntime { control_tx };

        runtime.write_all(b"input").expect("queue write");

        assert!(matches!(
            control_rx.try_recv().expect("control command"),
            NestedCliControl::Write(bytes) if bytes == b"input"
        ));
    }

    #[test]
    fn runtime_queues_kill_and_resize_control_commands() {
        let (control_tx, mut control_rx) = mpsc::channel(2);
        let mut runtime = NestedCliRuntime { control_tx };

        runtime.kill().expect("queue kill");
        runtime.resize(120, 40).expect("queue resize");

        assert!(matches!(
            control_rx.try_recv().expect("kill command"),
            NestedCliControl::Kill
        ));
        assert!(matches!(
            control_rx.try_recv().expect("resize command"),
            NestedCliControl::Resize {
                cols: 120,
                rows: 40
            }
        ));
    }
}
