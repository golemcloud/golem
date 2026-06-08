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
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
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
    active_view: TuiView,
    palette: CommandPalette,
    context: TuiContextInfo,
}

impl TuiApp {
    fn from_context(ctx: &Context) -> Self {
        Self {
            should_quit: false,
            active_view: TuiView::Dashboard,
            palette: CommandPalette::default(),
            context: TuiContextInfo::from_context(ctx),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if self.palette.open {
            self.handle_palette_key(key);
        } else {
            self.handle_global_key(key);
        }
    }

    fn handle_global_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.palette.open();
            }
            KeyCode::Char(':') => self.palette.open(),
            KeyCode::Char(']') | KeyCode::Tab => self.next_view(),
            KeyCode::Char('[') | KeyCode::BackTab => self.previous_view(),
            KeyCode::Char('1') => self.active_view = TuiView::Dashboard,
            KeyCode::Char('2') => self.active_view = TuiView::Environments,
            KeyCode::Char('3') => self.active_view = TuiView::Components,
            KeyCode::Char('4') => self.active_view = TuiView::Agents,
            KeyCode::Char('5') => self.active_view = TuiView::Output,
            _ => {}
        }
    }

    fn handle_palette_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.palette.close(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.palette.close();
            }
            KeyCode::Backspace => self.palette.backspace(),
            KeyCode::Down | KeyCode::Tab => {
                let action_count = filtered_actions(&self.palette.query).len();
                self.palette.next(action_count);
            }
            KeyCode::Up | KeyCode::BackTab => {
                let action_count = filtered_actions(&self.palette.query).len();
                self.palette.previous(action_count);
            }
            KeyCode::Enter => {
                if let Some(action) = filtered_actions(&self.palette.query)
                    .get(self.palette.selected)
                    .copied()
                {
                    self.execute_action(action.kind);
                    self.palette.close();
                }
            }
            KeyCode::Char(character)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.palette.push(character);
            }
            _ => {}
        }
    }

    fn execute_action(&mut self, action: TuiActionKind) {
        match action {
            TuiActionKind::SelectView(view) => self.active_view = view,
            TuiActionKind::Quit => self.should_quit = true,
        }
    }

    fn next_view(&mut self) {
        self.active_view = TuiView::from_index((self.active_view.index() + 1) % TuiView::ALL.len());
    }

    fn previous_view(&mut self) {
        self.active_view = TuiView::from_index(
            (self.active_view.index() + TuiView::ALL.len() - 1) % TuiView::ALL.len(),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiView {
    Dashboard,
    Environments,
    Components,
    Agents,
    Output,
}

impl TuiView {
    const ALL: [Self; 5] = [
        Self::Dashboard,
        Self::Environments,
        Self::Components,
        Self::Agents,
        Self::Output,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Environments => "Environments",
            Self::Components => "Components",
            Self::Agents => "Agents",
            Self::Output => "Output",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::Dashboard => "Selected context and quick actions.",
            Self::Environments => "Environment switching will appear here.",
            Self::Components => "Component monitoring will appear here.",
            Self::Agents => "Agent monitoring and management will appear here.",
            Self::Output => "Nested command output will appear here.",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|view| *view == self).unwrap_or(0)
    }

    fn from_index(index: usize) -> Self {
        Self::ALL[index]
    }
}

#[derive(Debug, Clone, Default)]
struct CommandPalette {
    open: bool,
    query: String,
    selected: usize,
}

impl CommandPalette {
    fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
    }

    fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selected = 0;
    }

    fn push(&mut self, character: char) {
        self.query.push(character);
        self.selected = 0;
    }

    fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    fn next(&mut self, action_count: usize) {
        if action_count > 0 {
            self.selected = (self.selected + 1) % action_count;
        }
    }

    fn previous(&mut self, action_count: usize) {
        if action_count > 0 {
            self.selected = (self.selected + action_count - 1) % action_count;
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
    let [header, tabs, body, footer_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
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

    render_tabs(frame, tabs, app.active_view);

    let dashboard = Paragraph::new(view_lines(app))
        .block(Block::default().borders(Borders::ALL).title(" Overview "))
        .wrap(Wrap { trim: false });
    frame.render_widget(dashboard, body);

    let footer =
        Paragraph::new("q/Esc/Ctrl-C quit | Ctrl-P/: command palette | [/] tabs | 1-5 jump")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
    frame.render_widget(footer, footer_area);

    if app.palette.open {
        render_palette(frame, app);
    }
}

fn render_tabs(frame: &mut Frame<'_>, area: Rect, active_view: TuiView) {
    let mut spans = Vec::new();

    for view in TuiView::ALL {
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }

        let title = if view == active_view {
            format!("[{}]", view.title())
        } else {
            format!(" {} ", view.title())
        };
        let style = if view == active_view {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(title, style));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn view_lines(app: &TuiApp) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![Span::styled(
            app.active_view.title(),
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::default(),
        Line::from(app.active_view.placeholder()),
    ];

    if app.active_view == TuiView::Dashboard {
        lines.extend([
            Line::default(),
            Line::from(format!("Application : {}", app.context.application)),
            Line::from(format!("Environment : {}", app.context.environment)),
            Line::from(format!("Server      : {}", app.context.server)),
            Line::from(format!("Config dir  : {}", app.context.config_dir)),
            Line::default(),
            Line::from("Scaffold ready. Next steps: command execution and live data."),
        ]);
    }

    lines
}

fn render_palette(frame: &mut Frame<'_>, app: &TuiApp) {
    let area = centered_rect(70, 50, frame.area());
    let actions = filtered_actions(&app.palette.query);
    let mut lines = vec![
        Line::from(vec![Span::styled(
            "Command Palette",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(format!("> {}", app.palette.query)),
        Line::default(),
    ];

    if actions.is_empty() {
        lines.push(Line::from("No matching commands"));
    } else {
        for (index, action) in actions.iter().take(8).enumerate() {
            let prefix = if index == app.palette.selected {
                "> "
            } else {
                "  "
            };
            let shortcut = action
                .shortcut
                .map(|shortcut| format!(" ({shortcut})"))
                .unwrap_or_default();
            let label = format!("{prefix}{}{}", action.label, shortcut);
            let style = if index == app.palette.selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{label:<28}"), style),
                Span::raw(action.description),
            ]));
        }
    }

    let block = Block::default().borders(Borders::ALL).title(" Search ");
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let [_, horizontal, _] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .areas(area);

    let [_, vertical, _] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .areas(horizontal);

    vertical
}

#[derive(Debug, Clone, Copy)]
struct TuiAction {
    label: &'static str,
    description: &'static str,
    shortcut: Option<&'static str>,
    kind: TuiActionKind,
}

#[derive(Debug, Clone, Copy)]
enum TuiActionKind {
    SelectView(TuiView),
    Quit,
}

const ACTIONS: [TuiAction; 6] = [
    TuiAction {
        label: "Go to Dashboard",
        description: "Switch to Dashboard view",
        shortcut: Some("1"),
        kind: TuiActionKind::SelectView(TuiView::Dashboard),
    },
    TuiAction {
        label: "Go to Environments",
        description: "Switch to Environments view",
        shortcut: Some("2"),
        kind: TuiActionKind::SelectView(TuiView::Environments),
    },
    TuiAction {
        label: "Go to Components",
        description: "Switch to Components view",
        shortcut: Some("3"),
        kind: TuiActionKind::SelectView(TuiView::Components),
    },
    TuiAction {
        label: "Go to Agents",
        description: "Switch to Agents view",
        shortcut: Some("4"),
        kind: TuiActionKind::SelectView(TuiView::Agents),
    },
    TuiAction {
        label: "Go to Output",
        description: "Switch to Output view",
        shortcut: Some("5"),
        kind: TuiActionKind::SelectView(TuiView::Output),
    },
    TuiAction {
        label: "Quit",
        description: "Exit the TUI",
        shortcut: Some("q"),
        kind: TuiActionKind::Quit,
    },
];

fn filtered_actions(query: &str) -> Vec<TuiAction> {
    let query = query.trim();
    if query.is_empty() {
        return ACTIONS.to_vec();
    }

    let matcher = SkimMatcherV2::default();
    let mut matches = ACTIONS
        .iter()
        .filter_map(|action| {
            let haystack = format!("{} {}", action.label, action.description);
            matcher
                .fuzzy_match(&haystack, query)
                .map(|score| (score, *action))
        })
        .collect::<Vec<_>>();

    matches.sort_by(|(left, _), (right, _)| right.cmp(left));
    matches.into_iter().map(|(_, action)| action).collect()
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
        let app = test_app();
        let frame = render_app_text(&app);

        assert!(frame.contains("Golem TUI"), "{frame}");
        assert!(frame.contains("sample-app"), "{frame}");
        assert!(frame.contains("Ctrl-P/: command palette"), "{frame}");
    }

    #[test]
    fn renders_active_tab() {
        let app = test_app();
        let frame = render_app_text(&app);

        assert!(frame.contains("[Dashboard]"), "{frame}");
        assert!(frame.contains("Environments"), "{frame}");
    }

    #[test]
    fn switches_tabs_with_keys() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char(']')));
        let frame = render_app_text(&app);
        assert!(frame.contains("[Environments]"), "{frame}");

        app.handle_key(key(KeyCode::Char('[')));
        let frame = render_app_text(&app);
        assert!(frame.contains("[Dashboard]"), "{frame}");
    }

    #[test]
    fn jumps_to_tab_with_number() {
        let mut app = test_app();

        app.handle_key(key(KeyCode::Char('3')));
        let frame = render_app_text(&app);

        assert!(frame.contains("[Components]"), "{frame}");
        assert!(
            frame.contains("Component monitoring will appear here."),
            "{frame}"
        );
    }

    #[test]
    fn opens_palette() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        let frame = render_app_text(&app);

        assert!(frame.contains("Command Palette"), "{frame}");
        assert!(frame.contains("Go to Dashboard"), "{frame}");
    }

    #[test]
    fn filters_palette() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "comp".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        let frame = render_app_text(&app);

        assert!(frame.contains("> comp"), "{frame}");
        assert!(frame.contains("Go to Components"), "{frame}");
    }

    #[test]
    fn executes_palette_action() {
        let mut app = test_app();

        app.handle_key(modified_key(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for character in "comp".chars() {
            app.handle_key(key(KeyCode::Char(character)));
        }
        app.handle_key(key(KeyCode::Enter));

        let frame = render_app_text(&app);
        assert!(frame.contains("[Components]"), "{frame}");
        assert!(!frame.contains("Command Palette"), "{frame}");
    }

    fn test_app() -> TuiApp {
        TuiApp {
            should_quit: false,
            active_view: TuiView::Dashboard,
            palette: CommandPalette::default(),
            context: TuiContextInfo {
                application: "sample-app".to_string(),
                environment: "local".to_string(),
                server: "local".to_string(),
                config_dir: "/tmp/golem-config".to_string(),
            },
        }
    }

    fn render_app_text(app: &TuiApp) -> String {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| render(frame, app)).unwrap();

        render_buffer_text(terminal.backend().buffer())
    }

    fn key(code: KeyCode) -> KeyEvent {
        modified_key(code, KeyModifiers::empty())
    }

    fn modified_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
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
