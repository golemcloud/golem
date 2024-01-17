use cap_std::time::SystemClock;
use cap_std::{ambient_authority, AmbientAuthority};
use cap_time_ext::SystemClockExt;
use wasmtime_wasi::preview2::HostMonotonicClock;

/// Using a SystemClock as a monotonic clock so instants are reusable between persisted executions
pub struct MonotonicClock {
    clock: cap_std::time::SystemClock,
}

impl MonotonicClock {
    pub fn new(ambient_authority: AmbientAuthority) -> Self {
        Self {
            clock: cap_std::time::SystemClock::new(ambient_authority),
        }
    }
}

impl HostMonotonicClock for MonotonicClock {
    fn resolution(&self) -> u64 {
        self.clock.resolution().as_nanos().try_into().unwrap()
    }

    fn now(&self) -> u64 {
        // Unwrap here and in `resolution` above; a `u64` is wide enough to
        // hold over 584 years of nanoseconds.
        self.clock
            .now()
            .duration_since(SystemClock::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .try_into()
            .unwrap()
    }
}

pub fn monotonic_clock() -> impl HostMonotonicClock + Send + Sync {
    MonotonicClock::new(ambient_authority())
}
