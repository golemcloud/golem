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

use super::*;

impl AtomicResourceEntry {
    pub fn per_invocation_http_call_limit(&self) -> u64 {
        self.per_invocation_http_call_limit.load(Ordering::Acquire)
    }

    pub fn per_invocation_rpc_call_limit(&self) -> u64 {
        self.per_invocation_rpc_call_limit.load(Ordering::Acquire)
    }

    /// Returns the number of HTTP calls remaining in this billing period from the
    /// local perspective: the server's last-known available count minus calls that
    /// have been made but not yet synced (unsynced) or are currently being synced
    /// (syncing).
    pub fn remaining_http_calls(&self) -> u64 {
        let revision_state = self.usage_revision_state.lock().unwrap();
        self.remaining_http_calls_with_revision_state(&revision_state)
    }

    pub(super) fn remaining_http_calls_with_revision_state(
        &self,
        revision_state: &UsageRevisionState,
    ) -> u64 {
        let available = self
            .available_http_calls_from_server
            .load(Ordering::Acquire);
        let unsynced = self.unsynced_http_calls.load(Ordering::Acquire);
        let syncing = self.syncing_http_calls.load(Ordering::Acquire);
        let pending = revision_state
            .pending
            .iter()
            .map(|captured| captured.update.http_call_count_delta)
            .fold(0, u64::saturating_add);
        available
            .saturating_sub(unsynced)
            .saturating_sub(syncing)
            .saturating_sub(pending)
    }

    /// Returns the number of RPC calls remaining in this billing period.
    pub fn remaining_rpc_calls(&self) -> u64 {
        let revision_state = self.usage_revision_state.lock().unwrap();
        self.remaining_rpc_calls_with_revision_state(&revision_state)
    }

    pub(super) fn remaining_rpc_calls_with_revision_state(
        &self,
        revision_state: &UsageRevisionState,
    ) -> u64 {
        let available = self.available_rpc_calls_from_server.load(Ordering::Acquire);
        let unsynced = self.unsynced_rpc_calls.load(Ordering::Acquire);
        let syncing = self.syncing_rpc_calls.load(Ordering::Acquire);
        let pending = revision_state
            .pending
            .iter()
            .map(|captured| captured.update.rpc_call_count_delta)
            .fold(0, u64::saturating_add);
        available
            .saturating_sub(unsynced)
            .saturating_sub(syncing)
            .saturating_sub(pending)
    }

    /// Records one outgoing HTTP call against the monthly account quota.
    ///
    /// Returns `false` when the remaining HTTP call budget is zero,
    /// signalling that the worker should be suspended at the next opportunity.
    pub fn record_http_call(&self) -> bool {
        let revision_state = self.usage_revision_state.lock().unwrap();
        if self.remaining_http_calls_with_revision_state(&revision_state) == 0 {
            return false;
        }
        self.unsynced_http_calls
            .try_update(Ordering::AcqRel, Ordering::Acquire, |uhc| {
                Some(uhc.saturating_add(1))
            })
            .ok();
        true
    }

    /// Records one outgoing RPC call against the monthly account quota.
    ///
    /// Returns `false` when the remaining RPC call budget is zero.
    pub fn record_rpc_call(&self) -> bool {
        let revision_state = self.usage_revision_state.lock().unwrap();
        if self.remaining_rpc_calls_with_revision_state(&revision_state) == 0 {
            return false;
        }
        self.unsynced_rpc_calls
            .try_update(Ordering::AcqRel, Ordering::Acquire, |urc| {
                Some(urc.saturating_add(1))
            })
            .ok();
        true
    }

    pub fn try_record_blob_storage_delta(&self, delta: i64) -> bool {
        let mut quota = self.blob_storage_quota.lock().unwrap();
        if delta <= 0 {
            quota.unsynced_delta = quota.unsynced_delta.saturating_add(delta);
            return true;
        }

        let pending = quota.unsynced_delta.saturating_add(quota.syncing_delta);
        let available = if pending >= 0 {
            quota.available_from_server.saturating_sub(pending as u64)
        } else {
            quota
                .available_from_server
                .saturating_add(pending.unsigned_abs())
        };
        if available < delta as u64 {
            return false;
        }
        quota.unsynced_delta = quota.unsynced_delta.saturating_add(delta);
        true
    }

    pub fn rollback_blob_storage_delta(&self, delta: i64) {
        let mut quota = self.blob_storage_quota.lock().unwrap();
        quota.unsynced_delta = quota.unsynced_delta.saturating_sub(delta);
    }

    #[cfg(test)]
    pub(crate) fn set_available_blob_storage_bytes(&self, value: u64) {
        self.blob_storage_quota
            .lock()
            .unwrap()
            .available_from_server = value;
    }

    #[cfg(test)]
    pub(crate) fn blob_storage_delta_for_test(&self) -> i64 {
        let quota = self.blob_storage_quota.lock().unwrap();
        quota.unsynced_delta.saturating_add(quota.syncing_delta)
    }
}
