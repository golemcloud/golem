use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use golem_worker_executor_base::host::managed_stdio::{ManagedStandardIo, ManagedStreamStatus};
use golem_worker_executor_base::services::worker_event::WorkerEventService;
use tokio::runtime::Handle;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tonic::codegen::Bytes;
use wasmtime_wasi::preview2::{
    HostInputStream, HostOutputStream, Stderr, StdinStream, StdoutStream, StreamError,
    StreamResult, Subscribe,
};

pub mod blobstore;
pub mod golem;
pub mod keyvalue;

#[derive(Clone)]
pub struct ManagedStdIn {
    runtime: Handle,
    state: Arc<Mutex<ManagedStdInState>>,
}

struct ManagedStdInState {
    io: ManagedStandardIo,
    current_handle: Option<JoinHandle<Result<(), anyhow::Error>>>,
    result: Option<tokio::sync::oneshot::Receiver<(Vec<u8>, ManagedStreamStatus)>>,
    last_error: Option<anyhow::Error>,
}

#[derive(Clone)]
pub struct ManagedStdOut {
    runtime: Handle,
    state: Arc<Mutex<ManagedStdOutState>>,
}

struct ManagedStdOutState {
    io: ManagedStandardIo,
    current_handle: Option<JoinHandle<anyhow::Result<()>>>,
    event_service: Arc<dyn WorkerEventService + Send + Sync>,
    last_error: Option<anyhow::Error>,
}

#[derive(Clone)]
pub struct ManagedStdErr {
    runtime: Handle,
    state: Arc<Mutex<ManagedStdErrState>>,
}

struct ManagedStdErrState {
    stderr: Stderr,
    event_service: Arc<dyn WorkerEventService + Send + Sync>,
}

impl ManagedStdIn {
    pub fn from_standard_io(runtime: Handle, io: ManagedStandardIo) -> Self {
        Self {
            runtime,
            state: Arc::new(Mutex::new(ManagedStdInState {
                io,
                current_handle: None,
                result: None,
                last_error: None,
            })),
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
            runtime,
            state: Arc::new(Mutex::new(ManagedStdOutState {
                io,
                current_handle: None,
                event_service,
                last_error: None,
            })),
        }
    }
}

impl ManagedStdErr {
    pub fn from_stderr(
        runtime: Handle,
        stderr: Stderr,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
    ) -> Self {
        Self {
            runtime,
            state: Arc::new(Mutex::new(ManagedStdErrState {
                stderr,
                event_service,
            })),
        }
    }
}

impl StdinStream for ManagedStdIn {
    fn stream(&self) -> Box<dyn HostInputStream> {
        Box::new(self.clone())
    }

    fn isatty(&self) -> bool {
        false
    }
}

impl StdoutStream for ManagedStdOut {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(self.clone())
    }

    fn isatty(&self) -> bool {
        false
    }
}

impl StdoutStream for ManagedStdErr {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(self.clone())
    }

    fn isatty(&self) -> bool {
        false
    }
}

#[async_trait]
impl Subscribe for ManagedStdIn {
    async fn ready(&mut self) {
        match self.state.lock().await.current_handle.take() {
            Some(handle) => {
                if let Err(err) = handle.await {
                    self.state.lock().await.last_error = Some(anyhow!(err));
                }
            }
            None => self.state.lock().await.last_error = None,
        }
    }
}

#[async_trait]
impl HostInputStream for ManagedStdIn {
    fn read(&mut self, size: usize) -> StreamResult<Bytes> {
        let mut state = self.runtime.block_on(self.state.lock());
        if let Some(err) = state.last_error.take() {
            return Err(StreamError::LastOperationFailed(err));
        }
        let mut to_read = None;
        let result = match state.result.take() {
            Some(rx) => {
                let (data, status) = self
                    .runtime
                    .block_on(rx)
                    .map_err(|err| StreamError::Trap(anyhow!(err)))?;
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
                if state.current_handle.is_some() {
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
            let mut io_clone = state.io.clone();
            let (tx, rx) = tokio::sync::oneshot::channel();
            state.result = Some(rx);
            let handle = self.runtime.spawn(async move {
                let mut buf = vec![0u8; to_read];
                let (read, status) = io_clone.read(&mut buf).await?;
                tx.send((buf[0..(read as usize)].to_vec(), status))
                    .map_err(|_| anyhow!("Failed to set read result"))?;
                Ok(())
            });
            state.current_handle = Some(handle);
        }

        Ok(result)
    }
}

#[async_trait]
impl Subscribe for ManagedStdOut {
    async fn ready(&mut self) {
        let mut state = self.state.lock().await;
        match state.current_handle.take() {
            Some(handle) => {
                if let Err(err) = handle.await {
                    state.last_error = Some(anyhow!(err));
                }
            }
            None => state.last_error = None,
        }
    }
}

#[async_trait]
impl HostOutputStream for ManagedStdOut {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        let mut state = self.runtime.block_on(self.state.lock());
        if let Some(err) = state.last_error.take() {
            return Err(StreamError::LastOperationFailed(err));
        }
        let mut io_clone = state.io.clone();

        state.event_service.emit_stdout(bytes.clone().to_vec());

        let handle = self
            .runtime
            .spawn(async move { io_clone.write(&bytes).await });
        state.current_handle = Some(handle);

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
        self.state.lock().await.stderr.stream().ready().await
    }
}

#[async_trait]
impl HostOutputStream for ManagedStdErr {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        let state = self.runtime.block_on(self.state.lock());
        let result = state.stderr.stream().write(bytes.clone());
        state.event_service.emit_stderr(bytes.to_vec());
        result
    }

    fn flush(&mut self) -> StreamResult<()> {
        self.runtime
            .block_on(self.state.lock())
            .stderr
            .stream()
            .flush()
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        self.runtime
            .block_on(self.state.lock())
            .stderr
            .stream()
            .check_write()
    }
}
