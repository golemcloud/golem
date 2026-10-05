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

//! How the session looks on a colour terminal: the agent's status, type and name, the directory,
//! the git branch and the last result as stone slabs in purple and grey above the line being
//! typed, and the session's own notices in the same style. A slab is text on a coloured
//! background. Slabs stand a cell apart and no block character is drawn, because a terminal
//! fills a background as an exact rectangle and makes no such promise for a character.

use std::time::Duration;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The colours the terminal can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Palette {
    /// 256 colours: the purples and the stone greys.
    Rich,
    /// The terminal's own sixteen colours, where magenta and grey stand in for them.
    Basic,
}

impl Palette {
    /// What the terminal says about itself. `var` reads an environment variable.
    pub fn detect(var: impl Fn(&str) -> Option<String>) -> Self {
        let term = var("TERM").unwrap_or_default();
        let rich = var("COLORTERM").is_some_and(|value| !value.is_empty())
            || ["256color", "direct", "kitty", "ghostty", "alacritty"]
                .iter()
                .any(|kind| term.contains(kind));
        if rich { Palette::Rich } else { Palette::Basic }
    }
}

/// A colour as a terminal reports and takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Asks the terminal for its background colour and then for its device attributes. Every
/// terminal answers the second, so its answer marks the end of the wait for the first.
pub const BACKGROUND_QUERY: &str = "\x1b]11;?\x07\x1b[c";

/// Whether `reply` holds the terminal's answer about its device attributes, the last thing
/// asked.
pub fn answered(reply: &[u8]) -> bool {
    const OPENING: &[u8] = b"\x1b[?";
    reply
        .windows(OPENING.len())
        .position(|window| window == OPENING)
        .is_some_and(|at| reply[at..].contains(&b'c'))
}

/// The background colour in a terminal's `reply`, when it gave one. The answer reads
/// `rgb:RRRR/GGGG/BBBB`, with one to four hexadecimal digits a part.
pub fn parse_background(reply: &[u8]) -> Option<Rgb> {
    const OPENING: &[u8] = b"\x1b]11;";
    let at = reply
        .windows(OPENING.len())
        .position(|window| window == OPENING)?;
    let answer = &reply[at + OPENING.len()..];
    let end = answer
        .iter()
        .position(|byte| matches!(byte, 0x07 | 0x1b))
        .unwrap_or(answer.len());
    let answer = std::str::from_utf8(&answer[..end]).ok()?;
    let parts = answer
        .strip_prefix("rgba:")
        .or_else(|| answer.strip_prefix("rgb:"))?;
    let mut parts = parts.split('/').map(|part| {
        // Each part is a fraction of the largest number its digits can write.
        let full = match part.len() {
            1 => 0xf,
            2 => 0xff,
            3 => 0xfff,
            4 => 0xffff,
            _ => return None,
        };
        let value = u32::from_str_radix(part, 16).ok()?;
        u8::try_from((value * 255 + full / 2) / full).ok()
    });
    Some(Rgb(parts.next()??, parts.next()??, parts.next()??))
}

/// The background a session shows in place of `background`: a little lighter on a dark one
/// and a little darker on a light one, with a hint of purple, so that it reads as another
/// shell without changing how the text on it reads.
pub fn session_shade(background: Rgb) -> Rgb {
    const PURPLE: Rgb = Rgb(143, 99, 255);
    let Rgb(red, green, blue) = background;
    let luma = (u32::from(red) * 299 + u32::from(green) * 587 + u32::from(blue) * 114) / 1000;
    let dark = luma < 128;
    let shade = |channel: u8, purple: u8| {
        let channel = u32::from(channel);
        // Six parts in a hundred towards white or black,
        let stepped = if dark {
            channel + (255 - channel) * 6 / 100
        } else {
            channel * 94 / 100
        };
        // then five in a hundred towards purple.
        ((stepped * 95 + u32::from(purple) * 5) / 100) as u8
    };
    Rgb(
        shade(red, PURPLE.0),
        shade(green, PURPLE.1),
        shade(blue, PURPLE.2),
    )
}

/// The SGR parameters of the band behind a prompt, for a terminal whose own background is
/// `background`. `truecolor` says whether the terminal shows any colour or only its 256.
pub fn band(background: Rgb, truecolor: bool) -> String {
    let Rgb(red, green, blue) = session_shade(background);
    if truecolor {
        return format!("48;2;{red};{green};{blue}");
    }
    // The nearest of the 256: the six steps a channel of the colour cube takes, or one of the
    // twenty-four greys.
    const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let distance = |candidate: (u8, u8, u8)| {
        let square = |a: u8, b: u8| u32::from(a.abs_diff(b)).pow(2);
        square(candidate.0, red) + square(candidate.1, green) + square(candidate.2, blue)
    };
    let nearest_step = |channel: u8| {
        (0..STEPS.len())
            .min_by_key(|&step| STEPS[step].abs_diff(channel))
            .unwrap_or(0)
    };
    let (r, g, b) = (nearest_step(red), nearest_step(green), nearest_step(blue));
    let cube = (16 + 36 * r + 6 * g + b, (STEPS[r], STEPS[g], STEPS[b]));
    let grey = (0..24u8)
        .map(|index| (232 + usize::from(index), 8 + 10 * index))
        .map(|(index, level)| (index, (level, level, level)))
        .min_by_key(|(_, colour)| distance(*colour))
        .unwrap_or(cube);
    let index = if distance(grey.1) < distance(cube.1) {
        grey.0
    } else {
        cube.0
    };
    format!("48;5;{index}")
}

/// What the agent is doing, as far as the session knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Idle: the next command runs at once.
    Ready,
    /// Something is running or waiting on the agent, so the next command queues behind it.
    Busy,
    /// The agent has failed or exited and runs no more commands.
    Failed,
    /// The status could not be read.
    Unknown,
}

/// An agent ID as its type and its constructor arguments. An argument that is a plain string
/// loses its quotes; anything else, and anything after the closing parenthesis, stays as written.
pub fn split_agent(agent: &str) -> (String, String) {
    let Some(open) = agent.find('(') else {
        return (agent.to_string(), String::new());
    };
    let rest = &agent[open + 1..];
    // The commas between the arguments and the parenthesis that closes them: the ones outside
    // every string and every nested value.
    let mut arguments = Vec::new();
    let mut start = 0;
    let mut closed = None;
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in rest.char_indices() {
        if quoted {
            match character {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
            continue;
        }
        match character {
            '"' => quoted = true,
            '(' | '[' | '{' => depth += 1,
            ')' if depth == 0 => {
                closed = Some(index);
                break;
            }
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                arguments.push(&rest[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    arguments.push(&rest[start..closed.unwrap_or(rest.len())]);
    let after = closed.map_or("", |index| rest[index + 1..].trim());
    let name = arguments
        .into_iter()
        .map(|argument| unquoted(argument.trim()))
        .chain([after])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    // The arguments are a list; what follows them is set apart by a space.
    let (arguments, after) = if after.is_empty() {
        (name.as_slice(), None)
    } else {
        (&name[..name.len() - 1], name.last())
    };
    let mut name = arguments.join(", ");
    if let Some(after) = after {
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(after);
    }
    (agent[..open].to_string(), name)
}

/// A plain string without its quotes; anything else as it is.
fn unquoted(argument: &str) -> &str {
    match argument
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        Some(inner) if !inner.contains(['"', '\\']) => inner,
        _ => argument,
    }
}

/// Starts each continued line. It first undoes the colour the editor gives what it draws, so it
/// is plainly dim.
pub const CONTINUATION: &str = "\x1b[0;2m\u{2503}\x1b[0m ";

/// What a slab is for, which decides its colours.
#[derive(Clone, Copy)]
enum Tone {
    /// Opens the group of slabs that describe the agent.
    Head,
    Type,
    Name,
    Directory,
    Branch,
    Failure,
    Quiet,
    Notice,
}

/// The SGR parameters that draw one slab.
struct Paint {
    background: String,
    /// The text on the background.
    foreground: String,
}

impl Tone {
    fn paint(self, palette: Palette) -> Paint {
        match palette {
            Palette::Rich => {
                let (background, foreground) = match self {
                    Tone::Head => (55, 231),
                    Tone::Type => (99, 16),
                    Tone::Name => (141, 16),
                    Tone::Directory => (240, 255),
                    Tone::Branch => (189, 16),
                    Tone::Failure => (160, 231),
                    Tone::Quiet => (237, 189),
                    Tone::Notice => (179, 16),
                };
                Paint {
                    background: format!("48;5;{background}"),
                    foreground: format!("38;5;{foreground}"),
                }
            }
            Palette::Basic => {
                let (background, foreground): (u8, u8) = match self {
                    Tone::Head => (45, 97),
                    Tone::Type => (105, 30),
                    Tone::Name => (47, 30),
                    Tone::Directory => (100, 97),
                    Tone::Branch => (107, 30),
                    Tone::Failure => (41, 97),
                    Tone::Quiet => (100, 97),
                    Tone::Notice => (43, 30),
                };
                Paint {
                    background: background.to_string(),
                    foreground: foreground.to_string(),
                }
            }
        }
    }
}

/// One slab of a row.
struct Block {
    tone: Tone,
    /// What is written between the slab's padding, with any styling of its own.
    text: String,
    /// The columns `text` takes.
    width: usize,
    /// Whether all of the text is bold; otherwise only what `text` itself marks.
    bold: bool,
}

/// The columns a slab takes besides its text: a space on both sides.
const FRAME: usize = 2;

/// The empty columns between two slabs.
const GAP: usize = 1;

impl Block {
    fn plain(tone: Tone, text: &str) -> Self {
        Self {
            tone,
            text: text.to_string(),
            width: text.width(),
            bold: true,
        }
    }

    /// A label, and its value in bold.
    fn labelled(tone: Tone, label: &str, value: &str) -> Self {
        Self {
            tone,
            text: format!("{label}: \x1b[1m{value}\x1b[22m"),
            width: label.width() + 2 + value.width(),
            bold: false,
        }
    }

    /// A value in bold behind the mark that says what it is; the value alone without a mark.
    fn marked(tone: Tone, mark: &str, value: &str) -> Self {
        if mark.is_empty() {
            return Self::plain(tone, value);
        }
        Self {
            tone,
            text: format!("{mark} \x1b[1m{value}\x1b[22m"),
            width: mark.width() + 1 + value.width(),
            bold: false,
        }
    }

    /// Marks the line as a `golem ssh` prompt, with a dot for what the agent is doing. The dot
    /// keeps the colours of a signal in either palette.
    fn ssh(readiness: Readiness, palette: Palette) -> Self {
        let (rich, basic, dot) = match readiness {
            Readiness::Ready => (114, 92, '\u{25cf}'),
            Readiness::Busy => (221, 93, '\u{25cf}'),
            Readiness::Failed => (203, 91, '\u{25cf}'),
            // A hollow dot, not a colour that could pass for a status.
            Readiness::Unknown => (250, 97, '\u{25cb}'),
        };
        let colour = match palette {
            Palette::Rich => format!("38;5;{rich}"),
            Palette::Basic => basic.to_string(),
        };
        Self {
            tone: Tone::Quiet,
            text: format!("ssh \x1b[{colour}m{dot}"),
            width: 5,
            bold: true,
        }
    }

    /// The slab: its text on its background, and nothing else. A terminal fills a cell's
    /// background as an exact rectangle, which it does not promise for any character.
    fn render(&self, palette: Palette) -> String {
        let Paint {
            background,
            foreground,
        } = self.tone.paint(palette);
        let weight = if self.bold { "1;" } else { "" };
        format!(
            "\x1b[{weight}{background};{foreground}m {} \x1b[0m",
            self.text
        )
    }
}

fn slab(tone: Tone, text: &str, palette: Palette) -> String {
    Block::plain(tone, text).render(palette)
}

fn row_width(blocks: &[Block]) -> usize {
    let slabs: usize = blocks.iter().map(|block| block.width + FRAME).sum();
    slabs + blocks.len().saturating_sub(1) * GAP
}

/// The slabs a cell apart. With a band behind them, the cell between two is the band's.
fn row(blocks: &[Block], palette: Palette, band: Option<&str>) -> String {
    let gap = match band {
        Some(band) => format!("\x1b[{band}m{}\x1b[0m", " ".repeat(GAP)),
        None => " ".repeat(GAP),
    };
    blocks
        .iter()
        .map(|block| block.render(palette))
        .collect::<Vec<_>>()
        .join(&gap)
}

/// A row of slabs on a band that runs from the start of the line to the window's right edge.
/// The line is filled with the band first and the slabs are drawn over it.
fn banded_row(blocks: &[Block], palette: Palette, band: Option<&str>) -> String {
    let fill = band
        .map(|band| format!("\x1b[{band}m\x1b[K\x1b[0m"))
        .unwrap_or_default();
    format!("{fill}{}", row(blocks, palette, band))
}

/// `text` cut at its end to `room` columns, with an ellipsis where it was cut.
fn clip_end(text: &str, room: usize) -> String {
    if text.width() <= room {
        return text.to_string();
    }
    let Some(room) = room.checked_sub(1) else {
        return String::new();
    };
    let mut kept = String::new();
    let mut width = 0;
    for character in text.chars() {
        width += character.width().unwrap_or(0);
        if width > room {
            break;
        }
        kept.push(character);
    }
    kept.push('\u{2026}');
    kept
}

/// `text` cut at its start to `room` columns, with an ellipsis where it was cut.
fn clip_start(text: &str, room: usize) -> String {
    if text.width() <= room {
        return text.to_string();
    }
    let Some(room) = room.checked_sub(1) else {
        return String::new();
    };
    let mut kept = Vec::new();
    let mut width = 0;
    for character in text.chars().rev() {
        width += character.width().unwrap_or(0);
        if width > room {
            break;
        }
        kept.push(character);
    }
    kept.push('\u{2026}');
    kept.into_iter().rev().collect()
}

/// The first line of the prompt: that this is `golem ssh` and what the agent is doing, then the
/// agent's type, its name (its constructor arguments), the directory and the git branch when
/// there is one. In a window too narrow for that the labels go first and then the longest
/// values are shortened. The name is left out for an agent without arguments, and the
/// directory until a command has run. With a `band`, the line is filled with it from edge to
/// edge, which sets the session's prompts apart from the shell it was started in.
pub fn context(
    readiness: Readiness,
    agent: &str,
    cwd: &str,
    branch: Option<&str>,
    palette: Palette,
    band: Option<&str>,
    columns: usize,
) -> String {
    let (kind, name) = split_agent(agent);
    let branch = branch.unwrap_or_default();

    let mut labelled = vec![
        Block::ssh(readiness, palette),
        Block::plain(Tone::Head, "Agent"),
        Block::labelled(Tone::Type, "Type", &kind),
    ];
    if !name.is_empty() {
        labelled.push(Block::labelled(Tone::Name, "Name", &name));
    }
    if !cwd.is_empty() {
        labelled.push(Block::plain(Tone::Directory, cwd));
    }
    if !branch.is_empty() {
        labelled.push(Block::labelled(Tone::Branch, "git", branch));
    }
    if row_width(&labelled) <= columns {
        return banded_row(&labelled, palette, band);
    }

    let ssh = Block::ssh(readiness, palette);
    let Some(room) = columns.checked_sub(row_width(std::slice::from_ref(&ssh))) else {
        return String::new();
    };
    // Each value with its mark, and whether its end is what identifies it rather than its start.
    let values: Vec<(Tone, &str, &str, bool)> = [
        (Tone::Type, "", kind.as_str(), false),
        (Tone::Name, "", name.as_str(), false),
        (Tone::Directory, "", cwd, true),
        (Tone::Branch, "git", branch, false),
    ]
    .into_iter()
    .filter(|(_, _, text, _)| !text.is_empty())
    .collect();
    let around: usize = values
        .iter()
        .map(|(tone, mark, _, _)| GAP + Block::marked(*tone, mark, "").width + FRAME)
        .sum();
    let room = room.saturating_sub(around);
    // No value is wider than `limit`, the widest that lets them all fit.
    let mut limit = values
        .iter()
        .map(|(_, _, text, _)| text.width())
        .max()
        .unwrap_or(0);
    while limit > 0
        && values
            .iter()
            .map(|(_, _, text, _)| text.width().min(limit))
            .sum::<usize>()
            > room
    {
        limit -= 1;
    }
    let mut blocks = vec![ssh];
    // A value needs a character and the mark that it was cut.
    if limit >= 2 {
        blocks.extend(values.into_iter().map(|(tone, mark, text, keep_end)| {
            let shown = if keep_end {
                clip_start(text, limit)
            } else {
                clip_end(text, limit)
            };
            Block::marked(tone, mark, &shown)
        }));
    }
    banded_row(&blocks, palette, band)
}

/// The last command's status when it was not zero, the commands waiting on the agent when there
/// are any, and how long the last command took when that is worth showing. It is drawn at the
/// right edge of the line [`context`] starts, over the same band.
pub fn result(
    status: u8,
    elapsed: Option<Duration>,
    queue: u64,
    palette: Palette,
    band: Option<&str>,
) -> String {
    let mut blocks = Vec::new();
    if status != 0 {
        blocks.push(Block::plain(Tone::Failure, &format!("\u{2717} {status}")));
    }
    if queue > 0 {
        blocks.push(Block::plain(Tone::Notice, &format!("queue {queue}")));
    }
    // A time that would read as zero says nothing.
    if let Some(elapsed) = elapsed.filter(|elapsed| elapsed.as_millis() >= 50) {
        blocks.push(Block::plain(Tone::Quiet, &elapsed_text(elapsed)));
    }
    row(&blocks, palette, band)
}

/// The marker the command is typed after, red after a failure.
pub fn marker(ok: bool) -> String {
    format!("\x1b[{}m\u{276f}\x1b[0m ", if ok { 32 } else { 31 })
}

/// A duration the way the prompt shows it: `0.4s`, `12s`, `1m 05s`, `1h 02m`.
pub fn elapsed_text(elapsed: Duration) -> String {
    let tenths = (elapsed.as_millis() + 50) / 100;
    let seconds = elapsed.as_secs();
    if tenths < 100 {
        format!("{}.{}s", tenths / 10, tenths % 10)
    } else if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60)
    }
}

/// The words shown beside the animation, one after another. They are about strength and about
/// lasting. None names something Golem really does to an agent, which would read as status.
const VERBS: [&str; 14] = [
    "reinforcing",
    "tempering",
    "concocting",
    "fortifying",
    "formulating",
    "reforging",
    "hardening",
    "refining",
    "annealing",
    "inscribing",
    "honing",
    "enduring",
    "anchoring",
    "recasting",
];

/// How many ticks a word stays.
const VERB_TICKS: usize = 36;

/// What the animation of the running line is drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loader {
    /// The word GOLEM in runes: built stroke by stroke, turned to stone and broken to dots,
    /// over and over.
    Runes,
    /// One gem that grows and shrinks, for where a font with runes cannot be counted on.
    Gem,
}

impl Loader {
    /// Runes, unless `GOLEM_SSH_RUNES` says no or the terminal is the Linux console, which has
    /// no font for them. `var` reads an environment variable.
    pub fn detect(var: impl Fn(&str) -> Option<String>) -> Self {
        let asked = var("GOLEM_SSH_RUNES").and_then(|value| {
            match value.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Some(true),
                "0" | "false" | "no" | "off" => Some(false),
                _ => None,
            }
        });
        let console = var("TERM").is_some_and(|term| term == "linux");
        if asked.unwrap_or(!console) {
            Loader::Runes
        } else {
            Loader::Gem
        }
    }
}

/// The colours of the animation and of the word beside it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ink {
    /// A dot nothing has grown from yet.
    Stone,
    Deep,
    Mid,
    Light,
    /// The brightest, and bold.
    Pale,
    /// A rune turned to stone: grey and bold.
    Rock,
    Dust,
}

impl Ink {
    const fn sgr(self, palette: Palette) -> &'static str {
        match (palette, self) {
            (Palette::Rich, Ink::Stone) => "38;5;240",
            (Palette::Rich, Ink::Deep) => "38;5;98",
            (Palette::Rich, Ink::Mid) => "38;5;99",
            (Palette::Rich, Ink::Light) => "38;5;141",
            (Palette::Rich, Ink::Pale) => "1;38;5;189",
            (Palette::Rich, Ink::Rock) => "1;38;5;250",
            (Palette::Rich, Ink::Dust) => "38;5;244",
            (Palette::Basic, Ink::Stone | Ink::Dust) => "90",
            (Palette::Basic, Ink::Deep | Ink::Mid) => "35",
            (Palette::Basic, Ink::Light) => "95",
            (Palette::Basic, Ink::Pale) => "1;97",
            (Palette::Basic, Ink::Rock) => "1;37",
        }
    }
}

/// Characters in their colours, with one escape sequence for each run of a colour.
fn inked(cells: impl IntoIterator<Item = (char, Ink)>, palette: Palette) -> String {
    let mut text = String::new();
    let mut current: Option<Ink> = None;
    for (character, ink) in cells {
        if current != Some(ink) {
            if current.is_some() {
                text.push_str("\x1b[0m");
            }
            text.push_str(&format!("\x1b[{}m", ink.sgr(palette)));
            current = Some(ink);
        }
        text.push(character);
    }
    if current.is_some() {
        text.push_str("\x1b[0m");
    }
    text
}

/// GOLEM in Elder Futhark: gebo, othala, laguz, ehwaz, mannaz.
const GOLEM: [char; 5] = ['\u{16b7}', '\u{16df}', '\u{16da}', '\u{16d6}', '\u{16d7}'];

/// One round of the runes, in ticks.
const ROUND: usize = 58;

/// The five cells of the rune loader at `tick`.
fn runes(tick: usize) -> [(char, Ink); 5] {
    let phase = tick % ROUND;
    std::array::from_fn(|cell| {
        let rune = GOLEM[cell];
        match phase {
            // Every cell grows from a dot through a stave, the stroke all runes share, into
            // its rune. The cells start two ticks apart.
            0..18 => match (phase.saturating_sub(cell * 2) / 3).min(3) {
                0 => ('\u{b7}', Ink::Stone),
                1 => ('\u{16c1}', Ink::Deep),
                2 => (rune, Ink::Mid),
                _ => (rune, Ink::Light),
            },
            18..22 => (rune, Ink::Light),
            // The finished word flashes,
            22..24 => (rune, Ink::Pale),
            // turns to stone one rune at a time, and stands for a moment.
            24..33 if phase - 24 >= cell => (rune, Ink::Rock),
            24..33 => (rune, Ink::Light),
            // Then the runes break one at a time, three ticks apart: four specks, three, two,
            // and the dot the next round grows from.
            _ => match (phase - 33).checked_sub(cell * 3).map(|since| since / 2) {
                None => (rune, Ink::Rock),
                Some(0) => ('\u{2058}', Ink::Rock),
                Some(1) => ('\u{2234}', Ink::Rock),
                Some(2) => ('\u{2025}', Ink::Dust),
                Some(_) => ('\u{b7}', Ink::Stone),
            },
        }
    })
}

/// The gem at `tick`: from a point to a cut stone and back, brightest when largest.
fn gem(tick: usize) -> (char, Ink) {
    const SHAPES: [(char, Ink); 10] = [
        ('\u{b7}', Ink::Stone),
        ('\u{22c4}', Ink::Deep),
        ('\u{25c7}', Ink::Mid),
        ('\u{25c8}', Ink::Mid),
        ('\u{25c6}', Ink::Light),
        ('\u{2756}', Ink::Pale),
        ('\u{25c6}', Ink::Light),
        ('\u{25c8}', Ink::Mid),
        ('\u{25c7}', Ink::Mid),
        ('\u{22c4}', Ink::Deep),
    ];
    // A shape lasts a tick and a half.
    SHAPES[tick * 2 / 3 % SHAPES.len()]
}

/// The animation at `tick`, always the same number of columns wide.
fn animation(tick: usize, loader: Loader, palette: Palette) -> String {
    match loader {
        Loader::Runes => inked(runes(tick), palette),
        Loader::Gem => inked([gem(tick)], palette),
    }
}

/// The word shown beside the animation at `tick`, with a highlight running across it.
fn verb(tick: usize, palette: Palette) -> String {
    let word = VERBS[tick / VERB_TICKS % VERBS.len()];
    let letters = word.chars().chain(['\u{2026}']);
    let length = word.len() + 1;
    // The highlight starts three letters before the word and runs on five past its end.
    let highlight = (tick % (length + 8)).checked_sub(3);
    inked(
        letters.enumerate().map(|(index, letter)| {
            let ink = match highlight.map(|at| at.abs_diff(index)) {
                Some(0) => Ink::Pale,
                Some(1) => Ink::Light,
                _ => Ink::Mid,
            };
            (letter, ink)
        }),
        palette,
    )
}

/// The line shown while a command runs: the animation, a word, the time and the way out.
pub fn running(tick: usize, elapsed: Duration, loader: Loader, palette: Palette) -> String {
    format!(
        "{} {} \x1b[2m{} \u{b7} ctrl+c stops waiting\x1b[0m",
        animation(tick, loader, palette),
        verb(tick, palette),
        elapsed_text(elapsed)
    )
}

/// The first lines of a session.
pub fn banner(agent: &str, tool: &str, palette: Palette) -> String {
    let name = match palette {
        Palette::Rich => "1;38;5;141",
        Palette::Basic => "1;35",
    };
    format!(
        "{} \x1b[{name}m{agent}\x1b[0m via {tool}\n\
         \x1b[2ma fresh shell per command \u{b7} only the directory carries over \u{b7} `help` \
         for more\x1b[0m",
        slab(Tone::Quiet, "golem ssh", palette)
    )
}

/// Ctrl+C stopped waiting for a command that runs on; it stops at `limit` at the latest.
/// `error` is why the cancel request failed, when it did.
pub fn detached(agent: &str, limit: &str, error: Option<&str>, palette: Palette) -> String {
    let mut notice = format!(
        "{} still running on {agent}\n\
         \x1b[2mstops at {limit} at the latest \u{b7} the next command waits behind it\x1b[0m",
        slab(Tone::Notice, "DETACHED", palette)
    );
    if let Some(error) = error {
        notice.push_str(&format!(
            "\n\x1b[2mthe cancel request failed: {error}\x1b[0m"
        ));
    }
    notice
}

/// Ctrl+C cancelled a command that had not started.
pub fn cancelled(palette: Palette) -> String {
    format!(
        "{} before it started",
        slab(Tone::Notice, "CANCELLED", palette)
    )
}

#[cfg(test)]
mod tests {
    use super::{
        BACKGROUND_QUERY, CONTINUATION, Loader, Palette, Readiness, Rgb, VERBS, animation,
        answered, band, banner, cancelled, context, detached, elapsed_text, marker,
        parse_background, result, running, session_shade, split_agent, verb,
    };
    use std::time::Duration;
    use test_r::test;
    use unicode_width::UnicodeWidthStr;

    const AGENT: &str = "BashOwner(\"you\")";
    const RICH: Palette = Palette::Rich;

    /// A slab as the 256-colour palette draws it: its background and text colours, whether all
    /// of it is bold, and what is written on it.
    fn slab(background: u8, foreground: u8, bold: bool, text: &str) -> String {
        let weight = if bold { "1;" } else { "" };
        format!("\x1b[{weight}48;5;{background};38;5;{foreground}m {text} \x1b[0m")
    }

    /// The text a terminal shows, without the styling.
    fn visible(styled: &str) -> String {
        let mut text = String::new();
        let mut characters = styled.chars();
        while let Some(character) = characters.next() {
            if character == '\x1b' {
                for styling in characters.by_ref() {
                    if styling == 'm' {
                        break;
                    }
                }
            } else {
                text.push(character);
            }
        }
        text
    }

    #[test]
    fn the_context_is_a_row_of_slabs_that_label_the_agent() {
        let ssh = slab(237, 189, true, "ssh \x1b[38;5;114m\u{25cf}");
        // Written out once in full. A slab is a coloured background and nothing else, so that
        // every terminal draws it as an exact rectangle.
        assert_eq!(
            ssh,
            "\x1b[1;48;5;237;38;5;189m ssh \x1b[38;5;114m\u{25cf} \x1b[0m"
        );
        // The slabs stand one cell apart.
        assert_eq!(
            context(
                Readiness::Ready,
                AGENT,
                "/work",
                Some("main"),
                RICH,
                None,
                100
            ),
            [
                ssh,
                slab(55, 231, true, "Agent"),
                slab(99, 16, false, "Type: \x1b[1mBashOwner\x1b[22m"),
                slab(141, 16, false, "Name: \x1b[1myou\x1b[22m"),
                slab(240, 255, true, "/work"),
                slab(189, 16, false, "git: \x1b[1mmain\x1b[22m"),
            ]
            .join(" ")
        );
        // Before the first command the directory is not known.
        assert_eq!(
            visible(&context(Readiness::Ready, AGENT, "", None, RICH, None, 80)),
            " ssh \u{25cf}   Agent   Type: BashOwner   Name: you "
        );
        // An agent without arguments has no name.
        let unnamed = visible(&context(
            Readiness::Ready,
            "Counter()",
            "/",
            None,
            RICH,
            None,
            80,
        ));
        assert!(unnamed.ends_with(" Type: Counter   / "), "{unnamed:?}");
        // No block character is drawn, so nothing depends on how a font draws one.
        for line in [
            context(
                Readiness::Ready,
                AGENT,
                "/work",
                Some("main"),
                RICH,
                None,
                100,
            ),
            result(1, Some(Duration::from_secs(2)), 1, RICH, None),
            banner(AGENT, "bash", RICH),
        ] {
            assert!(
                !line.chars().any(|c| ('\u{2580}'..='\u{259f}').contains(&c)),
                "{line:?}"
            );
        }
    }

    #[test]
    fn a_band_in_the_session_shade_runs_behind_the_prompts_slabs() {
        let shade = "48;2;48;45;53";
        let plain = context(Readiness::Ready, AGENT, "/work", None, RICH, None, 100);
        let banded = context(
            Readiness::Ready,
            AGENT,
            "/work",
            None,
            RICH,
            Some(shade),
            100,
        );
        // The whole line is filled first, so the band reaches the window's right edge,
        assert!(
            banded.starts_with("\x1b[48;2;48;45;53m\x1b[K\x1b[0m"),
            "{banded:?}"
        );
        // and the cell between two slabs is the band's too.
        assert!(
            banded.contains(
                "\x1b[0m\x1b[48;2;48;45;53m \x1b[0m\x1b[1;48;5;55;38;5;231m Agent \x1b[0m"
            ),
            "{banded:?}"
        );
        // The text does not change.
        assert_eq!(visible(&banded), visible(&plain));
        // What is shown at the right edge sits on the same band.
        assert_eq!(
            result(1, Some(Duration::from_secs(2)), 0, RICH, Some(shade)),
            format!(
                "{}\x1b[48;2;48;45;53m \x1b[0m{}",
                slab(160, 231, true, "\u{2717} 1"),
                slab(237, 189, true, "2.0s")
            )
        );
        // A window too narrow for even the dot gets no band either.
        assert_eq!(
            context(Readiness::Ready, AGENT, "/work", None, RICH, Some(shade), 3),
            ""
        );
    }

    #[test]
    fn the_dot_shows_what_the_agent_is_doing() {
        for (readiness, rich, basic) in [
            (Readiness::Ready, "38;5;114m\u{25cf}", "92m\u{25cf}"),
            (Readiness::Busy, "38;5;221m\u{25cf}", "93m\u{25cf}"),
            (Readiness::Failed, "38;5;203m\u{25cf}", "91m\u{25cf}"),
            // An unread status is a hollow dot, not a colour that could pass for one.
            (Readiness::Unknown, "38;5;250m\u{25cb}", "97m\u{25cb}"),
        ] {
            let line = context(readiness, AGENT, "", None, RICH, None, 80);
            assert!(
                line.contains(&format!(" ssh \x1b[{rich} \x1b[0m")),
                "{readiness:?}: {line:?}"
            );
            let line = context(readiness, AGENT, "", None, Palette::Basic, None, 80);
            assert!(
                line.contains(&format!(" ssh \x1b[{basic} \x1b[0m")),
                "{readiness:?}: {line:?}"
            );
        }
    }

    #[test]
    fn sixteen_colours_stand_in_where_there_are_no_more() {
        let line = context(
            Readiness::Ready,
            AGENT,
            "/work",
            None,
            Palette::Basic,
            None,
            100,
        );
        assert!(
            line.starts_with(
                "\x1b[1;100;97m ssh \x1b[92m\u{25cf} \x1b[0m \x1b[1;45;97m Agent \x1b[0m "
            ),
            "{line:?}"
        );
        assert!(
            !line.contains("38;5;") && !line.contains("48;5;"),
            "{line:?}"
        );
        // The text is the same in both palettes.
        assert_eq!(
            visible(&line),
            visible(&context(
                Readiness::Ready,
                AGENT,
                "/work",
                None,
                RICH,
                None,
                100
            ))
        );
    }

    #[test]
    fn the_terminals_answer_about_its_background_is_read() {
        for (reply, colour) in [
            (
                &b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07"[..],
                Some(Rgb(30, 30, 30)),
            ),
            // Ended the other way a terminal may end it, and followed by the second answer.
            (
                b"\x1b]11;rgb:ffff/8080/0000\x1b\\\x1b[?62;4c",
                Some(Rgb(255, 128, 0)),
            ),
            // One, two or three digits a part: each is a fraction of its own full scale.
            (b"\x1b]11;rgb:1e/1e/1e\x07", Some(Rgb(30, 30, 30))),
            (b"\x1b]11;rgb:f/0/8\x07", Some(Rgb(255, 0, 136))),
            (
                b"\x1b]11;rgba:1e1e/1e1e/1e1e/ffff\x07",
                Some(Rgb(30, 30, 30)),
            ),
            // A terminal that does not know the question answers only the second one.
            (b"\x1b[?62;4c", None),
            (b"\x1b]11;rgb:zz/00/00\x07", None),
            (b"\x1b]11;rgb:00/00\x07", None),
            (b"", None),
        ] {
            assert_eq!(
                parse_background(reply),
                colour,
                "{:?}",
                String::from_utf8_lossy(reply)
            );
        }
        assert!(answered(b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07\x1b[?62;4c"));
        assert!(answered(b"\x1b[?1;2c"));
        assert!(!answered(b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07"));
        assert!(!answered(b"\x1b[?62;4"));
        assert!(!answered(b""));
    }

    #[test]
    fn a_session_shows_a_slightly_lifted_purple_tinged_background() {
        // A dark background gets a little lighter, a light one a little darker,
        assert_eq!(session_shade(Rgb(30, 30, 30)), Rgb(48, 45, 53));
        assert_eq!(session_shade(Rgb(255, 255, 255)), Rgb(234, 232, 239));
        assert_eq!(session_shade(Rgb(0, 0, 0)), Rgb(21, 19, 27));
        // and either way the step is small and leans to purple: most blue, least green.
        for value in [0u8, 12, 30, 60, 127, 128, 200, 245, 255] {
            let Rgb(red, green, blue) = session_shade(Rgb(value, value, value));
            assert!(blue > red && red > green, "{value}: {red} {green} {blue}");
            for channel in [red, green, blue] {
                assert!(channel.abs_diff(value) <= 28, "{value}: {channel}");
            }
        }
        // The band behind a prompt is that shade, exactly where the terminal can show any
        // colour and as the nearest of its 256 where it cannot.
        assert_eq!(band(Rgb(30, 30, 30), true), "48;2;48;45;53");
        assert_eq!(band(Rgb(30, 30, 30), false), "48;5;236");
        assert_eq!(band(Rgb(255, 255, 255), false), "48;5;255");
        assert_eq!(BACKGROUND_QUERY, "\x1b]11;?\x07\x1b[c");
    }

    #[test]
    fn the_palette_follows_what_the_terminal_says() {
        let terminal = |set: &[(&str, &str)]| {
            let set: Vec<(String, String)> = set
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect();
            Palette::detect(move |name| {
                set.iter()
                    .find(|(known, _)| known == name)
                    .map(|(_, value)| value.clone())
            })
        };
        assert_eq!(terminal(&[("TERM", "xterm-256color")]), Palette::Rich);
        assert_eq!(terminal(&[("TERM", "screen-256color")]), Palette::Rich);
        assert_eq!(
            terminal(&[("TERM", "xterm"), ("COLORTERM", "truecolor")]),
            Palette::Rich
        );
        assert_eq!(terminal(&[("TERM", "xterm-kitty")]), Palette::Rich);
        for term in ["xterm", "linux", "vt100", "screen", ""] {
            assert_eq!(terminal(&[("TERM", term)]), Palette::Basic, "{term:?}");
        }
        assert_eq!(
            terminal(&[("TERM", "xterm"), ("COLORTERM", "")]),
            Palette::Basic
        );
        assert_eq!(terminal(&[]), Palette::Basic);
    }

    #[test]
    fn an_agent_id_splits_into_its_type_and_its_arguments() {
        for (agent, kind, name) in [
            ("BashOwner(\"you\")", "BashOwner", "you"),
            ("Cart(\"user-42\", \"eu\")", "Cart", "user-42, eu"),
            ("Cart(\"user-42\",\"eu\")", "Cart", "user-42, eu"),
            ("Counter()", "Counter", ""),
            ("Plain", "Plain", ""),
            ("Shard(7, true)", "Shard", "7, true"),
            // A comma inside a string does not separate arguments.
            ("T(\"a,b\", \"c\")", "T", "a,b, c"),
            // Anything that is not a plain string stays as written.
            (
                "Shop({id: 4, tags: [\"a\", \"b\"]}, 7)",
                "Shop",
                "{id: 4, tags: [\"a\", \"b\"]}, 7",
            ),
            ("T(\"say \\\"hi\\\"\")", "T", "\"say \\\"hi\\\"\""),
            // So does anything after the arguments.
            ("T(\"a\")[5f0c]", "T", "a [5f0c]"),
            ("T(\"unclosed", "T", "\"unclosed"),
            ("", "", ""),
        ] {
            assert_eq!(
                split_agent(agent),
                (kind.to_string(), name.to_string()),
                "{agent}"
            );
        }
    }

    #[test]
    fn the_result_shows_a_failure_the_queue_and_the_time() {
        assert_eq!(result(0, None, 0, RICH, None), "");
        assert_eq!(
            result(0, Some(Duration::from_millis(1_200)), 0, RICH, None),
            slab(237, 189, true, "1.2s")
        );
        assert_eq!(
            result(1, None, 0, RICH, None),
            slab(160, 231, true, "\u{2717} 1")
        );
        // A time that would read as zero says nothing.
        assert_eq!(
            result(0, Some(Duration::from_millis(40)), 0, RICH, None),
            ""
        );
        // Commands waiting on the agent are shown only when there are some.
        assert_eq!(
            result(0, None, 2, RICH, None),
            slab(179, 16, true, "queue 2")
        );
        assert_eq!(
            result(130, Some(Duration::from_millis(3_200)), 1, RICH, None),
            [
                slab(160, 231, true, "\u{2717} 130"),
                slab(179, 16, true, "queue 1"),
                slab(237, 189, true, "3.2s"),
            ]
            .join(" ")
        );
    }

    #[test]
    fn a_context_wider_than_the_window_drops_its_labels_and_then_shortens() {
        let agent = "Cart(\"user-8f3a2c\", \"eu-west\")";
        let cwd = "/srv/data/exports/2026/10";
        let wide = visible(&context(
            Readiness::Ready,
            agent,
            cwd,
            None,
            RICH,
            None,
            120,
        ));
        assert!(wide.contains("Type: Cart"), "{wide:?}");
        assert!(wide.contains("Name: user-8f3a2c, eu-west"), "{wide:?}");
        assert!(wide.contains(cwd), "{wide:?}");

        // The labels go first: without them everything is still whole.
        let unlabelled = visible(&context(Readiness::Ready, agent, cwd, None, RICH, None, 70));
        assert!(unlabelled.width() <= 70, "{unlabelled:?}");
        assert!(
            !unlabelled.contains("Type:") && !unlabelled.contains("Agent"),
            "{unlabelled:?}"
        );
        assert!(unlabelled.contains(" Cart "), "{unlabelled:?}");
        assert!(
            unlabelled.contains(" user-8f3a2c, eu-west "),
            "{unlabelled:?}"
        );
        assert!(unlabelled.contains(cwd), "{unlabelled:?}");
        assert!(!unlabelled.contains('\u{2026}'), "{unlabelled:?}");

        // Then the longest values are shortened. The start of the name and the end of the
        // directory are what identify them.
        let narrow = visible(&context(Readiness::Ready, agent, cwd, None, RICH, None, 52));
        assert!(narrow.width() <= 52, "{narrow:?}");
        assert!(narrow.starts_with(" ssh \u{25cf}  "), "{narrow:?}");
        assert!(narrow.contains(" Cart "), "{narrow:?}");
        assert!(narrow.contains(" user-8f3"), "{narrow:?}");
        assert!(narrow.contains("/2026/10 "), "{narrow:?}");
        assert_eq!(narrow.matches('\u{2026}').count(), 2, "{narrow:?}");

        // A short directory keeps all of itself; the name gives way.
        let narrow = visible(&context(
            Readiness::Ready,
            agent,
            "/work",
            None,
            RICH,
            None,
            40,
        ));
        assert!(
            narrow.width() <= 40 && narrow.ends_with(" /work ") && narrow.contains('\u{2026}'),
            "{narrow:?}"
        );
        // The branch is shortened like the other values and keeps its mark.
        let narrow = visible(&context(
            Readiness::Ready,
            agent,
            cwd,
            Some("feature/long-branch-name"),
            RICH,
            None,
            60,
        ));
        assert!(narrow.width() <= 60, "{narrow:?}");
        assert!(narrow.contains(" git feature/"), "{narrow:?}");
        // Wide characters count as the two columns they take.
        let wide = "\u{6771}\u{4eac}\u{6771}\u{4eac}\u{6771}\u{4eac}()";
        let narrow = visible(&context(Readiness::Ready, wide, "", None, RICH, None, 16));
        assert!(
            narrow.width() <= 16 && narrow.contains('\u{6771}'),
            "{narrow:?}"
        );
        // With room for the dot alone, the dot alone; with none, nothing rather than a wrap.
        assert_eq!(
            visible(&context(Readiness::Ready, agent, cwd, None, RICH, None, 9)),
            " ssh \u{25cf} "
        );
        assert_eq!(
            context(Readiness::Ready, agent, cwd, None, RICH, None, 6),
            ""
        );
    }

    #[test]
    fn elapsed_time_reads_at_every_scale() {
        for (millis, text) in [
            (40, "0.0s"),
            (400, "0.4s"),
            (3_250, "3.3s"),
            (9_960, "9s"),
            (12_900, "12s"),
            (65_000, "1m 05s"),
            (3_725_000, "1h 02m"),
        ] {
            assert_eq!(elapsed_text(Duration::from_millis(millis)), text);
        }
    }

    #[test]
    fn the_marker_is_red_after_a_failure() {
        assert_eq!(marker(true), "\x1b[32m\u{276f}\x1b[0m ");
        assert_eq!(marker(false), "\x1b[31m\u{276f}\x1b[0m ");
        assert_eq!(CONTINUATION, "\x1b[0;2m\u{2503}\x1b[0m ");
    }

    #[test]
    fn the_loader_builds_golem_in_runes_turns_it_to_stone_and_breaks_it_to_dots() {
        let shown = |tick| visible(&animation(tick, Loader::Runes, RICH));
        // Every cell starts as a dot and grows through a stave into its rune, left to right.
        assert_eq!(shown(0), "\u{b7}\u{b7}\u{b7}\u{b7}\u{b7}");
        assert_eq!(shown(6), "\u{16b7}\u{16c1}\u{b7}\u{b7}\u{b7}");
        assert_eq!(shown(17), "\u{16b7}\u{16df}\u{16da}\u{16d6}\u{16d7}");
        // The whole word flashes,
        assert_eq!(
            animation(22, Loader::Runes, RICH),
            "\x1b[1;38;5;189m\u{16b7}\u{16df}\u{16da}\u{16d6}\u{16d7}\x1b[0m"
        );
        // turns to stone one rune at a time, grey and heavy with nothing behind it,
        assert_eq!(
            animation(26, Loader::Runes, RICH),
            "\x1b[1;38;5;250m\u{16b7}\u{16df}\u{16da}\x1b[0m\x1b[38;5;141m\u{16d6}\u{16d7}\x1b[0m"
        );
        // and breaks one rune at a time into four specks, three, two and a dot.
        assert_eq!(shown(33), "\u{2058}\u{16df}\u{16da}\u{16d6}\u{16d7}");
        assert_eq!(shown(36), "\u{2234}\u{2058}\u{16da}\u{16d6}\u{16d7}");
        assert_eq!(shown(39), "\u{b7}\u{2234}\u{2058}\u{16d6}\u{16d7}");
        assert_eq!(shown(49), "\u{b7}\u{b7}\u{b7}\u{b7}\u{2025}");
        // The dots are where the next round starts, so the loop has no gap.
        assert_eq!(shown(57), shown(58));
        assert_eq!(shown(58 + 6), shown(6));
        for tick in 0..200 {
            assert_eq!(shown(tick).width(), 5, "tick {tick}");
        }
    }

    #[test]
    fn the_gem_stands_in_for_the_runes() {
        let shown: String = (0..15)
            .map(|tick| visible(&animation(tick, Loader::Gem, RICH)))
            .collect();
        assert_eq!(
            shown,
            "\u{b7}\u{b7}\u{22c4}\u{25c7}\u{25c7}\u{25c8}\u{25c6}\u{25c6}\u{2756}\u{25c6}\u{25c6}\u{25c8}\u{25c7}\u{25c7}\u{22c4}"
        );
        // At its largest it is at its palest.
        assert_eq!(
            animation(8, Loader::Gem, RICH),
            "\x1b[1;38;5;189m\u{2756}\x1b[0m"
        );
    }

    #[test]
    fn the_runes_give_way_where_they_cannot_be_drawn() {
        let terminal = |set: &[(&str, &str)]| {
            let set: Vec<(String, String)> = set
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect();
            Loader::detect(move |name| {
                set.iter()
                    .find(|(known, _)| known == name)
                    .map(|(_, value)| value.clone())
            })
        };
        assert_eq!(terminal(&[("TERM", "xterm-256color")]), Loader::Runes);
        assert_eq!(terminal(&[]), Loader::Runes);
        for no in ["0", "false", "no", "off", " OFF "] {
            assert_eq!(terminal(&[("GOLEM_SSH_RUNES", no)]), Loader::Gem, "{no:?}");
        }
        // The Linux console has no font for runes; saying yes outright still wins.
        assert_eq!(terminal(&[("TERM", "linux")]), Loader::Gem);
        assert_eq!(
            terminal(&[("TERM", "linux"), ("GOLEM_SSH_RUNES", "1")]),
            Loader::Runes
        );
        assert_eq!(terminal(&[("GOLEM_SSH_RUNES", "maybe")]), Loader::Runes);
    }

    #[test]
    fn the_word_changes_and_a_highlight_runs_across_it() {
        assert_eq!(visible(&verb(0, RICH)), "reinforcing\u{2026}");
        assert_eq!(visible(&verb(36, RICH)), "tempering\u{2026}");
        assert_eq!(
            visible(&verb(36 * VERBS.len(), RICH)),
            "reinforcing\u{2026}"
        );
        // The highlight is on the first letter three ticks in, and nowhere before that.
        assert_eq!(verb(0, RICH), "\x1b[38;5;99mreinforcing\u{2026}\x1b[0m");
        assert!(
            verb(3, RICH).starts_with(
                "\x1b[1;38;5;189mr\x1b[0m\x1b[38;5;141me\x1b[0m\x1b[38;5;99minforcing"
            ),
            "{:?}",
            verb(3, RICH)
        );
        for word in VERBS {
            assert!(
                word.len() <= 11 && word.bytes().all(|b| b.is_ascii_lowercase()),
                "{word}"
            );
            // A word that names something Golem really does to an agent would read as status.
            assert!(
                ![
                    "replaying",
                    "recovering",
                    "retrying",
                    "suspending",
                    "resuming",
                    "persisting",
                    "committing",
                    "upgrading",
                    "updating"
                ]
                .contains(&word),
                "{word}"
            );
        }
    }

    #[test]
    fn the_running_line_shows_the_animation_a_word_the_time_and_the_way_out() {
        assert_eq!(
            visible(&running(
                0,
                Duration::from_millis(12_900),
                Loader::Runes,
                RICH
            )),
            "\u{b7}\u{b7}\u{b7}\u{b7}\u{b7} reinforcing\u{2026} 12s \u{b7} ctrl+c stops waiting"
        );
        assert!(
            running(0, Duration::from_secs(1), Loader::Runes, RICH)
                .ends_with(" \x1b[2m1.0s \u{b7} ctrl+c stops waiting\x1b[0m")
        );
        assert_eq!(
            visible(&running(8, Duration::from_secs(70), Loader::Gem, RICH)),
            "\u{2756} reinforcing\u{2026} 1m 10s \u{b7} ctrl+c stops waiting"
        );
        // The longest line, an hour in, still fits the narrowest window the line is shown in.
        for tick in 0..(36 * VERBS.len()) {
            let line = visible(&running(
                tick,
                Duration::from_secs(3_599),
                Loader::Runes,
                RICH,
            ));
            assert!(line.width() <= 52, "tick {tick}: {line:?}");
        }
        // Sixteen colours are enough for it.
        let basic = running(26, Duration::from_secs(5), Loader::Runes, Palette::Basic);
        assert!(!basic.contains("38;5;"), "{basic:?}");
        assert_eq!(
            visible(&basic),
            visible(&running(26, Duration::from_secs(5), Loader::Runes, RICH))
        );
    }

    #[test]
    fn the_banner_names_the_agent_and_how_the_session_works() {
        let shown = banner(AGENT, "bash", RICH);
        assert_eq!(
            shown,
            format!(
                "{} \x1b[1;38;5;141mBashOwner(\"you\")\x1b[0m via bash\n\
                 \x1b[2ma fresh shell per command \u{b7} only the directory carries over \u{b7} `help` for more\x1b[0m",
                slab(237, 189, true, "golem ssh")
            )
        );
        assert!(visible(&shown).lines().all(|line| line.width() <= 80));
        assert!(banner(AGENT, "bash", Palette::Basic).contains("\x1b[1;35mBashOwner"));
    }

    #[test]
    fn the_notice_after_ctrl_c_says_what_the_command_does_now() {
        assert_eq!(
            detached(AGENT, "the tool's time limit", None, RICH),
            format!(
                "{} still running on BashOwner(\"you\")\n\
                 \x1b[2mstops at the tool's time limit at the latest \u{b7} the next command waits behind it\x1b[0m",
                slab(179, 16, true, "DETACHED")
            )
        );
        let failed = detached(AGENT, "its time limit (30 s)", Some("no answer"), RICH);
        assert!(
            failed.ends_with("\n\x1b[2mthe cancel request failed: no answer\x1b[0m"),
            "{failed:?}"
        );
        assert!(visible(&failed).contains("stops at its time limit (30 s) at the latest"));
        assert_eq!(
            cancelled(RICH),
            format!("{} before it started", slab(179, 16, true, "CANCELLED"))
        );
    }
}
