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
use std::sync::mpsc::Sender;
use std::thread;

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandExit {
    pub code: Option<i32>,
    pub success: bool,
}

pub struct NestedCliRuntime {
    writer: Box<dyn Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    pty_pair: PtyPair,
}

impl NestedCliRuntime {
    pub fn write_all(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn send_ctrl_c(&mut self) -> anyhow::Result<()> {
        self.write_all(&[3])
    }

    pub fn kill(&mut self) -> anyhow::Result<()> {
        self.killer.kill().context("Failed to kill nested CLI")
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.pty_pair.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
    }
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

    thread::spawn({
        let event_tx = event_tx.clone();
        move || {
            let mut buffer = [0_u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        let _ = event_tx.send(match target {
                            NestedCliTarget::Command => TuiEvent::CommandOutputClosed(None),
                            NestedCliTarget::Server => TuiEvent::ServerOutputClosed(None),
                        });
                        return;
                    }
                    Ok(n) => {
                        if event_tx
                            .send(match target {
                                NestedCliTarget::Command => {
                                    TuiEvent::CommandOutput(buffer[..n].to_vec())
                                }
                                NestedCliTarget::Server => {
                                    TuiEvent::ServerOutput(buffer[..n].to_vec())
                                }
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = event_tx.send(match target {
                            NestedCliTarget::Command => {
                                TuiEvent::CommandOutputClosed(Some(error.to_string()))
                            }
                            NestedCliTarget::Server => {
                                TuiEvent::ServerOutputClosed(Some(error.to_string()))
                            }
                        });
                        return;
                    }
                }
            }
        }
    });

    thread::spawn(move || {
        if let Ok(status) = child.wait() {
            let code = Some(status.exit_code() as i32);
            let exit = CommandExit {
                code,
                success: code == Some(0),
            };
            let _ = event_tx.send(match target {
                NestedCliTarget::Command => TuiEvent::CommandExited(exit),
                NestedCliTarget::Server => TuiEvent::ServerExited(exit),
            });
        }
    });

    Ok(NestedCliRuntime {
        writer,
        killer,
        pty_pair: pair,
    })
}
