use chrono::{DateTime, Timelike, Utc};
use golem_cli::model::component::ComponentView;
use golem_cli::model::{WorkerMetadataView, WorkersMetadataResponseView};

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

impl TrimDateTime for ComponentView {
    fn trim_date_time_ms(self) -> Self {
        Self {
            created_at: self.created_at.trim_date_time_ms(),
            ..self
        }
    }
}

impl TrimDateTime for WorkerMetadataView {
    fn trim_date_time_ms(self) -> Self {
        Self {
            created_at: self.created_at.trim_date_time_ms(),
            ..self
        }
    }
}

impl TrimDateTime for WorkersMetadataResponseView {
    fn trim_date_time_ms(self) -> Self {
        Self {
            workers: self.workers.trim_date_time_ms(),
            ..self
        }
    }
}
