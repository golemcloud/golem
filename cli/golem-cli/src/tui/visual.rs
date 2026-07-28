// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");

use ratatui::style::Color;
use std::cell::Cell;

#[cfg(feature = "tui-preview")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TuiVisualVariant {
    Production,
    Quiet,
    Dense,
    OpsContrast,
}

#[cfg(feature = "tui-preview")]
impl TuiVisualVariant {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Production => "Production",
            Self::Quiet => "Quiet",
            Self::Dense => "Dense",
            Self::OpsContrast => "Ops Contrast",
        }
    }

    pub(super) const ALL: [Self; 4] = [
        Self::Production,
        Self::Quiet,
        Self::Dense,
        Self::OpsContrast,
    ];
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
        }
    }

    #[cfg(feature = "tui-preview")]
    pub fn for_variant(variant: TuiVisualVariant) -> Self {
        let mut style = Self::production();
        match variant {
            TuiVisualVariant::Production => {}
            TuiVisualVariant::Quiet => {
                style.accent = Color::Rgb(190, 153, 91);
                style.accent_hover = Color::Rgb(217, 179, 112);
                style.panel_strong = Color::Rgb(22, 22, 29);
            }
            TuiVisualVariant::Dense => {
                style.accent = Color::Rgb(255, 190, 70);
                style.border_subtle = Color::Rgb(66, 59, 45);
                style.panel_strong = Color::Rgb(34, 31, 27);
            }
            TuiVisualVariant::OpsContrast => {
                style.surface = Color::Rgb(9, 16, 20);
                style.panel = Color::Rgb(14, 27, 33);
                style.panel_strong = Color::Rgb(18, 38, 44);
                style.accent = Color::Rgb(74, 210, 190);
                style.accent_hover = Color::Rgb(116, 236, 214);
            }
        }
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
