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

use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};

use super::SerializableDateTime;

impl From<wasmtime_wasi::p3::bindings::clocks::system_clock::Instant> for SerializableDateTime {
    fn from(value: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<SerializableDateTime> for wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
    fn from(value: SerializableDateTime) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime> for SerializableDateTime {
    fn from(value: wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime) -> Self {
        // The P2 `datetime` seconds field is unsigned; values beyond `i64::MAX`
        // (hundreds of billions of years from now) saturate instead of wrapping.
        Self {
            seconds: i64::try_from(value.seconds).unwrap_or(i64::MAX),
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<SerializableDateTime> for wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime {
    fn from(value: SerializableDateTime) -> Self {
        // The P2 `wall-clock` `datetime` type has unsigned seconds and cannot
        // represent pre-epoch instants, so those clamp to the epoch itself
        // (zeroing the nanoseconds too, otherwise the result would be *after*
        // the epoch and thus further from the original value).
        if value.seconds < 0 {
            Self {
                seconds: 0,
                nanoseconds: 0,
            }
        } else {
            Self {
                seconds: value.seconds as u64,
                nanoseconds: value.nanoseconds,
            }
        }
    }
}

impl From<SerializableDateTime> for SystemTime {
    fn from(value: SerializableDateTime) -> Self {
        // Pre-epoch instants are preserved: `seconds` is the floored second with
        // `nanoseconds` on top, so the offset before the epoch is
        // `|seconds| - nanoseconds`. The result is clamped to the Unix epoch only
        // if the platform's `SystemTime` cannot represent the instant at all
        // (e.g. pre-1601 dates on Windows).
        if value.seconds >= 0 {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::new(value.seconds as u64, value.nanoseconds))
                .unwrap_or(SystemTime::UNIX_EPOCH)
        } else {
            let before_epoch = Duration::new(value.seconds.unsigned_abs(), 0)
                .saturating_sub(Duration::new(0, value.nanoseconds));
            SystemTime::UNIX_EPOCH
                .checked_sub(before_epoch)
                .unwrap_or(SystemTime::UNIX_EPOCH)
        }
    }
}

impl From<SystemTime> for SerializableDateTime {
    fn from(value: SystemTime) -> Self {
        // Instants outside the representable `i64` second range saturate to the
        // nearest representable bound; everything else converts losslessly.
        match value.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(since_epoch) => match i64::try_from(since_epoch.as_secs()) {
                Ok(seconds) => Self {
                    seconds,
                    nanoseconds: since_epoch.subsec_nanos(),
                },
                Err(_) => Self {
                    seconds: i64::MAX,
                    nanoseconds: 999_999_999,
                },
            },
            Err(err) => {
                // Pre-epoch: represent as floored (negative) seconds plus a
                // non-negative nanosecond offset within that second. Computed in
                // i128 because the floored second of the earliest representable
                // instants would overflow i64 negation.
                let before_epoch = err.duration();
                let nanoseconds = before_epoch.subsec_nanos();
                let floored_seconds =
                    -i128::from(before_epoch.as_secs()) - i128::from(nanoseconds != 0);
                match i64::try_from(floored_seconds) {
                    Ok(seconds) => Self {
                        seconds,
                        nanoseconds: if nanoseconds == 0 {
                            0
                        } else {
                            1_000_000_000 - nanoseconds
                        },
                    },
                    Err(_) => Self {
                        seconds: i64::MIN,
                        nanoseconds: 0,
                    },
                }
            }
        }
    }
}

impl From<SerializableDateTime> for DateTime<Utc> {
    fn from(value: SerializableDateTime) -> Self {
        // Chrono cannot represent the full i64 second range (its bounds are
        // roughly ±262,000 years), so out-of-range values clamp to
        // `DateTime::<Utc>::MIN_UTC` / `MAX_UTC`.
        DateTime::<Utc>::from_timestamp(value.seconds, value.nanoseconds).unwrap_or({
            if value.seconds < 0 {
                DateTime::<Utc>::MIN_UTC
            } else {
                DateTime::<Utc>::MAX_UTC
            }
        })
    }
}

impl From<DateTime<Utc>> for SerializableDateTime {
    fn from(value: DateTime<Utc>) -> Self {
        // `timestamp()` is the floored second (negative for pre-epoch), matching
        // the `SerializableDateTime` representation directly. Chrono represents
        // leap seconds with a subsecond nano count >= 1e9; those clamp to the
        // last nanosecond of the preceding second.
        Self {
            seconds: value.timestamp(),
            nanoseconds: value.timestamp_subsec_nanos().min(999_999_999),
        }
    }
}
