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

//! A record of the output frames of invocation sessions that checks the output wire contract.

use anyhow::{anyhow, ensure};
use golem_api_grpc::proto::golem::worker::{DurableStreamMapping, OutputStreamItem, StreamEnd};
use golem_schema::proto::golem::common::Uuid as ProtoUuid;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Records the stream mappings, output items and stream ends that a test reads from one or more
/// sessions of an invocation, and checks on every frame that:
///
/// - an item continues its stream: its `producer_sequence` is the number of items the stream
///   delivered before it, counting `logical_item_count`;
/// - no item follows the `StreamEnd` of its stream, and a stream ends once;
/// - a `StreamEnd`'s `producer_sequence` is the number of items of its stream;
/// - no durable offset repeats among the items and ends;
/// - every frame of a stream names the same durable stream ID, and it is the ID of the handle
///   that the stream's mapping announced, whether the mapping arrives before or after the frame;
/// - every mapping has a durable stream ID, and a stream is not mapped to two IDs.
///
/// [`OutputLedger::ensure_complete`] checks at the end that every stream with frames was mapped
/// and every mapped stream ended. The test keeps its own checks of the values and of the order of
/// the other responses.
#[derive(Debug, Default)]
pub struct OutputLedger {
    mapped: BTreeMap<u64, ProtoUuid>,
    observed: BTreeMap<u64, Option<ProtoUuid>>,
    delivered: BTreeMap<u64, u64>,
    ended: BTreeSet<u64>,
    offsets: HashSet<Vec<u8>>,
    items: Vec<OutputStreamItem>,
    ends: Vec<StreamEnd>,
}

impl OutputLedger {
    /// Records the mappings of a `Result`, an `Accepted` or an output item.
    pub fn mappings(&mut self, mappings: &[DurableStreamMapping]) -> anyhow::Result<()> {
        for mapping in mappings {
            let stream = mapping.transport_stream_id;
            let id = mapping
                .handle
                .as_ref()
                .and_then(|handle| handle.stream_id)
                .ok_or_else(|| {
                    anyhow!("the mapping of stream {stream} has no durable stream ID")
                })?;
            if let Some(previous) = self.mapped.insert(stream, id) {
                ensure!(
                    previous == id,
                    "stream {stream} was mapped to {previous:?} and then to {id:?}"
                );
            }
            if let Some(observed) = self.observed.get(&stream) {
                ensure!(
                    *observed == Some(id),
                    "the frames of stream {stream} name {observed:?}, its mapping {id:?}"
                );
            }
        }
        Ok(())
    }

    /// Records an output item and the mappings it introduces.
    pub fn item(&mut self, item: &OutputStreamItem) -> anyhow::Result<()> {
        let stream = item.transport_stream_id;
        ensure!(
            !self.ended.contains(&stream),
            "an item of stream {stream} followed its StreamEnd"
        );
        let delivered = self.delivered.entry(stream).or_default();
        ensure!(
            item.producer_sequence == *delivered,
            "item of stream {stream} has producer sequence {}, expected {delivered}",
            item.producer_sequence
        );
        *delivered += item.logical_item_count;
        self.offset(stream, &item.durable_offset)?;
        self.durable_stream_id(stream, item.durable_stream_id)?;
        self.mappings(&item.new_stream_mappings)?;
        self.items.push(item.clone());
        Ok(())
    }

    /// Records the end of a stream.
    pub fn end(&mut self, end: &StreamEnd) -> anyhow::Result<()> {
        let stream = end.transport_stream_id;
        ensure!(self.ended.insert(stream), "stream {stream} ended twice");
        let delivered = self.delivered.get(&stream).copied().unwrap_or_default();
        ensure!(
            end.producer_sequence == delivered,
            "the StreamEnd of stream {stream} has producer sequence {}, but the stream \
             delivered {delivered} items",
            end.producer_sequence
        );
        self.offset(stream, &end.durable_offset)?;
        self.durable_stream_id(stream, end.durable_stream_id)?;
        self.ends.push(end.clone());
        Ok(())
    }

    /// Checks that every stream with an item or an end was mapped, and every mapped stream ended.
    pub fn ensure_complete(&self) -> anyhow::Result<()> {
        for stream in self.observed.keys() {
            ensure!(
                self.mapped.contains_key(stream),
                "stream {stream} delivered frames but was never mapped"
            );
        }
        for stream in self.mapped.keys() {
            ensure!(
                self.ended.contains(stream),
                "mapped stream {stream} did not end"
            );
        }
        Ok(())
    }

    /// The recorded items, in arrival order.
    pub fn items(&self) -> &[OutputStreamItem] {
        &self.items
    }

    /// The recorded stream ends, in arrival order.
    pub fn ends(&self) -> &[StreamEnd] {
        &self.ends
    }

    /// The transport IDs of the mapped streams.
    pub fn mapped_streams(&self) -> impl Iterator<Item = u64> + '_ {
        self.mapped.keys().copied()
    }

    /// The durable stream ID that the mapping of `stream` announced.
    pub fn mapped_id(&self, stream: u64) -> Option<ProtoUuid> {
        self.mapped.get(&stream).copied()
    }

    /// Whether `stream` ended.
    pub fn is_ended(&self, stream: u64) -> bool {
        self.ended.contains(&stream)
    }

    fn offset(&mut self, stream: u64, offset: &[u8]) -> anyhow::Result<()> {
        ensure!(
            self.offsets.insert(offset.to_vec()),
            "stream {stream} repeated the durable offset {offset:?}"
        );
        Ok(())
    }

    fn durable_stream_id(&mut self, stream: u64, id: Option<ProtoUuid>) -> anyhow::Result<()> {
        if let Some(previous) = self.observed.insert(stream, id) {
            ensure!(
                previous == id,
                "the frames of stream {stream} name {previous:?} and {id:?}"
            );
        }
        if let Some(mapped) = self.mapped.get(&stream) {
            ensure!(
                id == Some(*mapped),
                "a frame of stream {stream} names {id:?}, its mapping {mapped:?}"
            );
        }
        Ok(())
    }
}
