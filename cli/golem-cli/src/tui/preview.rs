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

use super::app::render_preview_buffer;
#[cfg(test)]
use super::app::render_production_preview_buffer;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy)]
struct PreviewCase {
    id: &'static str,
    title: &'static str,
    width: u16,
    height: u16,
}

impl PreviewCase {
    const fn new(id: &'static str, title: &'static str, width: u16, height: u16) -> Self {
        Self {
            id,
            title,
            width,
            height,
        }
    }
}

const CASES: &[PreviewCase] = &[
    PreviewCase::new("ops-overview", "Ops / overview", 120, 30),
    PreviewCase::new("ops-agents", "Ops / agents overview", 120, 30),
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
    for case in CASES {
        let buffer = render_preview_buffer(case.id, case.width, case.height)?;
        write!(
            cards,
            "<article data-case=\"{}\"><h2>{} · {}×{}</h2>{}</article>",
            escape_html(case.id),
            escape_html(case.title),
            case.width,
            case.height,
            buffer_html(&buffer)
        )?;
    }
    Ok(format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Golem TUI preview</title>
<link rel="preconnect" href="https://cdn.jsdelivr.net" crossorigin>
<style>@font-face{{font-family:'Golem Preview Fira';font-style:normal;font-weight:400;font-display:block;src:url('https://cdn.jsdelivr.net/npm/firacode@6.2.0/distr/woff2/FiraCode-Regular.woff2') format('woff2')}}@font-face{{font-family:'Golem Preview Fira';font-style:normal;font-weight:700;font-display:block;src:url('https://cdn.jsdelivr.net/npm/firacode@6.2.0/distr/woff2/FiraCode-Bold.woff2') format('woff2')}}@font-face{{font-family:'Golem Preview Iosevka Term';font-style:normal;font-weight:400;font-display:block;src:url('https://cdn.jsdelivr.net/gh/iosevka-webfonts/iosevka-term@73e346bc453d1423498e5b7ef71d9275eb3dbb1d/woff2/iosevka-term-regular.woff2') format('woff2')}}@font-face{{font-family:'Golem Preview Iosevka Term';font-style:normal;font-weight:700;font-display:block;src:url('https://cdn.jsdelivr.net/gh/iosevka-webfonts/iosevka-term@73e346bc453d1423498e5b7ef71d9275eb3dbb1d/woff2/iosevka-term-bold.woff2') format('woff2')}}:root{{color-scheme:dark;--preview-font:'Golem Preview Fira';--preview-size:14px}}body{{margin:0;background:#08080b;color:#ededf0;font:14px system-ui}}header{{position:sticky;top:0;z-index:2;padding:12px 20px;background:#111118;border-bottom:1px solid #333;display:flex;flex-wrap:wrap;gap:10px;align-items:center}}header strong{{margin-right:8px}}select,button{{font:inherit}}button{{margin-left:auto}}#font-sample{{font-family:var(--preview-font);font-size:var(--preview-size);line-height:1;font-variant-ligatures:none;color:#a8a8b4}}#font-status,#copy-status{{color:#a8a8b4}}#font-error{{flex-basis:100%;padding:8px 10px;background:#3a171b;color:#ffb4b9}}#copy-status{{min-width:4em}}main{{padding:18px;display:grid;grid-template-columns:max-content;gap:18px;overflow:auto}}article{{overflow:auto}}article[hidden]{{display:none}}h2{{font-size:13px;font-weight:600;color:#aaa;margin:0 0 6px}}.terminal-grid{{position:relative;display:inline-block;padding:12px;background:#0a0a0d;font-family:var(--preview-font);font-size:var(--preview-size);line-height:1;font-variant-ligatures:none;font-feature-settings:'liga' 0,'calt' 0;letter-spacing:0}}.terminal-layer{{display:block;margin:0;padding:0;font:inherit;line-height:inherit;letter-spacing:inherit;white-space:pre}}.terminal-background{{color:transparent}}.terminal-foreground{{position:absolute;inset:12px;background:transparent}}</style></head>
<body><header><strong>Production preview</strong><label>Case <select id="case"></select></label><label>Font <select id="font"><option value="fira">Fira Code</option><option value="iosevka">Iosevka Term</option></select></label><label>Size <select id="size"><option>12</option><option>13</option><option selected>14</option><option>15</option><option>16</option><option>18</option></select></label><span id="font-sample" title="Font rendering sample">Agpqy │┃┤ ○● ✓×·—…</span><span id="font-status" aria-live="polite"></span><button id="copy" type="button">Copy link</button><span id="copy-status" aria-live="polite"></span><div id="font-error" role="alert" hidden></div></header><main hidden>{cards}</main>
<script>const cards=[...document.querySelectorAll('article')],main=document.querySelector('main'),caseSelect=document.querySelector('#case'),font=document.querySelector('#font'),size=document.querySelector('#size'),fontStatus=document.querySelector('#font-status'),fontError=document.querySelector('#font-error'),copy=document.querySelector('#copy'),copyStatus=document.querySelector('#copy-status'),params=new URLSearchParams(location.search),fontConfigs={{fira:{{family:'Golem Preview Fira',label:'Fira Code'}},iosevka:{{family:'Golem Preview Iosevka Term',label:'Iosevka Term'}}}},metricGlyphs=[...'Agpqy│┃┤○●✓×·—…'];let showGeneration=0;
for(const value of [...new Set(cards.map(card=>card.dataset.case))]){{const card=cards.find(card=>card.dataset.case===value);caseSelect.add(new Option(card.querySelector('h2').textContent.replace(/ · \d+×\d+$/,''),value))}}
const cases=new Set([...caseSelect.options].map(option=>option.value)),sizes=new Set([...size.options].map(option=>option.value));caseSelect.value=cases.has(params.get('case'))?params.get('case'):caseSelect.options[0].value;font.value=fontConfigs[params.get('font')]?params.get('font'):'fira';size.value=sizes.has(params.get('size'))?params.get('size'):'14';
function glyphTouchesCellEdges(ctx,glyph,cellWidth,cellHeight,family){{const canvas=ctx.canvas;canvas.width=Math.ceil(cellWidth)+4;canvas.height=cellHeight;ctx.font=`400 ${{cellHeight}}px "${{family}}"`;ctx.textBaseline='top';ctx.fillStyle='#fff';ctx.fillText(glyph,2,0);const pixels=ctx.getImageData(0,0,canvas.width,canvas.height).data,rowHasInk=row=>{{for(let x=0;x<canvas.width;x++)if(pixels[(row*canvas.width+x)*4+3])return true;return false}};return (rowHasInk(0)||rowHasInk(1))&&(rowHasInk(canvas.height-1)||rowHasInk(canvas.height-2))}}
async function qualifyFont(key){{const config=fontConfigs[key],px=Number(size.value),sample=metricGlyphs.join('');await Promise.all([document.fonts.load(`400 ${{px}}px "${{config.family}}"`,sample),document.fonts.load(`700 ${{px}}px "${{config.family}}"`,sample)]);if(!document.fonts.check(`400 ${{px}}px "${{config.family}}"`,sample)||!document.fonts.check(`700 ${{px}}px "${{config.family}}"`,sample))throw new Error('font files did not load');const canvas=document.createElement('canvas'),ctx=canvas.getContext('2d');ctx.font=`400 ${{px}}px "${{config.family}}"`;const width=ctx.measureText('M').width,tolerance=Math.max(.05,width*.015);if(metricGlyphs.some(glyph=>Math.abs(ctx.measureText(glyph).width-width)>tolerance))throw new Error('regular glyphs do not share one cell width');ctx.font=`700 ${{px}}px "${{config.family}}"`;if(metricGlyphs.some(glyph=>Math.abs(ctx.measureText(glyph).width-width)>tolerance))throw new Error('bold glyphs do not match the regular cell width');if(!glyphTouchesCellEdges(ctx,'┃',width,px,config.family)||!glyphTouchesCellEdges(ctx,'│',width,px,config.family))throw new Error('vertical decorations do not join cell edges')}}
async function show(){{const generation=++showGeneration,config=fontConfigs[font.value];document.documentElement.style.setProperty('--preview-font',`'${{config.family}}'`);document.documentElement.style.setProperty('--preview-size',`${{size.value}}px`);main.hidden=true;fontError.hidden=true;fontStatus.textContent='checking font…';const results=await Promise.all(Object.keys(fontConfigs).map(async key=>{{try{{await qualifyFont(key);return [key,true]}}catch(error){{return [key,false,error]}}}}));if(generation!==showGeneration)return;for(const [key,ok,error] of results){{const option=[...font.options].find(item=>item.value===key);option.disabled=!ok&&key!==font.value;option.title=ok?'terminal grid verified':error.message}}const selected=results.find(([key])=>key===font.value);if(!selected[1]){{fontStatus.textContent='font unavailable';fontError.textContent=`${{config.label}} cannot render this terminal grid: ${{selected[2].message}}. Preview blocked to prevent fallback-font review.`;fontError.hidden=false;return}}const rejected=results.filter(([,ok])=>!ok).map(([key,,error])=>`${{fontConfigs[key].label}} rejected: ${{error.message}}`);fontStatus.textContent=['terminal grid verified',...rejected].join(' · ');main.hidden=false;cards.forEach(card=>card.hidden=card.dataset.case!==caseSelect.value);const next=new URLSearchParams();next.set('case',caseSelect.value);next.set('font',font.value);next.set('size',size.value);history.replaceState(null,'',`${{location.pathname}}?${{next}}${{location.hash}}`)}}
caseSelect.onchange=font.onchange=size.onchange=()=>void show();copy.onclick=async()=>{{try{{await navigator.clipboard.writeText(location.href);copyStatus.textContent='Copied'}}catch{{copyStatus.textContent='Copy failed'}}setTimeout(()=>copyStatus.textContent='',1500)}};void show();let revision;
setInterval(async()=>{{try{{const next=await fetch('/revision',{{cache:'no-store'}}).then(response=>response.text());if(revision&&revision!==next)location.reload();revision=next}}catch{{}}}},2000);</script></body></html>"#
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
        .and_then(|request| request.split_whitespace().nth(1))
        .unwrap_or("/");
    let (status, kind, body) = response_for_path(path, html, revision);
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    Ok(())
}

fn response_for_path<'a>(
    path: &str,
    html: &'a str,
    revision: &'a str,
) -> (&'static str, &'static str, &'a str) {
    match path.split('?').next().unwrap_or(path) {
        "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", html),
        "/revision" => ("200 OK", "text/plain; charset=utf-8", revision),
        _ => ("404 Not Found", "text/plain; charset=utf-8", "not found"),
    }
}

struct TerminalRestore;

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), Show, LeaveAlternateScreen);
    }
}

fn terminal_gallery() -> anyhow::Result<()> {
    let terminate = terminal_termination_flag()?;
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen, Hide)?;
    let _restore = TerminalRestore;
    let mut terminal = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
    let mut case_index = 0usize;
    let mut redraw = true;
    loop {
        if terminate.load(Ordering::Relaxed) {
            break;
        }
        if redraw {
            let size = terminal.size()?;
            let case = CASES[case_index];
            execute!(
                std::io::stdout(),
                crossterm::terminal::SetTitle(format!("Golem TUI preview: {}", case.title))
            )?;
            let buffer = render_preview_buffer(case.id, size.width, size.height)?;
            terminal.draw(|frame| *frame.buffer_mut() = buffer)?;
            redraw = false;
        }
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let terminal_event = event::read()?;
        apply_terminal_resize(&mut terminal, &terminal_event)?;
        match terminal_event {
            Event::Resize(_, _) => {}
            Event::Key(key) => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Up => case_index = case_index.checked_sub(1).unwrap_or(CASES.len() - 1),
                KeyCode::Down => case_index = (case_index + 1) % CASES.len(),
                _ => {}
            },
            _ => {}
        }
        redraw = true;
    }
    Ok(())
}

fn apply_terminal_resize<B: Backend>(
    terminal: &mut Terminal<B>,
    event: &Event,
) -> Result<bool, B::Error> {
    let Event::Resize(width, height) = *event else {
        return Ok(false);
    };
    terminal.resize(Rect::new(0, 0, width, height))?;
    Ok(true)
}

fn terminal_termination_flag() -> anyhow::Result<Arc<AtomicBool>> {
    let flag = Arc::new(AtomicBool::new(false));
    let signal_flag = flag.clone();
    std::thread::Builder::new()
        .name("tui-preview-signal".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let Ok(runtime) = runtime else {
                return;
            };
            runtime.block_on(async move {
                #[cfg(unix)]
                {
                    use tokio::signal::unix::{SignalKind, signal};
                    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
                        return;
                    };
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {}
                        _ = terminate.recv() => {}
                    }
                }
                #[cfg(not(unix))]
                let _ = tokio::signal::ctrl_c().await;
                signal_flag.store(true, Ordering::Relaxed);
            });
        })?;
    Ok(flag)
}

fn buffer_html(buffer: &Buffer) -> String {
    format!(
        "<div class=\"terminal-grid\">{}{}</div>",
        buffer_layer_html(buffer, HtmlLayer::Background),
        buffer_layer_html(buffer, HtmlLayer::Foreground)
    )
}

#[derive(Clone, Copy)]
enum HtmlLayer {
    Background,
    Foreground,
}

fn buffer_layer_html(buffer: &Buffer, layer: HtmlLayer) -> String {
    let (class, hidden) = match layer {
        HtmlLayer::Background => ("terminal-background", " aria-hidden=\"true\""),
        HtmlLayer::Foreground => ("terminal-foreground", ""),
    };
    let mut output = format!("<pre class=\"terminal-layer {class}\"{hidden}>");
    let area = buffer.area;
    for y in area.y..area.y + area.height {
        let mut run_style: Option<String> = None;
        for x in area.x..area.x + area.width {
            let Some(cell) = buffer.cell((x, y)) else {
                continue;
            };
            let style = match layer {
                HtmlLayer::Background => css_background_style(cell.style()),
                HtmlLayer::Foreground => css_foreground_style(cell.style()),
            };
            if run_style.as_ref() != Some(&style) {
                if run_style.is_some() {
                    output.push_str("</span>");
                }
                write!(output, "<span style=\"{style}\">").unwrap();
                run_style = Some(style);
            }
            match layer {
                HtmlLayer::Background => output.push(' '),
                HtmlLayer::Foreground => output.push_str(&escape_html(cell.symbol())),
            }
        }
        if run_style.is_some() {
            output.push_str("</span>");
        }
        output.push('\n');
    }
    output.push_str("</pre>");
    output
}

fn effective_colors(style: Style) -> (Option<Color>, Option<Color>) {
    if style.add_modifier.contains(Modifier::REVERSED) {
        (style.bg, style.fg)
    } else {
        (style.fg, style.bg)
    }
}

fn css_background_style(style: Style) -> String {
    let (_, background) = effective_colors(style);
    background
        .map(|color| format!("background-color:{};", css_color(color)))
        .unwrap_or_default()
}

fn css_foreground_style(style: Style) -> String {
    let (foreground, _) = effective_colors(style);
    let mut css = String::new();
    if let Some(color) = foreground {
        write!(css, "color:{};", css_color(color)).unwrap();
    }
    css.push_str(&css_modifiers(style));
    css
}

#[cfg(test)]
fn css_style(style: Style) -> String {
    let (foreground, background) = effective_colors(style);
    let mut css = String::new();
    if let Some(color) = foreground {
        write!(css, "color:{};", css_color(color)).unwrap();
    }
    if let Some(color) = background {
        write!(css, "background-color:{};", css_color(color)).unwrap();
    }
    css.push_str(&css_modifiers(style));
    css
}

fn css_modifiers(style: Style) -> String {
    let mut css = String::new();
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
        Color::Rgb(red, green, blue) => format!("#{red:02x}{green:02x}{blue:02x}"),
        Color::Indexed(index) => indexed_color(index),
    }
}

fn indexed_color(index: u8) -> String {
    if index < 16 {
        let colors = [
            "#000000", "#800000", "#008000", "#808000", "#000080", "#800080", "#008080", "#c0c0c0",
            "#808080", "#ff0000", "#00ff00", "#ffff00", "#0000ff", "#ff00ff", "#00ffff", "#ffffff",
        ];
        colors[index as usize].into()
    } else if index < 232 {
        let index = index - 16;
        let level = |value: u8| if value == 0 { 0 } else { 55 + value * 40 };
        format!(
            "#{:02x}{:02x}{:02x}",
            level(index / 36),
            level((index / 6) % 6),
            level(index % 6)
        )
    } else {
        let value = 8 + (index - 232) * 10;
        format!("#{value:02x}{value:02x}{value:02x}")
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
    use ratatui::backend::TestBackend;
    use std::collections::HashSet;
    use test_r::test;

    fn buffer_text(buffer: &Buffer) -> String {
        let area = buffer.area;
        let mut text = String::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buffer.cell((x, y)) {
                    text.push_str(cell.symbol());
                }
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn terminal_resize_event_propagates_dimensions_and_requests_redraw() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        assert!(apply_terminal_resize(&mut terminal, &Event::Resize(37, 11)).unwrap());
        assert_eq!(terminal.current_buffer_mut().area, Rect::new(0, 0, 37, 11));
        assert!(!apply_terminal_resize(&mut terminal, &Event::FocusGained).unwrap());
        assert_eq!(terminal.current_buffer_mut().area, Rect::new(0, 0, 37, 11));
    }

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
        assert!(css.contains("color:#000080"));
        assert!(css.contains("background-color:#800000"));
        assert!(css.contains("font-weight:bold"));
    }

    #[test]
    fn groups_runs_and_preserves_wide_symbols() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 3, 1));
        buffer[(0, 0)]
            .set_symbol("界")
            .set_fg(Color::Red)
            .set_bg(Color::Green);
        buffer[(1, 0)]
            .set_symbol("")
            .set_fg(Color::Red)
            .set_bg(Color::Green);
        buffer[(2, 0)]
            .set_symbol("<")
            .set_fg(Color::Blue)
            .set_bg(Color::Yellow);
        let html = buffer_html(&buffer);
        assert_eq!(html.matches("<span").count(), 4);
        assert!(html.contains("terminal-background"));
        assert!(html.contains("terminal-foreground"));
        assert!(html.contains("aria-hidden=\"true\""));
        assert!(html.contains("界"));
        assert!(html.contains("&lt;"));
        assert_eq!(html.matches("界").count(), 1);
        assert_eq!(html.matches("background-color:").count(), 2);
    }

    #[test]
    fn separates_reversed_foreground_and_background() {
        let style = Style::default()
            .fg(Color::Red)
            .bg(Color::Blue)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED);
        assert_eq!(css_background_style(style), "background-color:#800000;");
        assert_eq!(
            css_foreground_style(style),
            "color:#000080;font-weight:bold;"
        );
    }

    #[test]
    fn registered_cases_are_unique_and_render() {
        let mut ids = HashSet::new();
        for case in CASES {
            assert!(ids.insert(case.id));
            render_preview_buffer(case.id, case.width, case.height).unwrap();
        }
    }

    #[test]
    fn production_cases_render_at_representative_and_extreme_sizes() {
        for case_id in ["ops-overview", "ops-agents"] {
            for (width, height) in [(160, 40), (120, 30), (100, 8), (32, 24), (12, 4), (1, 1)] {
                let result =
                    std::panic::catch_unwind(|| render_preview_buffer(case_id, width, height));
                match result {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => {
                        panic!("{case_id} failed at {width}x{height}: {error}")
                    }
                    Err(_) => panic!("{case_id} panicked at {width}x{height}"),
                }
            }
        }
    }

    #[test]
    fn overview_fixture_contains_all_status_cards() {
        let text = buffer_text(&render_preview_buffer("ops-overview", 120, 30).unwrap());
        assert!(text.contains("Deployment"));
        assert!(text.contains("Agent signals · last 15 minutes"));
        assert!(text.contains("Local server & collector"));
        assert!(text.contains("Stream"));
        assert!(text.contains("Issues"));
        assert!(text.contains("12 Agent Types"));
    }

    #[test]
    fn production_fixture_contains_agent_overview_content() {
        let text = buffer_text(&render_preview_buffer("ops-agents", 120, 30).unwrap());
        assert!(text.contains("preview-app"));
        assert!(text.contains("local"));
        assert!(text.contains("Agents"));
        assert!(text.contains("checkout(cart-17)"));
        assert!(text.contains("orders(order-04)"));
    }

    #[test]
    fn rejects_unknown_cases() {
        assert!(render_preview_buffer("missing", 80, 24).is_err());
    }

    #[test]
    fn production_wrapper_matches_normal_rendering() {
        let normal = render_production_preview_buffer(100, 24, false).unwrap();
        let styled = render_production_preview_buffer(100, 24, true).unwrap();
        assert_eq!(normal, styled);
        assert_eq!(
            normal,
            render_preview_buffer("ops-agents", 100, 24).unwrap()
        );
    }

    #[test]
    fn gallery_exposes_only_production_case_controls() {
        let html = gallery_html().unwrap();
        assert!(html.contains("Production preview"));
        assert!(html.contains("data-case=\"ops-overview\""));
        assert!(html.contains("data-case=\"ops-agents\""));
        assert!(html.contains("id=\"case\""));
        assert!(html.contains("id=\"font\""));
        assert!(!html.contains("id=\"variant\""));
        assert!(!html.contains("Current focus"));
        assert!(!html.contains("fake OTLP"));
    }

    #[test]
    fn browser_preview_uses_grid_layers_and_font_checks() {
        let html = gallery_html().unwrap();
        assert!(html.contains("terminal-background"));
        assert!(html.contains("terminal-foreground"));
        assert!(html.contains("font-variant-ligatures:none"));
        assert!(html.contains("glyphTouchesCellEdges"));
        assert!(html.contains("Preview blocked to prevent fallback-font review."));
    }

    #[test]
    fn routes_ignore_queries_and_reject_unknown_paths() {
        assert_eq!(
            response_for_path("/?case=ops-agents", "page", "42"),
            ("200 OK", "text/html; charset=utf-8", "page")
        );
        assert_eq!(
            response_for_path("/revision?cache=0", "page", "42"),
            ("200 OK", "text/plain; charset=utf-8", "42")
        );
        assert_eq!(
            response_for_path("/missing", "page", "42"),
            ("404 Not Found", "text/plain; charset=utf-8", "not found")
        );
    }
}
