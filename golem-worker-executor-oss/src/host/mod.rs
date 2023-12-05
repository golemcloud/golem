use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use golem_worker_executor_base::host::managed_stdio::{ManagedStandardIo, ManagedStreamStatus};
use golem_worker_executor_base::services::worker_event::WorkerEventService;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tonic::codegen::Bytes;
use wasmtime_wasi::preview2::{HostInputStream, HostOutputStream, Stderr, StdinStream, StdoutStream, StreamError, StreamResult, Subscribe};

pub mod blobstore;
pub mod golem;
pub mod keyvalue;

pub struct ManagedStdIn {
    io: ManagedStandardIo,
    runtime: Handle,
    current_handle: Option<JoinHandle<Result<(), anyhow::Error>>>,
    result: Option<tokio::sync::oneshot::Receiver<(Vec<u8>, ManagedStreamStatus)>>,
    last_error: Option<anyhow::Error>,
}

pub struct ManagedStdOut {
    io: ManagedStandardIo,
    runtime: Handle,
    current_handle: Option<JoinHandle<anyhow::Result<()>>>,
    event_service: Arc<dyn WorkerEventService + Send + Sync>,
    last_error: Option<anyhow::Error>,
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
            last_error: None,
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
            last_error: None,
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

impl StdinStream for ManagedStdIn {
    fn stream(&self) -> Box<dyn HostInputStream> {
        Box::new(self)
    }

    fn isatty(&self) -> bool {
        false
    }
}

impl StdoutStream for ManagedStdOut {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(self)
    }

    fn isatty(&self) -> bool {
        false
    }
}

impl StdoutStream for ManagedStdErr {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(self)
    }

    fn isatty(&self) -> bool {
        false
    }
}

#[async_trait]
impl Subscribe for ManagedStdIn {
    async fn ready(&mut self) {
        match self.current_handle.take() {
            Some(handle) =>
                if let Err(err) = handle.await {
                    self.last_error = Some(anyhow!(err));
                },
            None => self.last_error = None,
        }
    }
}

#[async_trait]
impl HostInputStream for ManagedStdIn {
    fn read(&mut self, size: usize) -> StreamResult<Bytes> {
        if let Some(err) = self.last_error.take() {
            return Err(StreamError::LastOperationFailed(err));
        }
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
                if data.is_empty() && status == ManagedStreamStatus::Ended {
                    return Err(StreamError::Closed);
                } else {
                    Bytes::from(data)
                }
            }
            None => {
                if self.current_handle.is_some() {
                    // There is a read or skip already in progress, so we just return 0 bytes
                    Bytes::new()
                } else {
                    // We need to initiate a new async read
                    to_read = Some(size);
                    Bytes::new()
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
}

#[async_trait]
impl Subscribe for ManagedStdOut {
    async fn ready(&mut self) {
        match self.current_handle.take() {
            Some(handle) =>
                if let Err(err) = handle.await {
                    self.last_error = Some(anyhow!(err));
                },
            None => self.last_error = None,
        }
    }
}

#[async_trait]
impl HostOutputStream for ManagedStdOut {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        if let Some(err) = self.last_error.take() {
            return Err(StreamError::LastOperationFailed(err));
        }
        let mut io_clone = self.io.clone();

        self.event_service.emit_stdout(bytes.clone().to_vec());

        let handle = self
            .runtime
            .spawn(async move { io_clone.write(&bytes).await });
        self.current_handle = Some(handle);

        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        Ok(())
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        Ok(usize::MAX)
    }
}

#[async_trait]
impl Subscribe for ManagedStdErr {
    async fn ready(&mut self) {
        self.stderr.stream().ready().await
    }
}

#[async_trait]
impl HostOutputStream for ManagedStdErr {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        let result = self.stderr.stream().write(bytes.clone());
        self.event_service.emit_stderr(bytes.to_vec());
        result
    }

    fn flush(&mut self) -> StreamResult<()> {
        self.stderr.stream().flush()
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        self.stderr.stream().check_write()
    }
}
