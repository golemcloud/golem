use golem_rust::{agent_definition, agent_implementation};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[agent_definition]
pub trait Clocks {
    fn new(name: String) -> Self;

    fn use_std_time_apis(&self) -> (f64, f64, String);
    fn sleep_for(&self, seconds: f64) -> f64;
}

pub struct ClocksImpl {
    _name: String,
}

#[agent_implementation]
impl Clocks for ClocksImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn use_std_time_apis(&self) -> (f64, f64, String) {
        let odt_now: OffsetDateTime = OffsetDateTime::now_utc();

        let time_epoch = SystemTime::UNIX_EPOCH;
        let time_now = SystemTime::now();
        let elapsed1 = time_now.duration_since(time_epoch).unwrap().as_secs_f64();

        let instant1 = Instant::now();
        sleep(Duration::from_secs(2));
        let elapsed2 = instant1.elapsed().as_secs_f64();
        (elapsed1, elapsed2, odt_now.format(&Rfc3339).unwrap())
    }

    fn sleep_for(&self, seconds: f64) -> f64 {
        let instant1 = Instant::now();
        sleep(Duration::from_millis((seconds * 1000.0) as u64));
        let elapsed = instant1.elapsed().as_secs_f64();
        elapsed
    }
}
