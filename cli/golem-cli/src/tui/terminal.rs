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

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{Stdout, stdout};

pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    suspended: bool,
}

impl TerminalGuard {
    pub fn enter() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture, Hide) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = execute!(
                    std::io::stdout(),
                    Show,
                    DisableMouseCapture,
                    LeaveAlternateScreen
                );
                return Err(error.into());
            }
        };
        if let Err(error) = terminal.clear() {
            let _ = disable_raw_mode();
            let _ = execute!(
                terminal.backend_mut(),
                Show,
                DisableMouseCapture,
                LeaveAlternateScreen
            );
            return Err(error.into());
        }

        Ok(Self {
            terminal,
            suspended: false,
        })
    }

    pub fn draw<F>(&mut self, render_callback: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut ratatui::Frame<'_>),
    {
        if self.suspended {
            return Ok(());
        }
        self.terminal.draw(render_callback)?;
        Ok(())
    }

    pub fn suspend(&mut self) -> anyhow::Result<()> {
        if self.suspended {
            return Ok(());
        }
        let raw_mode_error = disable_raw_mode().err();
        let screen_error = execute!(
            self.terminal.backend_mut(),
            Show,
            DisableMouseCapture,
            LeaveAlternateScreen
        )
        .err();
        self.suspended = true;
        if let Some(error) = raw_mode_error {
            return Err(error.into());
        }
        if let Some(error) = screen_error {
            return Err(error.into());
        }
        Ok(())
    }

    pub fn resume(&mut self) -> anyhow::Result<()> {
        if !self.suspended {
            return Ok(());
        }
        if let Err(error) = enable_raw_mode() {
            self.restore_for_exit();
            return Err(error.into());
        }
        if let Err(error) = execute!(
            self.terminal.backend_mut(),
            EnterAlternateScreen,
            EnableMouseCapture,
            Hide
        ) {
            self.restore_for_exit();
            return Err(error.into());
        }
        if let Err(error) = self.terminal.clear() {
            self.restore_for_exit();
            return Err(error.into());
        }
        self.suspended = false;
        Ok(())
    }

    pub fn restore_for_exit(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            Show,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        self.suspended = true;
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore_for_exit();
    }
}
