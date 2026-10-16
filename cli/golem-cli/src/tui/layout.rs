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

use super::interaction::InteractionMode;
use super::navigation::{DevView, Workspace};
use super::ops::OpsDestination;
use ratatui::layout::Rect;
use std::cell::RefCell;

pub(super) const SHELL_COMPACT_WIDTH: u16 = 72;
pub(super) const SHELL_COMPACT_HEIGHT: u16 = 20;
const DEV_MIN_PRIMARY_WIDTH: u16 = 28;
const DEV_MIN_SUBVIEW_WIDTH: u16 = 20;
const DEV_MIN_PRIMARY_HEIGHT: u16 = 7;
const DEV_MIN_SUBVIEW_HEIGHT: u16 = 5;
const OPS_LIST_MIN_WIDTH: u16 = 36;
const OPS_DETAILS_MIN_WIDTH: u16 = 24;
const OPS_LIST_MIN_HEIGHT: u16 = 7;
const OPS_DETAILS_MIN_HEIGHT: u16 = 5;
const SPLIT_SIZE: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PanePlacement {
    Right,
    Bottom,
}

impl PanePlacement {
    pub(super) fn flipped(self) -> Self {
        match self {
            Self::Right => Self::Bottom,
            Self::Bottom => Self::Right,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Bottom => "bottom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DragTarget {
    DevPrimary,
    DevSecondary,
    OpsDetails,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScrollbarTarget {
    Dev(DevView),
    OpsList,
    OpsDetails,
    OpsDocument,
    OpsStream,
    MetricsDashboard,
    MetricsFamilies,
    MetricsSeries,
    ResourceKinds,
    ResourceItems,
    ResourceDetails,
    Help,
}

#[derive(Debug, Clone)]
pub(super) struct TuiLayoutState {
    pub(super) dev_visible: [bool; 3],
    pub(super) dev_placement: PanePlacement,
    pub(super) dev_primary_ratio: u16,
    pub(super) dev_secondary_ratio: u16,
    pub(super) ops_details_placement: PanePlacement,
    pub(super) ops_details_ratio: u16,
    pub(super) dragging: Option<DragTarget>,
    pub(super) drag_origin: Option<(u16, u16)>,
    pub(super) drag_moved: bool,
}

impl Default for TuiLayoutState {
    fn default() -> Self {
        Self {
            dev_visible: [true, false, false],
            dev_placement: PanePlacement::Right,
            dev_primary_ratio: 62,
            dev_secondary_ratio: 50,
            ops_details_placement: PanePlacement::Right,
            ops_details_ratio: 60,
            dragging: None,
            drag_origin: None,
            drag_moved: false,
        }
    }
}

impl TuiLayoutState {
    pub(super) fn dev_is_visible(&self, view: DevView) -> bool {
        self.dev_visible[view.index()]
    }

    pub(super) fn set_dev_visible(&mut self, view: DevView, visible: bool) {
        self.dev_visible[view.index()] = visible;
    }

    pub(super) fn visible_dev_views(&self, primary: DevView) -> Vec<DevView> {
        let mut views = vec![primary];
        views.extend(
            DevView::ALL
                .into_iter()
                .filter(|view| *view != primary && self.dev_is_visible(*view)),
        );
        views
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RegionKind {
    Workspace(Workspace),
    OpsTab(OpsDestination),
    Footer,
    WorkspaceBody,
    DevViewTitle(DevView),
    DevViewBody(DevView),
    DevPrimarySplit,
    DevSecondarySplit,
    OpsList,
    OplogMap,
    OpsDetails,
    OpsDetailsSplit,
    OpsStream,
    OverviewItem(usize),
    OverviewIssue(usize),
    MetricsPanel(usize),
    MetricsDashboard,
    MetricsChart,
    MetricsFamilies,
    MetricsSeries,
    MetricsSeriesToggle(usize),
    ResourceKinds,
    ResourceItems,
    ResourceDetails,
    OpsControl { group: usize, choice: usize },
    OpsLink(OpsDestination),
    AgentSsh,
    AgentSshInput,
    Scrollbar(ScrollbarTarget),
    DevSessionStatus(DevView),
    ContextPickerRow(usize),
    ContextConfirm,
    ContextCancel,
    PaletteAll,
    PaletteContext,
    QuitReturn,
    QuitForce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Region {
    pub(super) kind: RegionKind,
    pub(super) area: Rect,
}

#[derive(Debug, Clone)]
pub(super) struct LayoutSnapshot {
    pub(super) header: Rect,
    pub(super) workspace_body: Rect,
    pub(super) footer: Rect,
    pub(super) regions: Vec<Region>,
}

#[derive(Debug, Default)]
pub(super) struct UiGeometry {
    snapshot: RefCell<Option<LayoutSnapshot>>,
}

impl UiGeometry {
    pub(super) fn update(&self, snapshot: LayoutSnapshot) {
        self.snapshot.replace(Some(snapshot));
    }

    pub(super) fn add_region(&self, kind: RegionKind, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        if let Some(snapshot) = self.snapshot.borrow_mut().as_mut() {
            snapshot.regions.retain(|region| region.kind != kind);
            snapshot.regions.push(Region { kind, area });
        }
    }

    pub(super) fn remove_region(&self, kind: RegionKind) {
        if let Some(snapshot) = self.snapshot.borrow_mut().as_mut() {
            snapshot.regions.retain(|region| region.kind != kind);
        }
    }

    pub(super) fn snapshot(&self) -> Option<LayoutSnapshot> {
        self.snapshot.borrow().clone()
    }

    pub(super) fn region(&self, kind: RegionKind) -> Option<Rect> {
        self.snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.region(kind))
    }

    pub(super) fn hit_test(&self, x: u16, y: u16) -> Option<RegionKind> {
        self.snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.hit_test(x, y))
    }

    pub(super) fn hit_test_below_splits(&self, x: u16, y: u16) -> Option<RegionKind> {
        self.snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.hit_test_below_splits(x, y))
    }

    pub(super) fn workspace_body(&self) -> Option<Rect> {
        self.snapshot
            .borrow()
            .as_ref()
            .map(|snapshot| snapshot.workspace_body)
    }
}

impl LayoutSnapshot {
    pub(super) fn hit_test(&self, x: u16, y: u16) -> Option<RegionKind> {
        if let Some(split) = self.regions.iter().rev().find(|region| {
            matches!(
                region.kind,
                RegionKind::DevPrimarySplit
                    | RegionKind::DevSecondarySplit
                    | RegionKind::OpsDetailsSplit
            ) && contains(region.area, x, y)
        }) {
            return Some(split.kind);
        }
        self.regions
            .iter()
            .rev()
            .find(|region| contains(region.area, x, y))
            .map(|region| region.kind)
    }

    fn hit_test_below_splits(&self, x: u16, y: u16) -> Option<RegionKind> {
        self.regions
            .iter()
            .rev()
            .find(|region| {
                !matches!(
                    region.kind,
                    RegionKind::DevPrimarySplit
                        | RegionKind::DevSecondarySplit
                        | RegionKind::OpsDetailsSplit
                ) && contains(region.area, x, y)
            })
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
    pub(super) active_workspace: Workspace,
    pub(super) dev_primary: DevView,
    pub(super) focused_dev_view: DevView,
    pub(super) mode: InteractionMode,
    pub(super) context_picker_rows: Vec<Option<usize>>,
    pub(super) context_picker_selected: usize,
    pub(super) context_picker_prefix_height: usize,
    pub(super) ops_details_visible: bool,
    pub(super) dev_session_status_widths: [u16; 2],
    pub(super) layout: TuiLayoutState,
}

pub(super) fn compute(input: LayoutInput) -> LayoutSnapshot {
    let (header, body, footer) = shell_regions(input.area);
    let workspace_selector = Rect {
        height: footer.height.min(1),
        ..footer
    };
    let mut regions = vec![
        Region {
            kind: RegionKind::WorkspaceBody,
            area: body,
        },
        Region {
            kind: RegionKind::Footer,
            area: footer,
        },
    ];
    add_workspace_regions(workspace_selector, &mut regions);
    add_dev_session_status_regions(
        workspace_selector,
        input.dev_session_status_widths,
        &mut regions,
    );
    match input.active_workspace {
        Workspace::Dev => add_dev_regions(body, &input, &mut regions),
        Workspace::Ops => add_ops_regions(ops_content_area(body), &input, &mut regions),
    }
    add_modal_regions(&input, &mut regions);

    LayoutSnapshot {
        header,
        workspace_body: body,
        footer,
        regions,
    }
}

pub(super) fn ops_content_area(area: Rect) -> Rect {
    Rect {
        y: area.y.saturating_add(area.height.min(1)),
        height: area.height.saturating_sub(1),
        ..area
    }
}

fn shell_regions(area: Rect) -> (Rect, Rect, Rect) {
    if area.height == 0 {
        return (Rect::default(), Rect::default(), Rect::default());
    }
    if area.height == 1 {
        return (Rect::default(), area, Rect::default());
    }

    let header_height = 1;
    let compact = area.width < SHELL_COMPACT_WIDTH || area.height < SHELL_COMPACT_HEIGHT;
    let wanted_footer = if compact { 2 } else { 3 };
    let footer_height = wanted_footer.min(area.height.saturating_sub(header_height + 1));
    let body_height = area
        .height
        .saturating_sub(header_height)
        .saturating_sub(footer_height);
    let header = Rect::new(area.x, area.y, area.width, header_height);
    let body = Rect::new(area.x, header.bottom(), area.width, body_height);
    let footer = Rect::new(area.x, body.bottom(), area.width, footer_height);
    (header, body, footer)
}

fn add_workspace_regions(area: Rect, regions: &mut Vec<Region>) {
    let mut x = area.x.saturating_add(2);
    for workspace in Workspace::ALL {
        let width = workspace.title().chars().count() as u16 + 4;
        regions.push(Region {
            kind: RegionKind::Workspace(workspace),
            area: Rect::new(
                x,
                area.y,
                width.min(area.right().saturating_sub(x)),
                area.height,
            ),
        });
        x = x.saturating_add(width + 1);
    }
}

fn add_dev_session_status_regions(area: Rect, widths: [u16; 2], regions: &mut Vec<Region>) {
    const SELECTOR_MIN_WIDTH: u16 = 20;
    const GAP: u16 = 3;
    const DECORATOR_WIDTH: u16 = 4;

    let total = widths[0]
        .saturating_add(widths[1])
        .saturating_add(GAP)
        .saturating_add(DECORATOR_WIDTH)
        .saturating_add(1);
    if widths.contains(&0) || area.width < SELECTOR_MIN_WIDTH.saturating_add(total) {
        return;
    }

    let mut right = area.right().saturating_sub(3);
    for (view, width) in [DevView::Repl, DevView::Server]
        .into_iter()
        .zip(widths.into_iter().rev())
    {
        let x = right.saturating_sub(width);
        regions.push(Region {
            kind: RegionKind::DevSessionStatus(view),
            area: Rect::new(x, area.y, width, area.height.min(1)),
        });
        right = x.saturating_sub(GAP);
    }
}

fn add_dev_regions(area: Rect, input: &LayoutInput, regions: &mut Vec<Region>) {
    for (view, pane) in dev_view_areas(area, input) {
        let title = Rect {
            height: pane.height.min(1),
            ..pane
        };
        let body = Rect {
            y: title.bottom(),
            height: pane.height.saturating_sub(title.height),
            ..pane
        };
        regions.push(Region {
            kind: RegionKind::DevViewTitle(view),
            area: title,
        });
        regions.push(Region {
            kind: RegionKind::DevViewBody(view),
            area: body,
        });
    }
    if let Some((primary, secondary)) = dev_split_regions(area, input) {
        regions.push(Region {
            kind: RegionKind::DevPrimarySplit,
            area: primary,
        });
        if let Some(secondary) = secondary {
            regions.push(Region {
                kind: RegionKind::DevSecondarySplit,
                area: secondary,
            });
        }
    }
}

pub(super) fn dev_view_areas(area: Rect, input: &LayoutInput) -> Vec<(DevView, Rect)> {
    let visible = input.layout.visible_dev_views(input.dev_primary);
    if visible.len() == 1 || compact_body(area) {
        let view = if visible.contains(&input.focused_dev_view) {
            input.focused_dev_view
        } else {
            input.dev_primary
        };
        return vec![(view, area)];
    }

    let ratio = clamp_dev_primary_ratio(
        area,
        input.layout.dev_placement,
        input.layout.dev_primary_ratio,
    );
    let (primary, _, subviews) = split_area(area, input.layout.dev_placement, ratio);
    let mut result = vec![(input.dev_primary, primary)];
    let secondary_views = visible
        .into_iter()
        .filter(|view| *view != input.dev_primary)
        .collect::<Vec<_>>();
    let secondary_areas = split_subviews(
        subviews,
        input.layout.dev_placement,
        input.layout.dev_secondary_ratio,
        secondary_views.len(),
    );
    result.extend(secondary_views.into_iter().zip(secondary_areas));
    result
}

pub(super) fn dev_split_regions(area: Rect, input: &LayoutInput) -> Option<(Rect, Option<Rect>)> {
    let visible = input.layout.visible_dev_views(input.dev_primary);
    if visible.len() < 2 || compact_body(area) {
        return None;
    }
    let ratio = clamp_dev_primary_ratio(
        area,
        input.layout.dev_placement,
        input.layout.dev_primary_ratio,
    );
    let (_, primary, subviews) = split_area(area, input.layout.dev_placement, ratio);
    let secondary = split_subview_handle(
        subviews,
        input.layout.dev_placement,
        input.layout.dev_secondary_ratio,
        visible.len() - 1,
    );
    Some((primary, secondary))
}

fn add_ops_regions(area: Rect, input: &LayoutInput, regions: &mut Vec<Region>) {
    let (list, divider, details) = ops_pane_regions(
        area,
        input.ops_details_visible,
        input.layout.ops_details_placement,
        input.layout.ops_details_ratio,
    );
    regions.push(Region {
        kind: RegionKind::OpsList,
        area: list,
    });
    if let Some(details) = details {
        regions.push(Region {
            kind: RegionKind::OpsDetails,
            area: details,
        });
    }
    if let Some(divider) = divider {
        regions.push(Region {
            kind: RegionKind::OpsDetailsSplit,
            area: divider,
        });
    }
}

pub(super) fn ops_pane_regions(
    area: Rect,
    requested: bool,
    placement: PanePlacement,
    ratio: u16,
) -> (Rect, Option<Rect>, Option<Rect>) {
    if !ops_details_visible(area, requested, placement) {
        return (area, None, None);
    }
    let ratio = clamp_ops_details_ratio(area, placement, ratio);
    let (list, divider, details) = split_area(area, placement, ratio);
    (list, Some(divider), Some(details))
}

pub(super) fn ops_details_visible(area: Rect, requested: bool, placement: PanePlacement) -> bool {
    if !requested {
        return false;
    }
    match placement {
        PanePlacement::Right => {
            area.width >= OPS_LIST_MIN_WIDTH + OPS_DETAILS_MIN_WIDTH + SPLIT_SIZE
        }
        PanePlacement::Bottom => area.height >= OPS_LIST_MIN_HEIGHT + OPS_DETAILS_MIN_HEIGHT,
    }
}

pub(super) fn clamp_dev_primary_ratio(area: Rect, placement: PanePlacement, ratio: u16) -> u16 {
    let (total, first, second) = match placement {
        PanePlacement::Right => (area.width, DEV_MIN_PRIMARY_WIDTH, DEV_MIN_SUBVIEW_WIDTH),
        PanePlacement::Bottom => (area.height, DEV_MIN_PRIMARY_HEIGHT, DEV_MIN_SUBVIEW_HEIGHT),
    };
    clamp_ratio_for_min(
        total.saturating_sub(u16::from(placement == PanePlacement::Right)),
        ratio,
        first,
        second,
    )
}

pub(super) fn clamp_dev_secondary_ratio(area: Rect, placement: PanePlacement, ratio: u16) -> u16 {
    let (total, minimum) = match placement {
        PanePlacement::Right => (area.height, DEV_MIN_SUBVIEW_HEIGHT),
        PanePlacement::Bottom => (area.width, DEV_MIN_SUBVIEW_WIDTH),
    };
    let split_is_right = placement == PanePlacement::Bottom;
    clamp_ratio_for_min(
        total.saturating_sub(u16::from(split_is_right)),
        ratio,
        minimum,
        minimum,
    )
}

pub(super) fn clamp_ops_details_ratio(area: Rect, placement: PanePlacement, ratio: u16) -> u16 {
    let (total, first, second) = match placement {
        PanePlacement::Right => (area.width, OPS_LIST_MIN_WIDTH, OPS_DETAILS_MIN_WIDTH),
        PanePlacement::Bottom => (area.height, OPS_LIST_MIN_HEIGHT, OPS_DETAILS_MIN_HEIGHT),
    };
    clamp_ratio_for_min(
        total.saturating_sub(u16::from(placement == PanePlacement::Right)),
        ratio,
        first,
        second,
    )
}

pub(super) fn dev_primary_ratio_from_pointer(
    area: Rect,
    placement: PanePlacement,
    x: u16,
    y: u16,
) -> u16 {
    pointer_ratio(area, placement, x, y)
}

pub(super) fn dev_secondary_ratio_from_pointer(
    area: Rect,
    placement: PanePlacement,
    x: u16,
    y: u16,
) -> u16 {
    let cross = match placement {
        PanePlacement::Right => PanePlacement::Bottom,
        PanePlacement::Bottom => PanePlacement::Right,
    };
    pointer_ratio(area, cross, x, y)
}

pub(super) fn ops_details_ratio_from_pointer(
    area: Rect,
    placement: PanePlacement,
    x: u16,
    y: u16,
) -> u16 {
    pointer_ratio(area, placement, x, y)
}

fn pointer_ratio(area: Rect, placement: PanePlacement, x: u16, y: u16) -> u16 {
    match placement {
        PanePlacement::Right => ratio(
            x.saturating_sub(area.x),
            area.width.saturating_sub(SPLIT_SIZE),
        ),
        PanePlacement::Bottom => ratio(y.saturating_sub(area.y), area.height),
    }
}

fn split_area(area: Rect, placement: PanePlacement, ratio_value: u16) -> (Rect, Rect, Rect) {
    match placement {
        PanePlacement::Right => {
            let total = area.width.saturating_sub(SPLIT_SIZE);
            let first_width = (u32::from(total) * u32::from(ratio_value) / 100) as u16;
            let first = Rect {
                width: first_width,
                ..area
            };
            let divider = Rect::new(
                first.right(),
                area.y,
                SPLIT_SIZE.min(area.width.saturating_sub(first_width)),
                area.height,
            );
            let second = Rect::new(
                divider.right(),
                area.y,
                area.right().saturating_sub(divider.right()),
                area.height,
            );
            (first, divider, second)
        }
        PanePlacement::Bottom => {
            let total = area.height;
            let first_height = (u32::from(total) * u32::from(ratio_value) / 100) as u16;
            let first = Rect {
                height: first_height,
                ..area
            };
            let second = Rect::new(
                area.x,
                first.bottom(),
                area.width,
                area.bottom().saturating_sub(first.bottom()),
            );
            let divider = Rect::new(area.x, second.y, area.width, SPLIT_SIZE.min(second.height));
            (first, divider, second)
        }
    }
}

fn split_subviews(
    area: Rect,
    placement: PanePlacement,
    ratio_value: u16,
    count: usize,
) -> Vec<Rect> {
    match count {
        0 => Vec::new(),
        1 => vec![area],
        _ => {
            let cross = match placement {
                PanePlacement::Right => PanePlacement::Bottom,
                PanePlacement::Bottom => PanePlacement::Right,
            };
            let ratio_value = clamp_dev_secondary_ratio(area, placement, ratio_value);
            let (first, _, second) = split_area(area, cross, ratio_value);
            vec![first, second]
        }
    }
}

fn split_subview_handle(
    area: Rect,
    placement: PanePlacement,
    ratio_value: u16,
    count: usize,
) -> Option<Rect> {
    if count < 2 {
        return None;
    }
    let cross = match placement {
        PanePlacement::Right => PanePlacement::Bottom,
        PanePlacement::Bottom => PanePlacement::Right,
    };
    let ratio_value = clamp_dev_secondary_ratio(area, placement, ratio_value);
    let (_, divider, _) = split_area(area, cross, ratio_value);
    Some(divider)
}

fn compact_body(area: Rect) -> bool {
    area.width < SHELL_COMPACT_WIDTH || area.height < 12
}

fn add_modal_regions(input: &LayoutInput, regions: &mut Vec<Region>) {
    match input.mode {
        InteractionMode::ContextPicker => {
            let area = adaptive_data_popup_rect(input.area, 48, 10);
            let content = Rect {
                x: area.x.saturating_add(2),
                y: area.y.saturating_add(2),
                width: area.width.saturating_sub(4),
                height: area.height.saturating_sub(4),
            };
            let capacity = (content.height as usize)
                .saturating_sub(input.context_picker_prefix_height)
                .saturating_sub(2)
                .max(1);
            let selected_line = input
                .context_picker_rows
                .iter()
                .position(|index| *index == Some(input.context_picker_selected))
                .unwrap_or_default();
            let start = selected_line.saturating_sub(capacity.saturating_sub(1));
            for (offset, index) in input
                .context_picker_rows
                .iter()
                .skip(start)
                .take(capacity)
                .enumerate()
                .filter_map(|(offset, index)| index.map(|index| (offset, index)))
            {
                regions.push(Region {
                    kind: RegionKind::ContextPickerRow(index),
                    area: Rect::new(
                        content.x,
                        content
                            .y
                            .saturating_add(input.context_picker_prefix_height as u16)
                            .saturating_add(offset as u16),
                        content.width,
                        content.height.min(1),
                    ),
                });
            }
        }
        InteractionMode::ContextSwitchConfirm => {
            let area = centered(
                input.area,
                input.area.width.min(72),
                input.area.height.min(11),
            );
            let y = area
                .y
                .saturating_add(6)
                .min(area.bottom().saturating_sub(2));
            regions.push(Region {
                kind: RegionKind::ContextConfirm,
                area: Rect::new(area.x.saturating_add(2), y, 24.min(area.width), 1),
            });
            regions.push(Region {
                kind: RegionKind::ContextCancel,
                area: Rect::new(
                    area.x.saturating_add(28),
                    y,
                    24.min(area.width.saturating_sub(28)),
                    1,
                ),
            });
        }
        InteractionMode::Palette => {
            let area = adaptive_data_popup_rect(input.area, 36, 10);
            let content = Rect {
                x: area.x.saturating_add(2),
                y: area.y.saturating_add(2),
                width: area.width.saturating_sub(4),
                height: area.height.saturating_sub(4),
            };
            let half = content.width / 2;
            regions.push(Region {
                kind: RegionKind::PaletteAll,
                area: Rect::new(content.x, content.y, half, content.height.min(1)),
            });
            regions.push(Region {
                kind: RegionKind::PaletteContext,
                area: Rect::new(
                    content.x.saturating_add(half),
                    content.y,
                    content.width.saturating_sub(half),
                    content.height.min(1),
                ),
            });
        }
        InteractionMode::QuitBlocked => {
            let area = content_popup_rect(input.area, 58, 12);
            let content = Rect {
                x: area.x.saturating_add(2),
                y: area.y.saturating_add(2),
                width: area.width.saturating_sub(4),
                height: area.height.saturating_sub(4),
            };
            let gap = u16::from(content.width > 1);
            let return_width = content.width.saturating_sub(gap) / 2;
            regions.push(Region {
                kind: RegionKind::QuitReturn,
                area: Rect::new(content.x, content.y.saturating_add(4), return_width, 1),
            });
            regions.push(Region {
                kind: RegionKind::QuitForce,
                area: Rect::new(
                    content.x.saturating_add(return_width).saturating_add(gap),
                    content.y.saturating_add(4),
                    content.width.saturating_sub(return_width + gap),
                    1,
                ),
            });
        }
        _ => {}
    }
}

pub(super) fn adaptive_data_popup_rect(
    area: Rect,
    minimum_width: u16,
    minimum_height: u16,
) -> Rect {
    fn dimension(available: u16, minimum: u16) -> u16 {
        if available <= 2 {
            return available;
        }
        let maximum = available.saturating_sub(2);
        let target = ((u32::from(available) * 85) / 100) as u16;
        target.max(minimum.min(maximum)).min(maximum)
    }

    centered(
        area,
        dimension(area.width, minimum_width),
        dimension(area.height, minimum_height),
    )
}

pub(super) fn content_popup_rect(area: Rect, width: u16, height: u16) -> Rect {
    if area.width <= 2 || area.height <= 2 {
        return area;
    }
    let maximum_width = area.width.saturating_sub(2);
    let maximum_height = area.height.saturating_sub(2);
    centered(area, width.min(maximum_width), height.min(maximum_height))
}

pub(super) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn clamp_ratio_for_min(total: u16, ratio_value: u16, min_first: u16, min_second: u16) -> u16 {
    if total == 0 {
        return ratio_value.min(100);
    }
    if min_first.saturating_add(min_second) > total {
        return 50;
    }
    let min_ratio = ((u32::from(min_first) * 100).div_ceil(u32::from(total))).min(100) as u16;
    let max_ratio =
        ((u32::from(total.saturating_sub(min_second)) * 100) / u32::from(total)).min(100) as u16;
    ratio_value.clamp(min_ratio, max_ratio.max(min_ratio))
}

fn ratio(value: u16, total: u16) -> u16 {
    if total == 0 {
        0
    } else {
        ((u32::from(value) * 100) / u32::from(total)).min(100) as u16
    }
}

fn contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x && x < area.right() && y >= area.y && y < area.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn input() -> LayoutInput {
        LayoutInput {
            area: Rect::new(0, 0, 120, 32),
            active_workspace: Workspace::Dev,
            dev_primary: DevView::BuildDeploy,
            focused_dev_view: DevView::BuildDeploy,
            mode: InteractionMode::Normal,
            context_picker_rows: Vec::new(),
            context_picker_selected: 0,
            context_picker_prefix_height: 2,
            ops_details_visible: true,
            dev_session_status_widths: [0, 0],
            layout: TuiLayoutState::default(),
        }
    }

    #[test]
    fn one_dev_view_fills_the_body() {
        let input = input();
        let snapshot = compute(input);
        assert_eq!(
            snapshot
                .region(RegionKind::DevViewTitle(DevView::BuildDeploy))
                .unwrap()
                .width,
            snapshot.workspace_body.width
        );
        assert!(snapshot.region(RegionKind::DevPrimarySplit).is_none());
    }

    #[test]
    fn ui_geometry_exposes_only_the_latest_render_snapshot() {
        let geometry = UiGeometry::default();
        assert!(geometry.snapshot().is_none());

        geometry.update(compute(input()));
        let dev_body = geometry
            .region(RegionKind::DevViewBody(DevView::BuildDeploy))
            .unwrap();
        assert_eq!(
            geometry.hit_test(dev_body.x, dev_body.y),
            Some(RegionKind::DevViewBody(DevView::BuildDeploy))
        );

        let mut second_input = input();
        second_input.active_workspace = Workspace::Ops;
        geometry.update(compute(second_input));
        assert!(
            geometry
                .region(RegionKind::DevViewBody(DevView::BuildDeploy))
                .is_none()
        );
        assert!(geometry.region(RegionKind::OpsList).is_some());
    }

    #[test]
    fn resize_split_hit_targets_win_over_overlapping_pane_regions() {
        let overlap = Rect::new(10, 4, 1, 8);
        let snapshot = LayoutSnapshot {
            header: Rect::default(),
            workspace_body: Rect::new(0, 0, 30, 16),
            footer: Rect::default(),
            regions: vec![
                Region {
                    kind: RegionKind::OpsDetailsSplit,
                    area: overlap,
                },
                Region {
                    kind: RegionKind::OpsDetails,
                    area: Rect::new(10, 0, 20, 16),
                },
            ],
        };

        assert_eq!(
            snapshot.hit_test(overlap.x, overlap.y),
            Some(RegionKind::OpsDetailsSplit)
        );
    }

    #[test]
    fn right_and_bottom_subviews_use_physical_placement() {
        for placement in [PanePlacement::Right, PanePlacement::Bottom] {
            let mut input = input();
            input.layout.dev_placement = placement;
            input.layout.set_dev_visible(DevView::Server, true);
            let areas = dev_view_areas(input.area, &input);
            let primary = areas
                .iter()
                .find(|(view, _)| *view == DevView::BuildDeploy)
                .unwrap()
                .1;
            let server = areas
                .iter()
                .find(|(view, _)| *view == DevView::Server)
                .unwrap()
                .1;
            match placement {
                PanePlacement::Right => assert!(primary.x < server.x),
                PanePlacement::Bottom => assert!(primary.y < server.y),
            }
        }
    }

    #[test]
    fn bottom_split_reuses_the_second_pane_title_row() {
        let area = Rect::new(3, 4, 80, 20);
        let (first, divider, second) = split_area(area, PanePlacement::Bottom, 60);

        assert_eq!(first.bottom(), divider.y);
        assert_eq!(second.y, divider.y);
        assert_eq!(divider.height, 1);
        assert_eq!(second.height, area.bottom() - divider.y);
        assert_eq!(first.height + second.height, area.height);
    }

    #[test]
    fn three_views_expose_both_dividers() {
        let mut input = input();
        input.layout.set_dev_visible(DevView::Server, true);
        input.layout.set_dev_visible(DevView::Repl, true);
        let snapshot = compute(input);
        assert!(snapshot.region(RegionKind::DevPrimarySplit).is_some());
        assert!(snapshot.region(RegionKind::DevSecondarySplit).is_some());
    }

    #[test]
    fn ops_details_support_right_and_bottom() {
        for placement in [PanePlacement::Right, PanePlacement::Bottom] {
            let mut input = input();
            input.active_workspace = Workspace::Ops;
            input.layout.ops_details_placement = placement;
            let snapshot = compute(input);
            let list = snapshot.region(RegionKind::OpsList).unwrap();
            let details = snapshot.region(RegionKind::OpsDetails).unwrap();
            match placement {
                PanePlacement::Right => assert!(list.x < details.x),
                PanePlacement::Bottom => {
                    assert!(list.y < details.y);
                    let divider = snapshot.region(RegionKind::OpsDetailsSplit).unwrap();
                    assert_eq!(details.y, divider.y);
                    assert_eq!(
                        snapshot.hit_test(divider.x, divider.y),
                        Some(RegionKind::OpsDetailsSplit)
                    );
                }
            }
        }
    }

    #[test]
    fn compact_layout_retains_state_but_renders_one_pane() {
        let mut input = input();
        input.area.width = SHELL_COMPACT_WIDTH - 1;
        input.layout.set_dev_visible(DevView::Server, true);
        input.focused_dev_view = DevView::Server;
        let snapshot = compute(input);
        assert!(
            snapshot
                .region(RegionKind::DevViewBody(DevView::Server))
                .is_some()
        );
        assert!(
            snapshot
                .region(RegionKind::DevViewBody(DevView::BuildDeploy))
                .is_none()
        );
    }

    #[test]
    fn extreme_shell_sizes_never_overflow() {
        for area in [
            Rect::new(0, 0, 1, 1),
            Rect::new(0, 0, 8, 2),
            Rect::new(0, 0, 40, 8),
        ] {
            let mut input = input();
            input.area = area;
            let snapshot = compute(input);
            assert!(snapshot.header.bottom() <= area.bottom());
            assert!(snapshot.workspace_body.bottom() <= area.bottom());
            assert!(snapshot.footer.bottom() <= area.bottom());
        }
    }

    #[test]
    fn context_picker_rows_follow_adaptive_popup() {
        let mut input = input();
        input.mode = InteractionMode::ContextPicker;
        input.context_picker_rows = vec![None, Some(0), Some(1), None, Some(2)];
        input.context_picker_selected = 2;
        let snapshot = compute(input);
        assert!(snapshot.region(RegionKind::ContextPickerRow(2)).is_some());
    }

    #[test]
    fn content_popup_uses_requested_size_until_the_terminal_clamps_it() {
        assert_eq!(
            content_popup_rect(Rect::new(0, 0, 120, 40), 58, 12),
            Rect::new(31, 14, 58, 12)
        );
        assert_eq!(
            content_popup_rect(Rect::new(0, 0, 40, 8), 58, 12),
            Rect::new(1, 1, 38, 6)
        );
    }

    #[test]
    fn palette_and_quit_actions_have_modal_hit_regions() {
        let mut palette = input();
        palette.mode = InteractionMode::Palette;
        let palette = compute(palette);
        assert!(palette.region(RegionKind::PaletteAll).is_some());
        assert!(palette.region(RegionKind::PaletteContext).is_some());

        let mut quit = input();
        quit.mode = InteractionMode::QuitBlocked;
        let quit = compute(quit);
        let return_action = quit.region(RegionKind::QuitReturn).unwrap();
        let force_action = quit.region(RegionKind::QuitForce).unwrap();
        assert_eq!(return_action.y, force_action.y);
        assert!(return_action.right() < force_action.right());
        assert!(return_action.right() <= force_action.x);
    }
}
