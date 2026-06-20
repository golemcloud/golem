use golem_rust::{FromSchema, IntoSchema, agent_definition, agent_implementation};
use serde::{Deserialize, Serialize};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Clone, IntoSchema, FromSchema, Serialize, Deserialize)]
pub struct StdTimeApisResult {
    pub elapsed1: f64,
    pub elapsed2: f64,
    pub odt: String,
}

#[agent_definition]
pub trait Clocks {
    fn new(name: String) -> Self;

    fn use_std_time_apis(&self) -> StdTimeApisResult;
    fn sleep_for(&self, seconds: f64) -> f64;
    fn interruption(&self) -> String;
}

pub struct ClocksImpl {
    _name: String,
}

#[agent_implementation]
impl Clocks for ClocksImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn use_std_time_apis(&self) -> StdTimeApisResult {
        let odt_now: OffsetDateTime = OffsetDateTime::now_utc();

        let time_epoch = SystemTime::UNIX_EPOCH;
        let time_now = SystemTime::now();
        let elapsed1 = time_now.duration_since(time_epoch).unwrap().as_secs_f64();

        let instant1 = Instant::now();
        sleep(Duration::from_secs(2));
        let elapsed2 = instant1.elapsed().as_secs_f64();
        StdTimeApisResult {
            elapsed1,
            elapsed2,
            odt: odt_now.format(&Rfc3339).unwrap(),
        }
    }

    fn sleep_for(&self, seconds: f64) -> f64 {
        let instant1 = Instant::now();
        sleep(Duration::from_millis((seconds * 1000.0) as u64));
        let elapsed = instant1.elapsed().as_secs_f64();
        elapsed
    }

    fn interruption(&self) -> String {
        println!("Starting interruption test");
        for _ in 0..100 {
            sleep(Duration::from_millis(100));
        }

        "done".to_string()
    }
}

/// Stateful agent used to prove that `wasi:clocks/monotonic_clock.now` replays to an identical
/// value. `record_now` captures a single monotonic reading into agent state; after a
/// crash/restart the invocation is replayed, so the rebuilt state must equal the live reading,
/// which `get_recorded` reads back.
#[agent_definition]
pub trait MonotonicClockState {
    fn new(name: String) -> Self;

    /// Captures the current monotonic clock reading once and returns it. Subsequent calls return
    /// the same first reading.
    fn record_now(&mut self) -> u64;

    /// Returns the previously recorded monotonic clock reading.
    fn get_recorded(&self) -> u64;
}

pub struct MonotonicClockStateImpl {
    _name: String,
    recorded: Option<u64>,
}

#[agent_implementation]
impl MonotonicClockState for MonotonicClockStateImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            recorded: None,
        }
    }

    fn record_now(&mut self) -> u64 {
        let now = wasi::clocks::monotonic_clock::now();
        *self.recorded.get_or_insert(now)
    }

    fn get_recorded(&self) -> u64 {
        self.recorded
            .expect("record_now must be called before get_recorded")
    }
}
