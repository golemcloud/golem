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

#[derive(Clone, Copy)]
struct PreviewCase {
    id: &'static str,
    story: &'static str,
    title: &'static str,
    width: u16,
    height: u16,
}

const CASES: &[PreviewCase] = &[
    PreviewCase::new("home-idle", "home-idle", "Home / idle", 100, 24),
    PreviewCase::new("home-active", "home-active", "Home / active work", 100, 24),
    PreviewCase::new("dev-running", "dev-running", "Dev / running", 120, 30),
    PreviewCase::new("dev-completed", "dev-completed", "Dev / completed", 120, 30),
    PreviewCase::new("dev-failed", "dev-failed", "Dev / failed", 120, 30),
    PreviewCase::new(
        "dev-server-drawer",
        "dev-server-drawer",
        "Dev / server drawer",
        140,
        32,
    ),
    PreviewCase::new(
        "dev-layout-left",
        "dev-layout-left",
        "Dev / layout left",
        120,
        30,
    ),
    PreviewCase::new(
        "dev-layout-top",
        "dev-layout-top",
        "Dev / layout top",
        120,
        30,
    ),
    PreviewCase::new(
        "dev-layout-bottom",
        "dev-layout-bottom",
        "Dev / layout bottom",
        120,
        30,
    ),
    PreviewCase::new("ops-list", "ops-list", "Ops / list", 100, 24),
    PreviewCase::new("ops-details", "ops-details", "Ops / details", 120, 28),
    PreviewCase::new("ops-loading", "ops-loading", "Ops / loading", 100, 24),
    PreviewCase::new("ops-error", "ops-error", "Ops / error", 100, 24),
    PreviewCase::new("agent-inspect", "agent-inspect", "Agent / inspect", 120, 28),
    PreviewCase::new("palette", "palette", "Overlay / palette", 100, 24),
    PreviewCase::new("help", "help", "Overlay / help", 100, 28),
    PreviewCase::new(
        "context-picker",
        "context-picker",
        "Overlay / context picker",
        100,
        24,
    ),
    PreviewCase::new("loading", "loading", "Overlay / loading", 100, 24),
    PreviewCase::new(
        "confirmation",
        "confirmation",
        "Overlay / confirmation",
        100,
        24,
    ),
    PreviewCase::new("home-compact", "home-idle", "Home / compact", 72, 20),
    PreviewCase::new("ops-wide", "ops-details", "Ops / wide", 160, 32),
];

impl PreviewCase {
    const fn new(
        id: &'static str,
        story: &'static str,
        title: &'static str,
        width: u16,
        height: u16,
    ) -> Self {
        Self {
            id,
            story,
            title,
            width,
            height,
        }
    }
}

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
    for case in CASES {
        for variant in TuiVisualVariant::ALL {
            let buffer = render_preview_buffer(case.story, variant, case.width, case.height)?;
            write!(
                cards,
                "<article data-case=\"{}\" data-variant=\"{}\"><h2>{} · {} · {}×{}</h2>{}</article>",
                escape_html(case.id),
                escape_html(variant.name()),
                escape_html(case.title),
                variant.name(),
                case.width,
                case.height,
                buffer_html(&buffer)
            )?;
        }
    }
    Ok(format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Golem TUI previews</title>
<link rel="preconnect" href="https://fonts.googleapis.com"><link rel="preconnect" href="https://fonts.gstatic.com" crossorigin><link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Fira+Code:wght@400;700&amp;family=JetBrains+Mono:wght@400;700&amp;family=Noto+Sans+Symbols+2&amp;display=swap">
<style>:root{{color-scheme:dark;--preview-font:'Fira Code','Noto Sans Symbols 2',monospace;--preview-size:14px}}body{{margin:0;background:#08080b;color:#ededf0;font:14px system-ui}}header{{position:sticky;top:0;z-index:2;padding:12px 20px;background:#111118;border-bottom:1px solid #333;display:flex;flex-wrap:wrap;gap:10px;align-items:center}}select,button{{font:inherit}}button{{margin-left:auto}}#font-sample{{font-family:var(--preview-font);font-size:var(--preview-size);font-variant-ligatures:none;color:#a8a8b4}}#font-status,#copy-status{{color:#a8a8b4}}#copy-status{{min-width:4em}}main{{padding:18px;display:grid;gap:18px;overflow:auto}}article{{overflow:auto}}h2{{font-size:13px;font-weight:600;color:#aaa;margin:0 0 6px}}pre{{display:inline-block;margin:0;padding:12px;line-height:1;font-family:var(--preview-font);font-size:var(--preview-size);font-variant-ligatures:none;font-feature-settings:'liga' 0,'calt' 0;letter-spacing:0;background:#0a0a0d}}body[data-mode=compare] main{{grid-template-columns:repeat(4,max-content)}}body[data-mode=review] main,body[data-mode=coverage] main{{grid-template-columns:max-content}}article[hidden]{{display:none}}</style></head>
<body data-mode="review"><header><label>View <select id="mode"><option value="review">Review Production</option><option value="compare">Compare variants</option><option value="coverage">Variant coverage</option></select></label><label>Story <select id="case"></select></label><label>Variant <select id="variant"></select></label><label>Font <select id="font"><option value="fira">Fira Code</option><option value="jetbrains">JetBrains Mono</option><option value="system">System monospace</option></select></label><label>Size <select id="size"><option>12</option><option>13</option><option selected>14</option><option>15</option><option>16</option><option>18</option></select></label><span id="font-sample" title="Font rendering sample">Aa0│┤⠿</span><span id="font-status" aria-live="polite"></span><button id="copy" type="button">Copy link</button><span id="copy-status" aria-live="polite"></span></header><main>{cards}</main>
<script>const cards=[...document.querySelectorAll('article')],caseSelect=document.querySelector('#case'),variant=document.querySelector('#variant'),mode=document.querySelector('#mode'),font=document.querySelector('#font'),size=document.querySelector('#size'),fontStatus=document.querySelector('#font-status'),copy=document.querySelector('#copy'),copyStatus=document.querySelector('#copy-status'),params=new URLSearchParams(location.search),fontNames={{fira:'Fira Code',jetbrains:'JetBrains Mono'}},fontFamilies={{fira:"'Fira Code','Noto Sans Symbols 2',monospace",jetbrains:"'JetBrains Mono','Noto Sans Symbols 2',monospace",system:"monospace,'Noto Sans Symbols 2'"}};
for(const v of [...new Set(cards.map(x=>x.dataset.case))]){{const card=cards.find(x=>x.dataset.case===v);caseSelect.add(new Option(card.querySelector('h2').textContent.replace(/ · [^·]+ · \d+×\d+$/,''),v))}}for(const v of [...new Set(cards.map(x=>x.dataset.variant))])variant.add(new Option(v,v));
const modes=new Set([...mode.options].map(x=>x.value)),cases=new Set([...caseSelect.options].map(x=>x.value)),variants=new Set([...variant.options].map(x=>x.value)),sizes=new Set([...size.options].map(x=>x.value));mode.value=modes.has(params.get('mode'))?params.get('mode'):'review';caseSelect.value=cases.has(params.get('case'))?params.get('case'):caseSelect.options[0].value;variant.value=variants.has(params.get('variant'))?params.get('variant'):'Production';font.value=fontFamilies[params.get('font')]?params.get('font'):'fira';size.value=sizes.has(params.get('size'))?params.get('size'):'14';
async function reportFont(){{const name=fontNames[font.value];if(!name){{fontStatus.textContent='local font';return}}fontStatus.textContent='loading…';try{{await document.fonts.load(`${{size.value}}px "${{name}}"`,'Aa0│┤');fontStatus.textContent=document.fonts.check(`${{size.value}}px "${{name}}"`)?'webfont loaded':'font unavailable'}}catch{{fontStatus.textContent='font unavailable'}}}}
function show(){{document.body.dataset.mode=mode.value;if(mode.value==='review')variant.value='Production';const selectedVariant=variant.value;variant.disabled=mode.value==='review';caseSelect.disabled=mode.value==='coverage';document.documentElement.style.setProperty('--preview-font',fontFamilies[font.value]);document.documentElement.style.setProperty('--preview-size',`${{size.value}}px`);reportFont();cards.forEach(x=>x.hidden=mode.value==='compare'?x.dataset.case!==caseSelect.value:mode.value==='coverage'?x.dataset.variant!==selectedVariant:x.dataset.case!==caseSelect.value||x.dataset.variant!=='Production');const next=new URLSearchParams();next.set('mode',mode.value);next.set('case',caseSelect.value);next.set('variant',selectedVariant);next.set('font',font.value);next.set('size',size.value);history.replaceState(null,'',`${{location.pathname}}?${{next}}${{location.hash}}`)}}
mode.onchange=caseSelect.onchange=variant.onchange=font.onchange=size.onchange=show;copy.onclick=async()=>{{try{{await navigator.clipboard.writeText(location.href);copyStatus.textContent='Copied'}}catch{{copyStatus.textContent='Copy failed'}}setTimeout(()=>copyStatus.textContent='',1500)}};show();let revision;
setInterval(async()=>{{try{{const next=await fetch('/revision',{{cache:'no-store'}}).then(x=>x.text());if(revision&&revision!==next)location.reload();revision=next}}catch{{}}}},2000);</script></body></html>"#
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
        let case = CASES[story];
        let visual = TuiVisualVariant::ALL[variant];
        execute!(
            std::io::stdout(),
            crossterm::terminal::SetTitle(format!(
                "Golem TUI preview — {} — {}",
                case.title,
                visual.name()
            ))
        )?;
        let buffer = render_preview_buffer(case.story, visual, size.width, size.height)?;
        terminal.draw(|frame| *frame.buffer_mut() = buffer)?;
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Up => story = story.checked_sub(1).unwrap_or(CASES.len() - 1),
                KeyCode::Down => story = (story + 1) % CASES.len(),
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
        for case in CASES {
            for variant in TuiVisualVariant::ALL {
                render_preview_buffer(case.story, variant, case.width, case.height).unwrap();
            }
        }
    }

    #[test]
    fn preview_case_ids_are_unique() {
        let ids = CASES
            .iter()
            .map(|case| case.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), CASES.len());
        assert!(
            CASES
                .iter()
                .any(|case| case.id == "home-compact" && case.story == "home-idle")
        );
        assert!(
            CASES
                .iter()
                .any(|case| case.id == "ops-wide" && case.story == "ops-details")
        );
    }

    #[test]
    fn gallery_defaults_to_linkable_production_review() {
        let html = gallery_html().unwrap();
        assert!(html.contains("data-mode=\"review\""));
        assert!(html.contains("params.get('case')"));
        assert!(html.contains("params.get('font')"));
        assert!(html.contains("fonts.googleapis.com/css2"));
        assert!(html.contains("family=Fira+Code:wght@400;700"));
        assert!(html.contains("family=JetBrains+Mono:wght@400;700"));
        assert!(html.contains("family=Noto+Sans+Symbols+2"));
        assert!(html.contains("'Noto Sans Symbols 2',monospace"));
        assert!(html.contains("webfont loaded"));
        assert!(html.contains("font-variant-ligatures:none"));
        assert!(html.contains("}},2000)"));
        assert!(html.contains("history.replaceState"));
        assert!(html.contains("navigator.clipboard.writeText(location.href)"));
    }

    #[test]
    fn production_preview_matches_normal_rendering() {
        let normal = render_production_preview_buffer("home-idle", 100, 24).unwrap();
        let preview =
            render_preview_buffer("home-idle", TuiVisualVariant::Production, 100, 24).unwrap();
        assert_eq!(normal, preview);
    }
}
