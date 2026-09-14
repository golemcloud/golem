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

use std::time::Instant;

#[derive(Debug)]
pub(crate) struct ByteTimeAccumulator {
    byte_nanoseconds_per_unit: u128,
    last_sample: Instant,
    pending_units: u128,
    pending_byte_nanoseconds: u128,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ByteTimeSettlement {
    pub(crate) units: u128,
    pub(crate) remainder: u128,
}

impl ByteTimeAccumulator {
    pub(crate) fn new(byte_nanoseconds_per_unit: u128, now: Instant) -> Self {
        assert!(byte_nanoseconds_per_unit != 0);
        Self {
            byte_nanoseconds_per_unit,
            last_sample: now,
            pending_units: 0,
            pending_byte_nanoseconds: 0,
        }
    }

    pub(crate) fn accrue(&mut self, now: Instant, bytes: Option<u64>) {
        self.advance(now, bytes);
    }

    pub(crate) fn advance(&mut self, now: Instant, bytes: Option<u64>) -> bool {
        if now <= self.last_sample {
            return false;
        }

        let elapsed = now.saturating_duration_since(self.last_sample).as_nanos();
        self.last_sample = now;
        if let Some(bytes) = bytes {
            self.pending_byte_nanoseconds = self
                .pending_byte_nanoseconds
                .saturating_add(u128::from(bytes).saturating_mul(elapsed));
        }

        let units = self.pending_byte_nanoseconds / self.byte_nanoseconds_per_unit;
        self.pending_byte_nanoseconds %= self.byte_nanoseconds_per_unit;
        self.pending_units = self.pending_units.saturating_add(units);
        true
    }

    pub(crate) fn take_units(&mut self) -> i64 {
        let units = self.pending_units.min(i64::MAX as u128) as i64;
        self.pending_units -= units as u128;
        units
    }

    pub(crate) fn take_settlement(&mut self) -> ByteTimeSettlement {
        ByteTimeSettlement {
            units: std::mem::take(&mut self.pending_units),
            remainder: std::mem::take(&mut self.pending_byte_nanoseconds),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use test_r::test;

    #[test]
    fn units_above_the_batch_range_remain_pending() {
        let now = Instant::now();
        let mut accumulator = ByteTimeAccumulator::new(1, now);
        accumulator.accrue(now + Duration::from_nanos(2), Some(u64::MAX));

        assert_eq!(accumulator.take_units(), i64::MAX);
        assert_eq!(accumulator.take_units(), i64::MAX);
        assert_eq!(accumulator.take_units(), i64::MAX);
        assert_eq!(accumulator.take_units(), i64::MAX);
        assert_eq!(accumulator.take_units(), 2);
        assert_eq!(accumulator.take_units(), 0);
    }
}
