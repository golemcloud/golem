//! Use the same component task runtime as the SDK's tool imports.
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({ path: "wit", world: "shell-services", generate_all });
}

/// The longest wait the tool asks Golem's clock for at once. Golem suspends an agent whose timer
/// has `suspend_after` (10 s by default) or more to run, and the replay that resumes it can fail
/// to reproduce the call it was in; a wait made of shorter steps never leaves a call suspended.
const MAX_WAIT_STEP: Duration = Duration::from_secs(5);

#[cfg(target_arch = "wasm32")]
pub fn services() -> bash_shell::ExecutionServices {
    bash_shell::ExecutionServices {
        spawn_local: |future| {
            golem_rust::agentic::spawn_local(future);
        },
        yield_now: || Box::pin(wit_bindgen::yield_async()),
        sleep: |duration| Box::pin(sleep(duration)),
    }
}

/// Waits out `duration` on Golem's durable clock: one reading of the clock fixes where the wait
/// ends, then it waits until each of [`wait_marks`] in turn, so the steps add no drift.
#[cfg(target_arch = "wasm32")]
async fn sleep(duration: Duration) {
    use bindings::wasi::clocks::monotonic_clock;
    for mark in wait_marks(monotonic_clock::now(), duration) {
        monotonic_clock::wait_until(mark).await;
    }
}

/// The marks a wait of `duration` from `start` waits until: one per [`MAX_WAIT_STEP`] and the last
/// at the end. A zero wait still has one, so it stays a mark in the durable record.
fn wait_marks(start: u64, duration: Duration) -> impl Iterator<Item = u64> {
    let nanos = |duration: Duration| u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
    let end = start.saturating_add(nanos(duration));
    let step = nanos(MAX_WAIT_STEP);
    std::iter::successors(Some(start.saturating_add(step).min(end)), move |&mark| {
        (mark < end).then(|| mark.saturating_add(step).min(end))
    })
}

#[cfg(test)]
mod tests {
    use super::{MAX_WAIT_STEP, wait_marks};
    use std::time::Duration;

    const SECOND: u64 = 1_000_000_000;

    #[test]
    fn a_wait_is_made_of_steps_shorter_than_golems_suspension_threshold() {
        assert!(MAX_WAIT_STEP < Duration::from_secs(10));
        let marks = |seconds: f64| {
            wait_marks(7, Duration::from_secs_f64(seconds))
                .map(|mark| mark - 7)
                .collect::<Vec<_>>()
        };
        assert_eq!(marks(0.0), [0]);
        assert_eq!(marks(0.1), [SECOND / 10]);
        assert_eq!(marks(5.0), [5 * SECOND]);
        assert_eq!(marks(12.0), [5 * SECOND, 10 * SECOND, 12 * SECOND]);
        assert_eq!(marks(600.0).len(), 120);
        assert!(
            marks(600.0)
                .windows(2)
                .all(|pair| pair[1] - pair[0] == 5 * SECOND)
        );
    }

    #[test]
    fn a_wait_past_the_clocks_range_ends_at_its_limit() {
        let marks: Vec<_> = wait_marks(u64::MAX - 3 * SECOND, Duration::MAX).collect();
        assert_eq!(marks, [u64::MAX]);
        let marks: Vec<_> = wait_marks(u64::MAX - 12 * SECOND, Duration::from_secs(60)).collect();
        assert_eq!(marks.len(), 3);
        assert_eq!(marks.last(), Some(&u64::MAX));
    }
}
