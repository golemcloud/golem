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

use super::{CommandExit, ReplCommandSpec};
use anyhow::Context;
use portable_pty::{ChildKiller, CommandBuilder, PtyPair, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread;

pub struct PtyChild {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub killer: Box<dyn ChildKiller + Send + Sync>,
    pub exit_receiver: Receiver<CommandExit>,
    pub pair: PtyPair,
}

pub fn spawn_pty_command(spec: ReplCommandSpec) -> anyhow::Result<PtyChild> {
    let pty_system = native_pty_system();
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("Failed to open PTY")?;

    let mut command = CommandBuilder::new(spec.program);
    command.args(spec.args);
    command.cwd(spec.cwd);
    for (key, value) in spec.env {
        command.env(key, value);
    }

    let mut child = pair
        .slave
        .spawn_command(command)
        .context("Failed to spawn PTY command")?;
    let killer = child.clone_killer();
    let reader = pair
        .master
        .try_clone_reader()
        .context("Failed to clone PTY reader")?;
    let writer = pair
        .master
        .take_writer()
        .context("Failed to take PTY writer")?;

    let (exit_tx, exit_rx) = mpsc::channel();
    thread::spawn(move || {
        if let Ok(status) = child.wait() {
            let code = Some(status.exit_code() as i32);
            let _ = exit_tx.send(CommandExit {
                code,
                success: code == Some(0),
            });
        }
    });

    Ok(PtyChild {
        reader,
        writer,
        killer,
        exit_receiver: exit_rx,
        pair,
    })
}
