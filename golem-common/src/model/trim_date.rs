// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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
