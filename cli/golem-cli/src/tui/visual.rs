// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");

use ratatui::style::Color;
use std::cell::Cell;

#[cfg(feature = "tui-preview")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TuiVisualVariant {
    Production,
    FrameBase,
}

#[cfg(feature = "tui-preview")]
impl TuiVisualVariant {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Production => "Production",
            Self::FrameBase => "Frame Base",
        }
    }

    pub(super) const ALL: [Self; 2] = [Self::Production, Self::FrameBase];
}

#[cfg(feature = "tui-preview")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PaneEdgeStyle {
    Production,
    Shared,
}

#[cfg(feature = "tui-preview")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PaneTitleStyle {
    Production,
    HintNone,
}

#[cfg(feature = "tui-preview")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FooterLayoutStyle {
    Production,
    Joined,
}

#[cfg(feature = "tui-preview")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HeaderMetadataStyle {
    Labels,
    Dividers,
}

#[cfg(feature = "tui-preview")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ChromeTextStyle {
    Filled,
    PlainPadded,
}

#[cfg(feature = "tui-preview")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HeaderSeparatorStyle {
    Divider,
    Dot,
}

#[derive(Clone, Copy)]
pub(super) struct TuiVisualStyle {
    pub background: Color,
    pub surface: Color,
    pub panel: Color,
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
    pub success: Color,
    pub error: Color,
    pub footer_background: Color,
    #[cfg(feature = "tui-preview")]
    pub rail_glyph: &'static str,
    #[cfg(feature = "tui-preview")]
    pub focus_rail_glyph: &'static str,
    #[cfg(feature = "tui-preview")]
    pub focus_marker: &'static str,
    #[cfg(feature = "tui-preview")]
    pub idle_marker: &'static str,
    #[cfg(feature = "tui-preview")]
    pub fill_header: bool,
    #[cfg(feature = "tui-preview")]
    pub frame_overlays: bool,
    #[cfg(feature = "tui-preview")]
    pub overlay_background: Color,
    #[cfg(feature = "tui-preview")]
    pub input_background: Color,
    #[cfg(feature = "tui-preview")]
    pub input_text: Color,
    #[cfg(feature = "tui-preview")]
    pub selection_background: Color,
    #[cfg(feature = "tui-preview")]
    pub selection_text: Color,
    #[cfg(feature = "tui-preview")]
    pub table_odd_background: Color,
    #[cfg(feature = "tui-preview")]
    pub table_even_background: Color,
    #[cfg(feature = "tui-preview")]
    pub table_selected_odd_background: Color,
    #[cfg(feature = "tui-preview")]
    pub table_selected_even_background: Color,
    #[cfg(feature = "tui-preview")]
    pub table_selection_text: Color,
    #[cfg(feature = "tui-preview")]
    pub pane_edges: PaneEdgeStyle,
    #[cfg(feature = "tui-preview")]
    pub pane_titles: PaneTitleStyle,
    #[cfg(feature = "tui-preview")]
    pub footer_layout: FooterLayoutStyle,
    #[cfg(feature = "tui-preview")]
    pub footer_rule: Color,
    #[cfg(feature = "tui-preview")]
    pub shared_chrome_background: bool,
    #[cfg(feature = "tui-preview")]
    pub footer_closes: bool,
    #[cfg(feature = "tui-preview")]
    pub header_metadata: HeaderMetadataStyle,
    #[cfg(feature = "tui-preview")]
    pub chrome_text: ChromeTextStyle,
    #[cfg(feature = "tui-preview")]
    pub shortcut_key: Color,
    #[cfg(feature = "tui-preview")]
    pub header_separator: HeaderSeparatorStyle,
    #[cfg(feature = "tui-preview")]
    pub header_rail_gap: &'static str,
}

impl TuiVisualStyle {
    pub const fn production() -> Self {
        Self {
            background: Color::Rgb(10, 10, 13),
            surface: Color::Rgb(13, 13, 18),
            panel: Color::Rgb(20, 20, 27),
            panel_strong: Color::Rgb(26, 26, 34),
            border_subtle: Color::Rgb(42, 42, 53),
            border: Color::Rgb(58, 58, 72),
            text: Color::Rgb(237, 237, 240),
            text_secondary: Color::Rgb(168, 168, 180),
            text_muted: Color::Rgb(110, 110, 126),
            text_faint: Color::Rgb(74, 74, 85),
            accent: Color::Rgb(245, 176, 62),
            accent_hover: Color::Rgb(255, 197, 96),
            marker: Color::Rgb(224, 122, 61),
            success: Color::Rgb(134, 239, 172),
            error: Color::Rgb(224, 108, 117),
            footer_background: Color::Rgb(20, 20, 27),
            #[cfg(feature = "tui-preview")]
            rail_glyph: "┃",
            #[cfg(feature = "tui-preview")]
            focus_rail_glyph: "┃",
            #[cfg(feature = "tui-preview")]
            focus_marker: "●",
            #[cfg(feature = "tui-preview")]
            idle_marker: "○",
            #[cfg(feature = "tui-preview")]
            fill_header: true,
            #[cfg(feature = "tui-preview")]
            frame_overlays: false,
            #[cfg(feature = "tui-preview")]
            overlay_background: Color::Rgb(26, 26, 34),
            #[cfg(feature = "tui-preview")]
            input_background: Color::Rgb(38, 38, 49),
            #[cfg(feature = "tui-preview")]
            input_text: Color::Rgb(92, 180, 128),
            #[cfg(feature = "tui-preview")]
            selection_background: Color::Rgb(158, 158, 170),
            #[cfg(feature = "tui-preview")]
            selection_text: Color::Rgb(24, 24, 31),
            #[cfg(feature = "tui-preview")]
            table_odd_background: Color::Rgb(20, 20, 27),
            #[cfg(feature = "tui-preview")]
            table_even_background: Color::Rgb(24, 24, 31),
            #[cfg(feature = "tui-preview")]
            table_selected_odd_background: Color::Rgb(42, 42, 52),
            #[cfg(feature = "tui-preview")]
            table_selected_even_background: Color::Rgb(48, 48, 58),
            #[cfg(feature = "tui-preview")]
            table_selection_text: Color::Rgb(237, 237, 240),
            #[cfg(feature = "tui-preview")]
            pane_edges: PaneEdgeStyle::Production,
            #[cfg(feature = "tui-preview")]
            pane_titles: PaneTitleStyle::Production,
            #[cfg(feature = "tui-preview")]
            footer_layout: FooterLayoutStyle::Production,
            #[cfg(feature = "tui-preview")]
            footer_rule: Color::Rgb(74, 74, 85),
            #[cfg(feature = "tui-preview")]
            shared_chrome_background: false,
            #[cfg(feature = "tui-preview")]
            footer_closes: false,
            #[cfg(feature = "tui-preview")]
            header_metadata: HeaderMetadataStyle::Labels,
            #[cfg(feature = "tui-preview")]
            chrome_text: ChromeTextStyle::Filled,
            #[cfg(feature = "tui-preview")]
            shortcut_key: Color::Rgb(255, 197, 96),
            #[cfg(feature = "tui-preview")]
            header_separator: HeaderSeparatorStyle::Divider,
            #[cfg(feature = "tui-preview")]
            header_rail_gap: " ",
        }
    }

    #[cfg(feature = "tui-preview")]
    pub fn for_variant(variant: TuiVisualVariant) -> Self {
        let mut style = Self::production();
        if variant == TuiVisualVariant::Production {
            return style;
        }

        style.surface = Color::Rgb(11, 11, 15);
        style.panel = style.surface;
        style.panel_strong = Color::Rgb(16, 16, 21);
        style.border_subtle = Color::Rgb(55, 55, 65);
        style.border = Color::Rgb(72, 72, 84);
        style.accent = Color::Rgb(232, 165, 56);
        style.accent_hover = Color::Rgb(248, 187, 85);
        style.rail_glyph = "│";
        style.focus_rail_glyph = "│";
        style.focus_marker = ">";
        style.idle_marker = "·";
        style.fill_header = false;
        style.frame_overlays = true;
        style.overlay_background = Color::Rgb(27, 27, 36);
        style.input_background = Color::Rgb(38, 38, 49);
        style.input_text = Color::Rgb(92, 180, 128);
        style.selection_background = Color::Rgb(158, 158, 170);
        style.selection_text = Color::Rgb(24, 24, 31);
        style.table_odd_background = Color::Rgb(4, 4, 6);
        style.table_even_background = Color::Rgb(28, 28, 36);
        style.table_selected_odd_background = Color::Rgb(36, 36, 46);
        style.table_selected_even_background = Color::Rgb(64, 64, 78);
        style.table_selection_text = style.text;
        style.header_rail_gap = "";
        style.pane_edges = PaneEdgeStyle::Shared;
        style.footer_background = Color::Rgb(32, 32, 41);
        style.pane_titles = PaneTitleStyle::HintNone;
        style.footer_layout = FooterLayoutStyle::Joined;
        style.footer_rule = style.border_subtle;
        style.shared_chrome_background = true;
        style.footer_closes = true;
        style.header_metadata = HeaderMetadataStyle::Dividers;
        style.shortcut_key = Color::Rgb(181, 124, 48);
        style.chrome_text = ChromeTextStyle::PlainPadded;
        style.header_separator = HeaderSeparatorStyle::Dot;
        style.footer_background = style.surface;
        style
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
