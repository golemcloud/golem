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
//! background is; nothing about the terminal is changed, so there is nothing to put back.

use super::look;
use std::io::Write;

/// Set to `0`, it leaves the prompts without a band.
const VARIABLE: &str = "GOLEM_SSH_BACKGROUND";

/// The SGR parameters of the band, or `None` when `GOLEM_SSH_BACKGROUND` says no or the
/// terminal does not say what its background is. `var` reads an environment variable.
pub fn band(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    if !wanted(&var) {
        return None;
    }
    let background = look::parse_background(&ask()?)?;
    let truecolor = var("COLORTERM").is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "truecolor" | "24bit"
        )
    });
    Some(look::band(background, truecolor))
}

fn wanted(var: &impl Fn(&str) -> Option<String>) -> bool {
    !var(VARIABLE).is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

/// Asks the terminal for its background and collects what it answers. It waits a fifth of a
/// second at most, and less when the terminal answers the second question asked, which every
/// terminal understands.
#[cfg(unix)]
fn ask() -> Option<Vec<u8>> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use std::time::{Duration, Instant};

    const WAIT: Duration = Duration::from_millis(200);

    // Without raw mode the answer would wait for Enter and be echoed.
    crossterm::terminal::enable_raw_mode().ok()?;
    {
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(look::BACKGROUND_QUERY.as_bytes());
        let _ = stderr.flush();
    }
    let stdin = std::io::stdin();
    let deadline = Instant::now() + WAIT;
    let mut reply = Vec::new();
    while !look::answered(&reply) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: left.subsec_nanos().into(),
        };
        let mut waiting = [PollFd::new(&stdin, PollFlags::IN)];
        if !poll(&mut waiting, Some(&timeout)).is_ok_and(|ready| ready > 0) {
            break;
        }
        let mut buffer = [0u8; 64];
        match rustix::io::read(&stdin, &mut buffer[..]) {
            Ok(read) if read > 0 => reply.extend_from_slice(&buffer[..read]),
            _ => break,
        }
    }
    let _ = crossterm::terminal::disable_raw_mode();
    Some(reply)
}

/// Only terminals on Unix are asked.
#[cfg(not(unix))]
fn ask() -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::wanted;
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
}
