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

//! Semantic rendering primitives for the accepted TUI design language.
//!
//! Preview stories supply content and application state. These components own
//! the corresponding glyphs, spacing, alignment, and visual treatment so a
//! locked decision has one executable representation.

use crate::tui::visual::TuiVisualStyle;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Widget, Wrap,
};
use unicode_width::UnicodeWidthStr;

pub(super) fn fit(text: &str, width: usize) -> String {
    let display_width = UnicodeWidthStr::width(text);
    if display_width <= width {
        return format!("{text}{}", " ".repeat(width - display_width));
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }
    let target_width = width - 1;
    let mut fitted = String::new();
    for character in text.chars() {
        let mut candidate = fitted.clone();
        candidate.push(character);
        if UnicodeWidthStr::width(candidate.as_str()) > target_width {
            break;
        }
        fitted = candidate;
    }
    let fitted_width = UnicodeWidthStr::width(fitted.as_str());
    format!("{fitted}…{}", " ".repeat(target_width - fitted_width))
}

pub(super) struct SectionHeading<'a> {
    pub title: &'a str,
    pub detail: Option<&'a str>,
}

impl SectionHeading<'_> {
    pub fn line(self, visual: &TuiVisualStyle) -> Line<'static> {
        let mut spans = vec![Span::styled(
            self.title.to_string(),
            Style::default()
                .fg(visual.text)
                .add_modifier(Modifier::BOLD),
        )];
        if let Some(detail) = self.detail {
            spans.push(Span::styled(
                format!("  {detail}"),
                Style::default().fg(visual.text_muted),
            ));
        }
        Line::from(spans)
    }
}

pub(super) struct FieldRow<'a> {
    pub label: &'a str,
    pub value: &'a str,
    pub label_width: usize,
}

impl FieldRow<'_> {
    pub fn line(self, visual: &TuiVisualStyle) -> Line<'static> {
        Line::from(vec![
            Span::styled(
                fit(self.label, self.label_width),
                Style::default().fg(visual.text_muted),
            ),
            Span::raw("  "),
            Span::styled(self.value.to_string(), Style::default().fg(visual.text)),
        ])
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StatusKind {
    Running,
    Idle,
    Failed,
    Attention,
}

impl StatusKind {
    fn glyph(self) -> &'static str {
        match self {
            Self::Running => "●",
            Self::Idle => "○",
            Self::Failed => "×",
            Self::Attention => "!",
        }
    }

    fn style(self, visual: &TuiVisualStyle) -> Style {
        Style::default().fg(match self {
            Self::Running => visual.success,
            Self::Idle => visual.text_muted,
            Self::Failed => visual.error,
            Self::Attention => visual.marker,
        })
    }
}

pub(super) struct StatusMarker<'a> {
    pub kind: StatusKind,
    pub label: &'a str,
}

impl StatusMarker<'_> {
    pub fn spans(self, visual: &TuiVisualStyle) -> [Span<'static>; 2] {
        [
            Span::styled(self.kind.glyph(), self.kind.style(visual)),
            Span::styled(format!(" {}", self.label), Style::default().fg(visual.text)),
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NoticeKind {
    Info,
    Active,
    Success,
    Loading,
    Warning,
    Error,
    Unavailable,
    Empty,
}

pub(super) struct Notice<'a> {
    pub kind: NoticeKind,
    pub message: &'a str,
}

impl Notice<'_> {
    pub fn line(self, visual: &TuiVisualStyle) -> Line<'static> {
        let (label, glyph, color) = match self.kind {
            NoticeKind::Info => ("Notice", "i", visual.text_secondary),
            NoticeKind::Active => ("Active", "●", visual.success),
            NoticeKind::Success => ("Success", "✓", visual.success),
            NoticeKind::Loading => ("Loading", "…", visual.accent),
            NoticeKind::Warning => ("Warning", "!", visual.marker),
            NoticeKind::Error => ("Error", "×", visual.error),
            NoticeKind::Unavailable => ("Unavailable", "—", visual.text_muted),
            NoticeKind::Empty => ("Empty", "○", visual.text_muted),
        };
        Line::from(vec![
            Span::styled(glyph, Style::default().fg(color)),
            Span::styled(
                format!(" {label}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {}", self.message),
                Style::default().fg(visual.text_secondary),
            ),
        ])
    }
}

pub(super) struct ContentTableRow<'a> {
    pub cells: &'a [&'a str],
    pub widths: &'a [usize],
    pub header: bool,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CellPolicy {
    Ellipsis,
    WrapSelected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TableDecoration {
    Minimal,
    Rules,
    Zebra,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PaneTableColumn<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub width: u16,
    pub required: bool,
    pub default_visible: bool,
    pub policy: CellPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PaneTableState {
    pub selected: usize,
    pub horizontal_offset: u16,
    visible_columns: Vec<String>,
}

impl PaneTableState {
    pub fn new(columns: &[PaneTableColumn<'_>], selected: usize) -> Self {
        Self {
            selected,
            horizontal_offset: 0,
            visible_columns: columns
                .iter()
                .filter(|column| column.required || column.default_visible)
                .map(|column| column.id.to_string())
                .collect(),
        }
    }

    pub fn column_visible(&self, id: &str) -> bool {
        self.visible_columns.iter().any(|visible| visible == id)
    }

    pub fn set_column_visible(&mut self, columns: &[PaneTableColumn<'_>], id: &str, visible: bool) {
        let Some(column) = columns.iter().find(|column| column.id == id) else {
            return;
        };
        if column.required && !visible {
            return;
        }
        self.visible_columns.retain(|visible_id| visible_id != id);
        if visible {
            self.visible_columns.push(id.to_string());
        }
        self.horizontal_offset = 0;
    }

    #[allow(dead_code)]
    pub fn pan_left(&mut self) {
        self.horizontal_offset = self.horizontal_offset.saturating_sub(1);
    }

    #[allow(dead_code)]
    pub fn pan_right(&mut self, columns: &[PaneTableColumn<'_>], viewport_width: u16) {
        self.horizontal_offset = self
            .horizontal_offset
            .saturating_add(1)
            .min(table_virtual_width(columns, self).saturating_sub(viewport_width));
    }

    #[allow(dead_code)]
    pub fn clamp_offset(&mut self, columns: &[PaneTableColumn<'_>], viewport_width: u16) {
        self.horizontal_offset = self
            .horizontal_offset
            .min(table_virtual_width(columns, self).saturating_sub(viewport_width));
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ColumnChooserState {
    draft: PaneTableState,
    pub selected: usize,
}

impl ColumnChooserState {
    pub fn new(table: &PaneTableState) -> Self {
        Self {
            draft: table.clone(),
            selected: 0,
        }
    }

    #[allow(dead_code)]
    pub fn toggle(&mut self, columns: &[PaneTableColumn<'_>], index: usize) {
        let Some(column) = columns.get(index) else {
            return;
        };
        let visible = !self.draft.column_visible(column.id);
        self.draft.set_column_visible(columns, column.id, visible);
    }

    #[allow(dead_code)]
    pub fn apply(self, table: &mut PaneTableState) {
        *table = self.draft;
    }

    pub fn lines(
        &self,
        columns: &[PaneTableColumn<'_>],
        visual: &TuiVisualStyle,
    ) -> Vec<Line<'static>> {
        std::iter::once(
            DecisionTableRow {
                cells: &["Column", "Visibility"],
                widths: &[24, 12],
                header: true,
                selectable: true,
                selected: false,
            }
            .line(visual),
        )
        .chain(columns.iter().enumerate().map(|(index, column)| {
            let visibility = if column.required {
                "required"
            } else if self.draft.column_visible(column.id) {
                "☑ shown"
            } else {
                "☐ hidden"
            };
            DecisionTableRow {
                cells: &[column.title, visibility],
                widths: &[24, 12],
                header: false,
                selectable: true,
                selected: index == self.selected,
            }
            .line(visual)
        }))
        .collect()
    }
}

pub(super) struct PaneTable<'a> {
    pub columns: &'a [PaneTableColumn<'a>],
    pub rows: &'a [&'a [&'a str]],
    pub state: &'a PaneTableState,
    pub decoration: TableDecoration,
}

impl PaneTable<'_> {
    pub fn height(&self) -> usize {
        1 + self
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| self.row_height(index, row))
            .sum::<usize>()
    }

    pub fn render(self, frame: &mut Frame<'_>, area: Rect, visual: &TuiVisualStyle) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let marker_width = area.width.min(2);
        let data_area = Rect {
            x: area.x.saturating_add(marker_width),
            width: area.width.saturating_sub(marker_width),
            ..area
        };
        let virtual_width = table_virtual_width(self.columns, self.state);
        let virtual_area = Rect::new(0, 0, virtual_width.max(1), area.height);
        let mut virtual_buffer = Buffer::empty(virtual_area);
        let mut lines = vec![
            self.data_line(
                &self
                    .columns
                    .iter()
                    .map(|column| column.title)
                    .collect::<Vec<_>>(),
                true,
                false,
                false,
                visual,
            ),
        ];
        let mut markers = vec![(false, false)];
        for (index, row) in self.rows.iter().enumerate() {
            let selected = index == self.state.selected;
            let wrapped = self.row_lines(index, row);
            for values in wrapped {
                lines.push(self.data_line(&values, false, selected, index % 2 == 1, visual));
                markers.push((selected, index % 2 == 1));
            }
        }
        Paragraph::new(lines).render(virtual_area, &mut virtual_buffer);
        let offset = self
            .state
            .horizontal_offset
            .min(virtual_width.saturating_sub(data_area.width));
        for row in 0..area.height {
            let (selected, alternate) = markers.get(row as usize).copied().unwrap_or_default();
            let background = table_row_background(selected, alternate, self.decoration, visual);
            for x in 0..marker_width {
                let cell = &mut frame.buffer_mut()[(area.x + x, area.y + row)];
                cell.set_symbol(if selected && x == 0 { "▌" } else { " " })
                    .set_style(
                        Style::default()
                            .fg(if selected {
                                visual.table_selection_text
                            } else {
                                visual.text_muted
                            })
                            .bg(background),
                    );
            }
            for x in 0..data_area.width {
                let source_x = offset.saturating_add(x);
                let target = &mut frame.buffer_mut()[(data_area.x + x, data_area.y + row)];
                if source_x < virtual_width {
                    *target = virtual_buffer[(source_x, row)].clone();
                } else {
                    target.set_symbol(" ").set_bg(background);
                }
            }
        }
    }

    fn visible_columns(&self) -> impl Iterator<Item = (usize, &PaneTableColumn<'_>)> {
        self.columns
            .iter()
            .enumerate()
            .filter(|(_, column)| self.state.column_visible(column.id))
    }

    fn row_height(&self, index: usize, row: &[&str]) -> usize {
        if index != self.state.selected || self.decoration != TableDecoration::Zebra {
            return 1;
        }
        self.visible_columns()
            .filter(|(_, column)| column.policy == CellPolicy::WrapSelected)
            .map(|(index, column)| {
                wrap_cell(row.get(index).copied().unwrap_or(""), column.width).len()
            })
            .max()
            .unwrap_or(1)
    }

    fn row_lines(&self, index: usize, row: &[&str]) -> Vec<Vec<String>> {
        let height = self.row_height(index, row);
        (0..height)
            .map(|line_index| {
                self.columns
                    .iter()
                    .enumerate()
                    .map(|(column_index, column)| {
                        let value = row.get(column_index).copied().unwrap_or("");
                        if index == self.state.selected
                            && self.decoration == TableDecoration::Zebra
                            && column.policy == CellPolicy::WrapSelected
                        {
                            wrap_cell(value, column.width)
                                .get(line_index)
                                .cloned()
                                .unwrap_or_default()
                        } else if line_index == 0 {
                            fit(value, column.width as usize)
                        } else {
                            String::new()
                        }
                    })
                    .collect()
            })
            .collect()
    }

    fn data_line(
        &self,
        values: &[impl AsRef<str>],
        header: bool,
        selected: bool,
        alternate: bool,
        visual: &TuiVisualStyle,
    ) -> Line<'static> {
        let background = table_row_background(selected, alternate, self.decoration, visual);
        let mut spans = Vec::new();
        for (visible_index, (column_index, column)) in self.visible_columns().enumerate() {
            if visible_index > 0 {
                let separator = match self.decoration {
                    TableDecoration::Rules => "│",
                    TableDecoration::Minimal | TableDecoration::Zebra => " ",
                };
                spans.push(Span::styled(
                    separator,
                    Style::default().fg(visual.border_subtle).bg(background),
                ));
            }
            let value = values.get(column_index).map(AsRef::as_ref).unwrap_or("");
            let color = if selected {
                visual.table_selection_text
            } else if header {
                visual.text_muted
            } else if column_index == 0 {
                visual.text
            } else {
                visual.text_secondary
            };
            spans.push(Span::styled(
                fit(value, column.width as usize),
                Style::default()
                    .fg(color)
                    .bg(background)
                    .add_modifier(if header {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ));
        }
        Line::from(spans)
    }
}

fn table_virtual_width(columns: &[PaneTableColumn<'_>], state: &PaneTableState) -> u16 {
    let visible = columns
        .iter()
        .filter(|column| state.column_visible(column.id));
    let count = visible.clone().count() as u16;
    visible
        .map(|column| column.width)
        .fold(0_u16, u16::saturating_add)
        .saturating_add(count.saturating_sub(1))
}

fn table_row_background(
    selected: bool,
    alternate: bool,
    decoration: TableDecoration,
    visual: &TuiVisualStyle,
) -> ratatui::style::Color {
    if decoration != TableDecoration::Zebra {
        return if selected {
            visual.table_selected_odd_background
        } else {
            visual.surface
        };
    }
    match (alternate, selected) {
        (false, false) => visual.table_odd_background,
        (true, false) => visual.table_even_background,
        (false, true) => visual.table_selected_odd_background,
        (true, true) => visual.table_selected_even_background,
    }
}

fn wrap_cell(text: &str, width: u16) -> Vec<String> {
    let width = width as usize;
    if width == 0 {
        return vec![String::new()];
    }
    if text.is_empty() {
        return vec![" ".repeat(width)];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        let mut candidate = current.clone();
        candidate.push(character);
        if !current.is_empty() && UnicodeWidthStr::width(candidate.as_str()) > width {
            lines.push(fit(&current, width));
            current.clear();
            current.push(character);
        } else {
            current = candidate;
        }
        if UnicodeWidthStr::width(current.as_str()) > width {
            lines.push(fit(&current, width));
            current.clear();
        }
    }
    if !current.is_empty() {
        lines.push(fit(&current, width));
    }
    if lines.is_empty() {
        lines.push(" ".repeat(width));
    }
    lines
}

impl ContentTableRow<'_> {
    pub fn line(self, visual: &TuiVisualStyle) -> Line<'static> {
        let row_style = if self.selected {
            Style::default()
                .fg(visual.selection_text)
                .bg(visual.selection_background)
        } else {
            Style::default().fg(visual.text)
        };
        let mut spans = vec![Span::styled(
            if self.selected { "◆ " } else { "  " },
            row_style,
        )];
        for (index, (cell, width)) in self.cells.iter().zip(self.widths).enumerate() {
            let color = if self.header {
                visual.text_muted
            } else if index == 0 {
                visual.text
            } else {
                visual.text_secondary
            };
            let style = if self.selected {
                row_style
            } else {
                Style::default().fg(color)
            };
            spans.push(Span::styled(fit(cell, *width), style));
            if index + 1 < self.widths.len() {
                spans.push(Span::styled(" ", style));
            }
        }
        Line::from(spans)
    }
}

pub(super) struct OutputLine<'a> {
    pub stream: &'a str,
    pub text: &'a str,
}

impl OutputLine<'_> {
    pub fn line(self, width: usize, visual: &TuiVisualStyle) -> Line<'static> {
        let prefix = format!("{} ", self.stream);
        Line::from(vec![
            Span::styled(prefix.clone(), Style::default().fg(visual.text_muted)),
            Span::styled(
                fit(self.text, width.saturating_sub(prefix.chars().count())),
                Style::default().fg(visual.text_secondary),
            ),
        ])
    }
}

#[derive(Clone, Copy)]
pub(super) struct ContextPair<'a> {
    pub label: &'a str,
    pub value: &'a str,
}

pub(super) struct ContextHeader<'a> {
    pub pairs: &'a [ContextPair<'a>],
}

impl ContextHeader<'_> {
    pub fn render(self, frame: &mut Frame<'_>, area: Rect, visual: &TuiVisualStyle) {
        let surface = Style::default().fg(visual.text).bg(visual.surface);
        let mut spans = vec![
            Span::styled("┌", surface.fg(visual.border_subtle)),
            Span::styled(
                " GOLEM ",
                Style::default()
                    .fg(visual.accent)
                    .bg(visual.surface)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        for (index, pair) in self.pairs.iter().enumerate() {
            spans.push(Span::styled(
                if index == 0 { "· " } else { " · " },
                surface.fg(visual.border_subtle),
            ));
            spans.push(Span::styled(
                format!("{} ", pair.label),
                surface.fg(visual.text_muted),
            ));
            spans.push(Span::styled(
                pair.value.to_string(),
                surface.fg(visual.text).add_modifier(Modifier::BOLD),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)).style(surface), area);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PaneFocus {
    Active,
    Idle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PaneEnding {
    Continue,
    Top,
    Stacked,
}

pub(super) struct PaneHeader<'a> {
    pub title: &'a str,
    pub focus: PaneFocus,
    pub left_connector: Option<&'a str>,
    pub ending: PaneEnding,
}

#[derive(Clone, Copy)]
pub(super) struct PaneSpec<'a> {
    pub title: &'a str,
    pub focus: PaneFocus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PaneLayout {
    pub panes: Vec<Rect>,
    pub dividers: Vec<Rect>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PaneRole {
    Single,
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PaneScrollbarSlot {
    pub area: Rect,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct PaneResizeDividers {
    pub before: Option<Rect>,
    pub after: Option<Rect>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PaneRegion {
    pub index: usize,
    pub role: PaneRole,
    pub pane_area: Rect,
    pub content_area: Rect,
    pub scrollbar: Option<PaneScrollbarSlot>,
    pub resize_dividers: PaneResizeDividers,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(super) enum PaneHitTarget {
    Body { pane: usize },
    Scrollbar { pane: usize },
    ResizeDivider { before: usize, after: usize },
}

impl PaneLayout {
    pub fn horizontal(area: Rect, weights: &[u32]) -> Self {
        if weights.is_empty() || area.width == 0 || area.height == 0 {
            return Self {
                panes: Vec::new(),
                dividers: Vec::new(),
            };
        }
        let total = weights.iter().copied().sum::<u32>().max(1);
        let mut constraints = Vec::with_capacity(weights.len() * 2 - 1);
        for (index, weight) in weights.iter().enumerate() {
            if index > 0 {
                constraints.push(Constraint::Length(1));
            }
            constraints.push(Constraint::Ratio(
                u32::from(*weight > 0).max(*weight),
                total,
            ));
        }
        let areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(area);
        Self {
            panes: areas.iter().step_by(2).copied().collect(),
            dividers: areas.iter().skip(1).step_by(2).copied().collect(),
        }
    }

    pub fn regions(&self, content_heights: &[usize]) -> Vec<PaneRegion> {
        let pane_count = self.panes.len();
        self.panes
            .iter()
            .copied()
            .enumerate()
            .map(|(index, pane_area)| {
                let role = match (index, pane_count) {
                    (_, 1) => PaneRole::Single,
                    (0, _) => PaneRole::Left,
                    (index, count) if index + 1 == count => PaneRole::Right,
                    _ => PaneRole::Middle,
                };
                PaneRegion::new(
                    index,
                    role,
                    pane_area,
                    content_heights.get(index).copied().unwrap_or_default(),
                    PaneResizeDividers {
                        before: index
                            .checked_sub(1)
                            .and_then(|i| self.dividers.get(i).copied()),
                        after: self.dividers.get(index).copied(),
                    },
                )
            })
            .collect()
    }

    #[allow(dead_code)]
    pub fn hit_test(&self, regions: &[PaneRegion], x: u16, y: u16) -> Option<PaneHitTarget> {
        if let Some((index, _)) = self
            .dividers
            .iter()
            .enumerate()
            .find(|(_, area)| rect_contains(**area, x, y))
        {
            return Some(PaneHitTarget::ResizeDivider {
                before: index,
                after: index + 1,
            });
        }
        for region in regions {
            if region
                .resize_dividers
                .before
                .is_some_and(|area| rect_contains(area, x, y))
            {
                let before = region.index.checked_sub(1)?;
                return Some(PaneHitTarget::ResizeDivider {
                    before,
                    after: region.index,
                });
            }
            if region
                .resize_dividers
                .after
                .is_some_and(|area| rect_contains(area, x, y))
            {
                return Some(PaneHitTarget::ResizeDivider {
                    before: region.index,
                    after: region.index.saturating_add(1),
                });
            }
        }
        for region in regions {
            if region
                .scrollbar
                .is_some_and(|slot| rect_contains(slot.area, x, y))
            {
                return Some(PaneHitTarget::Scrollbar { pane: region.index });
            }
            if rect_contains(region.content_area, x, y) {
                return Some(PaneHitTarget::Body { pane: region.index });
            }
        }
        None
    }

    pub fn render_headers(
        &self,
        frame: &mut Frame<'_>,
        specs: &[PaneSpec<'_>],
        visual: &TuiVisualStyle,
    ) {
        for (index, (area, spec)) in self.panes.iter().zip(specs).enumerate() {
            PaneHeader {
                title: spec.title,
                focus: spec.focus,
                left_connector: (index == 0).then_some("├"),
                ending: if index + 1 == self.panes.len() {
                    PaneEnding::Top
                } else {
                    PaneEnding::Continue
                },
            }
            .render(frame, *area, visual);
        }
        let style = Style::default().fg(visual.border_subtle).bg(visual.surface);
        for divider in &self.dividers {
            if divider.width > 0 && divider.height > 0 {
                frame.buffer_mut()[(divider.x, divider.y)]
                    .set_symbol("┬")
                    .set_style(style);
            }
        }
    }

    pub fn render_body_boundaries(
        &self,
        frame: &mut Frame<'_>,
        regions: &[PaneRegion],
        visual: &TuiVisualStyle,
    ) {
        let style = Style::default().fg(visual.border_subtle).bg(visual.surface);
        if let Some(primary) = regions.first() {
            for y in primary.pane_area.y..primary.pane_area.bottom() {
                frame.buffer_mut()[(primary.pane_area.x, y)]
                    .set_symbol("│")
                    .set_style(style);
            }
        }
        for divider in &self.dividers {
            for y in divider.y..divider.bottom() {
                frame.buffer_mut()[(divider.x, y)]
                    .set_symbol("│")
                    .set_style(style);
            }
        }
    }

    pub fn junctions(&self) -> Vec<u16> {
        self.dividers.iter().map(|area| area.x).collect()
    }
}

impl PaneRegion {
    pub fn new(
        index: usize,
        role: PaneRole,
        pane_area: Rect,
        content_height: usize,
        resize_dividers: PaneResizeDividers,
    ) -> Self {
        let has_outer_spine = matches!(role, PaneRole::Single | PaneRole::Left);
        let left_inset = u16::from(has_outer_spine && pane_area.width > 0);
        let scrollbar = (content_height > pane_area.height as usize
            && pane_area.width > left_inset
            && pane_area.height > 0)
            .then(|| PaneScrollbarSlot {
                area: Rect::new(
                    pane_area.right().saturating_sub(1),
                    pane_area.y,
                    1,
                    pane_area.height,
                ),
            });
        let right_inset = u16::from(scrollbar.is_some());
        let content_area = Rect::new(
            pane_area.x.saturating_add(left_inset),
            pane_area.y,
            pane_area.width.saturating_sub(left_inset + right_inset),
            pane_area.height,
        );
        Self {
            index,
            role,
            pane_area,
            content_area,
            scrollbar,
            resize_dividers,
        }
    }
}

#[allow(dead_code)]
fn rect_contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x && x < area.right() && y >= area.y && y < area.bottom()
}

impl PaneHeader<'_> {
    pub fn render(self, frame: &mut Frame<'_>, area: Rect, visual: &TuiVisualStyle) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let surface = Style::default().fg(visual.text).bg(visual.surface);
        let rule = surface.fg(visual.border_subtle);
        let title_style = match self.focus {
            PaneFocus::Active => surface.fg(visual.accent_hover).add_modifier(Modifier::BOLD),
            PaneFocus::Idle => surface.fg(visual.text_muted),
        };
        let label = match self.focus {
            PaneFocus::Active => format!("[ {} ]", self.title),
            PaneFocus::Idle => format!("( {} )", self.title),
        };
        let prefix = self
            .left_connector
            .map_or_else(|| "─".to_string(), |connector| format!("{connector}─"));
        let ending = match self.ending {
            PaneEnding::Continue => "─",
            PaneEnding::Top => "┐",
            PaneEnding::Stacked => "┤",
        };
        let used = prefix.chars().count() + label.chars().count() + 1;
        let fill = (area.width as usize).saturating_sub(used);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(prefix, rule),
                Span::styled(label, title_style),
                Span::styled("─".repeat(fill), rule),
                Span::styled(ending, rule),
            ]))
            .style(surface),
            area,
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct KeyHint<'a> {
    pub key: &'a str,
    pub label: &'a str,
}

#[derive(Clone, Copy)]
pub(super) struct WorkspaceItem<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub active: bool,
}

pub(super) struct WorkspaceSelector<'a> {
    pub items: &'a [WorkspaceItem<'a>],
    pub junctions: &'a [u16],
}

impl WorkspaceSelector<'_> {
    pub fn render(self, frame: &mut Frame<'_>, area: Rect, visual: &TuiVisualStyle) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let surface = Style::default()
            .fg(visual.text_muted)
            .bg(visual.footer_background);
        let rule = surface.fg(visual.footer_rule);
        let mut spans = vec![Span::styled("├─", rule)];
        let mut used = 2usize;
        for (index, item) in self.items.iter().enumerate() {
            if index > 0 {
                spans.push(Span::styled("──", rule));
                used += 2;
            }
            let (open, close, color, modifier) = if item.active {
                ("[ ", " ]", visual.text_secondary, Modifier::BOLD)
            } else {
                ("( ", " )", visual.text_muted, Modifier::empty())
            };
            spans.push(Span::styled(open, surface.fg(color).add_modifier(modifier)));
            spans.push(key_span(item.key.to_string(), visual));
            spans.push(Span::styled(
                format!(" {}{close}", item.label),
                surface.fg(color).add_modifier(modifier),
            ));
            used += open.chars().count()
                + item.key.chars().count()
                + 1
                + item.label.chars().count()
                + close.chars().count();
        }
        spans.push(Span::styled(
            "─".repeat((area.width as usize).saturating_sub(used)),
            rule,
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)).style(surface), area);
        for junction_x in self.junctions {
            if *junction_x >= area.x.saturating_add(used as u16)
                && *junction_x < area.right().saturating_sub(1)
            {
                frame.buffer_mut()[(*junction_x, area.y)]
                    .set_symbol("┴")
                    .set_style(rule);
            }
        }
        frame.buffer_mut()[(area.right().saturating_sub(1), area.y)]
            .set_symbol("┘")
            .set_style(rule);
    }
}

pub(super) struct ShortcutRow<'a> {
    pub items: &'a [KeyHint<'a>],
    pub active: bool,
    pub left_glyph: &'a str,
    pub fallback: KeyHint<'a>,
}

impl ShortcutRow<'_> {
    pub fn pack<'a>(items: &[KeyHint<'a>], width: u16, max_rows: usize) -> Vec<Vec<KeyHint<'a>>> {
        if items.is_empty() || max_rows == 0 {
            return Vec::new();
        }
        let available = width.saturating_sub(2) as usize;
        let item_width =
            |item: &KeyHint<'_>| item.key.chars().count() + item.label.chars().count() + 1;
        let mut rows: Vec<Vec<KeyHint<'a>>> = Vec::new();
        let mut used = 0usize;
        for item in items {
            let separator = usize::from(used > 0) * 3;
            let required = separator + item_width(item);
            if used > 0 && used + required > available {
                if rows.len() >= max_rows {
                    break;
                }
                rows.push(Vec::new());
                used = 0;
            }
            if rows.is_empty() {
                rows.push(Vec::new());
            }
            let separator = usize::from(used > 0) * 3;
            used += separator + item_width(item);
            rows.last_mut().expect("shortcut row").push(*item);
        }
        rows
    }

    pub fn render(self, frame: &mut Frame<'_>, area: Rect, visual: &TuiVisualStyle) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let background = if self.active {
            visual.panel_strong
        } else {
            visual.footer_background
        };
        let row_style = Style::default().fg(visual.text_muted).bg(background);
        frame.render_widget(Paragraph::new("").style(row_style), area);
        frame.render_widget(
            Paragraph::new(format!("{} ", self.left_glyph))
                .style(Style::default().fg(visual.footer_rule).bg(background)),
            Rect {
                width: area.width.min(2),
                ..area
            },
        );
        let mut spans = Vec::new();
        let available = area.width.saturating_sub(2) as usize;
        let item_width =
            |item: &KeyHint<'_>| item.key.chars().count() + item.label.chars().count() + 1;
        let full_width = self.items.iter().map(item_width).sum::<usize>()
            + self.items.len().saturating_sub(1) * 3;
        let fallback_width = item_width(&self.fallback);
        let mut used = 0;
        let mut visible = Vec::new();
        for item in self.items {
            let separator = usize::from(!visible.is_empty()) * 3;
            let reserve = if full_width > available {
                separator + fallback_width
            } else {
                0
            };
            let width = item_width(item);
            if used + separator + width + reserve > available {
                break;
            }
            used += separator + width;
            visible.push(*item);
        }
        if full_width > available && !visible.iter().any(|item| item.key == self.fallback.key) {
            visible.push(self.fallback);
        }
        for (index, item) in visible.iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw("   "));
            }
            spans.push(key_span(item.key.to_string(), visual));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                item.label.to_string(),
                Style::default().fg(if self.active {
                    visual.text
                } else {
                    visual.text_muted
                }),
            ));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans))
                .style(row_style)
                .alignment(Alignment::Right),
            Rect {
                x: area.x.saturating_add(2),
                width: area.width.saturating_sub(2),
                ..area
            },
        );
    }
}

pub(super) fn key_span<'a>(key: impl Into<String>, visual: &TuiVisualStyle) -> Span<'a> {
    Span::styled(
        key.into(),
        Style::default()
            .fg(visual.shortcut_key)
            .add_modifier(Modifier::BOLD),
    )
}

pub(super) fn shortcut_line(
    items: &[KeyHint<'_>],
    visual: &TuiVisualStyle,
    alignment: Alignment,
) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(key_span(item.key.to_string(), visual));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            item.label.to_string(),
            Style::default().fg(visual.text_muted),
        ));
    }
    Line::from(spans).alignment(alignment)
}

pub(super) struct SearchInput<'a> {
    pub query: &'a str,
}

impl SearchInput<'_> {
    pub fn line(self, width: usize, visual: &TuiVisualStyle) -> Line<'static> {
        let used = 2 + self.query.chars().count();
        Line::from(vec![
            Span::styled(
                "› ",
                Style::default()
                    .fg(visual.text_muted)
                    .bg(visual.input_background),
            ),
            Span::styled(
                self.query.to_string(),
                Style::default()
                    .fg(visual.input_text)
                    .bg(visual.input_background),
            ),
            Span::styled(
                " ".repeat(width.saturating_sub(used)),
                Style::default().bg(visual.input_background),
            ),
        ])
    }
}

pub(super) struct SelectableRow<'a> {
    pub label: &'a str,
    pub selected: bool,
    pub unavailable: bool,
}

impl SelectableRow<'_> {
    pub fn line(self, width: usize, visual: &TuiVisualStyle) -> Line<'static> {
        let marker = if self.selected { "◆ " } else { "  " };
        let used = marker.chars().count() + self.label.chars().count();
        let style = if self.selected {
            Style::default()
                .fg(visual.selection_text)
                .bg(visual.selection_background)
                .add_modifier(Modifier::BOLD)
        } else if self.unavailable {
            Style::default().fg(visual.text_muted)
        } else {
            Style::default().fg(visual.text)
        };
        Line::from(Span::styled(
            format!(
                "{marker}{}{}",
                self.label,
                " ".repeat(width.saturating_sub(used))
            ),
            style,
        ))
        .alignment(Alignment::Center)
    }
}

pub(super) struct DecisionTableRow<'a> {
    pub cells: &'a [&'a str],
    pub widths: &'a [usize],
    pub header: bool,
    pub selectable: bool,
    pub selected: bool,
}

impl DecisionTableRow<'_> {
    pub fn line(self, visual: &TuiVisualStyle) -> Line<'static> {
        let selected_style = Style::default()
            .fg(visual.selection_text)
            .bg(visual.selection_background);
        let mut spans = Vec::new();
        if self.selectable {
            spans.push(Span::styled(
                if self.selected { "◆ " } else { "  " },
                if self.selected {
                    selected_style
                } else {
                    Style::default()
                },
            ));
        }
        for (index, (cell, width)) in self.cells.iter().zip(self.widths).enumerate() {
            let text = if cell.chars().count() < *width {
                format!("{cell}{}", " ".repeat(*width - cell.chars().count()))
            } else {
                cell.chars().take(*width).collect()
            };
            let color = if self.header {
                visual.text_muted
            } else if index == 0 {
                visual.text
            } else {
                visual.text_secondary
            };
            spans.push(Span::styled(
                text,
                if self.selected {
                    selected_style
                } else {
                    Style::default().fg(color)
                },
            ));
        }
        Line::from(spans).alignment(Alignment::Center)
    }
}

pub(super) struct OverlayFrame<'a> {
    pub title: Option<&'a str>,
}

impl OverlayFrame<'_> {
    pub fn render(
        self,
        frame: &mut Frame<'_>,
        area: Rect,
        lines: Vec<Line<'static>>,
        visual: &TuiVisualStyle,
    ) {
        frame.render_widget(Clear, area);
        let surface = Style::default()
            .fg(visual.text)
            .bg(visual.overlay_background);
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(
                Style::default()
                    .fg(visual.border)
                    .bg(visual.overlay_background),
            )
            .style(surface);
        if let Some(title) = self.title {
            block = block.title(overlay_title(title, visual));
        }
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(lines)
                .style(surface)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false }),
            Rect {
                x: area.x.saturating_add(2),
                y: area.y.saturating_add(2),
                width: area.width.saturating_sub(4),
                height: area.height.saturating_sub(4),
            },
        );
    }
}

fn overlay_title(title: &str, visual: &TuiVisualStyle) -> Line<'static> {
    let background = visual.overlay_background;
    Line::from(vec![
        Span::styled("─", Style::default().fg(visual.border).bg(background)),
        Span::styled("[ ", Style::default().fg(visual.text_muted).bg(background)),
        Span::styled(
            title.to_string(),
            Style::default()
                .fg(visual.accent)
                .bg(background)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ]", Style::default().fg(visual.text_muted).bg(background)),
    ])
}

pub(super) fn render_scrollbar(
    frame: &mut Frame<'_>,
    slot: PaneScrollbarSlot,
    state: &mut ScrollbarState,
    visual: &TuiVisualStyle,
) {
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_style(Style::default().fg(visual.text_muted))
            .track_style(Style::default().fg(visual.text_faint))
            .begin_style(Style::default().fg(visual.text_muted))
            .end_style(Style::default().fg(visual.text_muted)),
        slot.area,
        state,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;
    use test_r::test;

    fn render(width: u16, height: u16, draw: impl FnOnce(&mut Frame<'_>)) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(draw).unwrap();
        terminal.backend().buffer().clone()
    }

    fn text(buffer: &Buffer) -> String {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn pane_focus_is_visible_without_color_and_owns_its_ending() {
        let visual = TuiVisualStyle::for_variant(crate::tui::visual::TuiVisualVariant::FrameBase);
        let buffer = render(40, 2, |frame| {
            PaneHeader {
                title: "Active",
                focus: PaneFocus::Active,
                left_connector: Some("├"),
                ending: PaneEnding::Top,
            }
            .render(frame, Rect::new(0, 0, 40, 1), &visual);
            PaneHeader {
                title: "Idle",
                focus: PaneFocus::Idle,
                left_connector: Some("├"),
                ending: PaneEnding::Stacked,
            }
            .render(frame, Rect::new(0, 1, 40, 1), &visual);
        });
        let text = text(&buffer);
        assert!(text.contains("[ Active ]"));
        assert!(text.contains("( Idle )"));
        assert_eq!(buffer[(39, 0)].symbol(), "┐");
        assert_eq!(buffer[(39, 1)].symbol(), "┤");
        assert_eq!(buffer[(0, 0)].fg, buffer[(39, 0)].fg);
    }

    #[test]
    fn pane_layout_owns_one_or_more_pane_geometry_and_decorators() {
        let visual = TuiVisualStyle::for_variant(crate::tui::visual::TuiVisualVariant::FrameBase);
        let single = PaneLayout::horizontal(Rect::new(0, 0, 80, 1), &[1]);
        assert_eq!(single.panes, vec![Rect::new(0, 0, 80, 1)]);
        assert!(single.dividers.is_empty());

        let layout = PaneLayout::horizontal(Rect::new(0, 0, 80, 1), &[3, 2, 1]);
        let body_layout = PaneLayout::horizontal(Rect::new(0, 1, 80, 4), &[3, 2, 1]);
        let body_regions = body_layout.regions(&[0, 0, 0]);
        assert_eq!(layout.panes.len(), 3);
        assert_eq!(layout.dividers.len(), 2);
        assert!(
            layout
                .panes
                .windows(2)
                .zip(&layout.dividers)
                .all(|(panes, divider)| panes[0].right() == divider.x
                    && divider.right() == panes[1].x)
        );
        let buffer = render(80, 5, |frame| {
            layout.render_headers(
                frame,
                &[
                    PaneSpec {
                        title: "Primary",
                        focus: PaneFocus::Active,
                    },
                    PaneSpec {
                        title: "Secondary",
                        focus: PaneFocus::Idle,
                    },
                    PaneSpec {
                        title: "Third",
                        focus: PaneFocus::Idle,
                    },
                ],
                &visual,
            );
            body_layout.render_body_boundaries(frame, &body_regions, &visual);
        });
        assert!((1..5).all(|y| buffer[(0, y)].symbol() == "│"));
        for (divider, body_divider) in layout.dividers.iter().zip(&body_layout.dividers) {
            assert_eq!(divider.x, body_divider.x);
            assert_eq!(buffer[(divider.x, 0)].symbol(), "┬");
            assert!((1..5).all(|y| buffer[(divider.x, y)].symbol() == "│"));
            assert!((1..5).all(|y| buffer[(divider.x + 1, y)].symbol() != "│"));
        }
        assert_eq!(buffer[(79, 0)].symbol(), "┐");
        assert!((1..5).all(|y| buffer[(79, y)].symbol() == " "));
    }

    #[test]
    fn pane_regions_assign_roles_scrollbars_and_exact_content_widths() {
        for area in [
            Rect::new(4, 2, 80, 10),
            Rect::new(0, 0, 7, 2),
            Rect::new(0, 0, 1, 1),
        ] {
            let single = PaneLayout::horizontal(area, &[1]);
            let plain = single.regions(&[area.height as usize]);
            let scrolling = single.regions(&[area.height as usize + 1]);
            assert_eq!(plain[0].role, PaneRole::Single);
            assert!(plain[0].scrollbar.is_none());
            assert_eq!(
                scrolling[0].scrollbar.is_some(),
                plain[0].content_area.width > 0
            );
            assert_eq!(
                plain[0]
                    .content_area
                    .width
                    .saturating_sub(scrolling[0].content_area.width),
                u16::from(plain[0].content_area.width > 0)
            );

            let layout = PaneLayout::horizontal(area, &[1, 1, 1]);
            let plain = layout.regions(&[0, 0, 0]);
            let scrolling = layout.regions(&[usize::MAX; 3]);
            assert_eq!(
                scrolling
                    .iter()
                    .map(|region| region.role)
                    .collect::<Vec<_>>(),
                vec![PaneRole::Left, PaneRole::Middle, PaneRole::Right]
            );
            for (index, region) in scrolling.iter().enumerate() {
                let expected = plain[index].content_area.width > 0 && region.pane_area.height > 0;
                assert_eq!(region.scrollbar.is_some(), expected);
                if let Some(slot) = region.scrollbar {
                    assert_eq!(slot.area.x, region.pane_area.right() - 1);
                    assert_eq!(region.content_area.right(), slot.area.x);
                    assert!(
                        !region
                            .resize_dividers
                            .before
                            .into_iter()
                            .chain(region.resize_dividers.after)
                            .any(|divider| divider == slot.area)
                    );
                }
            }
            for index in 0..3 {
                assert_eq!(
                    plain[index]
                        .content_area
                        .width
                        .saturating_sub(scrolling[index].content_area.width),
                    u16::from(plain[index].content_area.width > 0)
                );
            }
        }
    }

    #[test]
    fn pane_hit_testing_keeps_scrollbars_and_resize_dividers_distinct() {
        let layout = PaneLayout::horizontal(Rect::new(0, 0, 30, 6), &[1, 1, 1]);
        let regions = layout.regions(&[20, 20, 20]);
        for region in &regions {
            let slot = region.scrollbar.unwrap();
            assert!(!layout.dividers.iter().any(|divider| *divider == slot.area));
            assert_eq!(
                layout.hit_test(&regions, slot.area.x, slot.area.y),
                Some(PaneHitTarget::Scrollbar { pane: region.index })
            );
        }
        for (index, divider) in layout.dividers.iter().enumerate() {
            assert_eq!(
                layout.hit_test(&regions, divider.x, divider.y),
                Some(PaneHitTarget::ResizeDivider {
                    before: index,
                    after: index + 1,
                })
            );
        }
        let body = regions[1].content_area;
        if body.width > 0 {
            assert_eq!(
                layout.hit_test(&regions, body.x, body.y),
                Some(PaneHitTarget::Body { pane: 1 })
            );
        }
    }

    #[test]
    fn pane_hit_testing_finds_nested_horizontal_resize_dividers() {
        let layout = PaneLayout::horizontal(Rect::new(0, 0, 30, 10), &[1, 1]);
        let primary = layout.regions(&[0, 0])[0];
        let handle = Rect::new(16, 4, 14, 1);
        let upper = PaneRegion::new(
            1,
            PaneRole::Right,
            Rect::new(16, 0, 14, 4),
            0,
            PaneResizeDividers {
                before: layout.dividers.first().copied(),
                after: Some(handle),
            },
        );
        let lower = PaneRegion::new(
            2,
            PaneRole::Right,
            Rect::new(16, 5, 14, 5),
            0,
            PaneResizeDividers {
                before: Some(handle),
                after: None,
            },
        );
        assert_eq!(
            layout.hit_test(&[primary, upper, lower], handle.x, handle.y),
            Some(PaneHitTarget::ResizeDivider {
                before: 1,
                after: 2,
            })
        );
    }

    #[test]
    fn left_pane_scrollbar_uses_the_trailing_edge_and_preserves_the_body_spine() {
        let visual = TuiVisualStyle::for_variant(crate::tui::visual::TuiVisualVariant::FrameBase);
        let header = PaneLayout::horizontal(Rect::new(0, 0, 20, 1), &[1, 1]);
        let body = PaneLayout::horizontal(Rect::new(0, 1, 20, 3), &[1, 1]);
        let regions = body.regions(&[10, 0]);
        let buffer = render(20, 4, |frame| {
            header.render_headers(
                frame,
                &[
                    PaneSpec {
                        title: "Left",
                        focus: PaneFocus::Active,
                    },
                    PaneSpec {
                        title: "Right",
                        focus: PaneFocus::Idle,
                    },
                ],
                &visual,
            );
            body.render_body_boundaries(frame, &regions, &visual);
            let mut state = ScrollbarState::new(10).viewport_content_length(3);
            render_scrollbar(frame, regions[0].scrollbar.unwrap(), &mut state, &visual);
        });
        assert_eq!(buffer[(0, 0)].symbol(), "├");
        assert_eq!(buffer[(0, 1)].symbol(), "│");
        let slot = regions[0].scrollbar.unwrap();
        assert_eq!(slot.area.x, regions[0].pane_area.right() - 1);
        assert_ne!(buffer[(slot.area.x, slot.area.y)].symbol(), " ");
        assert_eq!(buffer[(header.dividers[0].x, 0)].symbol(), "┬");
        assert_eq!(buffer[(body.dividers[0].x, 1)].symbol(), "│");
    }

    #[test]
    fn header_and_workspace_selector_own_connected_shell_grammar() {
        let visual = TuiVisualStyle::for_variant(crate::tui::visual::TuiVisualVariant::FrameBase);
        let buffer = render(72, 2, |frame| {
            ContextHeader {
                pairs: &[
                    ContextPair {
                        label: "app",
                        value: "example",
                    },
                    ContextPair {
                        label: "env",
                        value: "local",
                    },
                ],
            }
            .render(frame, Rect::new(0, 0, 72, 1), &visual);
            WorkspaceSelector {
                items: &[
                    WorkspaceItem {
                        key: "1",
                        label: "Home",
                        active: true,
                    },
                    WorkspaceItem {
                        key: "2",
                        label: "Dev",
                        active: false,
                    },
                ],
                junctions: &[50],
            }
            .render(frame, Rect::new(0, 1, 72, 1), &visual);
        });
        let text = text(&buffer);
        assert!(text.contains("┌ GOLEM · app example · env local"));
        assert!(text.contains("├─[ 1 Home ]──( 2 Dev )"));
        assert_eq!(buffer[(50, 1)].symbol(), "┴");
        assert_eq!(buffer[(71, 1)].symbol(), "┘");
    }

    #[test]
    fn workspace_selector_never_overwrites_a_label_with_a_junction() {
        let visual = TuiVisualStyle::for_variant(crate::tui::visual::TuiVisualVariant::FrameBase);
        let buffer = render(32, 1, |frame| {
            WorkspaceSelector {
                items: &[WorkspaceItem {
                    key: "1",
                    label: "Overview",
                    active: true,
                }],
                junctions: &[4],
            }
            .render(frame, frame.area(), &visual);
        });
        assert_eq!(buffer[(4, 0)].symbol(), "1");
        assert!(!text(&buffer).contains('┴'));
    }

    #[test]
    fn search_rows_share_alignment_and_selection_width() {
        let visual = TuiVisualStyle::for_variant(crate::tui::visual::TuiVisualVariant::FrameBase);
        let input = SearchInput { query: "dep" }.line(20, &visual);
        let selected = SelectableRow {
            label: "Deploy",
            selected: true,
            unavailable: false,
        }
        .line(20, &visual);
        let buffer = render(20, 2, |frame| {
            frame.render_widget(Paragraph::new(input), Rect::new(0, 0, 20, 1));
            frame.render_widget(Paragraph::new(selected), Rect::new(0, 1, 20, 1));
        });
        assert_eq!(buffer[(0, 0)].symbol(), "›");
        assert_eq!(buffer[(0, 0)].bg, visual.input_background);
        assert_eq!(buffer[(0, 1)].symbol(), "◆");
        assert!((0..20).all(|x| buffer[(x, 1)].bg == visual.selection_background));
    }

    #[test]
    fn content_table_truncates_by_terminal_width_and_fills_selected_width() {
        let visual = TuiVisualStyle::for_variant(crate::tui::visual::TuiVisualVariant::FrameBase);
        let line = ContentTableRow {
            cells: &["checkout-service", "running"],
            widths: &[8, 7],
            header: false,
            selected: true,
        }
        .line(&visual);
        let buffer = render(18, 1, |frame| {
            frame.render_widget(Paragraph::new(line), frame.area());
        });
        assert!(text(&buffer).starts_with("◆ checkou… running"));
        assert!((0..18).all(|x| buffer[(x, 0)].bg == visual.selection_background));
    }

    #[test]
    fn fit_and_wrap_respect_wide_and_combining_terminal_widths() {
        assert_eq!(UnicodeWidthStr::width(fit("abcd界x", 6).as_str()), 6);
        assert_eq!(fit("abcd界x", 6), "abcd… ");
        assert_eq!(fit("ab🙂cd", 5), "ab🙂…");
        assert_eq!(fit("e\u{301}cho", 5), "e\u{301}cho ");

        let wrapped = wrap_cell("界面e\u{301}cho", 4);
        assert_eq!(wrapped, vec!["界面", "e\u{301}cho"]);
        assert!(
            wrapped
                .iter()
                .all(|line| UnicodeWidthStr::width(line.as_str()) == 4)
        );
    }

    fn pane_table_columns() -> [PaneTableColumn<'static>; 3] {
        [
            PaneTableColumn {
                id: "name",
                title: "Name",
                width: 6,
                required: true,
                default_visible: true,
                policy: CellPolicy::Ellipsis,
            },
            PaneTableColumn {
                id: "owner",
                title: "Owner",
                width: 5,
                required: false,
                default_visible: false,
                policy: CellPolicy::Ellipsis,
            },
            PaneTableColumn {
                id: "description",
                title: "Description",
                width: 8,
                required: false,
                default_visible: true,
                policy: CellPolicy::WrapSelected,
            },
        ]
    }

    #[test]
    fn pane_table_columns_toggle_transactionally_and_required_columns_stay_visible() {
        let columns = pane_table_columns();
        let mut table = PaneTableState::new(&columns, 0);
        let original = table.clone();
        let mut cancelled = ColumnChooserState::new(&table);
        cancelled.toggle(&columns, 1);
        assert_eq!(table, original);

        let mut applied = ColumnChooserState::new(&table);
        applied.toggle(&columns, 1);
        applied.apply(&mut table);
        assert!(table.column_visible("owner"));
        table.set_column_visible(&columns, "name", false);
        assert!(table.column_visible("name"));
    }

    #[test]
    fn pane_table_horizontal_offset_clamps_and_keeps_marker_frozen() {
        let visual = TuiVisualStyle::for_variant(crate::tui::visual::TuiVisualVariant::FrameBase);
        let columns = pane_table_columns();
        let mut state = PaneTableState::new(&columns, 0);
        state.horizontal_offset = u16::MAX;
        state.clamp_offset(&columns, 8);
        assert_eq!(state.horizontal_offset, 7);
        state.pan_right(&columns, 8);
        assert_eq!(state.horizontal_offset, 7);
        let rows: &[&[&str]] = &[&["checkout", "team", "a long selected description"]];
        let buffer = render(10, 5, |frame| {
            PaneTable {
                columns: &columns,
                rows,
                state: &state,
                decoration: TableDecoration::Minimal,
            }
            .render(frame, frame.area(), &visual);
        });
        assert_eq!(buffer[(0, 1)].symbol(), "▌");
        state.pan_left();
        assert_eq!(state.horizontal_offset, 6);
    }

    #[test]
    fn pane_table_wraps_only_selected_rows_and_fills_their_full_height() {
        let visual = TuiVisualStyle::for_variant(crate::tui::visual::TuiVisualVariant::FrameBase);
        let columns = pane_table_columns();
        let state = PaneTableState::new(&columns, 0);
        let rows: &[&[&str]] = &[
            &["first", "one", "abcdefghijklmnop"],
            &["second", "two", "qrstuvwxyzabcdef"],
        ];
        let table = PaneTable {
            columns: &columns,
            rows,
            state: &state,
            decoration: TableDecoration::Zebra,
        };
        assert_eq!(table.height(), 4);
        assert_eq!(
            PaneTable {
                columns: &columns,
                rows,
                state: &state,
                decoration: TableDecoration::Minimal,
            }
            .height(),
            3
        );
        assert_eq!(
            PaneTable {
                columns: &columns,
                rows,
                state: &state,
                decoration: TableDecoration::Rules,
            }
            .height(),
            3
        );
        let buffer = render(24, 4, |frame| table.render(frame, frame.area(), &visual));
        assert_eq!(buffer[(0, 1)].symbol(), "▌");
        assert_eq!(buffer[(0, 2)].symbol(), "▌");
        assert!((0..24).all(|x| buffer[(x, 1)].bg == visual.table_selected_odd_background));
        assert!((0..24).all(|x| buffer[(x, 2)].bg == visual.table_selected_odd_background));
        assert!(text(&buffer).contains("qrstuvw…"));
    }

    #[test]
    fn pane_table_decorations_are_independent_candidates() {
        let visual = TuiVisualStyle::for_variant(crate::tui::visual::TuiVisualVariant::FrameBase);
        let columns = pane_table_columns();
        let rows: &[&[&str]] = &[&["first", "one", "short"], &["second", "two", "short"]];
        let render_table = |decoration, selected| {
            let state = PaneTableState::new(&columns, selected);
            render(24, 3, |frame| {
                PaneTable {
                    columns: &columns,
                    rows,
                    state: &state,
                    decoration,
                }
                .render(frame, frame.area(), &visual);
            })
        };
        let minimal = render_table(TableDecoration::Minimal, usize::MAX);
        let rules = render_table(TableDecoration::Rules, usize::MAX);
        let zebra = render_table(TableDecoration::Zebra, usize::MAX);
        let selected_odd = render_table(TableDecoration::Zebra, 0);
        let selected_even = render_table(TableDecoration::Zebra, 1);
        assert!(!text(&minimal).contains('│'));
        assert!(text(&rules).contains('│'));
        assert!(text(&rules).contains("Name  │"));
        assert_eq!(zebra[(2, 1)].bg, visual.table_odd_background);
        assert_eq!(zebra[(2, 2)].bg, visual.table_even_background);
        assert_eq!(
            selected_odd[(2, 1)].bg,
            visual.table_selected_odd_background
        );
        assert_eq!(
            selected_even[(2, 2)].bg,
            visual.table_selected_even_background
        );
    }

    #[test]
    fn content_states_have_stable_non_color_identity() {
        let visual = TuiVisualStyle::for_variant(crate::tui::visual::TuiVisualVariant::FrameBase);
        let mut spans = Vec::new();
        for marker in [
            StatusMarker {
                kind: StatusKind::Running,
                label: "running",
            },
            StatusMarker {
                kind: StatusKind::Idle,
                label: "idle",
            },
            StatusMarker {
                kind: StatusKind::Failed,
                label: "failed",
            },
            StatusMarker {
                kind: StatusKind::Attention,
                label: "attention",
            },
        ] {
            spans.extend(marker.spans(&visual));
        }
        let notices = [
            NoticeKind::Active,
            NoticeKind::Success,
            NoticeKind::Loading,
            NoticeKind::Warning,
            NoticeKind::Unavailable,
            NoticeKind::Error,
            NoticeKind::Empty,
            NoticeKind::Info,
        ]
        .map(|kind| {
            Notice {
                kind,
                message: "message",
            }
            .line(&visual)
        });
        let buffer = render(80, 9, |frame| {
            frame.render_widget(
                Paragraph::new(
                    std::iter::once(Line::from(spans))
                        .chain(notices)
                        .collect::<Vec<_>>(),
                ),
                frame.area(),
            );
        });
        let text = text(&buffer);
        for expected in [
            "● running",
            "○ idle",
            "× failed",
            "! attention",
            "Loading",
            "● Active",
            "✓ Success",
            "! Warning",
            "Unavailable",
            "Error",
            "Empty",
            "Notice",
        ] {
            assert!(text.contains(expected), "missing {expected:?}");
        }
        let active_row = text
            .lines()
            .position(|line| line.contains("● Active"))
            .expect("active notice row") as u16;
        assert_eq!(buffer[(0, active_row)].fg, visual.success);
    }

    #[test]
    fn fields_and_output_share_alignment_and_safe_truncation() {
        let visual = TuiVisualStyle::for_variant(crate::tui::visual::TuiVisualVariant::FrameBase);
        let buffer = render(20, 2, |frame| {
            frame.render_widget(
                Paragraph::new(vec![
                    FieldRow {
                        label: "Context",
                        value: "local",
                        label_width: 10,
                    }
                    .line(&visual),
                    OutputLine {
                        stream: "out",
                        text: "a deliberately long output line",
                    }
                    .line(20, &visual),
                ]),
                frame.area(),
            );
        });
        let text = text(&buffer);
        assert!(text.contains("Context     local"));
        assert!(text.contains("out a deliberately …"));
    }

    #[test]
    fn shortcuts_share_lowercase_muted_amber_grammar() {
        let visual = TuiVisualStyle::for_variant(crate::tui::visual::TuiVisualVariant::FrameBase);
        let line = shortcut_line(
            &[KeyHint {
                key: "enter",
                label: "Select",
            }],
            &visual,
            Alignment::Center,
        );
        let buffer = render(20, 1, |frame| {
            frame.render_widget(Paragraph::new(line), frame.area());
        });
        let key_x = (0..20).find(|x| buffer[(*x, 0)].symbol() == "e").unwrap();
        assert_eq!(buffer[(key_x, 0)].fg, visual.shortcut_key);
        assert_ne!(buffer[(key_x, 0)].fg, Color::Reset);
        assert!(text(&buffer).contains("enter Select"));
    }

    #[test]
    fn contextual_shortcut_rows_keep_the_spine_left_and_align_hints_right() {
        let visual = TuiVisualStyle::for_variant(crate::tui::visual::TuiVisualVariant::FrameBase);
        let buffer = render(40, 1, |frame| {
            ShortcutRow {
                items: &[
                    KeyHint {
                        key: "enter",
                        label: "Open",
                    },
                    KeyHint {
                        key: "u",
                        label: "Refresh",
                    },
                ],
                active: false,
                left_glyph: "│",
                fallback: KeyHint {
                    key: "ctrl+p",
                    label: "Commands",
                },
            }
            .render(frame, frame.area(), &visual);
        });
        let row = text(&buffer);
        assert!(row.starts_with("│ "));
        assert!(row.ends_with("enter Open   u Refresh"));
    }

    #[test]
    fn contextual_shortcuts_use_one_row_when_possible_and_at_most_two_when_narrow() {
        let items = [
            KeyHint {
                key: "r",
                label: "Run",
            },
            KeyHint {
                key: "ctrl+x l",
                label: "Layout",
            },
            KeyHint {
                key: "ctrl+x e",
                label: "Context",
            },
            KeyHint {
                key: "enter",
                label: "Open",
            },
            KeyHint {
                key: "u",
                label: "Refresh",
            },
        ];
        let wide = ShortcutRow::pack(&items, 100, 2);
        assert_eq!(wide, vec![items.to_vec()]);
        let narrow = ShortcutRow::pack(&items, 42, 2);
        assert_eq!(narrow.len(), 2);
        assert!(narrow.iter().all(|row| !row.is_empty()));
    }
}
