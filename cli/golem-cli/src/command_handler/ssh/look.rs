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

//! How the session looks on a colour terminal: the agent's status, type and name, the directory
//! and the last result as coloured blocks above the line being typed, and the session's own
//! notices in the same style.
//! Only the terminal's sixteen colours are used, so the look follows its colour scheme.

use std::time::Duration;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// How neighbouring blocks meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edges {
    /// Straight edges, which every font draws.
    Square,
    /// Pointed joins drawn with Powerline glyphs, which only patched fonts have.
    Pointed,
}

impl Edges {
    /// Pointed edges when `asked` for, straight ones when asked not to. Unasked, pointed edges
    /// are used only where the terminal draws the glyphs itself, so that nobody sees the boxes
    /// a font without them shows.
    pub fn choose(asked: Option<bool>, terminal_draws_them: bool) -> Self {
        if asked.unwrap_or(terminal_draws_them) {
            Edges::Pointed
        } else {
            Edges::Square
        }
    }
}

/// What a `GOLEM_SSH_POWERLINE` value asks for; `None` when it says neither yes nor no.
pub fn powerline_setting(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Whether the terminal draws Powerline glyphs itself, whatever its font: kitty, WezTerm and
/// Ghostty do. `var` reads an environment variable. Inside a terminal multiplexer the
/// variables say where the session was started, not where it is shown, so there the answer is
/// no.
pub fn draws_powerline(var: impl Fn(&str) -> Option<String>) -> bool {
    if var("TMUX").is_some() || var("STY").is_some() {
        return false;
    }
    var("KITTY_WINDOW_ID").is_some()
        || matches!(var("TERM_PROGRAM").as_deref(), Some("WezTerm" | "ghostty"))
        || matches!(
            var("TERM").as_deref(),
            Some("xterm-kitty" | "xterm-ghostty")
        )
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

/// A block's colours.
#[derive(Clone, Copy)]
enum Tone {
    /// Opens the group of blocks that describe the agent.
    Head,
    Type,
    Name,
    Directory,
    Branch,
    Failure,
    Quiet,
    Notice,
}

impl Tone {
    /// The SGR codes of the block's background and of the text on it.
    const fn codes(self) -> (u8, u8) {
        match self {
            Tone::Head => (46, 30),
            Tone::Type => (106, 30),
            Tone::Name => (47, 30),
            Tone::Directory => (44, 97),
            Tone::Branch => (42, 30),
            Tone::Failure => (41, 97),
            Tone::Quiet => (100, 97),
            Tone::Notice => (43, 30),
        }
    }

    /// The SGR code that draws text in the block's background colour.
    const fn edge(self) -> u8 {
        self.codes().0 - 10
    }
}

/// One block of a row.
struct Block {
    tone: Tone,
    /// What is written between the block's padding, with any styling of its own.
    text: String,
    /// The columns `text` takes.
    width: usize,
    /// Whether all of the text is bold; otherwise only what `text` itself marks.
    bold: bool,
}

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

    /// Marks the line as a `golem ssh` prompt, with a dot for what the agent is doing.
    fn ssh(readiness: Readiness) -> Self {
        let (colour, dot) = match readiness {
            Readiness::Ready => (92, '\u{25cf}'),
            Readiness::Busy => (93, '\u{25cf}'),
            Readiness::Failed => (91, '\u{25cf}'),
            // A hollow dot, not a colour that could pass for a status.
            Readiness::Unknown => (97, '\u{25cb}'),
        };
        Self {
            tone: Tone::Quiet,
            text: format!("ssh \x1b[{colour}m{dot}"),
            width: 5,
            bold: true,
        }
    }

    fn render(&self) -> String {
        let (background, foreground) = self.tone.codes();
        let weight = if self.bold { "1;" } else { "" };
        format!(
            "\x1b[{weight}{background};{foreground}m {} \x1b[0m",
            self.text
        )
    }
}

fn block(tone: Tone, text: &str) -> String {
    Block::plain(tone, text).render()
}

/// The columns each block takes besides its text: a space on both sides, and a pointed edge.
const fn frame(edges: Edges) -> usize {
    match edges {
        Edges::Square => 2,
        Edges::Pointed => 3,
    }
}

fn row_width(blocks: &[Block], edges: Edges) -> usize {
    blocks.iter().map(|block| block.width + frame(edges)).sum()
}

/// Blocks that start at the left edge. A pointed edge follows each block, in the colours of
/// the block and of what comes after it.
fn row_from_left(blocks: &[Block], edges: Edges) -> String {
    let mut row = String::new();
    for (index, block) in blocks.iter().enumerate() {
        row.push_str(&block.render());
        if edges == Edges::Pointed {
            let edge = block.tone.edge();
            match blocks.get(index + 1) {
                Some(next) => {
                    row.push_str(&format!(
                        "\x1b[{edge};{}m\u{e0b0}\x1b[0m",
                        next.tone.codes().0
                    ));
                }
                None => row.push_str(&format!("\x1b[{edge}m\u{e0b0}\x1b[0m")),
            }
        }
    }
    row
}

/// Blocks that end at the right edge. A pointed edge leads each block, in the colours of the
/// block and of what comes before it.
fn row_to_right(blocks: &[Block], edges: Edges) -> String {
    let mut row = String::new();
    let mut previous: Option<Tone> = None;
    for block in blocks {
        if edges == Edges::Pointed {
            let edge = block.tone.edge();
            match previous {
                Some(previous) => {
                    row.push_str(&format!(
                        "\x1b[{edge};{}m\u{e0b2}\x1b[0m",
                        previous.codes().0
                    ));
                }
                None => row.push_str(&format!("\x1b[{edge}m\u{e0b2}\x1b[0m")),
            }
        }
        row.push_str(&block.render());
        previous = Some(block.tone);
    }
    row
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
/// directory until a command has run.
pub fn context(
    readiness: Readiness,
    agent: &str,
    cwd: &str,
    branch: Option<&str>,
    edges: Edges,
    columns: usize,
) -> String {
    let (kind, name) = split_agent(agent);
    let branch = branch.unwrap_or_default();
    // The branch glyph is a Powerline one, like the pointed edges.
    let git = match edges {
        Edges::Square => "git",
        Edges::Pointed => "\u{e0a0}",
    };

    let mut labelled = vec![Block::ssh(readiness), Block::plain(Tone::Head, "Agent")];
    labelled.push(Block::labelled(Tone::Type, "Type", &kind));
    if !name.is_empty() {
        labelled.push(Block::labelled(Tone::Name, "Name", &name));
    }
    if !cwd.is_empty() {
        labelled.push(Block::plain(Tone::Directory, cwd));
    }
    if !branch.is_empty() {
        labelled.push(Block::marked(Tone::Branch, git, branch));
    }
    if row_width(&labelled, edges) <= columns {
        return row_from_left(&labelled, edges);
    }

    let ssh = Block::ssh(readiness);
    let Some(room) = columns.checked_sub(row_width(std::slice::from_ref(&ssh), edges)) else {
        return String::new();
    };
    // Each value with its mark, and whether its end is what identifies it rather than its start.
    let values: Vec<(Tone, &str, &str, bool)> = [
        (Tone::Type, "", kind.as_str(), false),
        (Tone::Name, "", name.as_str(), false),
        (Tone::Directory, "", cwd, true),
        (Tone::Branch, git, branch, false),
    ]
    .into_iter()
    .filter(|(_, _, text, _)| !text.is_empty())
    .collect();
    let around: usize = values
        .iter()
        .map(|(tone, mark, _, _)| Block::marked(*tone, mark, "").width + frame(edges))
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
    row_from_left(&blocks, edges)
}

/// The last command's status when it was not zero, the commands waiting on the agent when there
/// are any, and how long the last command took when that is worth showing.
pub fn result(status: u8, elapsed: Option<Duration>, queue: u64, edges: Edges) -> String {
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
    row_to_right(&blocks, edges)
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

/// The line shown while a command runs.
pub fn running(elapsed: Duration) -> String {
    format!(
        "{}{} \x1b[2mctrl+c stops waiting\x1b[0m",
        block(Tone::Notice, "RUNNING"),
        block(Tone::Quiet, &elapsed_text(elapsed))
    )
}

/// The first lines of a session.
pub fn banner(agent: &str, tool: &str) -> String {
    format!(
        "{} \x1b[1;36m{agent}\x1b[0m via {tool}\n\
         \x1b[2ma fresh shell per command \u{b7} only the directory carries over \u{b7} `help` \
         for more\x1b[0m",
        block(Tone::Quiet, "golem ssh")
    )
}

/// Ctrl+C stopped waiting for a command that runs on; it stops at `limit` at the latest.
/// `error` is why the cancel request failed, when it did.
pub fn detached(agent: &str, limit: &str, error: Option<&str>) -> String {
    let mut notice = format!(
        "{} still running on {agent}\n\
         \x1b[2mstops at {limit} at the latest \u{b7} the next command waits behind it\x1b[0m",
        block(Tone::Notice, "DETACHED")
    );
    if let Some(error) = error {
        notice.push_str(&format!(
            "\n\x1b[2mthe cancel request failed: {error}\x1b[0m"
        ));
    }
    notice
}

/// Ctrl+C cancelled a command that had not started.
pub fn cancelled() -> String {
    format!("{} before it started", block(Tone::Notice, "CANCELLED"))
}

#[cfg(test)]
mod tests {
    use super::{
        CONTINUATION, Edges, Readiness, banner, cancelled, context, detached, draws_powerline,
        elapsed_text, marker, powerline_setting, result, running, split_agent,
    };
    use std::time::Duration;
    use test_r::test;
    use unicode_width::UnicodeWidthStr;

    const AGENT: &str = "BashOwner(\"you\")";
    const SSH_READY: &str = "\x1b[1;100;97m ssh \x1b[92m\u{25cf} \x1b[0m";
    const HEAD: &str = "\x1b[1;46;30m Agent \x1b[0m";

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
    fn the_context_labels_the_agent_type_and_name() {
        assert_eq!(
            context(Readiness::Ready, AGENT, "/work", None, Edges::Square, 80),
            format!(
                "{SSH_READY}{HEAD}\x1b[106;30m Type: \x1b[1mBashOwner\x1b[22m \x1b[0m\
                 \x1b[47;30m Name: \x1b[1myou\x1b[22m \x1b[0m\x1b[1;44;97m /work \x1b[0m"
            )
        );
        // Before the first command the directory is not known.
        assert_eq!(
            visible(&context(
                Readiness::Ready,
                AGENT,
                "",
                None,
                Edges::Square,
                80
            )),
            " ssh \u{25cf}  Agent  Type: BashOwner  Name: you "
        );
        // An agent without arguments has no name.
        assert_eq!(
            visible(&context(
                Readiness::Ready,
                "Counter()",
                "/",
                None,
                Edges::Square,
                80
            )),
            " ssh \u{25cf}  Agent  Type: Counter  / "
        );
    }

    #[test]
    fn the_dot_shows_what_the_agent_is_doing() {
        for (readiness, dot) in [
            (Readiness::Ready, "\x1b[92m\u{25cf}"),
            (Readiness::Busy, "\x1b[93m\u{25cf}"),
            (Readiness::Failed, "\x1b[91m\u{25cf}"),
            // An unread status is a hollow dot, not a colour that could pass for one.
            (Readiness::Unknown, "\x1b[97m\u{25cb}"),
        ] {
            let line = context(readiness, AGENT, "", None, Edges::Square, 80);
            assert!(
                line.starts_with(&format!("\x1b[1;100;97m ssh {dot} \x1b[0m")),
                "{readiness:?}: {line:?}"
            );
        }
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
        assert_eq!(result(0, None, 0, Edges::Square), "");
        assert_eq!(
            result(0, Some(Duration::from_millis(1_200)), 0, Edges::Square),
            "\x1b[1;100;97m 1.2s \x1b[0m"
        );
        assert_eq!(
            result(1, None, 0, Edges::Square),
            "\x1b[1;41;97m \u{2717} 1 \x1b[0m"
        );
        // A time that would read as zero says nothing.
        assert_eq!(
            result(0, Some(Duration::from_millis(40)), 0, Edges::Square),
            ""
        );
        // Commands waiting on the agent are shown only when there are some.
        assert_eq!(
            result(0, None, 2, Edges::Square),
            "\x1b[1;43;30m queue 2 \x1b[0m"
        );
        assert_eq!(
            result(130, Some(Duration::from_millis(3_200)), 1, Edges::Square),
            "\x1b[1;41;97m \u{2717} 130 \x1b[0m\x1b[1;43;30m queue 1 \x1b[0m\x1b[1;100;97m 3.2s \x1b[0m"
        );
    }

    #[test]
    fn pointed_edges_join_blocks_in_the_colours_of_both() {
        assert_eq!(
            context(Readiness::Ready, "A(\"x\")", "/w", None, Edges::Pointed, 80),
            format!(
                "{SSH_READY}\x1b[90;46m\u{e0b0}\x1b[0m{HEAD}\x1b[36;106m\u{e0b0}\x1b[0m\
                 \x1b[106;30m Type: \x1b[1mA\x1b[22m \x1b[0m\x1b[96;47m\u{e0b0}\x1b[0m\
                 \x1b[47;30m Name: \x1b[1mx\x1b[22m \x1b[0m\x1b[37;44m\u{e0b0}\x1b[0m\
                 \x1b[1;44;97m /w \x1b[0m\x1b[34m\u{e0b0}\x1b[0m"
            )
        );
        assert_eq!(
            result(1, Some(Duration::from_millis(400)), 0, Edges::Pointed),
            "\x1b[31m\u{e0b2}\x1b[0m\x1b[1;41;97m \u{2717} 1 \x1b[0m\x1b[90;41m\u{e0b2}\x1b[0m\x1b[1;100;97m 0.4s \x1b[0m"
        );
        assert_eq!(result(0, None, 0, Edges::Pointed), "");
    }

    #[test]
    fn a_context_wider_than_the_window_drops_its_labels_and_then_shortens() {
        let agent = "Cart(\"user-8f3a2c\", \"eu-west\")";
        let cwd = "/srv/data/exports/2026/10";
        for edges in [Edges::Square, Edges::Pointed] {
            let wide = visible(&context(Readiness::Ready, agent, cwd, None, edges, 120));
            assert!(wide.contains("Type: Cart"), "{wide:?}");
            assert!(wide.contains("Name: user-8f3a2c, eu-west"), "{wide:?}");
            assert!(wide.contains(cwd), "{wide:?}");

            // The labels go first: without them everything is still whole.
            let unlabelled = visible(&context(Readiness::Ready, agent, cwd, None, edges, 70));
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
            let narrow = visible(&context(Readiness::Ready, agent, cwd, None, edges, 40));
            assert!(narrow.width() <= 40, "{narrow:?}");
            assert!(narrow.starts_with(" ssh \u{25cf} "), "{narrow:?}");
            assert!(narrow.contains(" Cart "), "{narrow:?}");
            assert!(narrow.contains(" user-8f3"), "{narrow:?}");
            assert!(narrow.contains("/2026/10 "), "{narrow:?}");
            assert_eq!(narrow.matches('\u{2026}').count(), 2, "{narrow:?}");
        }
        // A short directory keeps all of itself; the name gives way.
        let narrow = visible(&context(
            Readiness::Ready,
            agent,
            "/work",
            None,
            Edges::Square,
            34,
        ));
        assert!(
            narrow.width() <= 34 && narrow.ends_with(" /work "),
            "{narrow:?}"
        );
        // Wide characters count as the two columns they take.
        let wide = "\u{6771}\u{4eac}\u{6771}\u{4eac}\u{6771}\u{4eac}()";
        let narrow = visible(&context(
            Readiness::Ready,
            wide,
            "",
            None,
            Edges::Square,
            16,
        ));
        assert!(
            narrow.width() <= 16 && narrow.contains('\u{6771}'),
            "{narrow:?}"
        );
        // With room for the dot alone, the dot alone; with none, nothing rather than a wrap.
        assert_eq!(
            visible(&context(
                Readiness::Ready,
                agent,
                cwd,
                None,
                Edges::Square,
                9
            )),
            " ssh \u{25cf} "
        );
        assert_eq!(
            context(Readiness::Ready, agent, cwd, None, Edges::Square, 3),
            ""
        );
    }

    #[test]
    fn the_branch_follows_the_directory_behind_a_git_mark() {
        let square = context(
            Readiness::Ready,
            AGENT,
            "/work",
            Some("main"),
            Edges::Square,
            80,
        );
        assert!(
            square.ends_with(
                "\x1b[1;44;97m /work \x1b[0m\x1b[42;30m git \x1b[1mmain\x1b[22m \x1b[0m"
            ),
            "{square:?}"
        );
        // The branch glyph is a Powerline one, so it is drawn only with the pointed edges.
        let pointed = context(
            Readiness::Ready,
            AGENT,
            "/work",
            Some("main"),
            Edges::Pointed,
            80,
        );
        assert!(
            pointed.ends_with(
                "\x1b[1;44;97m /work \x1b[0m\x1b[34;42m\u{e0b0}\x1b[0m\x1b[42;30m \u{e0a0} \x1b[1mmain\x1b[22m \x1b[0m\x1b[32m\u{e0b0}\x1b[0m"
            ),
            "{pointed:?}"
        );
        // In a narrow window the branch is shortened like the other values and keeps its mark.
        let narrow = visible(&context(
            Readiness::Ready,
            "Cart(\"user-8f3a2c\", \"eu-west\")",
            "/srv/data/exports/2026/10",
            Some("feature/long-branch-name"),
            Edges::Square,
            60,
        ));
        assert!(narrow.width() <= 60, "{narrow:?}");
        assert!(narrow.contains(" git feature/"), "{narrow:?}");
        assert!(!narrow.contains("Type:"), "{narrow:?}");
    }

    #[test]
    fn pointed_edges_are_used_only_when_asked_for_or_sure_to_show() {
        assert_eq!(Edges::choose(None, false), Edges::Square);
        assert_eq!(Edges::choose(None, true), Edges::Pointed);
        // What the user asked for wins either way.
        assert_eq!(Edges::choose(Some(true), false), Edges::Pointed);
        assert_eq!(Edges::choose(Some(false), true), Edges::Square);
    }

    #[test]
    fn the_powerline_variable_says_yes_no_or_nothing() {
        for value in ["1", "true", "yes", "on", " ON "] {
            assert_eq!(powerline_setting(value), Some(true), "{value:?}");
        }
        for value in ["0", "false", "no", "off", "False"] {
            assert_eq!(powerline_setting(value), Some(false), "{value:?}");
        }
        for value in ["", "maybe", "2"] {
            assert_eq!(powerline_setting(value), None, "{value:?}");
        }
    }

    #[test]
    fn only_terminals_that_draw_the_glyphs_themselves_get_them_unasked() {
        let terminal = |set: &[(&str, &str)]| {
            let set: Vec<(String, String)> = set
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect();
            draws_powerline(move |name| {
                set.iter()
                    .find(|(known, _)| known == name)
                    .map(|(_, value)| value.clone())
            })
        };
        assert!(terminal(&[("KITTY_WINDOW_ID", "1")]));
        assert!(terminal(&[("TERM", "xterm-kitty")]));
        assert!(terminal(&[("TERM_PROGRAM", "WezTerm")]));
        assert!(terminal(&[("TERM_PROGRAM", "ghostty")]));
        assert!(terminal(&[("TERM", "xterm-ghostty")]));
        // These draw them only with a patched font or under some settings.
        for program in ["vscode", "iTerm.app", "Apple_Terminal", "tmux"] {
            assert!(!terminal(&[("TERM_PROGRAM", program)]), "{program}");
        }
        assert!(!terminal(&[("TERM", "xterm-256color")]));
        assert!(!terminal(&[]));
        // A multiplexer may be shown in another terminal than the one it was started in.
        assert!(!terminal(&[
            ("KITTY_WINDOW_ID", "1"),
            ("TMUX", "/tmp/tmux-501/default,1,0")
        ]));
        assert!(!terminal(&[
            ("TERM_PROGRAM", "WezTerm"),
            ("STY", "1234.pts-0.host")
        ]));
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
    fn the_running_line_shows_the_time_and_the_way_out() {
        assert_eq!(
            running(Duration::from_millis(12_900)),
            "\x1b[1;43;30m RUNNING \x1b[0m\x1b[1;100;97m 12s \x1b[0m \x1b[2mctrl+c stops waiting\x1b[0m"
        );
        // The longest line, an hour in, still fits the narrowest window the line is shown in.
        assert!(visible(&running(Duration::from_secs(3_599))).width() <= 48);
    }

    #[test]
    fn the_banner_names_the_agent_and_how_the_session_works() {
        let banner = banner(AGENT, "bash");
        assert_eq!(
            banner,
            "\x1b[1;100;97m golem ssh \x1b[0m \x1b[1;36mBashOwner(\"you\")\x1b[0m via bash\n\
             \x1b[2ma fresh shell per command \u{b7} only the directory carries over \u{b7} `help` for more\x1b[0m"
        );
        assert!(visible(&banner).lines().all(|line| line.width() <= 80));
    }

    #[test]
    fn the_notice_after_ctrl_c_says_what_the_command_does_now() {
        assert_eq!(
            detached(AGENT, "the tool's time limit", None),
            "\x1b[1;43;30m DETACHED \x1b[0m still running on BashOwner(\"you\")\n\
             \x1b[2mstops at the tool's time limit at the latest \u{b7} the next command waits behind it\x1b[0m"
        );
        let failed = detached(AGENT, "its time limit (30 s)", Some("no answer"));
        assert!(
            failed.ends_with("\n\x1b[2mthe cancel request failed: no answer\x1b[0m"),
            "{failed:?}"
        );
        assert!(visible(&failed).contains("stops at its time limit (30 s) at the latest"));
        assert_eq!(
            cancelled(),
            "\x1b[1;43;30m CANCELLED \x1b[0m before it started"
        );
    }
}
