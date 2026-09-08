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
use async_zip::tokio::read::seek::ZipFileReader;
use futures::TryStreamExt;
use golem_common::model::agent::AgentFileContentHash;
use golem_common::model::component::ArchiveFilePath;
use golem_service_base::replayable_stream::ReplayableStream;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_stream::Stream;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tokio_util::io::ReaderStream;

pub struct ComponentFilesArchiveReader {
    _archive: NamedTempFile,
    reader: ZipFileReader<BufReader<tokio::fs::File>>,
}

impl ComponentFilesArchiveReader {
    pub async fn open(archive: NamedTempFile) -> Result<Self, ComponentError> {
        let reopened = archive.reopen().map_err(anyhow::Error::from)?;
        let buf_reader = BufReader::new(tokio::fs::File::from_std(reopened));
        let reader = ZipFileReader::with_tokio(buf_reader)
            .await
            .map_err(anyhow::Error::from)?;

        Ok(Self {
            _archive: archive,
            reader,
        })
    }

    pub fn len(&self) -> usize {
        self.reader.file().entries().len()
    }

    pub fn file_path(&self, index: usize) -> Result<Option<ArchiveFilePath>, ComponentError> {
        let entry = self.reader.file().entries().get(index).ok_or_else(|| {
            ComponentError::MalformedComponentArchive {
                message: format!("Missing ZIP entry at index {index}"),
            }
        })?;

        if entry.dir().map_err(anyhow::Error::from)? {
            Ok(None)
        } else {
            initial_component_file_path_from_zip_entry(entry).map(Some)
        }
    }

    pub async fn extract(&mut self, index: usize) -> Result<PreparedComponentFile, ComponentError> {
        let expected_crc = self
            .reader
            .file()
            .entries()
            .get(index)
            .ok_or_else(|| ComponentError::MalformedComponentArchive {
                message: format!("Missing ZIP entry at index {index}"),
            })?
            .crc32();

        let mut entry_reader = self
            .reader
            .reader_with_entry(index)
            .await
            .map_err(anyhow::Error::from)?;

        let file = tokio::task::spawn_blocking(NamedTempFile::new)
            .await
            .map_err(anyhow::Error::from)?
            .map_err(anyhow::Error::from)?;
        let reopened = file.reopen().map_err(anyhow::Error::from)?;
        let mut output = tokio::fs::File::from_std(reopened);
        let mut hasher = blake3::Hasher::new();
        let mut size = 0u64;

        {
            let mut input = (&mut entry_reader).compat();
            let mut buffer = vec![0u8; 64 * 1024];

            loop {
                let read = input.read(&mut buffer).await.map_err(anyhow::Error::from)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
                output
                    .write_all(&buffer[..read])
                    .await
                    .map_err(anyhow::Error::from)?;
                size += read as u64;
            }
        }
        output.flush().await.map_err(anyhow::Error::from)?;

        if entry_reader.compute_hash() != expected_crc {
            return Err(ComponentError::MalformedComponentArchive {
                message: format!("CRC32 check failed for ZIP entry at index {index}"),
            });
        }

        Ok(PreparedComponentFile {
            file: Arc::new(file),
            hash: AgentFileContentHash(hasher.finalize().into()),
            size,
        })
    }
}

pub struct PreparedComponentFile {
    file: Arc<NamedTempFile>,
    pub hash: AgentFileContentHash,
    pub size: u64,
}

impl ReplayableStream for PreparedComponentFile {
    type Item = Result<Vec<u8>, anyhow::Error>;
    type Error = anyhow::Error;

    async fn make_stream(
        &self,
    ) -> Result<impl Stream<Item = Self::Item> + Send + 'static, Self::Error> {
        let file = self.file.clone();
        let reopened = tokio::task::spawn_blocking(move || file.reopen()).await??;
        let stream = ReaderStream::new(tokio::fs::File::from_std(reopened));
        let mapped_stream = stream.map_ok(|b| b.to_vec()).map_err(|e| e.into());
        Ok(Box::pin(mapped_stream))
    }

    async fn length(&self) -> Result<u64, Self::Error> {
        Ok(self.size)
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
    use futures::{StreamExt, pin_mut};
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

    #[test]
    async fn archive_is_parsed_once_and_entries_are_prepared_with_hash_and_size() {
        let archive =
            test_archive(&[("first.txt", b"first"), ("nested/second.txt", b"second")]).await;
        let mut reader = ComponentFilesArchiveReader::open(archive).await.unwrap();

        assert_eq!(reader.len(), 2);
        assert_eq!(
            reader.file_path(1).unwrap().unwrap().to_abs_string(),
            "/nested/second.txt"
        );

        let prepared = reader.extract(1).await.unwrap();
        assert_eq!(prepared.size, 6);
        assert_eq!(prepared.hash.0, blake3::hash(b"second").into());

        let stream = prepared.make_stream().await.unwrap();
        pin_mut!(stream);
        let mut contents = Vec::new();
        while let Some(chunk) = stream.next().await {
            contents.extend(chunk.unwrap());
        }
        assert_eq!(contents, b"second");
    }
}
