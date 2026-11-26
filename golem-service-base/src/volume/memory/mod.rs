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

use super::{
    Cursor, Entry, FileReader, FileState, Node, Page, ReadStream, VolumeClient, VolumeError,
    VolumeFile, VolumePath, VolumeRecovery,
};
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::ops::Range;
use std::sync::{Arc, Mutex};

pub use super::placement::verdict as place;

/// A file store with optional retained states. Only its control handle changes files.
#[derive(Debug, Clone)]
pub struct InMemoryVolume {
    files: Arc<Mutex<Files>>,
}

/// Competing writes, state retention and failures for an isolated file store.
#[derive(Debug, Clone)]
pub struct MemoryControl {
    files: Arc<Mutex<Files>>,
}

struct Files {
    retained: bool,
    page_size: usize,
    paths: BTreeMap<VolumePath, Vec<StoredFile>>,
    next_failure: Option<VolumeError>,
}

// Bytes and file names can contain credentials reflected by a store.
impl Debug for Files {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("Files")
    }
}

struct StoredFile {
    state: FileState,
    bytes: Bytes,
}

fn stored_file(
    path: &VolumePath,
    bytes: Bytes,
    retained: bool,
    version: uuid::Uuid,
) -> Result<StoredFile, VolumeError> {
    if path.as_str().is_empty() {
        return Err(VolumeError::UnrepresentablePath);
    }
    Ok(StoredFile {
        state: FileState::from_reference(
            format!("\"{:x}\"", Sha256::digest(&bytes)).into(),
            retained.then(|| version.to_string().into()),
        ),
        bytes,
    })
}

fn written(mut history: Vec<StoredFile>, retained: bool, file: StoredFile) -> Vec<StoredFile> {
    if !retained {
        history.clear();
    }
    history.push(file);
    history
}

fn forgotten(mut history: Vec<StoredFile>, state: &FileState) -> Vec<StoredFile> {
    history.retain(|file| &file.state != state);
    history
}

fn selected<'a>(
    files: &'a Files,
    path: &VolumePath,
    state: Option<&FileState>,
) -> Result<&'a StoredFile, VolumeError> {
    let history = files.paths.get(path);
    match state {
        Some(state) => history
            .and_then(|history| history.iter().find(|file| &file.state == state))
            .ok_or(VolumeError::Changed),
        None => history
            .and_then(|history| history.last())
            .ok_or(VolumeError::NotFound),
    }
}

fn next_call(files: &mut Files) -> Result<(), VolumeError> {
    files.next_failure.take().map_or(Ok(()), Err)
}

fn child_prefix(directory: &VolumePath) -> Box<str> {
    if directory.as_str().is_empty() {
        "".into()
    } else {
        format!("{}/", directory.as_str()).into()
    }
}

fn node(files: &Files, path: &VolumePath) -> Option<Node> {
    selected(files, path, None)
        .ok()
        .map(|file| Node::File {
            size: file.bytes.len() as u64,
        })
        .or_else(|| {
            let prefix = child_prefix(path);
            (path.as_str().is_empty()
                || files
                    .paths
                    .keys()
                    .any(|path| path.as_str().starts_with(prefix.as_ref())))
            .then_some(Node::Directory)
        })
}

#[derive(Debug, Serialize, Deserialize)]
struct ListCursor {
    directory: VolumePath,
    last: VolumePath,
}

fn page(
    files: &Files,
    directory: &VolumePath,
    after: Option<&Cursor>,
) -> Result<Page, VolumeError> {
    let after = after
        .map(|cursor| {
            let cursor: ListCursor =
                serde_json::from_str(cursor.as_str()).map_err(|_| VolumeError::InvalidRequest)?;
            if &cursor.directory != directory {
                return Err(VolumeError::InvalidRequest);
            }
            Ok(cursor.last)
        })
        .transpose()?;
    let prefix = child_prefix(directory);
    let children = files
        .paths
        .iter()
        .filter_map(|(path, history)| {
            let relative = path.as_str().strip_prefix(prefix.as_ref())?;
            if relative.is_empty() {
                return None;
            }
            let child = relative.split('/').next()?;
            let path = VolumePath::new(format!("{prefix}{child}")).ok()?;
            let node = if child == relative {
                Node::File {
                    size: history.last()?.bytes.len() as u64,
                }
            } else {
                Node::Directory
            };
            Some((path, node))
        })
        .fold(BTreeMap::new(), |mut entries, (path, node)| {
            entries.entry(path).or_insert(node);
            entries
        });
    let mut entries = children
        .into_iter()
        .filter(|(path, _)| after.as_ref().is_none_or(|after| path > after))
        .take(files.page_size.saturating_add(1))
        .map(|(path, node)| Entry { path, node })
        .collect::<Vec<_>>();
    let after = if entries.len() > files.page_size {
        entries.pop();
        let last = entries.last().unwrap().path.clone();
        Some(Cursor::new(
            serde_json::to_string(&ListCursor {
                directory: directory.clone(),
                last,
            })
            .unwrap(),
        ))
    } else {
        None
    };
    Ok(Page {
        entries: entries.into_boxed_slice(),
        after,
    })
}

impl InMemoryVolume {
    pub fn new(retained: bool, page_size: usize) -> Result<(Self, MemoryControl), VolumeError> {
        if page_size == 0 {
            return Err(VolumeError::InvalidRequest);
        }
        let files = Arc::new(Mutex::new(Files {
            retained,
            page_size,
            paths: BTreeMap::new(),
            next_failure: None,
        }));
        Ok((
            Self {
                files: files.clone(),
            },
            MemoryControl { files },
        ))
    }

    fn open_at(
        &self,
        path: &VolumePath,
        recorded: Option<&FileState>,
    ) -> Result<VolumeFile, VolumeError> {
        let mut files = self.files.lock().unwrap();
        next_call(&mut files)?;
        let file = selected(&files, path, recorded)?;
        Ok(VolumeFile::new(
            file.bytes.len() as u64,
            file.state.clone(),
            file.state.reference().1.is_some(),
            Arc::new(MemoryReader {
                files: self.files.clone(),
                path: path.clone(),
            }),
        ))
    }
}

impl MemoryControl {
    /// Writes a new current file as a competing writer.
    pub fn write(&self, path: &VolumePath, bytes: Bytes) -> Result<FileState, VolumeError> {
        let version = uuid::Uuid::new_v4();
        let mut files = self.files.lock().unwrap();
        let retained = files.retained;
        let file = stored_file(path, bytes, retained, version)?;
        let state = file.state.clone();
        let history = files.paths.entry(path.clone()).or_default();
        *history = written(std::mem::take(history), retained, file);
        Ok(state)
    }

    /// Removes the recorded state, also when an open file still refers to it.
    pub fn forget(&self, path: &VolumePath, state: &FileState) {
        let mut files = self.files.lock().unwrap();
        if let Some(history) = files.paths.get_mut(path) {
            *history = forgotten(std::mem::take(history), state);
            if history.is_empty() {
                files.paths.remove(path);
            }
        }
    }

    pub fn fail_next(&self, error: VolumeError) {
        self.files.lock().unwrap().next_failure = Some(error);
    }
}

#[async_trait]
impl VolumeClient for InMemoryVolume {
    async fn stat(&self, path: &VolumePath) -> Result<Option<Node>, VolumeError> {
        let mut files = self.files.lock().unwrap();
        next_call(&mut files)?;
        Ok(node(&files, path))
    }

    async fn list(
        &self,
        directory: &VolumePath,
        after: Option<&Cursor>,
    ) -> Result<Page, VolumeError> {
        let mut files = self.files.lock().unwrap();
        next_call(&mut files)?;
        page(&files, directory, after)
    }

    async fn open(&self, path: &VolumePath) -> Result<VolumeFile, VolumeError> {
        self.open_at(path, None)
    }
}

#[async_trait]
impl VolumeRecovery for InMemoryVolume {
    async fn open_recorded(
        &self,
        path: &VolumePath,
        recorded: &FileState,
    ) -> Result<VolumeFile, VolumeError> {
        self.open_at(path, Some(recorded))
    }
}

#[derive(Debug)]
struct MemoryReader {
    files: Arc<Mutex<Files>>,
    path: VolumePath,
}

#[async_trait]
impl FileReader for MemoryReader {
    async fn read(&self, state: &FileState, range: Range<u64>) -> Result<ReadStream, VolumeError> {
        let mut files = self.files.lock().unwrap();
        next_call(&mut files)?;
        let file = selected(&files, &self.path, Some(state))?;
        let bytes = file.bytes.slice(range.start as usize..range.end as usize);
        Ok(Box::pin(futures::stream::once(async move { Ok(bytes) })))
    }
}

#[cfg(test)]
mod tests;
