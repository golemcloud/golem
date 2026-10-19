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

use chrono::{DateTime, Datelike, TimeZone, Utc};
use golem_common::model::account_usage::AccountUsagePeriod;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MeteringTime {
    pub(crate) instant: Instant,
    pub(crate) utc: DateTime<Utc>,
}

impl MeteringTime {
    pub(crate) fn now() -> Self {
        let before = Instant::now();
        let utc = Utc::now();
        let after = Instant::now();
        Self::between(before, utc, after)
    }

    /// Associates a wall-clock sample with the midpoint of its monotonic sampling bracket.
    /// The pairing uncertainty is at most half the bracket width; the clocks are not atomic.
    pub(crate) fn between(before: Instant, utc: DateTime<Utc>, after: Instant) -> Self {
        let uncertainty = after.saturating_duration_since(before);
        Self {
            instant: before + uncertainty / 2,
            utc,
        }
    }

    pub(crate) fn at_instant(self, instant: Instant) -> Self {
        let utc = if instant >= self.instant {
            self.utc
                + chrono::Duration::from_std(instant.duration_since(self.instant))
                    .unwrap_or(chrono::Duration::MAX)
        } else {
            self.utc
                - chrono::Duration::from_std(self.instant.duration_since(instant))
                    .unwrap_or(chrono::Duration::MAX)
        };
        Self { instant, utc }
    }
}

#[derive(Debug)]
pub(crate) struct ByteTimeAccumulator {
    byte_nanoseconds_per_unit: u128,
    last_sample: MeteringTime,
    pending_byte_nanoseconds: BTreeMap<AccountUsagePeriod, u128>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ByteTimeSettlement {
    pub(crate) units: u128,
    pub(crate) remainder: u128,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PeriodByteTimeSettlement {
    pub(crate) period: AccountUsagePeriod,
    pub(crate) usage: ByteTimeSettlement,
}

impl ByteTimeAccumulator {
    pub(crate) fn new(byte_nanoseconds_per_unit: u128, now: MeteringTime) -> Self {
        assert!(byte_nanoseconds_per_unit != 0);
        Self {
            byte_nanoseconds_per_unit,
            last_sample: now,
            pending_byte_nanoseconds: BTreeMap::new(),
        }
    }

    pub(crate) fn accrue(&mut self, now: MeteringTime, bytes: Option<u64>) {
        self.advance(now, bytes);
    }

    pub(crate) fn advance(&mut self, now: MeteringTime, bytes: Option<u64>) -> bool {
        if now.instant <= self.last_sample.instant {
            return false;
        }

        let elapsed = now
            .instant
            .saturating_duration_since(self.last_sample.instant)
            .as_nanos();
        let last_sample = self.last_sample;
        self.last_sample = now;
        if let Some(bytes) = bytes {
            self.accrue_by_period(last_sample, now, bytes, elapsed);
        }
        true
    }

    fn accrue_by_period(
        &mut self,
        start: MeteringTime,
        end: MeteringTime,
        bytes: u64,
        elapsed: u128,
    ) {
        let start_period = period_at(start.utc);
        let mut period = start_period;
        let mut assigned = 0;
        loop {
            let boundary = next_period_start(period);
            if boundary >= end.utc {
                break;
            }
            let boundary_offset = (boundary - start.utc)
                .to_std()
                .unwrap_or(Duration::ZERO)
                .as_nanos()
                .min(elapsed);
            self.add_byte_nanoseconds(period, bytes, boundary_offset.saturating_sub(assigned));
            assigned = boundary_offset;
            period = period_at(boundary);
        }
        self.add_byte_nanoseconds(period, bytes, elapsed.saturating_sub(assigned));
    }

    fn add_byte_nanoseconds(&mut self, period: AccountUsagePeriod, bytes: u64, elapsed: u128) {
        let pending = self.pending_byte_nanoseconds.entry(period).or_default();
        *pending = pending.saturating_add(u128::from(bytes).saturating_mul(elapsed));
    }

    #[cfg(test)]
    pub(crate) fn take_units(&mut self) -> i64 {
        let Some(pending) = self.pending_byte_nanoseconds.values_mut().next() else {
            return 0;
        };
        let units = (*pending / self.byte_nanoseconds_per_unit).min(i64::MAX as u128) as i64;
        *pending -= (units as u128).saturating_mul(self.byte_nanoseconds_per_unit);
        units
    }

    pub(crate) fn take_settlements(&mut self) -> Vec<PeriodByteTimeSettlement> {
        std::mem::take(&mut self.pending_byte_nanoseconds)
            .into_iter()
            .map(|(period, byte_nanoseconds)| PeriodByteTimeSettlement {
                period,
                usage: ByteTimeSettlement {
                    units: byte_nanoseconds / self.byte_nanoseconds_per_unit,
                    remainder: byte_nanoseconds % self.byte_nanoseconds_per_unit,
                },
            })
            .collect()
    }
}

fn period_at(at: DateTime<Utc>) -> AccountUsagePeriod {
    AccountUsagePeriod {
        year: at.year(),
        month: at.month(),
    }
}

fn next_period_start(period: AccountUsagePeriod) -> DateTime<Utc> {
    let (year, month) = if period.month == 12 {
        (period.year + 1, 1)
    } else {
        (period.year, period.month + 1)
    };
    Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .single()
        .expect("account usage period must have a valid successor")
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn at(instant: Instant, utc: DateTime<Utc>) -> MeteringTime {
        MeteringTime { instant, utc }
    }

    fn settlement(year: i32, month: u32, units: u128, remainder: u128) -> PeriodByteTimeSettlement {
        PeriodByteTimeSettlement {
            period: AccountUsagePeriod { year, month },
            usage: ByteTimeSettlement { units, remainder },
        }
    }

    #[test]
    fn units_above_the_batch_range_remain_pending() {
        let now = Instant::now();
        let utc = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).single().unwrap();
        let mut accumulator = ByteTimeAccumulator::new(1, at(now, utc));
        accumulator.accrue(
            at(
                now + Duration::from_nanos(2),
                utc + chrono::Duration::nanoseconds(2),
            ),
            Some(u64::MAX),
        );

        assert_eq!(accumulator.take_units(), i64::MAX);
        assert_eq!(accumulator.take_units(), i64::MAX);
        assert_eq!(accumulator.take_units(), i64::MAX);
        assert_eq!(accumulator.take_units(), i64::MAX);
        assert_eq!(accumulator.take_units(), 2);
        assert_eq!(accumulator.take_units(), 0);
    }

    #[test]
    fn settlements_split_exactly_at_utc_month_boundaries() {
        let now = Instant::now();
        let boundary = Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).single().unwrap();
        let mut accumulator =
            ByteTimeAccumulator::new(10, at(now, boundary - chrono::Duration::nanoseconds(2)));

        accumulator.accrue(
            at(
                now + Duration::from_nanos(7),
                boundary + chrono::Duration::nanoseconds(5),
            ),
            Some(3),
        );

        assert_eq!(
            accumulator.take_settlements(),
            vec![settlement(2026, 12, 0, 6), settlement(2027, 1, 1, 5),]
        );
    }

    #[test]
    fn interval_ending_at_utc_month_boundary_emits_only_the_old_period() {
        let now = Instant::now();
        let boundary = Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).single().unwrap();
        let elapsed = Duration::from_nanos(2);
        let mut accumulator =
            ByteTimeAccumulator::new(10, at(now, boundary - chrono::Duration::nanoseconds(2)));

        accumulator.accrue(at(now + elapsed, boundary), Some(3));

        let settlements = accumulator.take_settlements();
        assert_eq!(settlements, vec![settlement(2026, 12, 0, 6)]);
        assert_eq!(
            settlements
                .iter()
                .map(|settlement| settlement.usage.units * 10 + settlement.usage.remainder)
                .sum::<u128>(),
            u128::from(3_u8) * elapsed.as_nanos()
        );
    }

    #[test]
    fn backward_utc_movement_attributes_monotonic_elapsed_to_the_start_period() {
        let now = Instant::now();
        let start = Utc.with_ymd_and_hms(2030, 2, 1, 0, 0, 0).single().unwrap();
        let elapsed = Duration::from_nanos(7);
        let mut accumulator = ByteTimeAccumulator::new(10, at(now, start));

        accumulator.accrue(
            at(now + elapsed, start - chrono::Duration::nanoseconds(1)),
            Some(3),
        );

        assert_eq!(
            accumulator.take_settlements(),
            vec![settlement(2030, 2, 2, 1)]
        );
    }

    #[test]
    fn settlements_span_multiple_utc_month_boundaries() {
        let now = Instant::now();
        let start = Utc
            .with_ymd_and_hms(2030, 1, 31, 23, 59, 59)
            .single()
            .unwrap();
        let end = Utc.with_ymd_and_hms(2030, 4, 1, 0, 0, 1).single().unwrap();
        let elapsed = (end - start).to_std().unwrap();
        let mut accumulator = ByteTimeAccumulator::new(1_000_000_000, at(now, start));

        accumulator.accrue(at(now + elapsed, end), Some(1));

        let settlements = accumulator.take_settlements();
        assert_eq!(
            settlements,
            vec![
                settlement(2030, 1, 1, 0),
                settlement(2030, 2, 28 * 24 * 60 * 60, 0),
                settlement(2030, 3, 31 * 24 * 60 * 60, 0),
                settlement(2030, 4, 1, 0),
            ]
        );
        assert_eq!(
            settlements
                .iter()
                .map(|settlement| settlement.usage.units)
                .sum::<u128>(),
            elapsed.as_secs() as u128
        );
    }

    #[test]
    fn settlements_count_all_of_leap_year_february() {
        let now = Instant::now();
        let start = Utc.with_ymd_and_hms(2028, 2, 1, 0, 0, 0).single().unwrap();
        let end = Utc.with_ymd_and_hms(2028, 3, 1, 0, 0, 1).single().unwrap();
        let elapsed = (end - start).to_std().unwrap();
        let mut accumulator = ByteTimeAccumulator::new(1_000_000_000, at(now, start));

        accumulator.accrue(at(now + elapsed, end), Some(1));

        let settlements = accumulator.take_settlements();
        assert_eq!(
            settlements,
            vec![
                settlement(2028, 2, 29 * 24 * 60 * 60, 0),
                settlement(2028, 3, 1, 0),
            ]
        );
        assert_eq!(
            settlements
                .iter()
                .map(|settlement| settlement.usage.units)
                .sum::<u128>(),
            elapsed.as_secs() as u128
        );
    }

    #[test]
    fn wall_clock_sample_uses_the_midpoint_of_its_monotonic_bracket() {
        let before = Instant::now();
        let after = before + Duration::from_nanos(10);
        let boundary = Utc.with_ymd_and_hms(2030, 2, 1, 0, 0, 0).single().unwrap();

        assert_eq!(
            MeteringTime::between(before, boundary, after),
            MeteringTime {
                instant: before + Duration::from_nanos(5),
                utc: boundary,
            }
        );
    }
}
