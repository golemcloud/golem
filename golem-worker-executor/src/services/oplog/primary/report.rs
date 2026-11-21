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

use crate::services::oplog::OplogReadError;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use std::collections::{BTreeMap, VecDeque};

type Entries = BTreeMap<OplogIndex, OplogEntry>;

#[derive(Default)]
pub(super) struct ReportState {
    last_reported: OplogIndex,
    retained: VecDeque<Entries>,
}

pub(super) fn initial(last_reported: OplogIndex) -> ReportState {
    ReportState {
        last_reported,
        retained: VecDeque::new(),
    }
}

pub(super) enum ReportEvent {
    AutoCommitted {
        entries: Entries,
        max_retained: usize,
        listener_notified: bool,
    },
    Completed {
        prefix: Result<Entries, OplogReadError>,
        explicit: Entries,
        committed_through: OplogIndex,
    },
}

pub(super) fn missing_prefix(
    state: &ReportState,
    previously_committed_through: OplogIndex,
) -> Option<(OplogIndex, u64)> {
    let read_through = state
        .retained
        .iter()
        .find_map(|batch| batch.first_key_value())
        .map_or(previously_committed_through, |(first, _)| first.previous());
    (state.last_reported < read_through).then(|| {
        (
            state.last_reported.next(),
            read_through.as_u64() - state.last_reported.as_u64(),
        )
    })
}

pub(super) fn transition(
    mut state: ReportState,
    event: ReportEvent,
) -> (ReportState, Result<Entries, OplogReadError>) {
    let result = match event {
        ReportEvent::AutoCommitted {
            entries,
            max_retained,
            listener_notified,
        } => {
            if !entries.is_empty() {
                state.retained.push_back(entries);
                let excess = state.retained.len().saturating_sub(max_retained);
                state.retained.drain(..excess);
                if !listener_notified {
                    state.retained.clear();
                }
            }
            Ok(Entries::new())
        }
        ReportEvent::Completed {
            prefix: Err(error), ..
        } => {
            // Retry must read the whole unreported range, including the explicit commit.
            state.retained.clear();
            Err(error)
        }
        ReportEvent::Completed {
            prefix: Ok(mut prefix),
            mut explicit,
            committed_through,
        } => {
            let mut retained = std::mem::take(&mut state.retained)
                .into_iter()
                .flatten()
                .collect();
            prefix.append(&mut retained);
            prefix.append(&mut explicit);
            state.last_reported = committed_through;
            Ok(prefix)
        }
    };
    (state, result)
}
