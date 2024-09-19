use chrono::{DateTime, Timelike, Utc};

// See: https://github.com/golemcloud/golem/issues/939
pub trait TrimDateTime {
    fn trim_date_time_ms(self) -> Self;
}

impl<T: TrimDateTime> TrimDateTime for Option<T> {
    fn trim_date_time_ms(self) -> Self {
        self.map(|s| s.trim_date_time_ms())
    }
}

impl<T: TrimDateTime> TrimDateTime for Vec<T> {
    fn trim_date_time_ms(self) -> Self {
        self.into_iter().map(|s| s.trim_date_time_ms()).collect()
    }
}

impl TrimDateTime for DateTime<Utc> {
    fn trim_date_time_ms(self) -> Self {
        self.with_nanosecond(self.timestamp_subsec_millis() * 1_000_000)
            .expect("Failed to set nanoseconds while trimming")
    }
}
