use std::sync::Arc;

use anyhow::{anyhow, Error};
use async_trait::async_trait;
use golem_worker_executor_base::host::managed_stdio::{ManagedStandardIo, ManagedStreamStatus};
use golem_worker_executor_base::services::worker_event::WorkerEventService;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tonic::codegen::Bytes;
use wasmtime_wasi::preview2::{HostInputStream, HostOutputStream, Stderr, StreamState};

pub mod blobstore;
pub mod golem;
pub mod keyvalue;

pub struct ManagedStdIn {
    io: ManagedStandardIo,
    runtime: Handle,
    current_handle: Option<JoinHandle<Result<(), anyhow::Error>>>,
    result: Option<tokio::sync::oneshot::Receiver<(Vec<u8>, ManagedStreamStatus)>>,
}

pub struct ManagedStdOut {
    io: ManagedStandardIo,
    runtime: Handle,
    current_handle: Option<JoinHandle<anyhow::Result<()>>>,
    event_service: Arc<dyn WorkerEventService + Send + Sync>,
}

pub struct ManagedStdErr {
    stderr: Stderr,
    event_service: Arc<dyn WorkerEventService + Send + Sync>,
}

impl ManagedStdIn {
    pub fn from_standard_io(runtime: Handle, io: ManagedStandardIo) -> Self {
        Self {
            io,
            runtime,
            current_handle: None,
            result: None,
        }
    }
}

impl ManagedStdOut {
    pub fn from_standard_io(
        runtime: Handle,
        io: ManagedStandardIo,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
    ) -> Self {
        Self {
            io,
            runtime,
            current_handle: None,
            event_service,
        }
    }
}

impl ManagedStdErr {
    pub fn from_stderr(
        stderr: Stderr,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
    ) -> Self {
        Self {
            stderr,
            event_service,
        }
    }
}

fn convert_status(value: ManagedStreamStatus) -> StreamState {
    match value {
        ManagedStreamStatus::Open => StreamState::Open,
        ManagedStreamStatus::Ended => StreamState::Closed,
    }
}

#[async_trait]
impl HostInputStream for ManagedStdIn {
    fn read(&mut self, size: usize) -> Result<(Bytes, StreamState), anyhow::Error> {
        let mut to_read = None;
        let result = match self.result.take() {
            Some(rx) => {
                let (data, status) = self.runtime.block_on(rx)?;
                // Result of the previous async read
                let remaining = size as i64 - data.len() as i64;
                if remaining > 0 {
                    // Spawn a new async read to get more
                    to_read = Some(remaining as usize);
                }
                // Returning with the previously read chunk
                (Bytes::from(data), convert_status(status))
            }
            None => {
                if self.current_handle.is_some() {
                    // There is a read or skip already in progress, so we just return 0 bytes
                    (Bytes::new(), StreamState::Open)
                } else {
                    // We need to initiate a new async read
                    to_read = Some(size);
                    (Bytes::new(), StreamState::Open)
                }
            }
        };
        if let Some(to_read) = to_read {
            let mut io_clone = self.io.clone();
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.result = Some(rx);
            let handle = self.runtime.spawn(async move {
                let mut buf = vec![0u8; to_read];
                let (read, status) = io_clone.read(&mut buf).await?;
                tx.send((buf[0..(read as usize)].to_vec(), status))
                    .map_err(|_| anyhow!("Failed to set read result"))?;
                Ok(())
            });
            self.current_handle = Some(handle);
        }

        Ok(result)
    }

    fn skip(&mut self, nelem: usize) -> Result<(usize, StreamState), anyhow::Error> {
        // TODO: implementation that does not allocate nelem bytes
        let (bytes, state) = self.read(nelem)?;
        Ok((bytes.len(), state))
    }

    async fn ready(&mut self) -> Result<(), anyhow::Error> {
        match self.current_handle.take() {
            Some(handle) => handle.await?,
            None => Ok(()),
        }
    }
}

#[async_trait]
impl HostOutputStream for ManagedStdOut {
    fn write(&mut self, bytes: Bytes) -> Result<(usize, StreamState), anyhow::Error> {
        let mut io_clone = self.io.clone();
        let n = bytes.len();

        self.event_service.emit_stdout(bytes.clone().to_vec());

        let handle = self
            .runtime
            .spawn(async move { io_clone.write(&bytes).await });
        self.current_handle = Some(handle);

        Ok((n, StreamState::Open))
    }

    async fn ready(&mut self) -> anyhow::Result<()> {
        match self.current_handle.take() {
            Some(handle) => handle.await?,
            None => Ok(()),
        }
    }
}

#[async_trait]
impl HostOutputStream for ManagedStdErr {
    fn write(&mut self, bytes: Bytes) -> Result<(usize, StreamState), Error> {
        let result = self.stderr.write(bytes.clone());
        self.event_service.emit_stderr(bytes.to_vec());
        result
    }

    async fn ready(&mut self) -> Result<(), Error> {
        self.stderr.ready().await
    }
}
