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

//! The band behind each prompt of a session, a shade off the terminal's own background, which
//! sets the session apart from the shell it was started in. The terminal is asked what its
//! background is while the session connects, so a slow terminal has that long to answer. Nothing
//! about how the terminal looks is changed.
//!
//! An answer that arrives after the session has stopped waiting goes to the line editor, whose
//! edit mode takes it out of the line. The prompts then have no band. An answer that the end
//! of the wait cut in two is told to the editor, which has to know its start.

use super::look;
use std::io::Write;
use std::time::{Duration, Instant};

/// Set to `0`, it leaves the prompts without a band.
const VARIABLE: &str = "GOLEM_SSH_BACKGROUND";

/// The least the terminal is given to answer.
const WAIT: Duration = Duration::from_millis(500);

/// What came from the terminal while the question was open.
#[derive(Default)]
pub struct Answer {
    /// The SGR parameters of the band, when the terminal said what its background is.
    pub band: Option<String>,
    /// What was typed meanwhile, which would otherwise be lost.
    pub typed_ahead: String,
    /// The start of an answer that the end of the wait cut off. Its rest goes to the line
    /// editor, which has to know what came before to tell it from typed text.
    pub unfinished: Vec<u8>,
}

/// The question put to the terminal, open until its answer is collected.
pub struct Query {
    asked: Instant,
    truecolor: bool,
    /// The terminal's input modes as they were, to put back.
    #[cfg(unix)]
    modes: Option<rustix::termios::Termios>,
}

impl Query {
    /// Asks the terminal for its background, unless `GOLEM_SSH_BACKGROUND` says no. `var` reads
    /// an environment variable. Only terminals on Unix are asked.
    ///
    /// While the question is open the terminal neither echoes nor waits for Enter, so that its
    /// answer is not shown and can be read as it arrives. Ctrl+C and everything written to the
    /// terminal work as usual.
    pub fn send(var: impl Fn(&str) -> Option<String>) -> Option<Self> {
        if !wanted(&var) {
            return None;
        }
        let truecolor = var("COLORTERM").is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "truecolor" | "24bit"
            )
        });
        Self::ask(truecolor)
    }

    #[cfg(unix)]
    fn ask(truecolor: bool) -> Option<Self> {
        use rustix::termios::{
            LocalModes, OptionalActions, SpecialCodeIndex, tcgetattr, tcsetattr,
        };

        let stdin = std::io::stdin();
        let modes = tcgetattr(&stdin).ok()?;
        let mut quiet = modes.clone();
        quiet
            .local_modes
            .remove(LocalModes::ICANON | LocalModes::ECHO);
        quiet.special_codes[SpecialCodeIndex::VMIN] = 1;
        quiet.special_codes[SpecialCodeIndex::VTIME] = 0;
        tcsetattr(&stdin, OptionalActions::Now, &quiet).ok()?;
        {
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(look::BACKGROUND_QUERY.as_bytes());
            let _ = stderr.flush();
        }
        Some(Self {
            asked: Instant::now(),
            truecolor,
            modes: Some(modes),
        })
    }

    #[cfg(not(unix))]
    fn ask(_truecolor: bool) -> Option<Self> {
        None
    }

    /// Collects the answer and puts the terminal's input modes back.
    pub fn finish(mut self) -> Answer {
        let bytes = self.close();
        Answer {
            band: look::parse_background(&bytes)
                .map(|background| look::band(background, self.truecolor)),
            typed_ahead: typed_ahead(&bytes),
            unfinished: unfinished(&bytes),
        }
    }

    /// Reads what the terminal sent since the question, waiting for the rest of the answer at
    /// most until the question is [`WAIT`] old, and puts the input modes back.
    #[cfg(unix)]
    fn close(&mut self) -> Vec<u8> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        use rustix::termios::{OptionalActions, tcsetattr};

        let Some(modes) = self.modes.take() else {
            return Vec::new();
        };
        let stdin = std::io::stdin();
        let deadline = self.asked + WAIT;
        let mut bytes = Vec::new();
        loop {
            // With the answer complete, only what has already arrived is still taken.
            let left = if look::answered(&bytes) {
                Duration::ZERO
            } else {
                deadline.saturating_duration_since(Instant::now())
            };
            let timeout = Timespec {
                tv_sec: 0,
                tv_nsec: left.subsec_nanos().into(),
            };
            let mut waiting = [PollFd::new(&stdin, PollFlags::IN)];
            if !poll(&mut waiting, Some(&timeout)).is_ok_and(|ready| ready > 0) {
                break;
            }
            let mut buffer = [0u8; 256];
            match rustix::io::read(&stdin, &mut buffer[..]) {
                Ok(read) if read > 0 => bytes.extend_from_slice(&buffer[..read]),
                _ => break,
            }
        }
        let _ = tcsetattr(&stdin, OptionalActions::Now, &modes);
        bytes
    }

    #[cfg(not(unix))]
    fn close(&mut self) -> Vec<u8> {
        Vec::new()
    }
}

impl Drop for Query {
    /// A session that ends before its first prompt still takes the answer off the terminal, so
    /// that it does not arrive at the shell as if typed.
    fn drop(&mut self) {
        self.close();
    }
}

/// The start of an answer at the end of `bytes`, when the answer is cut off there.
fn unfinished(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == 0x1b)
        .map(|(at, _)| &bytes[at..])
        .find(|rest| look::arrived(rest) == look::Arrived::Part)
        .unwrap_or_default()
        .to_vec()
}

fn wanted(var: &impl Fn(&str) -> Option<String>) -> bool {
    !var(VARIABLE).is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

/// What a person typed among `bytes` read from the terminal: the first line of it, without the
/// terminal's own answers (`ESC ]` up to BEL or `ESC \`, and `ESC [` up to its final byte) and
/// without control characters. It is offered back at the first prompt and not run.
fn typed_ahead(bytes: &[u8]) -> String {
    let mut typed = Vec::new();
    let mut rest = bytes;
    while let Some((&byte, after)) = rest.split_first() {
        rest = after;
        if byte != 0x1b {
            typed.push(byte);
            continue;
        }
        match rest.first() {
            Some(b']') => {
                let end = rest
                    .iter()
                    .position(|&byte| byte == 0x07)
                    .map(|at| at + 1)
                    .or_else(|| {
                        rest.windows(2)
                            .position(|pair| pair == b"\x1b\\")
                            .map(|at| at + 2)
                    })
                    .unwrap_or(rest.len());
                rest = &rest[end..];
            }
            Some(b'[') => {
                let end = rest[1..]
                    .iter()
                    .position(|byte| (0x40..=0x7e).contains(byte))
                    .map_or(rest.len(), |at| at + 2);
                rest = &rest[end..];
            }
            _ => {}
        }
    }
    String::from_utf8_lossy(&typed)
        .split(['\r', '\n'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{typed_ahead, unfinished, wanted};
    use test_r::test;

    #[test]
    fn the_band_is_on_unless_the_variable_says_no() {
        let with = |value: Option<&str>| {
            let value = value.map(str::to_string);
            wanted(&move |name: &str| {
                (name == "GOLEM_SSH_BACKGROUND")
                    .then(|| value.clone())
                    .flatten()
            })
        };
        assert!(with(None));
        assert!(with(Some("1")));
        assert!(with(Some("")));
        for no in ["0", "false", "no", "off", " Off "] {
            assert!(!with(Some(no)), "{no:?}");
        }
    }

    #[test]
    fn the_start_of_an_answer_that_the_wait_cut_off_is_kept() {
        for (bytes, start) in [
            // The escape that opens the answer came in time, and nothing after it.
            (&b"\x1b]"[..], &b"\x1b]"[..]),
            (b"\x1b", b"\x1b"),
            // Something was typed before it.
            (b"ls\x1b]11;rgb:14", b"\x1b]11;rgb:14"),
            // Only the first half of what ends it came.
            (b"\x1b]11;rgb:f/f/f\x1b", b"\x1b]11;rgb:f/f/f\x1b"),
            // The background came whole, the device attributes did not.
            (b"\x1b]11;rgb:f/f/f\x07\x1b[?6", b"\x1b[?6"),
        ] {
            assert_eq!(unfinished(bytes), start, "{bytes:?}");
        }
        for whole in [
            &b""[..],
            b"ls",
            b"\x1b]11;rgb:1414/1313/1b1b\x07",
            b"\x1b]11;rgb:1414/1313/1b1b\x07\x1b[?62;c",
            b"\x1b[?62;c",
            // An arrow key, and something typed after it.
            b"a\x1b[Ab",
        ] {
            assert_eq!(unfinished(whole), b"", "{whole:?}");
        }
    }

    #[test]
    fn what_was_typed_is_told_apart_from_the_terminals_answers() {
        let answers = b"\x1b]11;rgb:1414/1313/1b1b\x07\x1b[?62;c";
        assert_eq!(typed_ahead(answers), "");
        assert_eq!(typed_ahead(b""), "");

        // Typed before, between and after the answers.
        let mut mixed = b"ec".to_vec();
        mixed.extend_from_slice(b"\x1b]11;rgb:0000/0000/0000\x1b\\");
        mixed.extend_from_slice(b"ho ");
        mixed.extend_from_slice(b"\x1b[?1;2c");
        mixed.extend_from_slice("caf\u{e9}".as_bytes());
        assert_eq!(typed_ahead(&mixed), "echo caf\u{e9}");

        // Only the first line is offered back, and it is not run.
        assert_eq!(typed_ahead(b"ls -la\rrm -rf x\r"), "ls -la");
        // An arrow key or a stray control character types nothing.
        assert_eq!(typed_ahead(b"a\x1b[Ab\x03c\x7f"), "abc");
        // An answer cut short takes the rest with it and leaves nothing typed by mistake.
        assert_eq!(typed_ahead(b"x\x1b]11;rgb:14"), "x");
    }
}
