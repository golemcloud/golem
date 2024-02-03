mod bindings;

use crate::bindings::Guest;

use std::time::*;
use std::thread::sleep;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

struct Component;

impl Guest for Component {
    fn run() -> (f64, f64, String) {
        let odt_now: OffsetDateTime = OffsetDateTime::now_utc();

        let time_epoch = SystemTime::UNIX_EPOCH;
        let time_now = SystemTime::now();
        let elapsed1 = time_now.duration_since(time_epoch).unwrap().as_secs_f64();

        let instant1 = Instant::now();
        sleep(Duration::from_secs(2));
        let elapsed2 = instant1.elapsed().as_secs_f64();
        (elapsed1, elapsed2, odt_now.format(&Rfc3339).unwrap())
    }

    fn sleep_for(seconds: f64) -> f64 {
        let instant1 = Instant::now();
        sleep(Duration::from_millis((seconds * 1000.0) as u64));
        let elapsed = instant1.elapsed().as_secs_f64();
        elapsed
    }
}
