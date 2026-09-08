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

const CASES: &[PreviewCase] = &[
    PreviewCase::new("shell-default", "Shell / default", 100, 24),
    PreviewCase::new("shell-scrollbar", "Shell / scrollbar", 100, 24),
    PreviewCase::new("content-density", "Content / density and states", 100, 28),
    PreviewCase::new("content-split", "Content / split panes", 120, 28),
    PreviewCase::new("content-scrolling", "Content / scrolling", 100, 18),
    PreviewCase::new("content-long", "Content / long and narrow", 54, 24),
    PreviewCase::new("split-focus", "Panels / splits and focus", 120, 28),
    PreviewCase::new(
        "split-scrollbars",
        "Panels / splits with scrollbars",
        120,
        28,
    ),
    PreviewCase::new(
        "three-pane-scrollbars",
        "Panels / three scrollbar roles",
        120,
        28,
    ),
    PreviewCase::new("shortcut-leader", "Shortcuts / leader", 100, 24),
    PreviewCase::new("overlay-search", "Overlay / search", 100, 24),
    PreviewCase::new("overlay-decision", "Overlay / decision", 100, 24),
    PreviewCase::new("overlay-error", "Overlay / error", 100, 24),
    PreviewCase::new("overlay-nested", "Overlay / nested modal", 100, 24),
    PreviewCase::new("table-decoration", "Tables / decoration", 120, 32),
    PreviewCase::new("table-long", "Tables / long and panned", 100, 24),
    PreviewCase::new("table-details", "Tables / selected details", 120, 28),
    PreviewCase::new("table-columns", "Tables / column chooser", 100, 24),
];

#[derive(Clone, Copy)]
struct CurrentFocus {
    id: &'static str,
    title: &'static str,
    decision: &'static str,
    case_ids: &'static [&'static str],
    queued_next: &'static str,
    variants: &'static [TuiVisualVariant],
}

const CURRENT_FOCUS: CurrentFocus = CurrentFocus {
    id: "pane-data-tables",
    title: "Pane data tables",
    decision: "Compare main-pane table decoration and validate long columns, selected-row expansion, optional details, and column visibility.",
    case_ids: &[
        "table-decoration",
        "table-long",
        "table-details",
        "table-columns",
    ],
    queued_next: "Adaptive and minimal layouts",
    variants: &[TuiVisualVariant::FrameBase],
};

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
            let buffer = render_preview_buffer(case.id, variant, case.width, case.height)?;
            write!(
                cards,
                "<article data-case=\"{}\" data-variant=\"{}\" data-focus=\"{}\"><h2>{} · {} · {}×{}</h2>{}</article>",
                escape_html(case.id),
                escape_html(variant.name()),
                CURRENT_FOCUS.case_ids.contains(&case.id)
                    && CURRENT_FOCUS.variants.contains(&variant),
                escape_html(case.title),
                variant.name(),
                case.width,
                case.height,
                buffer_html(&buffer)
            )?;
        }
    }
    let focus_title = escape_html(CURRENT_FOCUS.title);
    let focus_decision = escape_html(CURRENT_FOCUS.decision);
    let focus_case = CURRENT_FOCUS
        .case_ids
        .iter()
        .map(|case_id| {
            CASES
                .iter()
                .find(|case| case.id == *case_id)
                .map(|case| escape_html(case.title))
                .ok_or_else(|| {
                    anyhow::anyhow!("current TUI preview focus references an unknown case")
                })
        })
        .collect::<anyhow::Result<Vec<_>>>()?
        .join(" + ");
    let queued_next = escape_html(CURRENT_FOCUS.queued_next);
    let focus_id = escape_html(CURRENT_FOCUS.id);
    Ok(format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Golem TUI design lab</title>
<link rel="preconnect" href="https://cdn.jsdelivr.net" crossorigin>
<style>@font-face{{font-family:'Golem Preview Fira';font-style:normal;font-weight:400;font-display:block;src:url('https://cdn.jsdelivr.net/npm/firacode@6.2.0/distr/woff2/FiraCode-Regular.woff2') format('woff2')}}@font-face{{font-family:'Golem Preview Fira';font-style:normal;font-weight:700;font-display:block;src:url('https://cdn.jsdelivr.net/npm/firacode@6.2.0/distr/woff2/FiraCode-Bold.woff2') format('woff2')}}@font-face{{font-family:'Golem Preview Iosevka Term';font-style:normal;font-weight:400;font-display:block;src:url('https://cdn.jsdelivr.net/gh/iosevka-webfonts/iosevka-term@73e346bc453d1423498e5b7ef71d9275eb3dbb1d/woff2/iosevka-term-regular.woff2') format('woff2')}}@font-face{{font-family:'Golem Preview Iosevka Term';font-style:normal;font-weight:700;font-display:block;src:url('https://cdn.jsdelivr.net/gh/iosevka-webfonts/iosevka-term@73e346bc453d1423498e5b7ef71d9275eb3dbb1d/woff2/iosevka-term-bold.woff2') format('woff2')}}:root{{color-scheme:dark;--preview-font:'Golem Preview Fira';--preview-size:14px}}body{{margin:0;background:#08080b;color:#ededf0;font:14px system-ui}}header{{position:sticky;top:0;z-index:2;padding:12px 20px;background:#111118;border-bottom:1px solid #333;display:flex;flex-wrap:wrap;gap:10px;align-items:center}}select,button{{font:inherit}}button{{margin-left:auto}}#font-sample{{font-family:var(--preview-font);font-size:var(--preview-size);line-height:1;font-variant-ligatures:none;color:#a8a8b4}}#font-status,#copy-status{{color:#a8a8b4}}#font-error{{flex-basis:100%;padding:8px 10px;background:#3a171b;color:#ffb4b9}}#copy-status{{min-width:4em}}main{{padding:18px;display:grid;gap:18px;overflow:auto}}article{{overflow:auto}}h2{{font-size:13px;font-weight:600;color:#aaa;margin:0 0 6px}}.terminal-grid{{position:relative;display:inline-block;padding:12px;background:#0a0a0d;font-family:var(--preview-font);font-size:var(--preview-size);line-height:1;font-variant-ligatures:none;font-feature-settings:'liga' 0,'calt' 0;letter-spacing:0}}.terminal-layer{{display:block;margin:0;padding:0;font:inherit;line-height:inherit;letter-spacing:inherit;white-space:pre}}.terminal-background{{color:transparent}}.terminal-foreground{{position:absolute;inset:12px;background:transparent}}body[data-mode=compare] main{{grid-template-columns:repeat(2,max-content)}}body[data-mode=review] main,body[data-mode=coverage] main{{grid-template-columns:max-content}}article[hidden]{{display:none}}</style></head>
<style>header{{align-items:flex-start}}.focus-summary{{display:grid;gap:3px;min-width:36em}}.focus-summary strong{{font-size:15px}}.focus-summary span{{color:#a8a8b4}}details{{border-left:1px solid #444;padding-left:10px}}details>div{{display:flex;flex-wrap:wrap;gap:10px;align-items:center;margin-top:8px}}body[data-mode=focus] main{{grid-template-columns:repeat(2,max-content)}}body[data-mode=production] main,body[data-mode=coverage] main{{grid-template-columns:max-content}}</style>
<body data-mode="focus" data-focus-id="{focus_id}"><header><div class="focus-summary"><strong>Current focus: {focus_title}</strong><span>{focus_decision}</span><span>Case: {focus_case} · Queued next: {queued_next}</span></div><button id="copy" type="button">Copy link</button><span id="copy-status" aria-live="polite"></span><details id="tools"><summary>Tools</summary><div><label>View <select id="mode"><option value="focus">Current focus</option><option value="production">Production</option><option value="coverage">Variant coverage</option></select></label><label>Case <select id="case"></select></label><label>Variant <select id="variant"></select></label><label>Font <select id="font"><option value="fira">Fira Code</option><option value="iosevka">Iosevka Term</option></select></label><label>Size <select id="size"><option>12</option><option>13</option><option selected>14</option><option>15</option><option>16</option><option>18</option></select></label><span id="font-sample" title="Font rendering sample">Agpqy │┃┤ ○● ✓×·—…</span><span id="font-status" aria-live="polite"></span></div></details><div id="font-error" role="alert" hidden></div></header><main hidden>{cards}</main>
<script>const cards=[...document.querySelectorAll('article')],main=document.querySelector('main'),caseSelect=document.querySelector('#case'),variant=document.querySelector('#variant'),tools=document.querySelector('#tools'),mode=document.querySelector('#mode'),font=document.querySelector('#font'),size=document.querySelector('#size'),fontStatus=document.querySelector('#font-status'),fontError=document.querySelector('#font-error'),copy=document.querySelector('#copy'),copyStatus=document.querySelector('#copy-status'),params=new URLSearchParams(location.search),fontConfigs={{fira:{{family:'Golem Preview Fira',label:'Fira Code'}},iosevka:{{family:'Golem Preview Iosevka Term',label:'Iosevka Term'}}}},metricGlyphs=[...'Agpqy│┃┤○●✓×·—…'];let showGeneration=0;
for(const v of [...new Set(cards.map(x=>x.dataset.case))]){{const card=cards.find(x=>x.dataset.case===v);caseSelect.add(new Option(card.querySelector('h2').textContent.replace(/ · [^·]+ · \d+×\d+$/,''),v))}}for(const v of [...new Set(cards.map(x=>x.dataset.variant))])variant.add(new Option(v,v));
const modes=new Set([...mode.options].map(x=>x.value)),cases=new Set([...caseSelect.options].map(x=>x.value)),variants=new Set([...variant.options].map(x=>x.value)),sizes=new Set([...size.options].map(x=>x.value)),requestedMode=params.get('mode')==='compare'?'focus':params.get('mode')==='review'?'production':params.get('mode');mode.value=modes.has(requestedMode)?requestedMode:'focus';caseSelect.value=cases.has(params.get('case'))?params.get('case'):caseSelect.options[0].value;variant.value=variants.has(params.get('variant'))?params.get('variant'):'Production';font.value=fontConfigs[params.get('font')]?params.get('font'):'fira';size.value=sizes.has(params.get('size'))?params.get('size'):'14';if(mode.value!=='focus')tools.open=true;
function glyphTouchesCellEdges(ctx,glyph,cellWidth,cellHeight,family){{const canvas=ctx.canvas;canvas.width=Math.ceil(cellWidth)+4;canvas.height=cellHeight;ctx.font=`400 ${{cellHeight}}px "${{family}}"`;ctx.textBaseline='top';ctx.fillStyle='#fff';ctx.fillText(glyph,2,0);const pixels=ctx.getImageData(0,0,canvas.width,canvas.height).data,rowHasInk=row=>{{for(let x=0;x<canvas.width;x++)if(pixels[(row*canvas.width+x)*4+3])return true;return false}};return (rowHasInk(0)||rowHasInk(1))&&(rowHasInk(canvas.height-1)||rowHasInk(canvas.height-2))}}
async function qualifyFont(key){{const config=fontConfigs[key],px=Number(size.value),sample=metricGlyphs.join('');await Promise.all([document.fonts.load(`400 ${{px}}px "${{config.family}}"`,sample),document.fonts.load(`700 ${{px}}px "${{config.family}}"`,sample)]);if(!document.fonts.check(`400 ${{px}}px "${{config.family}}"`,sample)||!document.fonts.check(`700 ${{px}}px "${{config.family}}"`,sample))throw new Error('font files did not load');const canvas=document.createElement('canvas'),ctx=canvas.getContext('2d');ctx.font=`400 ${{px}}px "${{config.family}}"`;const width=ctx.measureText('M').width,tolerance=Math.max(.05,width*.015);if(metricGlyphs.some(g=>Math.abs(ctx.measureText(g).width-width)>tolerance))throw new Error('regular glyphs do not share one cell width');ctx.font=`700 ${{px}}px "${{config.family}}"`;if(metricGlyphs.some(g=>Math.abs(ctx.measureText(g).width-width)>tolerance))throw new Error('bold glyphs do not match the regular cell width');if(!glyphTouchesCellEdges(ctx,'┃',width,px,config.family)||!glyphTouchesCellEdges(ctx,'│',width,px,config.family))throw new Error('vertical decorations do not join cell edges');return true}}
async function show(){{const generation=++showGeneration;document.body.dataset.mode=mode.value;if(mode.value!=='focus')tools.open=true;const selectedVariant=variant.value,config=fontConfigs[font.value];variant.disabled=mode.value!=='coverage';caseSelect.disabled=mode.value!=='production';document.documentElement.style.setProperty('--preview-font',`'${{config.family}}'`);document.documentElement.style.setProperty('--preview-size',`${{size.value}}px`);main.hidden=true;fontError.hidden=true;fontStatus.textContent='checking font…';const results=await Promise.all(Object.keys(fontConfigs).map(async key=>{{try{{await qualifyFont(key);return [key,true]}}catch(error){{return [key,false,error]}}}}));if(generation!==showGeneration)return;for(const [key,ok,error] of results){{const option=[...font.options].find(x=>x.value===key);option.disabled=!ok&&key!==font.value;option.title=ok?'terminal grid verified':error.message}}const selected=results.find(([key])=>key===font.value);if(!selected[1]){{fontStatus.textContent='font unavailable';fontError.textContent=`${{config.label}} cannot render this terminal grid: ${{selected[2].message}}. Preview blocked to prevent fallback-font review.`;fontError.hidden=false;return}}const rejected=results.filter(([,ok])=>!ok).map(([key,,error])=>`${{fontConfigs[key].label}} rejected: ${{error.message}}`);fontStatus.textContent=['terminal grid verified',...rejected].join(' · ');main.hidden=false;cards.forEach(x=>x.hidden=mode.value==='focus'?x.dataset.focus!=='true':mode.value==='coverage'?x.dataset.variant!==selectedVariant:x.dataset.case!==caseSelect.value||x.dataset.variant!=='Production');const next=new URLSearchParams();next.set('mode',mode.value);if(mode.value==='focus')next.set('focus',document.body.dataset.focusId);if(mode.value==='production')next.set('case',caseSelect.value);if(mode.value==='coverage')next.set('variant',selectedVariant);next.set('font',font.value);next.set('size',size.value);history.replaceState(null,'',`${{location.pathname}}?${{next}}${{location.hash}}`)}}
mode.onchange=caseSelect.onchange=variant.onchange=font.onchange=size.onchange=()=>void show();copy.onclick=async()=>{{try{{await navigator.clipboard.writeText(location.href);copyStatus.textContent='Copied'}}catch{{copyStatus.textContent='Copy failed'}}setTimeout(()=>copyStatus.textContent='',1500)}};void show();let revision;
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
    println!("Golem TUI design lab: http://{address}");
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
    let mut case_index = CASES
        .iter()
        .position(|case| case.id == CURRENT_FOCUS.case_ids[0])
        .ok_or_else(|| anyhow::anyhow!("current TUI preview focus references an unknown case"))?;
    let mut variant = 0usize;
    let mut production_reference = false;
    let mut redraw = true;
    loop {
        if terminate.load(Ordering::Relaxed) {
            break;
        }
        if redraw {
            let size = terminal.size()?;
            let case = CASES[case_index];
            let visual = if production_reference {
                TuiVisualVariant::Production
            } else {
                CURRENT_FOCUS.variants[variant]
            };
            execute!(
                std::io::stdout(),
                crossterm::terminal::SetTitle(format!(
                    "Golem TUI focus: {} — {} — {}",
                    CURRENT_FOCUS.title,
                    case.title,
                    visual.name()
                ))
            )?;
            let buffer = render_preview_buffer(case.id, visual, size.width, size.height)?;
            terminal.draw(|frame| *frame.buffer_mut() = buffer)?;
            redraw = false;
        }
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let event = event::read()?;
        apply_terminal_resize(&mut terminal, &event)?;
        match event {
            Event::Resize(_, _) => {}
            Event::Key(key) => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Up => case_index = case_index.checked_sub(1).unwrap_or(CASES.len() - 1),
                KeyCode::Down => case_index = (case_index + 1) % CASES.len(),
                KeyCode::Left => {
                    production_reference = false;
                    variant = variant
                        .checked_sub(1)
                        .unwrap_or(CURRENT_FOCUS.variants.len() - 1)
                }
                KeyCode::Right => {
                    production_reference = false;
                    variant = (variant + 1) % CURRENT_FOCUS.variants.len();
                }
                KeyCode::Char('p') => production_reference = !production_reference,
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
    let mut out = format!("<pre class=\"terminal-layer {class}\"{hidden}>");
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
                    out.push_str("</span>");
                }
                write!(out, "<span style=\"{style}\">").unwrap();
                run_style = Some(style);
            }
            match layer {
                HtmlLayer::Background => out.push(' '),
                HtmlLayer::Foreground => out.push_str(&escape_html(cell.symbol())),
            }
        }
        if run_style.is_some() {
            out.push_str("</span>");
        }
        out.push('\n');
    }
    out.push_str("</pre>");
    out
}

fn effective_colors(style: Style) -> (Option<Color>, Option<Color>) {
    if style.add_modifier.contains(Modifier::REVERSED) {
        (style.bg, style.fg)
    } else {
        (style.fg, style.bg)
    }
}

fn css_background_style(style: Style) -> String {
    let (_, bg) = effective_colors(style);
    bg.map(|color| format!("background-color:{};", css_color(color)))
        .unwrap_or_default()
}

fn css_foreground_style(style: Style) -> String {
    let (fg, _) = effective_colors(style);
    let mut css = String::new();
    if let Some(color) = fg {
        write!(css, "color:{};", css_color(color)).unwrap();
    }
    css.push_str(&css_modifiers(style));
    css
}

#[cfg(test)]
fn css_style(style: Style) -> String {
    let (fg, bg) = effective_colors(style);
    let mut css = String::new();
    if let Some(color) = fg {
        write!(css, "color:{};", css_color(color)).unwrap();
    }
    if let Some(color) = bg {
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
    use ratatui::backend::TestBackend;
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
        assert!(!apply_terminal_resize(&mut terminal, &Event::FocusGained,).unwrap());
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
        assert!(
            css.contains("color:#000080")
                && css.contains("background-color:#800000")
                && css.contains("font-weight:bold")
        );
    }
    #[test]
    fn groups_runs_and_preserves_wide_symbols() {
        let mut b = Buffer::empty(ratatui::layout::Rect::new(0, 0, 3, 1));
        b[(0, 0)]
            .set_symbol("界")
            .set_fg(Color::Red)
            .set_bg(Color::Green);
        b[(1, 0)]
            .set_symbol("")
            .set_fg(Color::Red)
            .set_bg(Color::Green);
        b[(2, 0)]
            .set_symbol("<")
            .set_fg(Color::Blue)
            .set_bg(Color::Yellow);
        let html = buffer_html(&b);
        assert_eq!(html.matches("<span").count(), 4);
        assert!(html.contains("terminal-background"));
        assert!(html.contains("terminal-foreground"));
        assert!(html.contains("aria-hidden=\"true\""));
        assert!(html.contains("界") && html.contains("&lt;"));
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
    fn renders_every_design_lab_case_and_variant() {
        for case in CASES {
            for variant in TuiVisualVariant::ALL {
                render_preview_buffer(case.id, variant, case.width, case.height).unwrap();
            }
        }
    }

    #[test]
    fn every_story_renders_at_wide_short_narrow_and_extreme_sizes() {
        for case in CASES {
            for (width, height) in [(160, 40), (100, 8), (32, 24), (12, 4), (1, 1)] {
                let result = std::panic::catch_unwind(|| {
                    render_preview_buffer(case.id, TuiVisualVariant::FrameBase, width, height)
                });
                match result {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => panic!("{} failed at {width}x{height}: {error}", case.id),
                    Err(_) => panic!("{} panicked at {width}x{height}", case.id),
                }
            }
        }
    }

    #[test]
    fn design_lab_covers_foundation_states_and_progressive_shortcuts() {
        let render = |scene| {
            buffer_text(
                &render_preview_buffer(scene, TuiVisualVariant::Production, 120, 28).unwrap(),
            )
        };

        let shell = render("shell-default");
        assert!(
            shell.contains("app:")
                && shell.contains("server:")
                && shell.contains("ctrl+x")
                && shell.contains("Quit")
        );

        let content = render("content-density");
        assert!(
            content.contains("Unavailable")
                && content.contains("Loading")
                && content.contains("Active")
                && content.contains("Success")
                && content.contains("Warning")
                && content.contains("Error")
                && content.contains("Empty")
                && content.contains("Output")
        );
        assert!(!content.contains("Content hierarchy"));
        assert!(!content.contains("tab focus"));

        let frame_content = buffer_text(
            &render_preview_buffer("content-density", TuiVisualVariant::FrameBase, 100, 24)
                .unwrap(),
        );
        let mut content_rows = frame_content.lines();
        assert!(content_rows.next().unwrap().starts_with("┌ GOLEM"));
        assert!(content_rows.next().unwrap().starts_with("├─[ Content ]"));
        assert!(!frame_content.contains("> Content hierarchy"));
        let content_buffer =
            render_preview_buffer("content-density", TuiVisualVariant::FrameBase, 100, 24).unwrap();
        assert!((2..20).all(|y| content_buffer[(0, y)].symbol() == "│"));

        let split_content = render("content-split");
        assert!(split_content.contains("● running") && split_content.contains("× failed"));

        let long_content = render("content-long");
        assert!(long_content.contains("Location") && long_content.contains("…"));

        let split = render("split-focus");
        assert!(split.contains("● Focused panel") && split.contains("○ Secondary"));

        let leader = render("shortcut-leader");
        assert!(
            leader.contains("workspace") && leader.contains("layout") && leader.contains("help")
        );

        let search = render("overlay-search");
        assert!(search.contains("enter Select") && search.contains("esc Close"));

        let decision = render("overlay-decision");
        assert!(decision.contains("Confirm") && decision.contains("Cancel"));

        let error = render("overlay-error");
        assert!(error.contains("Deployment failed") && error.contains("Existing deployment state"));

        let nested = render("overlay-nested");
        assert!(nested.contains("Action unavailable") && nested.contains("Return to confirmation"));
    }

    #[test]
    fn split_boundary_connects_to_footer_without_a_terminal_edge_rail() {
        let buffer =
            render_preview_buffer("split-focus", TuiVisualVariant::FrameBase, 120, 28).unwrap();
        assert!(
            !has_adjacent_vertical_rails(&buffer),
            "{}",
            buffer_text(&buffer)
        );
        assert!(!has_adjacent_horizontal_rules(&buffer));
        let text = buffer_text(&buffer);
        assert!(text.contains("[ Focused panel ]"));
        assert!(text.contains("( Secondary )"));
        assert!(!text.contains("tab next"));
        assert!(text.contains("GOLEM · app preview-app · env local · server local"));
        assert_eq!(buffer[(119, 1)].symbol(), "┐");
        assert_eq!(buffer[(119, 2)].symbol(), " ");
        assert_eq!(buffer[(119, 25)].symbol(), "┘");
        assert!((2..25).any(|y| buffer[(119, y)].symbol() == "┤"));
        assert_eq!(buffer[(119, 27)].symbol(), "h");
        let split_footer_x = (0..120)
            .find(|x| buffer[(*x, 25)].symbol() == "┴")
            .expect("split rail should join an unobstructed footer rule");
        assert_eq!(buffer[(split_footer_x, 24)].symbol(), "│");

        let single =
            render_preview_buffer("shell-default", TuiVisualVariant::FrameBase, 100, 24).unwrap();
        assert_eq!(single[(99, 1)].symbol(), "┐");
        assert_eq!(single[(99, 2)].symbol(), " ");
        assert_eq!(single[(99, 21)].symbol(), "┘");
        assert_eq!(single[(99, 23)].symbol(), "h");

        let leader = buffer_text(
            &render_preview_buffer("shortcut-leader", TuiVisualVariant::FrameBase, 100, 24)
                .unwrap(),
        );
        assert!(leader.contains("Workspace") && leader.contains("Context"));
        assert!(leader.contains("Commands") && !leader.contains("More"));

        let compact = buffer_text(
            &render_preview_buffer("shell-default", TuiVisualVariant::FrameBase, 38, 12).unwrap(),
        );
        assert!(compact.contains("ctrl+pCommands") || compact.contains("ctrl+p Commands"));

        let content =
            render_preview_buffer("content-split", TuiVisualVariant::FrameBase, 120, 28).unwrap();
        assert!(!has_adjacent_vertical_rails(&content));
        assert!((2..25).all(|y| content[(0, y)].symbol() == "│"));
        assert_eq!(content[(119, 1)].symbol(), "┐");
        assert!((2..25).all(|y| content[(119, y)].symbol() != "│"));
        assert_eq!(content[(119, 25)].symbol(), "┘");
        let divider_x = (0..120)
            .find(|x| content[(*x, 1)].symbol() == "┬")
            .expect("content split should have one shared header junction");
        assert!((2..25).all(|y| content[(divider_x, y)].symbol() == "│"));
        assert_eq!(content[(divider_x, 25)].symbol(), "┴");
        let output_y = (2..25)
            .find(|y| {
                (divider_x + 1..120)
                    .map(|x| content[(x, *y)].symbol())
                    .collect::<String>()
                    .starts_with("out Primary content")
            })
            .expect("right pane output row");
        assert_eq!(content[(119, output_y)].symbol(), "…");
    }

    #[test]
    fn scrollbar_cases_contrast_edge_content_with_real_scrollbar_cells() {
        let plain =
            render_preview_buffer("shell-default", TuiVisualVariant::FrameBase, 100, 24).unwrap();
        let scrolling =
            render_preview_buffer("shell-scrollbar", TuiVisualVariant::FrameBase, 100, 24).unwrap();
        assert_eq!(plain[(99, 1)].symbol(), "┐");
        assert_eq!(scrolling[(99, 1)].symbol(), "┐");
        assert_ne!(plain[(99, 6)].symbol(), " ");
        assert_ne!(plain[(99, 6)].symbol(), scrolling[(99, 6)].symbol());
        assert!((2..20).any(|y| matches!(scrolling[(99, y)].symbol(), "▲" | "║" | "█" | "▼")));
        for y in 2..20 {
            if matches!(scrolling[(99, y)].symbol(), "▲" | "║" | "█" | "▼") {
                assert!(matches!(
                    scrolling[(99, y)].fg,
                    Color::Rgb(110, 110, 126) | Color::Rgb(74, 74, 85)
                ));
            }
        }

        let multi = render_preview_buffer("split-scrollbars", TuiVisualVariant::FrameBase, 120, 28)
            .unwrap();
        assert_eq!(multi[(119, 1)].symbol(), "┐");
        assert!((2..24).any(|y| matches!(multi[(119, y)].symbol(), "▲" | "║" | "█" | "▼")));
        let divider_x = (0..120)
            .find(|x| multi[(*x, 1)].symbol() == "┬")
            .expect("split header divider");
        let primary_scrollbar_x = divider_x - 1;
        assert!((2..25).any(|y| matches!(
            multi[(primary_scrollbar_x, y)].symbol(),
            "▲" | "║" | "█" | "▼"
        )));
        assert!((2..25).any(|y| {
            multi[(primary_scrollbar_x - 1, y)].symbol() == "…"
                && matches!(
                    multi[(primary_scrollbar_x, y)].symbol(),
                    "▲" | "║" | "█" | "▼"
                )
        }));
        assert!((2..25).any(|y| {
            multi[(118, y)].symbol() == "…"
                && matches!(multi[(119, y)].symbol(), "▲" | "║" | "█" | "▼")
        }));
        assert!((0..120).any(|x| multi[(x, 25)].symbol() == "┴"));
    }

    #[test]
    fn pane_table_focus_covers_decoration_panning_details_and_columns() {
        let decoration =
            render_preview_buffer("table-decoration", TuiVisualVariant::FrameBase, 120, 32)
                .unwrap();
        let decoration_text = buffer_text(&decoration);
        assert!(decoration_text.contains("Minimal"));
        assert!(decoration_text.contains("Cell rules"));
        assert!(decoration_text.contains("Odd / even"));
        assert!(decoration_text.contains('▌'));
        assert!(
            (0..32).any(|y| { (1..120).any(|x| decoration[(x, y)].bg == Color::Rgb(28, 28, 36)) })
        );

        let long =
            render_preview_buffer("table-long", TuiVisualVariant::FrameBase, 100, 24).unwrap();
        let long_text = buffer_text(&long);
        assert!(long_text.contains("Owner") && long_text.contains("Description"));
        assert!(long_text.contains("durable payment"));
        assert_eq!(long[(1, 3)].symbol(), "▌");
        assert_eq!(long[(1, 4)].symbol(), "▌");
        assert_eq!(long[(1, 3)].bg, Color::Rgb(36, 36, 46));
        assert_eq!(long[(1, 4)].bg, Color::Rgb(36, 36, 46));

        let details =
            render_preview_buffer("table-details", TuiVisualVariant::FrameBase, 120, 28).unwrap();
        let divider_x = (0..120)
            .find(|x| details[(*x, 1)].symbol() == "┬")
            .expect("table/details divider");
        assert!((2..25).all(|y| details[(divider_x, y)].symbol() == "│"));
        assert!(
            (2..25).any(|y| matches!(details[(divider_x - 1, y)].symbol(), "▲" | "║" | "█" | "▼"))
        );
        assert!((2..25).any(|y| matches!(details[(119, y)].symbol(), "▲" | "║" | "█" | "▼")));
        assert!(buffer_text(&details).contains("Tab focuses details"));

        let chooser = buffer_text(
            &render_preview_buffer("table-columns", TuiVisualVariant::FrameBase, 100, 24).unwrap(),
        );
        assert!(chooser.contains("[ Columns ]"));
        assert!(chooser.contains("Column") && chooser.contains("Visibility"));
        assert!(chooser.contains("Name") && chooser.contains("required"));
        assert!(chooser.contains("Kind") && chooser.contains("☑ shown"));
        assert!(chooser.contains("Owner") && chooser.contains("☐ hidden"));
        assert!(chooser.contains("↑/↓ Navigate"));
        assert!(chooser.contains("space Toggle") && chooser.contains("enter Apply"));
    }

    #[test]
    fn pane_spine_has_one_owner_and_one_consistent_style() {
        for story in ["content-density", "content-long", "content-split"] {
            let buffer =
                render_preview_buffer(story, TuiVisualVariant::FrameBase, 120, 28).unwrap();
            let spine = &buffer[(0, 2)];
            for y in 2..25 {
                assert_eq!(buffer[(0, y)].symbol(), "│", "{story} row {y}");
                assert_eq!(buffer[(0, y)].fg, spine.fg, "{story} row {y}");
                assert_eq!(buffer[(0, y)].bg, spine.bg, "{story} row {y}");
                assert_ne!(buffer[(1, y)].symbol(), "│", "{story} row {y}");
            }
        }
    }

    #[test]
    fn overlays_use_complete_outer_borders_and_one_distinct_surface() {
        let search =
            render_preview_buffer("overlay-search", TuiVisualVariant::FrameBase, 100, 24).unwrap();
        assert_eq!(search[(16, 6)].symbol(), "┌");
        assert_eq!(search[(83, 6)].symbol(), "┐");
        assert_eq!(search[(16, 17)].symbol(), "└");
        assert_eq!(search[(83, 17)].symbol(), "┘");
        assert_eq!(search[(40, 6)].symbol(), "─");
        assert_eq!(search[(40, 17)].symbol(), "─");
        assert_eq!(search[(16, 10)].symbol(), "│");
        assert_eq!(search[(83, 10)].symbol(), "│");
        assert_ne!(search[(16, 6)].bg, search[(15, 6)].bg);
        assert_eq!(search[(16, 6)].bg, search[(17, 7)].bg);
        assert_eq!(search[(17, 7)].bg, search[(40, 9)].bg);
        assert_ne!(search[(17, 7)].bg, search[(1, 2)].bg);
        assert_eq!(search[(16, 6)].fg, search[(83, 17)].fg);
        assert_eq!(search[(18, 6)].symbol(), "[");
        assert_eq!(search[(18, 6)].fg, Color::Rgb(110, 110, 126));
        assert_eq!(search[(20, 6)].symbol(), "C");
        assert_eq!(search[(20, 6)].fg, Color::Rgb(232, 165, 56));
        assert_eq!(search[(18, 8)].symbol(), "›");
        assert_eq!(search[(18, 8)].fg, Color::Rgb(110, 110, 126));
        assert_eq!(search[(18, 8)].bg, Color::Rgb(38, 38, 49));
        assert_eq!(search[(20, 8)].symbol(), "d");
        assert_eq!(search[(20, 8)].fg, Color::Rgb(92, 180, 128));
        assert_ne!(search[(18, 8)].bg, search[(17, 8)].bg);
        assert_eq!(search[(18, 7)].symbol(), " ");
        assert_eq!(search[(18, 7)].bg, search[(17, 8)].bg);
        assert_eq!(search[(18, 9)].symbol(), " ");
        assert_eq!(search[(18, 9)].bg, search[(17, 8)].bg);
        assert_eq!(search[(17, 8)].symbol(), " ");
        assert_eq!(search[(82, 8)].symbol(), " ");
        assert_eq!(search[(21, 10)].symbol(), "◆");
        assert_eq!(search[(23, 10)].symbol(), "D");
        assert_eq!(search[(23, 11)].symbol(), "O");
        assert_eq!(search[(23, 12)].symbol(), "D");
        assert_eq!(search[(21, 10)].bg, Color::Rgb(158, 158, 170));
        assert_eq!(search[(78, 10)].bg, Color::Rgb(158, 158, 170));
        assert_ne!(search[(20, 10)].bg, Color::Rgb(158, 158, 170));
        assert_ne!(search[(79, 10)].bg, Color::Rgb(158, 158, 170));
        assert!((18..82).any(|x| search[(x, 14)].fg == Color::Rgb(181, 124, 48)));
        assert!((18..82).any(|x| search[(x, 14)].fg == Color::Rgb(110, 110, 126)));
        let search_text = buffer_text(&search);
        assert!(search_text.contains("─[ Commands ]"));
        let mut rows = search_text.lines();
        assert!(rows.next().unwrap().starts_with("┌ GOLEM"));
        assert!(rows.next().unwrap().starts_with("├─[ Overview ]"));

        let nested =
            render_preview_buffer("overlay-nested", TuiVisualVariant::FrameBase, 100, 24).unwrap();
        assert_eq!(nested[(24, 8)].symbol(), "┌");
        assert_eq!(nested[(75, 14)].symbol(), "┘");
        assert_eq!(nested[(24, 8)].bg, nested[(25, 9)].bg);

        let decision =
            render_preview_buffer("overlay-decision", TuiVisualVariant::FrameBase, 100, 24)
                .unwrap();
        assert!(buffer_text(&decision).contains("─[ Stop running work? ]"));
        assert_eq!(decision[(20, 6)].fg, Color::Rgb(232, 165, 56));
        assert!((18..82).all(|x| decision[(x, 9)].symbol() == " "));
        assert_eq!(decision[(31, 10)].symbol(), "J");
        assert_eq!(decision[(55, 10)].symbol(), "S");
        assert_eq!(decision[(31, 11)].symbol(), "b");
        assert_eq!(decision[(55, 11)].symbol(), "r");
        assert_eq!(decision[(31, 12)].symbol(), "a");
        assert_eq!(decision[(55, 12)].symbol(), "c");

        let error =
            render_preview_buffer("overlay-error", TuiVisualVariant::FrameBase, 100, 24).unwrap();
        assert!(buffer_text(&error).contains("─[ Deployment failed ]"));
        assert_eq!(error[(22, 7)].fg, Color::Rgb(232, 165, 56));
        assert!((26..74).all(|x| nested[(x, 11)].symbol() == " "));
    }

    fn has_adjacent_vertical_rails(buffer: &Buffer) -> bool {
        let area = buffer.area;
        (area.y..area.y + area.height).any(|y| {
            (area.x..area.x + area.width.saturating_sub(1))
                .any(|x| buffer[(x, y)].symbol() == "│" && buffer[(x + 1, y)].symbol() == "│")
        })
    }

    fn has_adjacent_horizontal_rules(buffer: &Buffer) -> bool {
        let area = buffer.area;
        let rule_rows = (area.y..area.y + area.height)
            .map(|y| {
                (area.x..area.x + area.width)
                    .filter(|x| buffer[(*x, y)].symbol() == "─")
                    .count()
                    > 8
            })
            .collect::<Vec<_>>();
        rule_rows.windows(2).any(|rows| rows[0] && rows[1])
    }

    #[test]
    fn frame_base_keeps_its_current_spacing_and_decorators() {
        let base_shell = buffer_text(
            &render_preview_buffer("shell-default", TuiVisualVariant::FrameBase, 100, 24).unwrap(),
        );
        let mut base_rows = base_shell.lines();
        assert!(base_rows.next().unwrap().starts_with("┌ GOLEM"));
        assert!(base_rows.next().unwrap().starts_with("├─[ Overview ]"));

        let split = buffer_text(
            &render_preview_buffer("split-focus", TuiVisualVariant::FrameBase, 120, 28).unwrap(),
        );
        assert!(split.contains('│') && split.contains('─'));
        assert!(split.contains('├') && split.contains('┬'));
        assert!(split.contains("[ Focused panel"));
        assert!(split.contains("( Secondary )"));
        assert!(split.contains("Focus uses shape"), "{split}");

        let base_content = buffer_text(
            &render_preview_buffer("content-density", TuiVisualVariant::FrameBase, 120, 28)
                .unwrap(),
        );
        assert!(base_content.contains("├─[ Content ]"));
        assert!(!base_content.contains("> Content hierarchy"));

        let production_content = buffer_text(
            &render_preview_buffer("content-density", TuiVisualVariant::Production, 120, 28)
                .unwrap(),
        );
        assert!(!production_content.contains("┃Resources"));
    }

    #[test]
    fn preview_case_ids_are_unique() {
        let ids = CASES
            .iter()
            .map(|case| case.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), CASES.len());
        assert_eq!(CASES.len(), 18);
        assert_eq!(CASES[0].id, "shell-default");
        assert!(CASES.iter().any(|case| case.id == "split-focus"));
        assert!(CASES.iter().any(|case| case.id == "content-split"));
        assert!(CASES.iter().any(|case| case.id == "content-scrolling"));
        assert!(CASES.iter().any(|case| case.id == "table-details"));
    }

    #[test]
    fn current_focus_is_small_unique_and_renderable() {
        assert!((1..=4).contains(&CURRENT_FOCUS.variants.len()));
        assert!((1..=4).contains(&CURRENT_FOCUS.case_ids.len()));
        for case_id in CURRENT_FOCUS.case_ids {
            assert!(CASES.iter().any(|case| case.id == *case_id));
        }
        for (index, variant) in CURRENT_FOCUS.variants.iter().enumerate() {
            assert_ne!(*variant, TuiVisualVariant::Production);
            assert!(!CURRENT_FOCUS.variants[..index].contains(variant));
        }
    }

    #[test]
    fn gallery_defaults_to_linkable_current_focus() {
        let html = gallery_html().unwrap();
        assert!(html.contains("data-mode=\"focus\""));
        assert!(html.contains("Current focus: Pane data tables"));
        assert!(html.contains("Queued next: Adaptive and minimal layouts"));
        assert_eq!(html.matches("data-focus=\"true\"").count(), 4);
        assert!(html.contains("<details id=\"tools\">"));
        assert!(!html.contains("<details id=\"tools\" open>"));
        assert!(html.contains("if(mode.value!=='focus')tools.open=true"));
        assert!(html.contains("params.get('mode')==='compare'?'focus'"));
        assert!(html.contains("params.get('case')"));
        assert!(!html.contains("params.get('experiment')"));
        assert!(html.contains("x.dataset.focus!=='true'"));
        assert!(html.contains("params.get('font')"));
        assert!(html.contains("firacode@6.2.0/distr/woff2/FiraCode-Regular.woff2"));
        assert!(html.contains("FiraCode-Bold.woff2"));
        assert!(html.contains(
            "iosevka-term@73e346bc453d1423498e5b7ef71d9275eb3dbb1d/woff2/iosevka-term-regular.woff2"
        ));
        assert!(html.contains("iosevka-term-bold.woff2"));
        assert!(!html.contains("fonts.googleapis.com"));
        assert!(!html.contains("JetBrains Mono"));
        assert!(!html.contains("Noto Sans Symbols"));
        assert!(!html.contains("System monospace"));
        assert!(!html.contains("@latest"));
        assert!(html.contains("terminal grid verified"));
        assert!(html.contains("rejected:"));
        assert!(html.contains("Preview blocked to prevent fallback-font review"));
        assert!(html.contains("glyphTouchesCellEdges"));
        assert!(html.contains("metricGlyphs"));
        assert!(html.contains("font-variant-ligatures:none"));
        assert!(html.contains("}},2000)"));
        assert!(html.contains("history.replaceState"));
        assert!(html.contains("navigator.clipboard.writeText(location.href)"));
    }

    #[test]
    fn preview_routes_serve_gallery_revision_and_not_found() {
        assert_eq!(
            response_for_path("/?case=shell-default", "gallery", "revision-a"),
            ("200 OK", "text/html; charset=utf-8", "gallery")
        );
        assert_eq!(
            response_for_path("/revision", "gallery", "revision-a"),
            ("200 OK", "text/plain; charset=utf-8", "revision-a")
        );
        assert_eq!(
            response_for_path("/revision", "gallery", "revision-b").2,
            "revision-b"
        );
        assert_eq!(
            response_for_path("/missing", "gallery", "revision-a"),
            ("404 Not Found", "text/plain; charset=utf-8", "not found")
        );
    }

    #[test]
    fn production_preview_matches_normal_rendering() {
        let normal = render_production_preview_buffer(100, 24, false).unwrap();
        let styled = render_production_preview_buffer(100, 24, true).unwrap();
        assert_eq!(normal, styled);
    }
}
