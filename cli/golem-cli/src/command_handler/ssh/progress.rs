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

//! The line shown while a command runs: the elapsed time and how to stop waiting, as a block
//! line with colours and behind a spinner without.

use super::look;

use std::io::Write;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const FRAMES: [&str; 8] = [
    "\u{28fe}", "\u{28fd}", "\u{28fb}", "\u{28bf}", "\u{287f}", "\u{28df}", "\u{28ef}", "\u{28f7}",
];

/// Commands that finish sooner never show the line.
const DELAY: Duration = Duration::from_millis(350);
const TICK: Duration = Duration::from_millis(110);

/// The columns the line needs. A narrower terminal would wrap it, and a wrapped line cannot be
/// erased, so there the indicator is not shown.
const WIDTH: usize = 48;

/// Whether a terminal `columns` wide can show the indicator on one line.
pub fn fits(columns: u16) -> bool {
    usize::from(columns) >= WIDTH
}

/// Erases the line the indicator was drawn on.
pub const ERASE: &str = "\r\x1b[2K";

/// One redraw of the line. It returns to the start of the line and clears what was there.
pub fn frame(tick: usize, elapsed: Duration, colorize: bool) -> String {
    if colorize {
        return format!("\r{}\x1b[K", look::running(elapsed));
    }
    let spinner = FRAMES[tick % FRAMES.len()];
    format!(
        "\r{spinner} running\u{2026} {}s \u{b7} Ctrl+C to stop waiting\x1b[K",
        elapsed.as_secs()
    )
}

/// Draws the indicator from its own thread until dropped, then erases it.
pub struct Ticker {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Ticker {
    /// Starts drawing on stderr.
    pub fn start(colorize: bool) -> Self {
        Self::start_on(std::io::stderr(), colorize)
    }

    fn start_on(mut output: impl Write + Send + 'static, colorize: bool) -> Self {
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let started = Instant::now();
            let mut tick = 0;
            // Nothing is ever sent: the channel closing is the signal to stop.
            while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(TICK) {
                let elapsed = started.elapsed();
                if elapsed < DELAY {
                    continue;
                }
                let _ = output.write_all(frame(tick, elapsed, colorize).as_bytes());
                let _ = output.flush();
                tick += 1;
            }
            if tick > 0 {
                let _ = output.write_all(ERASE.as_bytes());
                let _ = output.flush();
            }
        });
        Self {
            stop: Some(stop),
            thread: Some(thread),
        }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ERASE, Ticker, fits, frame};
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use test_r::test;

    #[derive(Clone, Default)]
    struct Screen(Arc<Mutex<Vec<u8>>>);

    impl Screen {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for Screen {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_frame_shows_the_spinner_the_seconds_and_the_way_out() {
        assert_eq!(
            frame(0, Duration::from_millis(12_900), false),
            "\r\u{28fe} running\u{2026} 12s \u{b7} Ctrl+C to stop waiting\x1b[K"
        );
        // With colours it is the block look's line.
        assert_eq!(
            frame(9, Duration::from_secs(1), true),
            "\r\x1b[1;43;30m RUNNING \x1b[0m\x1b[1;100;97m 1.0s \x1b[0m \x1b[2mctrl+c stops waiting\x1b[0m\x1b[K"
        );
    }

    #[test]
    fn the_line_fits_the_width_it_asks_for() {
        assert!(fits(80) && fits(48));
        assert!(!fits(47));
        // The longest line: an hour, the most a command may run.
        let longest = frame(0, Duration::from_secs(3600), false);
        let visible = longest
            .trim_start_matches('\r')
            .trim_end_matches("\x1b[K")
            .chars()
            .count();
        assert!(visible <= 48, "{visible}");
    }

    #[test]
    fn a_quick_command_never_shows_the_line() {
        let screen = Screen::default();
        drop(Ticker::start_on(screen.clone(), false));
        assert_eq!(screen.text(), "");
    }

    #[test]
    fn a_slow_command_shows_the_line_and_erases_it_at_the_end() {
        let screen = Screen::default();
        let ticker = Ticker::start_on(screen.clone(), false);
        std::thread::sleep(Duration::from_millis(700));
        drop(ticker);
        let text = screen.text();
        assert!(
            text.starts_with("\r\u{28fe} running\u{2026} 0s"),
            "{text:?}"
        );
        assert!(text.ends_with(ERASE), "{text:?}");
    }
}
