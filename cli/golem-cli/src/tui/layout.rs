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

use super::app::{
    AgentInspectPane, AgentsViewMode, ContextPickerStep, DevPanel, TuiMode, TuiWorkspace,
};
use ratatui::layout::Rect;

pub(super) const DEV_MIN_PRIMARY_SIZE: u16 = 28;
pub(super) const DEV_MIN_SECONDARY_SIZE: u16 = 16;
pub(super) const DEV_MIN_STACKED_SIZE: u16 = 3;
pub(super) const DRAWER_MIN_WIDTH: u16 = 24;
pub(super) const DRAWER_MAX_WIDTH: u16 = 72;
pub(super) const DRAWER_DEFAULT_RATIO: u16 = 38;
const SPLIT_SIZE: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DevLayoutPreset {
    Right,
    Left,
    Top,
    Bottom,
}

impl DevLayoutPreset {
    pub(super) fn next(self) -> Self {
        match self {
            Self::Right => Self::Left,
            Self::Left => Self::Top,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Right,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Left => "left",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }

    fn primary_is_horizontal(self) -> bool {
        matches!(self, Self::Right | Self::Left)
    }

    fn primary_first(self) -> bool {
        matches!(self, Self::Right | Self::Bottom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DragTarget {
    DevPrimary,
    DevSecondary,
    ServerDrawer,
}

#[derive(Debug, Clone)]
pub(super) struct TuiLayoutState {
    pub(super) dev_preset: DevLayoutPreset,
    pub(super) dev_primary_ratio: u16,
    pub(super) dev_secondary_ratio: u16,
    pub(super) server_drawer_open: bool,
    pub(super) server_drawer_ratio: u16,
    pub(super) dragging: Option<DragTarget>,
}

impl Default for TuiLayoutState {
    fn default() -> Self {
        Self {
            dev_preset: DevLayoutPreset::Right,
            dev_primary_ratio: 62,
            dev_secondary_ratio: 34,
            server_drawer_open: false,
            server_drawer_ratio: DRAWER_DEFAULT_RATIO,
            dragging: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RegionKind {
    HeaderTab(TuiWorkspace),
    Footer,
    WorkspaceBody,
    DevPanelTitle(DevPanel),
    DevPanelBody(DevPanel),
    DevPrimarySplit,
    DevSecondarySplit,
    OpsList,
    OpsDetails,
    OpsInspectPane(AgentInspectPane),
    ContextPickerRow(usize),
    ContextConfirm,
    ContextCancel,
    ServerDrawer,
    ServerDrawerSplit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Region {
    pub(super) kind: RegionKind,
    pub(super) area: Rect,
}

#[derive(Debug, Clone)]
pub(super) struct LayoutSnapshot {
    pub(super) header: Rect,
    pub(super) tabs: Rect,
    pub(super) separator: Rect,
    pub(super) body: Rect,
    pub(super) workspace_body: Rect,
    pub(super) footer: Rect,
    pub(super) server_drawer: Option<Rect>,
    pub(super) regions: Vec<Region>,
}

impl LayoutSnapshot {
    pub(super) fn hit_test(&self, x: u16, y: u16) -> Option<RegionKind> {
        self.regions
            .iter()
            .rev()
            .find(|region| contains(region.area, x, y))
            .map(|region| region.kind)
    }

    pub(super) fn region(&self, kind: RegionKind) -> Option<Rect> {
        self.regions
            .iter()
            .find(|region| region.kind == kind)
            .map(|region| region.area)
    }
}

#[derive(Debug, Clone)]
pub(super) struct LayoutInput {
    pub(super) area: Rect,
    pub(super) active_workspace: TuiWorkspace,
    pub(super) focused_dev_panel: DevPanel,
    pub(super) mode: TuiMode,
    pub(super) context_picker_rows: usize,
    pub(super) context_picker_step: ContextPickerStep,
    pub(super) agents_view_mode: AgentsViewMode,
    pub(super) agent_details_visible: bool,
    pub(super) layout: TuiLayoutState,
}

pub(super) fn compute(input: LayoutInput) -> LayoutSnapshot {
    let [header, tabs, separator, body, footer] = vertical_split(
        input.area,
        &[
            SplitSpec::Length(1),
            SplitSpec::Length(1),
            SplitSpec::Length(1),
            SplitSpec::Min(1),
            SplitSpec::Length(1),
        ],
    );
    let mut regions = Vec::new();
    add_tab_regions(tabs, &mut regions);
    regions.push(Region {
        kind: RegionKind::Footer,
        area: footer,
    });

    let (workspace_body, drawer_handle, server_drawer) = split_server_drawer(
        body,
        input.layout.server_drawer_open,
        input.layout.server_drawer_ratio,
    );
    if let Some(handle) = drawer_handle {
        regions.push(Region {
            kind: RegionKind::ServerDrawerSplit,
            area: handle,
        });
    }
    if let Some(drawer) = server_drawer {
        regions.push(Region {
            kind: RegionKind::ServerDrawer,
            area: drawer,
        });
    }
    regions.push(Region {
        kind: RegionKind::WorkspaceBody,
        area: workspace_body,
    });

    match input.active_workspace {
        TuiWorkspace::Home => {}
        TuiWorkspace::Dev => add_dev_regions(workspace_body, &input, &mut regions),
        TuiWorkspace::Ops => add_ops_regions(workspace_body, &input, &mut regions),
    }
    add_modal_regions(&input, &mut regions);

    LayoutSnapshot {
        header,
        tabs,
        separator,
        body,
        workspace_body,
        footer,
        server_drawer,
        regions,
    }
}

pub(super) fn clamp_dev_primary_ratio(area: Rect, preset: DevLayoutPreset, ratio: u16) -> u16 {
    let total = if preset.primary_is_horizontal() {
        area.width
    } else {
        area.height
    }
    .saturating_sub(SPLIT_SIZE);
    clamp_ratio_for_min(total, ratio, DEV_MIN_PRIMARY_SIZE, DEV_MIN_SECONDARY_SIZE)
}

pub(super) fn clamp_dev_secondary_ratio(area: Rect, preset: DevLayoutPreset, ratio: u16) -> u16 {
    let panels = secondary_panel_count(false) as u16;
    let total = if preset.primary_is_horizontal() {
        area.height
    } else {
        area.width
    }
    .saturating_sub(panels.saturating_sub(1) * SPLIT_SIZE);
    clamp_ratio_for_min(total, ratio, DEV_MIN_STACKED_SIZE, DEV_MIN_STACKED_SIZE)
}

pub(super) fn clamp_drawer_ratio(area: Rect, ratio: u16) -> u16 {
    let max_width = DRAWER_MAX_WIDTH.min(area.width.saturating_sub(DEV_MIN_PRIMARY_SIZE));
    let min_width = DRAWER_MIN_WIDTH.min(max_width);
    clamp_ratio_for_min_max(area.width, ratio, min_width, max_width)
}

pub(super) fn ratio_from_pointer(area: Rect, preset: DevLayoutPreset, x: u16, y: u16) -> u16 {
    let offset = if preset.primary_is_horizontal() {
        x.saturating_sub(area.x)
    } else {
        y.saturating_sub(area.y)
    };
    ratio(offset, primary_extent(area, preset))
}

pub(super) fn secondary_ratio_from_pointer(
    area: Rect,
    preset: DevLayoutPreset,
    x: u16,
    y: u16,
) -> u16 {
    let offset = if preset.primary_is_horizontal() {
        y.saturating_sub(area.y)
    } else {
        x.saturating_sub(area.x)
    };
    let total = if preset.primary_is_horizontal() {
        area.height
    } else {
        area.width
    };
    ratio(offset, total)
}

pub(super) fn drawer_ratio_from_pointer(body: Rect, x: u16) -> u16 {
    let drawer_width = body.x.saturating_add(body.width).saturating_sub(x);
    ratio(drawer_width, body.width)
}

fn add_dev_regions(area: Rect, input: &LayoutInput, regions: &mut Vec<Region>) {
    if let Some((primary_handle, secondary_handle)) = dev_split_regions(area, input) {
        regions.push(Region {
            kind: RegionKind::DevPrimarySplit,
            area: primary_handle,
        });
        if let Some(secondary_handle) = secondary_handle {
            regions.push(Region {
                kind: RegionKind::DevSecondarySplit,
                area: secondary_handle,
            });
        }
    }
    let panels = dev_panel_areas(area, input);
    for (panel, panel_area) in panels {
        if panel_area.width == 0 || panel_area.height == 0 {
            continue;
        }
        let title = Rect {
            height: panel_area.height.min(1),
            ..panel_area
        };
        let body = Rect {
            y: panel_area.y.saturating_add(1),
            height: panel_area.height.saturating_sub(1),
            ..panel_area
        };
        regions.push(Region {
            kind: RegionKind::DevPanelTitle(panel),
            area: title,
        });
        regions.push(Region {
            kind: RegionKind::DevPanelBody(panel),
            area: body,
        });
        if panel == DevPanel::Agents {
            add_ops_regions(body, input, regions);
        }
    }
}

pub(super) fn dev_panel_areas(area: Rect, input: &LayoutInput) -> Vec<(DevPanel, Rect)> {
    if area.width < 90 || area.height < 18 {
        return vec![(input.focused_dev_panel, area)];
    }

    let primary_ratio = clamp_dev_primary_ratio(
        area,
        input.layout.dev_preset,
        input.layout.dev_primary_ratio,
    );
    let (primary, handle, secondary) = split_primary(area, input.layout.dev_preset, primary_ratio);
    let mut result = Vec::new();
    let repl_area = if input.layout.dev_preset.primary_first() {
        primary
    } else {
        secondary
    };
    let side_area = if input.layout.dev_preset.primary_first() {
        secondary
    } else {
        primary
    };
    result.push((DevPanel::Repl, repl_area));
    if handle.width > 0 && handle.height > 0 {
        result.push((DevPanel::Repl, Rect::default()));
    }

    let panels = secondary_panels(input.layout.server_drawer_open);
    let side_regions = split_secondary(
        side_area,
        input.layout.dev_preset,
        input.layout.dev_secondary_ratio,
        panels.len(),
    );
    result.extend(panels.into_iter().zip(side_regions));
    result
}

pub(super) fn dev_split_regions(area: Rect, input: &LayoutInput) -> Option<(Rect, Option<Rect>)> {
    if area.width < 90 || area.height < 18 {
        return None;
    }
    let primary_ratio = clamp_dev_primary_ratio(
        area,
        input.layout.dev_preset,
        input.layout.dev_primary_ratio,
    );
    let (_, primary_handle, secondary) =
        split_primary(area, input.layout.dev_preset, primary_ratio);
    let panels = secondary_panels(input.layout.server_drawer_open);
    let secondary_handle = split_secondary_handle(
        secondary,
        input.layout.dev_preset,
        input.layout.dev_secondary_ratio,
        panels.len(),
    );
    Some((primary_handle, secondary_handle))
}

fn add_ops_regions(area: Rect, input: &LayoutInput, regions: &mut Vec<Region>) {
    let content = Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(1),
        ..area
    };
    if input.agents_view_mode == AgentsViewMode::Inspect {
        let [left, right] = horizontal_split(
            content,
            &[SplitSpec::Percentage(50), SplitSpec::Percentage(50)],
        );
        regions.push(Region {
            kind: RegionKind::OpsInspectPane(AgentInspectPane::Oplog),
            area: left,
        });
        regions.push(Region {
            kind: RegionKind::OpsInspectPane(AgentInspectPane::Stream),
            area: right,
        });
    } else if input.agent_details_visible {
        let [list, details] = horizontal_split(
            content,
            &[SplitSpec::Percentage(60), SplitSpec::Percentage(40)],
        );
        regions.push(Region {
            kind: RegionKind::OpsList,
            area: list,
        });
        regions.push(Region {
            kind: RegionKind::OpsDetails,
            area: details,
        });
    } else {
        regions.push(Region {
            kind: RegionKind::OpsList,
            area: content,
        });
    }
}

fn add_modal_regions(input: &LayoutInput, regions: &mut Vec<Region>) {
    match input.mode {
        TuiMode::ContextPicker => {
            let height =
                context_picker_height(input.context_picker_rows.min(10), input.area.height);
            let width = input.area.width.min(72).max(36);
            let area = centered(input.area, width, height);
            let first_row = area.y.saturating_add(3);
            for index in 0..input.context_picker_rows.min(10) {
                regions.push(Region {
                    kind: RegionKind::ContextPickerRow(index),
                    area: Rect {
                        x: area.x.saturating_add(2),
                        y: first_row.saturating_add(index as u16),
                        width: area.width.saturating_sub(4),
                        height: 1,
                    },
                });
            }
            if input.context_picker_step == ContextPickerStep::AppEnvironments {
                // Row numbering remains local to the visible environment list.
            }
        }
        TuiMode::ContextSwitchConfirm => {
            let area = centered(
                input.area,
                input.area.width.min(72),
                input.area.height.min(11),
            );
            regions.push(Region {
                kind: RegionKind::ContextConfirm,
                area: Rect {
                    x: area.x.saturating_add(2),
                    y: area.y.saturating_add(area.height.saturating_sub(2)),
                    width: 24,
                    height: 1,
                },
            });
            regions.push(Region {
                kind: RegionKind::ContextCancel,
                area: Rect {
                    x: area.x.saturating_add(28),
                    y: area.y.saturating_add(area.height.saturating_sub(2)),
                    width: 24,
                    height: 1,
                },
            });
        }
        _ => {}
    }
}

fn add_tab_regions(tabs: Rect, regions: &mut Vec<Region>) {
    let mut x = tabs.x.saturating_add(2);
    for (index, workspace) in TuiWorkspace::ALL.iter().copied().enumerate() {
        if index > 0 {
            x = x.saturating_add(2);
        }
        let width = (format!("[{}] {}", index + 1, workspace.title())
            .chars()
            .count() as u16)
            .saturating_add(2);
        regions.push(Region {
            kind: RegionKind::HeaderTab(workspace),
            area: Rect {
                x,
                y: tabs.y,
                width,
                height: tabs.height,
            },
        });
        x = x.saturating_add(width);
    }
}

fn split_server_drawer(body: Rect, open: bool, ratio: u16) -> (Rect, Option<Rect>, Option<Rect>) {
    if !open || body.width <= DRAWER_MIN_WIDTH.saturating_add(DEV_MIN_PRIMARY_SIZE) {
        return (body, None, None);
    }
    let clamped = clamp_drawer_ratio(body, ratio);
    let drawer_width = ((body.width as u32 * clamped as u32) / 100)
        .max(DRAWER_MIN_WIDTH as u32)
        .min(DRAWER_MAX_WIDTH as u32)
        .min(body.width.saturating_sub(DEV_MIN_PRIMARY_SIZE) as u32) as u16;
    let handle = Rect {
        x: body.x + body.width - drawer_width - SPLIT_SIZE,
        y: body.y,
        width: SPLIT_SIZE,
        height: body.height,
    };
    let workspace = Rect {
        width: body
            .width
            .saturating_sub(drawer_width)
            .saturating_sub(SPLIT_SIZE),
        ..body
    };
    let drawer = Rect {
        x: handle.x.saturating_add(SPLIT_SIZE),
        y: body.y,
        width: drawer_width,
        height: body.height,
    };
    (workspace, Some(handle), Some(drawer))
}

fn split_primary(area: Rect, preset: DevLayoutPreset, ratio: u16) -> (Rect, Rect, Rect) {
    let horizontal = preset.primary_is_horizontal();
    let total = primary_extent(area, preset).saturating_sub(SPLIT_SIZE);
    let first_size = ((total as u32 * ratio as u32) / 100) as u16;
    if horizontal {
        let first = Rect {
            width: first_size,
            ..area
        };
        let handle = Rect {
            x: first.x.saturating_add(first.width),
            y: area.y,
            width: SPLIT_SIZE,
            height: area.height,
        };
        let second = Rect {
            x: handle.x.saturating_add(SPLIT_SIZE),
            y: area.y,
            width: area
                .width
                .saturating_sub(first.width)
                .saturating_sub(SPLIT_SIZE),
            height: area.height,
        };
        (first, handle, second)
    } else {
        let first = Rect {
            height: first_size,
            ..area
        };
        let handle = Rect {
            x: area.x,
            y: first.y.saturating_add(first.height),
            width: area.width,
            height: SPLIT_SIZE,
        };
        let second = Rect {
            x: area.x,
            y: handle.y.saturating_add(SPLIT_SIZE),
            width: area.width,
            height: area
                .height
                .saturating_sub(first.height)
                .saturating_sub(SPLIT_SIZE),
        };
        (first, handle, second)
    }
}

fn split_secondary(area: Rect, preset: DevLayoutPreset, ratio: u16, count: usize) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![area];
    }
    let handle = split_secondary_handle(area, preset, ratio, count).unwrap_or_default();
    if preset.primary_is_horizontal() {
        let first_height = handle.y.saturating_sub(area.y);
        let first = Rect {
            height: first_height,
            ..area
        };
        let remaining = Rect {
            x: area.x,
            y: handle.y.saturating_add(SPLIT_SIZE),
            width: area.width,
            height: area
                .height
                .saturating_sub(first_height)
                .saturating_sub(SPLIT_SIZE),
        };
        if count == 2 {
            vec![first, remaining]
        } else {
            let [second, third] = vertical_split(
                remaining,
                &[SplitSpec::Percentage(50), SplitSpec::Percentage(50)],
            );
            vec![first, second, third]
        }
    } else {
        let first_width = handle.x.saturating_sub(area.x);
        let first = Rect {
            width: first_width,
            ..area
        };
        let remaining = Rect {
            x: handle.x.saturating_add(SPLIT_SIZE),
            y: area.y,
            width: area
                .width
                .saturating_sub(first_width)
                .saturating_sub(SPLIT_SIZE),
            height: area.height,
        };
        if count == 2 {
            vec![first, remaining]
        } else {
            let [second, third] = horizontal_split(
                remaining,
                &[SplitSpec::Percentage(50), SplitSpec::Percentage(50)],
            );
            vec![first, second, third]
        }
    }
}

fn split_secondary_handle(
    area: Rect,
    preset: DevLayoutPreset,
    ratio_value: u16,
    count: usize,
) -> Option<Rect> {
    if count < 2 {
        return None;
    }
    let ratio_value = clamp_dev_secondary_ratio(area, preset, ratio_value);
    if preset.primary_is_horizontal() {
        let total = area.height.saturating_sub(SPLIT_SIZE);
        let first_height = ((total as u32 * ratio_value as u32) / 100) as u16;
        Some(Rect {
            x: area.x,
            y: area.y.saturating_add(first_height),
            width: area.width,
            height: SPLIT_SIZE,
        })
    } else {
        let total = area.width.saturating_sub(SPLIT_SIZE);
        let first_width = ((total as u32 * ratio_value as u32) / 100) as u16;
        Some(Rect {
            x: area.x.saturating_add(first_width),
            y: area.y,
            width: SPLIT_SIZE,
            height: area.height,
        })
    }
}

fn secondary_panels(drawer_open: bool) -> Vec<DevPanel> {
    if drawer_open {
        vec![DevPanel::Output, DevPanel::Agents]
    } else {
        vec![DevPanel::Output, DevPanel::Server, DevPanel::Agents]
    }
}

fn secondary_panel_count(drawer_open: bool) -> usize {
    secondary_panels(drawer_open).len()
}

fn primary_extent(area: Rect, preset: DevLayoutPreset) -> u16 {
    if preset.primary_is_horizontal() {
        area.width
    } else {
        area.height
    }
}

fn clamp_ratio_for_min(total: u16, ratio: u16, min_first: u16, min_second: u16) -> u16 {
    if min_first.saturating_add(min_second) > total {
        let min_first = total / 2;
        let min_second = total.saturating_sub(min_first);
        return clamp_ratio_for_min(total, ratio, min_first, min_second);
    }
    let max_first = total.saturating_sub(min_second).max(min_first);
    clamp_ratio_for_min_max(total, ratio, min_first, max_first)
}

fn clamp_ratio_for_min_max(total: u16, ratio_value: u16, min: u16, max: u16) -> u16 {
    if total == 0 || min > max {
        return ratio_value.min(100);
    }
    let min_ratio = ((min as u32 * 100).div_ceil(total as u32)).min(100) as u16;
    let max_ratio = ((max as u32 * 100) / total as u32).min(100) as u16;
    ratio_value.clamp(min_ratio, max_ratio.max(min_ratio))
}

fn ratio(value: u16, total: u16) -> u16 {
    if total == 0 {
        0
    } else {
        ((value as u32 * 100) / total as u32).min(100) as u16
    }
}

fn context_picker_height(rows: usize, terminal_height: u16) -> u16 {
    let base = rows.saturating_add(6) as u16;
    base.min(terminal_height.saturating_sub(2)).max(7)
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x
        && x < area.x.saturating_add(area.width)
        && y >= area.y
        && y < area.y.saturating_add(area.height)
}

#[derive(Clone, Copy)]
enum SplitSpec {
    Length(u16),
    Percentage(u16),
    Min(u16),
}

fn vertical_split<const N: usize>(area: Rect, specs: &[SplitSpec; N]) -> [Rect; N] {
    split(area, specs, false)
}

fn horizontal_split<const N: usize>(area: Rect, specs: &[SplitSpec; N]) -> [Rect; N] {
    split(area, specs, true)
}

fn split<const N: usize>(area: Rect, specs: &[SplitSpec; N], horizontal: bool) -> [Rect; N] {
    let total = if horizontal { area.width } else { area.height };
    let fixed: u16 = specs
        .iter()
        .map(|spec| match *spec {
            SplitSpec::Length(value) => value,
            _ => 0,
        })
        .sum();
    let percentage_total = total.saturating_sub(fixed);
    let mut sizes = [0u16; N];
    let mut used = 0u16;
    let mut min_index = None;
    for (index, spec) in specs.iter().copied().enumerate() {
        sizes[index] = match spec {
            SplitSpec::Length(value) => value.min(total.saturating_sub(used)),
            SplitSpec::Percentage(value) => ((percentage_total as u32 * value as u32) / 100) as u16,
            SplitSpec::Min(value) => {
                min_index = Some(index);
                value
            }
        };
        used = used.saturating_add(sizes[index]);
    }
    if let Some(index) = min_index {
        sizes[index] = sizes[index].saturating_add(total.saturating_sub(used));
    }

    let mut cursor_x = area.x;
    let mut cursor_y = area.y;
    std::array::from_fn(|index| {
        let rect = if horizontal {
            let rect = Rect {
                x: cursor_x,
                y: area.y,
                width: sizes[index].min(area.x.saturating_add(area.width).saturating_sub(cursor_x)),
                height: area.height,
            };
            cursor_x = cursor_x.saturating_add(rect.width);
            rect
        } else {
            let rect = Rect {
                x: area.x,
                y: cursor_y,
                width: area.width,
                height: sizes[index]
                    .min(area.y.saturating_add(area.height).saturating_sub(cursor_y)),
            };
            cursor_y = cursor_y.saturating_add(rect.height);
            rect
        };
        rect
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn input(preset: DevLayoutPreset) -> LayoutInput {
        LayoutInput {
            area: Rect::new(0, 0, 120, 32),
            active_workspace: TuiWorkspace::Dev,
            focused_dev_panel: DevPanel::Repl,
            mode: TuiMode::Normal,
            context_picker_rows: 0,
            context_picker_step: ContextPickerStep::Targets,
            agents_view_mode: AgentsViewMode::List,
            agent_details_visible: true,
            layout: TuiLayoutState {
                dev_preset: preset,
                ..TuiLayoutState::default()
            },
        }
    }

    #[test]
    fn dev_presets_place_repl_on_expected_side() {
        for preset in [
            DevLayoutPreset::Right,
            DevLayoutPreset::Left,
            DevLayoutPreset::Top,
            DevLayoutPreset::Bottom,
        ] {
            let snapshot = compute(input(preset));
            let repl = snapshot
                .region(RegionKind::DevPanelBody(DevPanel::Repl))
                .unwrap_or_else(|| panic!("missing repl for {preset:?}: {:?}", snapshot.regions));
            let output = snapshot
                .region(RegionKind::DevPanelBody(DevPanel::Output))
                .unwrap_or_else(|| panic!("missing output for {preset:?}: {:?}", snapshot.regions));
            match preset {
                DevLayoutPreset::Right => assert!(repl.x < output.x),
                DevLayoutPreset::Left => assert!(repl.x > output.x),
                DevLayoutPreset::Top => assert!(repl.y > output.y),
                DevLayoutPreset::Bottom => assert!(repl.y < output.y),
            }
        }
    }

    #[test]
    fn drawer_splits_body_and_exposes_handle() {
        let mut input = input(DevLayoutPreset::Right);
        input.layout.server_drawer_open = true;
        let snapshot = compute(input);

        assert!(snapshot.server_drawer.is_some());
        assert!(snapshot.region(RegionKind::ServerDrawerSplit).is_some());
        assert!(snapshot.workspace_body.width < snapshot.body.width);
    }

    #[test]
    fn hit_testing_returns_topmost_region() {
        let snapshot = compute(input(DevLayoutPreset::Right));
        let output = snapshot
            .region(RegionKind::DevPanelBody(DevPanel::Output))
            .unwrap();

        assert_eq!(
            snapshot.hit_test(output.x + 1, output.y + 1),
            Some(RegionKind::DevPanelBody(DevPanel::Output))
        );
    }

    #[test]
    fn ratio_clamps_preserve_minimum_panel_sizes() {
        let area = Rect::new(0, 0, 100, 30);

        assert!(clamp_dev_primary_ratio(area, DevLayoutPreset::Right, 1) > 1);
        assert!(clamp_dev_primary_ratio(area, DevLayoutPreset::Right, 99) < 99);
        assert!(clamp_drawer_ratio(area, 99) < 99);
    }
}
