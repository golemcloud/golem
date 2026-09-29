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

use ratatui::layout::Position;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub struct TerminalScreen {
    parser: vt100::Parser,
    max_scrollback: usize,
    has_output: bool,
}

impl TerminalScreen {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, 2000),
            max_scrollback: 0,
            has_output: false,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.has_output |= !bytes.is_empty();
        self.parser.process(bytes);
        self.update_max_scrollback();
    }

    pub fn feed_local_line(&mut self, line: impl AsRef<str>) {
        self.feed(format!("\x1b[2m{}\x1b[0m\r\n", line.as_ref()).as_bytes());
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
        self.update_max_scrollback();
    }

    pub fn scroll_up(&mut self, rows: usize) {
        let offset = self.parser.screen().scrollback().saturating_add(rows);
        self.parser.screen_mut().set_scrollback(offset);
    }

    pub fn scroll_down(&mut self, rows: usize) {
        let offset = self.parser.screen().scrollback().saturating_sub(rows);
        self.parser.screen_mut().set_scrollback(offset);
    }

    pub fn scroll_top(&mut self) {
        self.parser.screen_mut().set_scrollback(usize::MAX);
    }

    pub fn scroll_bottom(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    pub fn scrollback(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub fn max_scrollback(&self) -> usize {
        self.max_scrollback
    }

    pub fn total_lines(&self) -> usize {
        let (rows, _) = self.parser.screen().size();
        self.max_scrollback.saturating_add(rows as usize)
    }

    pub fn is_empty(&self) -> bool {
        !self.has_output
    }

    fn update_max_scrollback(&mut self) {
        let current = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(usize::MAX);
        self.max_scrollback = self.parser.screen().scrollback();
        self.parser.screen_mut().set_scrollback(current);
    }

    pub fn cursor_position(&self, origin: Position) -> Option<Position> {
        if self.parser.screen().hide_cursor() || self.scrollback() > 0 {
            return None;
        }

        let (row, col) = self.parser.screen().cursor_position();
        Some(Position {
            x: origin.x.saturating_add(col),
            y: origin.y.saturating_add(row),
        })
    }

    pub fn lines(&self) -> Vec<Line<'static>> {
        let (rows, cols) = self.parser.screen().size();
        (0..rows)
            .map(|row| {
                let mut spans = Vec::new();
                for col in 0..cols {
                    let Some(cell) = self.parser.screen().cell(row, col) else {
                        spans.push(Span::raw(" "));
                        continue;
                    };
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let content = if cell.has_contents() {
                        cell.contents().to_string()
                    } else {
                        " ".to_string()
                    };
                    spans.push(Span::styled(content, cell_style(cell)));
                }
                Line::from(spans)
            })
            .collect()
    }

    #[cfg(test)]
    pub fn plain_text(&self) -> String {
        self.parser.screen().contents()
    }
}

fn cell_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();

    if let Some(color) = vt_color(cell.fgcolor()) {
        style = style.fg(color);
    }
    if let Some(color) = vt_color(cell.bgcolor()) {
        style = style.bg(color);
    }
    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.dim() {
        style = style.add_modifier(Modifier::DIM);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }

    style
}

fn vt_color(color: vt100::Color) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
        vt100::Color::Idx(index) => Some(match index {
            0 => Color::Black,
            1 => Color::Red,
            2 => Color::Green,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Magenta,
            6 => Color::Cyan,
            7 => Color::Gray,
            8 => Color::DarkGray,
            9 => Color::LightRed,
            10 => Color::LightGreen,
            11 => Color::LightYellow,
            12 => Color::LightBlue,
            13 => Color::LightMagenta,
            14 => Color::LightCyan,
            15 => Color::White,
            index => Color::Indexed(index),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn feeds_plain_text() {
        let mut screen = TerminalScreen::new(3, 20);

        screen.feed(b"hello");

        assert!(screen.plain_text().contains("hello"));
        assert!(screen.lines()[0].to_string().contains("hello"));
    }

    #[test]
    fn handles_cursor_movement() {
        let mut screen = TerminalScreen::new(3, 20);

        screen.feed(b"hello\x1b[2DXY");

        assert!(screen.plain_text().contains("helXY"));
    }

    #[test]
    fn exposes_cursor_position() {
        let mut screen = TerminalScreen::new(3, 20);

        screen.feed(b"abc");

        assert_eq!(
            screen.cursor_position(Position::new(10, 5)),
            Some(Position::new(13, 5))
        );
    }

    #[test]
    fn renders_sgr_styles() {
        let mut screen = TerminalScreen::new(3, 20);

        screen.feed(b"\x1b[31;1mred");

        let line = &screen.lines()[0];
        assert_eq!(
            line.spans[0].style,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        );
    }

    #[test]
    fn resizes_screen() {
        let mut screen = TerminalScreen::new(3, 20);

        screen.resize(4, 10);

        assert_eq!(screen.lines().len(), 4);
    }

    #[test]
    fn exposes_bounded_scrollback_navigation() {
        let mut screen = TerminalScreen::new(2, 10);
        screen.feed(b"one\r\ntwo\r\nthree\r\nfour");

        screen.scroll_up(1);
        assert_eq!(screen.scrollback(), 1);
        assert!(screen.plain_text().contains("two"));
        assert!(screen.cursor_position(Position::new(0, 0)).is_none());

        screen.scroll_top();
        assert_eq!(screen.scrollback(), 2);
        screen.scroll_down(1);
        assert_eq!(screen.scrollback(), 1);
        screen.scroll_bottom();
        assert_eq!(screen.scrollback(), 0);
        assert!(screen.plain_text().contains("four"));
    }
}
