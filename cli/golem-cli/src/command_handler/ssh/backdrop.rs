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
//! The answer is read here, where it still has its framing. One that arrives later goes to the
//! line editor, which takes it for typed text, so a terminal that has not answered when the
//! session has connected is waited for. The answer of a terminal slower than the longest wait
//! shows on the line being typed and stays there: nothing typed is ever changed.

use super::look;
use std::io::Write;
use std::time::{Duration, Instant};

/// Set to `0`, it leaves the prompts without a band.
const VARIABLE: &str = "GOLEM_SSH_BACKGROUND";

/// The least the terminal is given to answer.
const WAIT: Duration = Duration::from_millis(500);

/// The most the terminal is given, when it has not said what its background is.
const LONGEST_WAIT: Duration = Duration::from_secs(3);

/// What came from the terminal while the question was open.
#[derive(Default)]
pub struct Answer {
    /// The SGR parameters of the band, when the terminal said what its background is.
    pub band: Option<String>,
    /// What was typed meanwhile, which would otherwise be lost.
    pub typed_ahead: String,
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
        }
    }

    /// Reads what the terminal sent since the question, waiting for the rest of the answer for
    /// as long as [`wait_left`] gives it, and puts the input modes back.
    #[cfg(unix)]
    fn close(&mut self) -> Vec<u8> {
        use rustix::termios::{OptionalActions, tcsetattr};

        let Some(modes) = self.modes.take() else {
            return Vec::new();
        };
        let stdin = std::io::stdin();
        let bytes = read_answers(&stdin, self.asked);
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

/// Reads from `input` what the terminal sent since it was asked at `asked`, for as long as
/// [`wait_left`] gives it.
#[cfg(unix)]
fn read_answers(input: impl std::os::fd::AsFd, asked: Instant) -> Vec<u8> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};

    let mut bytes = Vec::new();
    while let Ok(timeout) = Timespec::try_from(wait_left(&bytes, asked.elapsed())) {
        let mut waiting = [PollFd::new(&input, PollFlags::IN)];
        if !poll(&mut waiting, Some(&timeout)).is_ok_and(|ready| ready > 0) {
            break;
        }
        let mut buffer = [0u8; 256];
        match rustix::io::read(&input, &mut buffer[..]) {
            Ok(read) if read > 0 => bytes.extend_from_slice(&buffer[..read]),
            _ => break,
        }
    }
    bytes
}

/// How much longer the terminal is given to answer, with `bytes` read from it on a question
/// `age` old. With the answer complete, only what has already arrived is still taken.
///
/// What a terminal sends after the wait goes to the line editor. The editor drops a late answer
/// about the device attributes, but one about the background it takes for typed text. So a
/// terminal that has said what its background is has [`WAIT`] for the rest, and any other has
/// [`LONGEST_WAIT`].
fn wait_left(bytes: &[u8], age: Duration) -> Duration {
    let wait = if look::answered(bytes) {
        Duration::ZERO
    } else if background_is_in(bytes) {
        WAIT
    } else {
        LONGEST_WAIT
    };
    wait.saturating_sub(age)
}

/// Whether the terminal's answer about its background is in `bytes` up to its end, which is BEL
/// or `ESC \`.
fn background_is_in(bytes: &[u8]) -> bool {
    const OPENING: &[u8] = b"\x1b]11;";
    bytes
        .windows(OPENING.len())
        .position(|window| window == OPENING)
        .is_some_and(|at| {
            let answer = &bytes[at + OPENING.len()..];
            answer.contains(&0x07) || answer.windows(2).any(|pair| pair == b"\x1b\\")
        })
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
    use super::{LONGEST_WAIT, WAIT, typed_ahead, wait_left, wanted};
    use std::time::Duration;
    use test_r::test;

    #[test]
    fn a_terminal_that_has_answered_is_not_waited_for() {
        // The device attributes are answered last, with or without a colour before them.
        for answers in [
            &b"\x1b[?62;c"[..],
            b"ls\x1b[?1;2c",
            b"\x1b]11;rgb:1414/1313/1b1b\x07\x1b[?62;c",
        ] {
            assert_eq!(wait_left(answers, Duration::ZERO), Duration::ZERO);
        }
    }

    #[test]
    fn a_terminal_that_has_not_said_its_background_is_given_the_longest_wait() {
        let past_the_least = WAIT + Duration::from_millis(100);
        // Nothing yet, only typed text, or an answer cut off: what is still to come would reach
        // the line editor, which takes it for typed text.
        for bytes in [
            &b""[..],
            b"ls",
            b"\x1b]11;rgb:14",
            b"\x1b]11;rgb:1414/1313/1b1b",
        ] {
            assert_eq!(wait_left(bytes, Duration::ZERO), LONGEST_WAIT);
            assert_eq!(
                wait_left(bytes, past_the_least),
                LONGEST_WAIT - past_the_least
            );
            assert_eq!(wait_left(bytes, LONGEST_WAIT), Duration::ZERO);
            assert_eq!(wait_left(bytes, LONGEST_WAIT * 2), Duration::ZERO);
        }
    }

    #[test]
    fn a_terminal_that_said_its_background_has_the_least_wait_for_the_rest() {
        let early = Duration::from_millis(100);
        // Only the device attributes are still to come, and the line editor does not take those
        // for typed text. The answer ends with BEL or with `ESC \`, and it is in also when it
        // names no colour that can be read.
        for background in [
            &b"\x1b]11;rgb:1414/1313/1b1b\x07"[..],
            b"\x1b]11;rgb:1414/1313/1b1b\x1b\\",
            b"ls\x1b]11;rgb:f/f/f\x07",
            b"\x1b]11;#141313\x07",
        ] {
            assert_eq!(wait_left(background, early), WAIT - early);
            assert_eq!(wait_left(background, WAIT), Duration::ZERO);
            assert_eq!(wait_left(background, LONGEST_WAIT), Duration::ZERO);
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_answer_that_comes_after_the_least_wait_is_read_whole() {
        use super::{look, read_answers};
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        use std::time::Instant;

        let (mut terminal, input) = UnixStream::pair().unwrap();
        // The session took longer to connect than the least wait, and the terminal has still
        // more than a second to answer.
        let age = WAIT + Duration::from_millis(400);
        let asked = Instant::now().checked_sub(age).unwrap();
        let terminal = std::thread::spawn(move || {
            // Something is typed, and then the terminal answers.
            std::thread::sleep(Duration::from_millis(200));
            terminal.write_all(b"ls").unwrap();
            std::thread::sleep(Duration::from_millis(100));
            terminal
                .write_all(b"\x1b]11;rgb:1414/1313/1b1b\x07\x1b[?62;c")
                .unwrap();
            // It stays open, as a terminal does.
            terminal
        });
        let bytes = read_answers(&input, asked);
        let waited = asked.elapsed();
        assert_eq!(
            look::parse_background(&bytes),
            Some(look::Rgb(0x14, 0x13, 0x1b))
        );
        assert_eq!(typed_ahead(&bytes), "ls");
        // The answer ends the wait.
        assert!(waited < LONGEST_WAIT, "{waited:?}");
        drop(terminal.join().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn a_terminal_that_answers_nothing_is_waited_for_until_the_longest_wait() {
        use super::read_answers;
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        use std::time::Instant;

        let (mut terminal, input) = UnixStream::pair().unwrap();
        terminal.write_all(b"ls").unwrap();
        // The question is nearly as old as the longest wait.
        let left = Duration::from_millis(200);
        let asked = Instant::now().checked_sub(LONGEST_WAIT - left).unwrap();
        let started = Instant::now();
        let bytes = read_answers(&input, asked);
        let waited = started.elapsed();
        assert_eq!(bytes, b"ls");
        assert!(waited >= left - Duration::from_millis(50), "{waited:?}");
        assert!(waited < left + Duration::from_secs(1), "{waited:?}");
    }

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
