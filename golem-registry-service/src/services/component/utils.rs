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

use super::ComponentError;
use async_zip::ZipEntry;
use async_zip::tokio::read::fs::ZipFileReader;
use futures::AsyncReadExt;
use golem_common::model::component::ArchiveFilePath;
use golem_service_base::replayable_stream::ReplayableStream;
use std::collections::HashSet;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio_stream::Stream;

pub struct ComponentFilesArchiveReader {
    _archive: Arc<NamedTempFile>,
    reader: ZipFileReader,
}

#[derive(Clone, Debug)]
pub struct ComponentFileArchiveEntry {
    pub index: usize,
    pub path: ArchiveFilePath,
    pub declared_size: u64,
    crc32: u32,
}

impl ComponentFilesArchiveReader {
    pub async fn open(archive: NamedTempFile) -> Result<Self, ComponentError> {
        let archive = Arc::new(archive);
        let reader = ZipFileReader::new(archive.path())
            .await
            .map_err(malformed_archive_error)?;

        Ok(Self {
            _archive: archive,
            reader,
        })
    }

    pub fn len(&self) -> usize {
        self.reader.file().entries().len()
    }

    pub fn entry(&self, index: usize) -> Result<Option<ComponentFileArchiveEntry>, ComponentError> {
        let entry = self.reader.file().entries().get(index).ok_or_else(|| {
            ComponentError::MalformedComponentArchive {
                message: format!("Missing ZIP entry at index {index}"),
            }
        })?;

        if entry.dir().map_err(malformed_archive_error)? {
            Ok(None)
        } else {
            Ok(Some(ComponentFileArchiveEntry {
                index,
                path: initial_component_file_path_from_zip_entry(entry)?,
                declared_size: entry.uncompressed_size(),
                crc32: entry.crc32(),
            }))
        }
    }

    pub fn stream(
        &self,
        entry: ComponentFileArchiveEntry,
        max_uncompressed_file_size: u64,
    ) -> PreparedComponentFile {
        PreparedComponentFile {
            reader: self.reader.clone(),
            entry,
            max_uncompressed_file_size,
        }
    }

    pub fn referenced_entries(
        &self,
        referenced_paths: &HashSet<ArchiveFilePath>,
        max_uncompressed_file_size: u64,
        max_uncompressed_archive_size: u64,
    ) -> Result<Vec<ComponentFileArchiveEntry>, ComponentError> {
        let mut archive_paths = HashSet::new();
        let mut referenced_entries = Vec::new();
        let mut declared_archive_size = 0u64;

        for index in 0..self.len() {
            let Some(entry) = self.entry(index)? else {
                continue;
            };

            if !archive_paths.insert(entry.path.clone()) {
                return Err(ComponentError::MalformedComponentArchive {
                    message: format!(
                        "Multiple ZIP entries normalize to the same path '{}'",
                        entry.path
                    ),
                });
            }
            if !referenced_paths.contains(&entry.path) {
                continue;
            }
            if entry.declared_size > max_uncompressed_file_size {
                return Err(ComponentError::MalformedComponentArchive {
                    message: format!(
                        "ZIP entry '{}' declares an uncompressed size of {} bytes, exceeding the maximum of {} bytes",
                        entry.path, entry.declared_size, max_uncompressed_file_size
                    ),
                });
            }

            declared_archive_size = declared_archive_size
                .checked_add(entry.declared_size)
                .ok_or_else(|| ComponentError::MalformedComponentArchive {
                    message: "Total declared uncompressed archive size overflowed".to_string(),
                })?;
            if declared_archive_size > max_uncompressed_archive_size {
                return Err(ComponentError::MalformedComponentArchive {
                    message: format!(
                        "Referenced ZIP entries declare a total uncompressed size of {declared_archive_size} bytes, exceeding the maximum of {max_uncompressed_archive_size} bytes"
                    ),
                });
            }

            referenced_entries.push(entry);
        }

        Ok(referenced_entries)
    }
}

pub struct PreparedComponentFile {
    reader: ZipFileReader,
    entry: ComponentFileArchiveEntry,
    max_uncompressed_file_size: u64,
}

impl ReplayableStream for PreparedComponentFile {
    type Item = Result<Vec<u8>, anyhow::Error>;
    type Error = anyhow::Error;

    async fn make_stream(
        &self,
    ) -> Result<impl Stream<Item = Self::Item> + Send + 'static, Self::Error> {
        let reader = self.reader.clone();
        let index = self.entry.index;
        let path = self.entry.path.clone();
        let declared_size = self.entry.declared_size;
        let expected_crc = self.entry.crc32;
        let max_uncompressed_file_size = self.max_uncompressed_file_size;
        let entry_reader = reader
            .reader_without_entry(index)
            .await
            .map_err(archive_stream_error)?;

        Ok(futures::stream::try_unfold(
            (entry_reader, 0u64),
            move |(mut entry_reader, actual_size)| {
                let path = path.clone();
                async move {
                    let mut buffer = vec![0u8; 64 * 1024];
                    let read = entry_reader
                        .read(&mut buffer)
                        .await
                        .map_err(archive_stream_error)?;

                    if read == 0 {
                        if actual_size != declared_size {
                            return Err(archive_stream_error(format!(
                                "ZIP entry '{path}' has declared uncompressed size {declared_size}, but produced {actual_size} bytes"
                            )));
                        }
                        if entry_reader.compute_hash() != expected_crc {
                            return Err(archive_stream_error(format!(
                                "CRC32 check failed for ZIP entry '{path}'"
                            )));
                        }
                        return Ok(None);
                    }

                    let actual_size = actual_size + read as u64;
                    if actual_size > declared_size {
                        return Err(archive_stream_error(format!(
                            "ZIP entry '{path}' exceeded its declared uncompressed size of {declared_size} bytes"
                        )));
                    }
                    if actual_size > max_uncompressed_file_size {
                        return Err(archive_stream_error(format!(
                            "ZIP entry '{path}' exceeded the maximum uncompressed file size of {max_uncompressed_file_size} bytes"
                        )));
                    }

                    buffer.truncate(read);
                    Ok(Some((buffer, (entry_reader, actual_size))))
                }
            },
        ))
    }

    async fn length(&self) -> Result<u64, Self::Error> {
        Ok(self.entry.declared_size)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct ArchiveStreamError(String);

fn archive_stream_error(error: impl std::fmt::Display) -> anyhow::Error {
    anyhow::Error::new(ArchiveStreamError(error.to_string()))
}

pub fn map_archive_stream_error(error: anyhow::Error) -> ComponentError {
    if let Some(error) = error.downcast_ref::<ArchiveStreamError>() {
        ComponentError::MalformedComponentArchive {
            message: error.to_string(),
        }
    } else {
        ComponentError::InternalError(error)
    }
}

fn malformed_archive_error(error: impl std::fmt::Display) -> ComponentError {
    ComponentError::MalformedComponentArchive {
        message: error.to_string(),
    }
}

fn initial_component_file_path_from_zip_entry(
    entry: &ZipEntry,
) -> Result<ArchiveFilePath, ComponentError> {
    let file_path =
        entry
            .filename()
            .as_str()
            .map_err(|e| ComponentError::MalformedComponentArchive {
                message: format!("Failed to convert filename to string: {e}"),
            })?;

    // convert windows path separators to unix and sanitize the path
    let file_path: String = file_path
        .replace('\\', "/")
        .split('/')
        .map(sanitize_filename::sanitize)
        .collect::<Vec<_>>()
        .join("/");

    ArchiveFilePath::from_abs_str(&format!("/{file_path}")).map_err(|e| {
        ComponentError::MalformedComponentArchive {
            message: format!("Failed to convert path to ArchiveFilePath: {e}"),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_zip::tokio::write::ZipFileWriter;
    use async_zip::{Compression, ZipEntryBuilder};
    use futures::{StreamExt, TryStreamExt, pin_mut};
    use std::io::{Seek, SeekFrom, Write};
    use test_r::test;

    async fn test_archive(entries: &[(&str, &[u8])]) -> NamedTempFile {
        let archive = NamedTempFile::new().unwrap();
        let file = tokio::fs::File::from_std(archive.reopen().unwrap());
        let mut writer = ZipFileWriter::with_tokio(file);

        for (path, contents) in entries {
            let entry = ZipEntryBuilder::new((*path).into(), Compression::Deflate);
            writer.write_entry_whole(entry, contents).await.unwrap();
        }
        writer.close().await.unwrap();
        archive
    }

    fn referenced(paths: &[&str]) -> HashSet<ArchiveFilePath> {
        paths
            .iter()
            .map(|path| ArchiveFilePath::from_abs_str(path).unwrap())
            .collect()
    }

    fn patch_central_directory_u32(archive: &NamedTempFile, field_offset: usize, value: u32) {
        let bytes = std::fs::read(archive.path()).unwrap();
        let header = bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .expect("central directory header");
        let mut file = archive.reopen().unwrap();
        file.seek(SeekFrom::Start((header + field_offset) as u64))
            .unwrap();
        file.write_all(&value.to_le_bytes()).unwrap();
        file.flush().unwrap();
    }

    #[test]
    async fn archive_entry_stream_is_replayable() {
        let archive =
            test_archive(&[("first.txt", b"first"), ("nested/second.txt", b"second")]).await;
        let reader = ComponentFilesArchiveReader::open(archive).await.unwrap();
        let entry = reader.entry(1).unwrap().unwrap();

        assert_eq!(entry.path.to_abs_string(), "/nested/second.txt");
        assert_eq!(entry.declared_size, 6);

        let prepared = reader.stream(entry, 1024);
        for _ in 0..2 {
            let stream = prepared.make_stream().await.unwrap();
            pin_mut!(stream);
            let mut contents = Vec::new();
            while let Some(chunk) = stream.next().await {
                contents.extend(chunk.unwrap());
            }
            assert_eq!(contents, b"second");
        }
    }

    #[test]
    async fn duplicate_normalized_paths_are_rejected_during_preflight() {
        let archive = test_archive(&[("nested/file.txt", b"a"), ("nested\\file.txt", b"b")]).await;
        let reader = ComponentFilesArchiveReader::open(archive).await.unwrap();

        let error = reader
            .referenced_entries(&referenced(&["/nested/file.txt"]), 1024, 1024)
            .unwrap_err();
        assert!(matches!(
            error,
            ComponentError::MalformedComponentArchive { .. }
        ));
    }

    #[test]
    async fn declared_file_and_archive_limits_are_enforced() {
        let archive = test_archive(&[("first.txt", b"12345"), ("second.txt", b"67890")]).await;
        let reader = ComponentFilesArchiveReader::open(archive).await.unwrap();
        let paths = referenced(&["/first.txt", "/second.txt"]);

        let file_error = reader.referenced_entries(&paths, 4, 100).unwrap_err();
        assert!(matches!(
            file_error,
            ComponentError::MalformedComponentArchive { .. }
        ));

        let archive_error = reader.referenced_entries(&paths, 5, 9).unwrap_err();
        assert!(matches!(
            archive_error,
            ComponentError::MalformedComponentArchive { .. }
        ));
    }

    #[test]
    async fn oversized_unreferenced_entries_are_skipped() {
        let archive = test_archive(&[("used.txt", b"ok"), ("unused.txt", b"too large")]).await;
        let reader = ComponentFilesArchiveReader::open(archive).await.unwrap();

        let entries = reader
            .referenced_entries(&referenced(&["/used.txt"]), 2, 2)
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.to_abs_string(), "/used.txt");
    }

    #[test]
    async fn each_stream_pass_validates_actual_size() {
        let archive = test_archive(&[("file.txt", b"contents")]).await;
        patch_central_directory_u32(&archive, 24, 7);
        let reader = ComponentFilesArchiveReader::open(archive).await.unwrap();
        let prepared = reader.stream(reader.entry(0).unwrap().unwrap(), 1024);

        for _ in 0..2 {
            let error = prepared
                .make_stream()
                .await
                .unwrap()
                .try_collect::<Vec<_>>()
                .await
                .unwrap_err();
            assert!(matches!(
                map_archive_stream_error(error),
                ComponentError::MalformedComponentArchive { .. }
            ));
        }
    }

    #[test]
    async fn each_stream_pass_validates_crc32() {
        let archive = test_archive(&[("file.txt", b"contents")]).await;
        patch_central_directory_u32(&archive, 16, 0);
        let reader = ComponentFilesArchiveReader::open(archive).await.unwrap();
        let prepared = reader.stream(reader.entry(0).unwrap().unwrap(), 1024);

        for _ in 0..2 {
            let error = prepared
                .make_stream()
                .await
                .unwrap()
                .try_collect::<Vec<_>>()
                .await
                .unwrap_err();
            assert!(matches!(
                map_archive_stream_error(error),
                ComponentError::MalformedComponentArchive { .. }
            ));
        }
    }
}
