// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use tokio::fs::File;
use tokio::sync::{oneshot, Mutex, MutexGuard};
use tokio::task::{self, JoinHandle};
use tokio_util::bytes::Bytes;
use zip::{result::ZipResult, write::FileOptions, ZipWriter};

enum AsyncZipWriterCommand {
    StartFileFromPath(
        Arc<PathBuf>,
        FileOptions<'static, ()>,
        oneshot::Sender<ZipResult<()>>,
    ),
    WriteAll(Bytes, oneshot::Sender<std::io::Result<()>>),
    Finish(oneshot::Sender<ZipResult<File>>),
    Drop,
}

// TODO: Replace with the crate `async-zip` once it produces valid archives or with an alternative

#[derive(Clone)]
pub struct AsyncZipWriter(Arc<Mutex<AsyncZipWriterHandle>>);

impl AsyncZipWriter {
    pub async fn new(archive_file: File) -> Self {
        let (command_sender, command_receiver) = channel();
        let archive_file = archive_file.into_std().await;
        let task_handle = task::spawn_blocking(move || {
            let mut zip_writer = ZipWriter::new(archive_file);
            while let Ok(command) = command_receiver.recv() {
                match command {
                    AsyncZipWriterCommand::StartFileFromPath(path, options, response_sender) => {
                        response_sender
                            .send(zip_writer.start_file_from_path(path.as_ref(), options))
                            .expect("Failed to send `ZipWriter::start_file_from_path` response")
                    }
                    AsyncZipWriterCommand::WriteAll(bytes, response_sender) => response_sender
                        .send(zip_writer.write_all(&bytes))
                        .expect("Failed to send `ZipWriter::write_all` response"),
                    AsyncZipWriterCommand::Finish(response_sender) => {
                        response_sender
                            .send(zip_writer.finish().map(File::from_std))
                            .expect("Failed to send `ZipWriter::finish` response");
                        break;
                    }
                    AsyncZipWriterCommand::Drop => break,
                }
            }
        });
        let handle = AsyncZipWriterHandle {
            command_sender,
            task_handle: Some(task_handle),
            finished: false,
        };
        Self(Arc::new(Mutex::new(handle)))
    }

    pub async fn lock(&self) -> Result<MutexGuard<AsyncZipWriterHandle>, String> {
        let handle = self.0.lock().await;
        if handle.finished {
            Err("Cannot lock finished 'AsyncZipWriter'".to_owned())
        } else {
            Ok(handle)
        }
    }
}

pub struct AsyncZipWriterHandle {
    command_sender: Sender<AsyncZipWriterCommand>,
    task_handle: Option<JoinHandle<()>>,
    finished: bool,
}

impl AsyncZipWriterHandle {
    pub async fn start_file_from_path(
        &self,
        path: Arc<PathBuf>,
        options: FileOptions<'static, ()>,
    ) -> ZipResult<()> {
        let (response_sender, response_receiver) = oneshot::channel();

        self.command_sender
            .send(AsyncZipWriterCommand::StartFileFromPath(
                path,
                options,
                response_sender,
            ))
            .expect("Failed to send 'AsyncZipWriterCommand::StartFileFromPath'");

        response_receiver
            .await
            .expect("Failed to receive 'ZipWriter::finish' response")
    }

    pub async fn write_all(&self, bytes: Bytes) -> std::io::Result<()> {
        let (response_sender, response_receiver) = oneshot::channel();

        self.command_sender
            .send(AsyncZipWriterCommand::WriteAll(bytes, response_sender))
            .expect("Failed to send 'AsyncZipWriterCommand::WriteAll'");

        response_receiver
            .await
            .expect("Failed to receive 'ZipWriter::write_all' response")
    }

    pub async fn finish(&mut self) -> ZipResult<File> {
        let (response_sender, response_receiver) = oneshot::channel();

        self.command_sender
            .send(AsyncZipWriterCommand::Finish(response_sender))
            .expect("Failed to send 'AsyncZipWriterCommand::Finish'");

        self.finished = true;

        let response = response_receiver
            .await
            .expect("Failed to receive 'ZipWriter::finish' response");

        if let Some(task_handle) = self.task_handle.take() {
            task_handle
                .await
                .expect("'AsyncZipWriter' task has been canceled or panicked")
        }
        response
    }
}

impl Drop for AsyncZipWriterHandle {
    fn drop(&mut self) {
        let _ = self.command_sender.send(AsyncZipWriterCommand::Drop);
    }
}
