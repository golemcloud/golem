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

use crate::context::Context;
use crate::model::app_raw::{BuiltinServer, Server};
use crate::tui::terminal::TerminalGuard;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::sync::Arc;

pub fn run(ctx: Arc<Context>) -> anyhow::Result<()> {
    let mut app = TuiApp::from_context(ctx.as_ref());
    let mut terminal = TerminalGuard::enter()?;

    terminal.draw(|frame| render(frame, &app))?;

    while !app.should_quit {
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                app.handle_key(key);
                terminal.draw(|frame| render(frame, &app))?;
            }
            Event::Resize(_, _) => {
                terminal.draw(|frame| render(frame, &app))?;
            }
            _ => {}
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct TuiApp {
    should_quit: bool,
    context: TuiContextInfo,
}

impl TuiApp {
    fn from_context(ctx: &Context) -> Self {
        Self {
            should_quit: false,
            context: TuiContextInfo::from_context(ctx),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone)]
struct TuiContextInfo {
    application: String,
    environment: String,
    server: String,
    config_dir: String,
}

impl TuiContextInfo {
    fn from_context(ctx: &Context) -> Self {
        let manifest_environment = ctx.manifest_environment();
        let application = manifest_environment
            .map(|environment| environment.application_name.0.clone())
            .unwrap_or_else(|| "no application manifest".to_string());
        let environment = manifest_environment
            .map(|environment| environment.environment_name.0.clone())
            .unwrap_or_else(|| "profile/default".to_string());
        let server = manifest_environment
            .map(|environment| {
                environment
                    .environment
                    .server
                    .as_ref()
                    .map(format_server)
                    .unwrap_or_else(|| "local".to_string())
            })
            .unwrap_or_else(|| ctx.worker_service_url().to_string());

        Self {
            application,
            environment,
            server,
            config_dir: ctx.config_dir().display().to_string(),
        }
    }
}

fn format_server(server: &Server) -> String {
    match server {
        Server::Builtin(BuiltinServer::Local) => "local".to_string(),
        Server::Builtin(BuiltinServer::Cloud) => "cloud".to_string(),
        Server::Custom(custom) => custom.url.to_string(),
    }
}

fn render(frame: &mut Frame<'_>, app: &TuiApp) {
    let [header, body, footer_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

    let title = Paragraph::new(Line::from(vec![
        Span::styled("Golem TUI", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(
            &app.context.environment,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, header);

    let dashboard = Paragraph::new(vec![
        Line::from(vec![Span::styled(
            "Dashboard",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::default(),
        Line::from(format!("Application : {}", app.context.application)),
        Line::from(format!("Environment : {}", app.context.environment)),
        Line::from(format!("Server      : {}", app.context.server)),
        Line::from(format!("Config dir  : {}", app.context.config_dir)),
        Line::default(),
        Line::from("Scaffold ready. Next steps: tabs, command palette, nested CLI jobs."),
    ])
    .block(Block::default().borders(Borders::ALL).title(" Overview "))
    .wrap(Wrap { trim: false });
    frame.render_widget(dashboard, body);

    let footer = Paragraph::new("q/Esc/Ctrl-C quit | Ctrl-P command palette soon")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(footer, footer_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use test_r::test;

    #[test]
    fn renders_dashboard_frame() {
        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = TuiApp {
            should_quit: false,
            context: TuiContextInfo {
                application: "sample-app".to_string(),
                environment: "local".to_string(),
                server: "local".to_string(),
                config_dir: "/tmp/golem-config".to_string(),
            },
        };

        terminal.draw(|frame| render(frame, &app)).unwrap();

        let frame = render_buffer_text(terminal.backend().buffer());
        assert!(frame.contains("Golem TUI"), "{frame}");
        assert!(frame.contains("sample-app"), "{frame}");
        assert!(frame.contains("Ctrl-P command palette soon"), "{frame}");
    }

    fn render_buffer_text(buffer: &Buffer) -> String {
        let area = buffer.area;
        let mut output = String::new();

        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    output.push_str(cell.symbol());
                }
            }
            output.push('\n');
        }

        output
    }
}
