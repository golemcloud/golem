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

//! The line shown while a command runs: the elapsed time and how to stop waiting, behind the
//! rune loader with colours and behind a spinner without. It is as long as the window allows.

use super::look::{self, Loader, Palette};

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

/// The columns the full line needs.
const WIDTH: u16 = 52;

/// The columns the short line needs. A narrower window would wrap it, and a wrapped line cannot
/// be erased, so there the indicator is not shown.
const SHORT_WIDTH: u16 = 24;

/// Erases the line the indicator was drawn on.
pub const ERASE: &str = "\r\x1b[2K";

/// One redraw of the line for a window `columns` wide: the full line, a short one, or none where
/// even that would wrap. It returns to the start of the line and clears what was there.
pub fn frame(
    tick: usize,
    elapsed: Duration,
    look: Option<(Palette, Loader)>,
    columns: u16,
) -> Option<String> {
    if columns < SHORT_WIDTH {
        return None;
    }
    let full = columns >= WIDTH;
    let line = match look {
        Some((palette, loader)) if full => look::running(tick, elapsed, loader, palette),
        Some((palette, loader)) => look::running_compact(tick, elapsed, loader, palette),
        None => {
            let spinner = FRAMES[tick % FRAMES.len()];
            let seconds = elapsed.as_secs();
            if full {
                format!("{spinner} running\u{2026} {seconds}s \u{b7} Ctrl+C to stop waiting")
            } else {
                format!("{spinner} {seconds}s \u{b7} Ctrl+C")
            }
        }
    };
    Some(format!("\r{line}\x1b[K"))
}

/// Draws the indicator from its own thread until dropped, then erases it.
pub struct Ticker {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Ticker {
    /// Starts drawing on stderr, as the loader when there are colours to draw it in. `columns`
    /// is asked for the window's width before every redraw, so the line follows a resize.
    pub fn start(
        look: Option<(Palette, Loader)>,
        columns: impl Fn() -> u16 + Send + 'static,
    ) -> Self {
        Self::start_on(std::io::stderr(), look, columns)
    }

    fn start_on(
        mut output: impl Write + Send + 'static,
        look: Option<(Palette, Loader)>,
        columns: impl Fn() -> u16 + Send + 'static,
    ) -> Self {
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let started = Instant::now();
            let mut tick = 0;
            // Whether a line is on the screen that has to be erased.
            let mut drawn = false;
            // Nothing is ever sent: the channel closing is the signal to stop.
            while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(TICK) {
                let elapsed = started.elapsed();
                if elapsed < DELAY {
                    continue;
                }
                match frame(tick, elapsed, look, columns()) {
                    Some(line) => {
                        let _ = output.write_all(line.as_bytes());
                        drawn = true;
                    }
                    // The window became too narrow for any line.
                    None if drawn => {
                        let _ = output.write_all(ERASE.as_bytes());
                        drawn = false;
                    }
                    None => {}
                }
                let _ = output.flush();
                tick += 1;
            }
            if drawn {
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
    use super::{ERASE, Ticker, frame};
    use crate::command_handler::ssh::look::{self, Loader, Palette};
    use std::io::Write;
    use std::sync::atomic::{AtomicU16, Ordering};
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
            frame(0, Duration::from_millis(12_900), None, 80).unwrap(),
            "\r\u{28fe} running\u{2026} 12s \u{b7} Ctrl+C to stop waiting\x1b[K"
        );
        // With colours it is the loader's line, redrawn in place like the spinner.
        let look = Some((Palette::Rich, Loader::Runes));
        assert_eq!(
            frame(9, Duration::from_secs(1), look, 80).unwrap(),
            format!(
                "\r{}\x1b[K",
                look::running(9, Duration::from_secs(1), Loader::Runes, Palette::Rich)
            )
        );
    }

    #[test]
    fn a_narrow_window_gets_a_shorter_line_and_a_very_narrow_one_none() {
        assert_eq!(
            frame(0, Duration::from_millis(12_900), None, 40).unwrap(),
            "\r\u{28fe} 12s \u{b7} Ctrl+C\x1b[K"
        );
        let look = Some((Palette::Rich, Loader::Runes));
        assert_eq!(
            frame(9, Duration::from_secs(1), look, 24).unwrap(),
            format!(
                "\r{}\x1b[K",
                look::running_compact(9, Duration::from_secs(1), Loader::Runes, Palette::Rich)
            )
        );
        // A line that wrapped could not be erased, so there it is not drawn.
        assert_eq!(frame(0, Duration::from_secs(1), None, 23), None);
        assert_eq!(frame(0, Duration::from_secs(1), look, 0), None);
    }

    #[test]
    fn every_line_fits_the_width_it_is_drawn_at() {
        // The longest lines: an hour, the most a command may run.
        for (columns, limit) in [(52, 52), (51, 24), (24, 24)] {
            let longest = frame(0, Duration::from_secs(3600), None, columns).unwrap();
            let visible = longest
                .trim_start_matches('\r')
                .trim_end_matches("\x1b[K")
                .chars()
                .count();
            assert!(visible <= limit, "{columns}: {visible}");
        }
    }

    #[test]
    fn a_quick_command_never_shows_the_line() {
        let screen = Screen::default();
        drop(Ticker::start_on(screen.clone(), None, || 80));
        assert_eq!(screen.text(), "");
    }

    #[test]
    fn a_slow_command_shows_the_line_and_erases_it_at_the_end() {
        let screen = Screen::default();
        let ticker = Ticker::start_on(screen.clone(), None, || 80);
        std::thread::sleep(Duration::from_millis(700));
        drop(ticker);
        let text = screen.text();
        assert!(
            text.starts_with("\r\u{28fe} running\u{2026} 0s"),
            "{text:?}"
        );
        assert!(text.ends_with(ERASE), "{text:?}");
    }

    #[test]
    fn the_line_follows_the_window_as_it_is_resized() {
        let screen = Screen::default();
        let columns = Arc::new(AtomicU16::new(80));
        let window = columns.clone();
        let ticker = Ticker::start_on(screen.clone(), None, move || window.load(Ordering::Relaxed));
        std::thread::sleep(Duration::from_millis(600));
        columns.store(30, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(400));
        columns.store(10, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(400));
        let narrow = screen.text();
        columns.store(80, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(400));
        drop(ticker);
        let text = screen.text();

        // Wide, then the short line, then nothing but the erasing of what was there.
        let short = text.find("s \u{b7} Ctrl+C\x1b[K").expect("the short line");
        assert!(text[..short].contains("Ctrl+C to stop waiting"), "{text:?}");
        assert!(narrow.ends_with(ERASE), "{narrow:?}");
        assert_eq!(narrow.matches(ERASE).count(), 1, "{narrow:?}");
        // Widened again, the full line is back.
        assert!(
            text[narrow.len()..].contains("Ctrl+C to stop waiting"),
            "{text:?}"
        );
        assert!(text.ends_with(ERASE), "{text:?}");
    }
}
