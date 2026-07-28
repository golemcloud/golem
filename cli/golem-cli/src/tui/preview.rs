// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");

use super::app::render_preview_buffer;
#[cfg(test)]
use super::app::render_production_preview_buffer;
use super::visual::TuiVisualVariant;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const STORIES: &[(&str, &str, u16, u16)] = &[
    ("home-idle", "Home / idle", 100, 24),
    ("home-active", "Home / active work", 100, 24),
    ("dev-running", "Dev / running", 120, 30),
    ("dev-completed", "Dev / completed", 120, 30),
    ("dev-failed", "Dev / failed", 120, 30),
    ("dev-server-drawer", "Dev / server drawer", 140, 32),
    ("dev-layout-left", "Dev / layout left", 120, 30),
    ("dev-layout-top", "Dev / layout top", 120, 30),
    ("dev-layout-bottom", "Dev / layout bottom", 120, 30),
    ("ops-list", "Ops / list", 100, 24),
    ("ops-details", "Ops / details", 120, 28),
    ("ops-loading", "Ops / loading", 100, 24),
    ("ops-error", "Ops / error", 100, 24),
    ("agent-inspect", "Agent / inspect", 120, 28),
    ("palette", "Overlay / palette", 100, 24),
    ("help", "Overlay / help", 100, 28),
    ("context-picker", "Overlay / context picker", 100, 24),
    ("loading", "Overlay / loading", 100, 24),
    ("confirmation", "Overlay / confirmation", 100, 24),
    ("home-idle", "Home / compact", 72, 20),
    ("ops-details", "Ops / wide", 160, 32),
];

pub fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref().unwrap_or("serve") {
        "serve" => serve(args.next().as_deref().unwrap_or("127.0.0.1:4173")),
        "terminal" => terminal_gallery(),
        "export" => export(args.next().map(PathBuf::from)),
        mode => {
            anyhow::bail!("unknown preview mode `{mode}` (expected serve, terminal, or export)")
        }
    }
}

fn gallery_html() -> anyhow::Result<String> {
    let mut cards = String::new();
    for (story, title, width, height) in STORIES {
        for variant in TuiVisualVariant::ALL {
            let buffer = render_preview_buffer(story, variant, *width, *height)?;
            write!(
                cards,
                "<article data-story=\"{}\" data-variant=\"{}\"><h2>{} · {}</h2>{}</article>",
                escape_html(story),
                escape_html(variant.name()),
                escape_html(title),
                variant.name(),
                buffer_html(&buffer)
            )?;
        }
    }
    Ok(format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Golem TUI previews</title>
<style>:root{{color-scheme:dark}}body{{margin:0;background:#08080b;color:#ededf0;font:14px system-ui}}header{{position:sticky;top:0;z-index:2;padding:12px 20px;background:#111118;border-bottom:1px solid #333}}select{{margin-right:12px}}main{{padding:18px;display:grid;gap:18px}}article{{overflow:auto}}h2{{font-size:13px;font-weight:600;color:#aaa;margin:0 0 6px}}pre{{display:inline-block;margin:0;padding:12px;line-height:1;font:12px/1 ui-monospace,SFMono-Regular,Consolas,monospace;background:#0a0a0d}}body[data-mode=story] main{{grid-template-columns:repeat(4,minmax(0,1fr))}}body[data-mode=variant] main{{grid-template-columns:1fr}}article[hidden]{{display:none}}</style></head>
<body data-mode="story"><header><select id="mode"><option value="story">one story / all variants</option><option value="variant">all stories / one variant</option></select><select id="story"></select><select id="variant"></select></header><main>{cards}</main>
<script>const cards=[...document.querySelectorAll('article')],story=document.querySelector('#story'),variant=document.querySelector('#variant'),mode=document.querySelector('#mode');
for(const v of [...new Set(cards.map(x=>x.dataset.story))])story.add(new Option(v,v));for(const v of [...new Set(cards.map(x=>x.dataset.variant))])variant.add(new Option(v,v));
function show(){{document.body.dataset.mode=mode.value;cards.forEach(x=>x.hidden=mode.value==='story'?x.dataset.story!==story.value:x.dataset.variant!==variant.value)}}mode.onchange=story.onchange=variant.onchange=show;show();let revision;
setInterval(async()=>{{try{{const next=await fetch('/revision',{{cache:'no-store'}}).then(x=>x.text());if(revision&&revision!==next)location.reload();revision=next}}catch{{}}}},700);</script></body></html>"#
    ))
}

fn export(path: Option<PathBuf>) -> anyhow::Result<()> {
    let path = path.unwrap_or_else(|| {
        let target = std::env::var_os("CARGO_MAKE_CRATE_TARGET_DIRECTORY")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target"));
        target.join("tui-preview/index.html")
    });
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, gallery_html()?)?;
    println!("exported {}", path.display());
    Ok(())
}

fn serve(address: &str) -> anyhow::Result<()> {
    let html = gallery_html()?;
    let revision = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .to_string();
    let listener = TcpListener::bind(address)?;
    println!("Golem TUI preview: http://{address}");
    for stream in listener.incoming() {
        respond(stream?, &html, &revision)?;
    }
    Ok(())
}

fn respond(mut stream: TcpStream, html: &str, revision: &str) -> anyhow::Result<()> {
    let mut request = [0; 2048];
    let len = stream.read(&mut request)?;
    let path = std::str::from_utf8(&request[..len])
        .ok()
        .and_then(|r| r.split_whitespace().nth(1))
        .unwrap_or("/");
    let (status, kind, body) = match path.split('?').next().unwrap_or(path) {
        "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", html),
        "/revision" => ("200 OK", "text/plain; charset=utf-8", revision),
        _ => ("404 Not Found", "text/plain; charset=utf-8", "not found"),
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    Ok(())
}

struct TerminalRestore;
impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), Show, LeaveAlternateScreen);
    }
}

fn terminal_gallery() -> anyhow::Result<()> {
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen, Hide)?;
    let _restore = TerminalRestore;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let (mut story, mut variant) = (0usize, 0usize);
    loop {
        let size = terminal.size()?;
        let (id, title, _, _) = STORIES[story];
        let visual = TuiVisualVariant::ALL[variant];
        execute!(
            std::io::stdout(),
            crossterm::terminal::SetTitle(format!(
                "Golem TUI preview — {title} — {}",
                visual.name()
            ))
        )?;
        let buffer = render_preview_buffer(id, visual, size.width, size.height)?;
        terminal.draw(|frame| *frame.buffer_mut() = buffer)?;
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Up => story = story.checked_sub(1).unwrap_or(STORIES.len() - 1),
                KeyCode::Down => story = (story + 1) % STORIES.len(),
                KeyCode::Left => {
                    variant = variant
                        .checked_sub(1)
                        .unwrap_or(TuiVisualVariant::ALL.len() - 1)
                }
                KeyCode::Right => variant = (variant + 1) % TuiVisualVariant::ALL.len(),
                _ => {}
            }
        }
    }
    Ok(())
}

fn buffer_html(buffer: &Buffer) -> String {
    let mut out = String::from("<pre>");
    let area = buffer.area;
    for y in area.y..area.y + area.height {
        let mut run_style: Option<Style> = None;
        for x in area.x..area.x + area.width {
            let Some(cell) = buffer.cell((x, y)) else {
                continue;
            };
            let style = cell.style();
            if run_style != Some(style) {
                if run_style.is_some() {
                    out.push_str("</span>");
                }
                write!(out, "<span style=\"{}\">", css_style(style)).unwrap();
                run_style = Some(style);
            }
            out.push_str(&escape_html(cell.symbol()));
        }
        if run_style.is_some() {
            out.push_str("</span>");
        }
        out.push('\n');
    }
    out.push_str("</pre>");
    out
}

fn css_style(style: Style) -> String {
    let reversed = style.add_modifier.contains(Modifier::REVERSED);
    let (fg, bg) = if reversed {
        (style.bg, style.fg)
    } else {
        (style.fg, style.bg)
    };
    let mut css = String::new();
    if let Some(color) = fg {
        write!(css, "color:{};", css_color(color)).unwrap();
    }
    if let Some(color) = bg {
        write!(css, "background-color:{};", css_color(color)).unwrap();
    }
    if style.add_modifier.contains(Modifier::BOLD) {
        css.push_str("font-weight:bold;");
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        css.push_str("font-style:italic;");
    }
    if style
        .add_modifier
        .intersects(Modifier::UNDERLINED | Modifier::CROSSED_OUT)
    {
        css.push_str("text-decoration:");
        if style.add_modifier.contains(Modifier::UNDERLINED) {
            css.push_str("underline ");
        }
        if style.add_modifier.contains(Modifier::CROSSED_OUT) {
            css.push_str("line-through");
        }
        css.push(';');
    }
    if style.add_modifier.contains(Modifier::DIM) {
        css.push_str("opacity:.65;");
    }
    if style.add_modifier.contains(Modifier::HIDDEN) {
        css.push_str("visibility:hidden;");
    }
    css
}

fn css_color(color: Color) -> String {
    match color {
        Color::Reset => "inherit".into(),
        Color::Black => "#000000".into(),
        Color::Red => "#800000".into(),
        Color::Green => "#008000".into(),
        Color::Yellow => "#808000".into(),
        Color::Blue => "#000080".into(),
        Color::Magenta => "#800080".into(),
        Color::Cyan => "#008080".into(),
        Color::Gray => "#c0c0c0".into(),
        Color::DarkGray => "#808080".into(),
        Color::LightRed => "#ff0000".into(),
        Color::LightGreen => "#00ff00".into(),
        Color::LightYellow => "#ffff00".into(),
        Color::LightBlue => "#0000ff".into(),
        Color::LightMagenta => "#ff00ff".into(),
        Color::LightCyan => "#00ffff".into(),
        Color::White => "#ffffff".into(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(i) => indexed_color(i),
    }
}
fn indexed_color(i: u8) -> String {
    if i < 16 {
        let colors = [
            "#000000", "#800000", "#008000", "#808000", "#000080", "#800080", "#008080", "#c0c0c0",
            "#808080", "#ff0000", "#00ff00", "#ffff00", "#0000ff", "#ff00ff", "#00ffff", "#ffffff",
        ];
        colors[i as usize].into()
    } else if i < 232 {
        let i = i - 16;
        let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        format!(
            "#{:02x}{:02x}{:02x}",
            level(i / 36),
            level((i / 6) % 6),
            level(i % 6)
        )
    } else {
        let v = 8 + (i - 232) * 10;
        format!("#{v:02x}{v:02x}{v:02x}")
    }
}
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;
    #[test]
    fn escapes_html() {
        assert_eq!(escape_html("<&\"'>"), "&lt;&amp;&quot;&#39;&gt;");
    }
    #[test]
    fn converts_color_forms() {
        assert_eq!(css_color(Color::Rgb(1, 2, 3)), "#010203");
        assert_eq!(css_color(Color::Indexed(196)), "#ff0000");
        assert_eq!(css_color(Color::LightCyan), "#00ffff");
    }
    #[test]
    fn preserves_modifiers_and_reverse() {
        let css = css_style(
            Style::default()
                .fg(Color::Red)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        );
        assert!(
            css.contains("color:#000080")
                && css.contains("background-color:#800000")
                && css.contains("font-weight:bold")
        );
    }
    #[test]
    fn groups_runs_and_preserves_wide_symbols() {
        let mut b = Buffer::empty(ratatui::layout::Rect::new(0, 0, 3, 1));
        b[(0, 0)].set_symbol("界").set_fg(Color::Red);
        b[(1, 0)].set_symbol("").set_fg(Color::Red);
        b[(2, 0)].set_symbol("<").set_fg(Color::Blue);
        let html = buffer_html(&b);
        assert_eq!(html.matches("<span").count(), 2);
        assert!(html.contains("界") && html.contains("&lt;"));
    }
    #[test]
    fn renders_every_story_and_variant() {
        for (story, _, width, height) in STORIES {
            for variant in TuiVisualVariant::ALL {
                render_preview_buffer(story, variant, *width, *height).unwrap();
            }
        }
    }

    #[test]
    fn production_preview_matches_normal_rendering() {
        let normal = render_production_preview_buffer("home-idle", 100, 24).unwrap();
        let preview =
            render_preview_buffer("home-idle", TuiVisualVariant::Production, 100, 24).unwrap();
        assert_eq!(normal, preview);
    }
}
