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

use ratatui::style::Color;
use std::cell::Cell;

#[derive(Clone, Copy)]
pub(super) struct TuiVisualStyle {
    pub surface: Color,
    pub panel_strong: Color,
    pub border_subtle: Color,
    pub border: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_faint: Color,
    pub accent: Color,
    pub accent_hover: Color,
    pub marker: Color,
    pub info: Color,
    pub success: Color,
    pub error: Color,
    pub footer_background: Color,
    pub overlay_background: Color,
    pub input_background: Color,
    pub input_text: Color,
    pub selection_background: Color,
    pub selection_text: Color,
    pub table_odd_background: Color,
    pub table_even_background: Color,
    pub table_selected_odd_background: Color,
    pub table_selected_even_background: Color,
    pub table_selection_text: Color,
    pub footer_rule: Color,
    pub shortcut_key: Color,
}

impl TuiVisualStyle {
    pub const fn production() -> Self {
        Self {
            surface: Color::Rgb(11, 11, 15),
            panel_strong: Color::Rgb(16, 16, 21),
            border_subtle: Color::Rgb(55, 55, 65),
            border: Color::Rgb(72, 72, 84),
            text: Color::Rgb(237, 237, 240),
            text_secondary: Color::Rgb(168, 168, 180),
            text_muted: Color::Rgb(110, 110, 126),
            text_faint: Color::Rgb(74, 74, 85),
            accent: Color::Rgb(232, 165, 56),
            accent_hover: Color::Rgb(248, 187, 85),
            marker: Color::Rgb(224, 122, 61),
            info: Color::Rgb(103, 190, 205),
            success: Color::Rgb(134, 239, 172),
            error: Color::Rgb(224, 108, 117),
            footer_background: Color::Rgb(11, 11, 15),
            overlay_background: Color::Rgb(27, 27, 36),
            input_background: Color::Rgb(38, 38, 49),
            input_text: Color::Rgb(92, 180, 128),
            selection_background: Color::Rgb(158, 158, 170),
            selection_text: Color::Rgb(24, 24, 31),
            table_odd_background: Color::Rgb(4, 4, 6),
            table_even_background: Color::Rgb(28, 28, 36),
            table_selected_odd_background: Color::Rgb(45, 38, 25),
            table_selected_even_background: Color::Rgb(68, 56, 36),
            table_selection_text: Color::Rgb(237, 237, 240),
            footer_rule: Color::Rgb(55, 55, 65),
            shortcut_key: Color::Rgb(181, 124, 48),
        }
    }
}

thread_local! { static ACTIVE_STYLE: Cell<TuiVisualStyle> = const { Cell::new(TuiVisualStyle::production()) }; }

pub(super) fn active_style() -> TuiVisualStyle {
    ACTIVE_STYLE.get()
}

pub(super) fn with_style<T>(style: &TuiVisualStyle, render: impl FnOnce() -> T) -> T {
    ACTIVE_STYLE.with(|active| {
        let previous = active.replace(*style);
        let result = render();
        active.set(previous);
        result
    })
}
