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

pub mod connection;
mod keys;
pub mod memory;
pub mod placement;
pub mod s3;
mod transport;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use std::fmt::Debug;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;

pub use golem_common::model::volume::{
    Cursor, Entry, FileState, Node, Page, VolumeError, VolumePath,
};
pub use s3::{connect, place};

pub type ReadStream = Pin<Box<dyn Stream<Item = Result<Bytes, VolumeError>> + Send>>;

/// Live access to files and directories in a volume.
#[async_trait]
pub trait VolumeClient: Debug + Send + Sync {
    /// Describes the file or directory, or returns None when the path does not exist.
    async fn stat(&self, path: &VolumePath) -> Result<Option<Node>, VolumeError>;
    /// Returns one page of direct children without folder markers.
    async fn list(
        &self,
        directory: &VolumePath,
        after: Option<&Cursor>,
    ) -> Result<Page, VolumeError>;
    /// Opens the file in its current state.
    async fn open(&self, path: &VolumePath) -> Result<VolumeFile, VolumeError>;
}

/// Access to file states that were recorded earlier.
#[async_trait]
pub trait VolumeRecovery: Debug + Send + Sync {
    /// Opens the recorded state, or fails as Changed when the store no longer has it.
    async fn open_recorded(
        &self,
        path: &VolumePath,
        recorded: &FileState,
    ) -> Result<VolumeFile, VolumeError>;
}

#[async_trait]
pub(super) trait FileReader: Debug + Send + Sync {
    async fn read(&self, state: &FileState, range: Range<u64>) -> Result<ReadStream, VolumeError>;
}

/// An open file's metadata and read adapter. It holds no connection.
#[derive(Debug)]
pub struct VolumeFile {
    size: u64,
    state: FileState,
    replayable: bool,
    reader: Arc<dyn FileReader>,
}

impl VolumeFile {
    pub(super) fn new(
        size: u64,
        state: FileState,
        replayable: bool,
        reader: Arc<dyn FileReader>,
    ) -> Self {
        Self {
            size,
            state,
            replayable,
            reader,
        }
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// Streams bytes of the open state. Empty and past-EOF ranges send no request.
    pub async fn read(&self, range: Range<u64>) -> Result<ReadStream, VolumeError> {
        if range.start > range.end {
            return Err(VolumeError::InvalidRequest);
        }
        if range.is_empty() || range.start >= self.size {
            return Ok(Box::pin(futures::stream::empty()));
        }
        self.reader
            .read(&self.state, range.start..range.end.min(self.size))
            .await
    }

    pub fn state(&self) -> &FileState {
        &self.state
    }

    pub fn replayable(&self) -> bool {
        self.replayable
    }
}
