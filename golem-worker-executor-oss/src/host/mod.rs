use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use async_trait::async_trait;
use golem_worker_executor_base::host::managed_stdio::{ManagedStandardIo, ManagedStreamStatus};
use golem_worker_executor_base::services::worker_event::WorkerEventService;
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
    state: Arc<ManagedStdInState>,
}

struct ManagedStdInState {
    incoming: flume::Receiver<Result<Bytes, StreamError>>,
    demand: flume::Sender<usize>,
    remainder_rx: flume::Receiver<Bytes>,
    remainder_tx: flume::Sender<Bytes>,
    handle: JoinHandle<()>,
}

impl ManagedStdIn {
    pub async fn from_standard_io(io: ManagedStandardIo) -> Self {
        let (demand_tx, demand_rx) = flume::unbounded();
        let (incoming_tx, incoming_rx) = flume::unbounded();
        let (remainder_tx, remainder_rx) = flume::unbounded();

        let mut io_clone = io.clone();
        let handle = tokio::spawn(async move {
            loop {
                let mut demand = match demand_rx.recv_async().await {
                    Ok(demand) => demand,
                    Err(err) => {
                        let _ = incoming_tx.send(Err(StreamError::Trap(anyhow!(err))));
                        break;
                    }
                };

                while demand > 0 {
                    let mut buf = vec![0u8; demand];
                    match io_clone.read(&mut buf).await {
                        Ok((read, status)) => {
                            let _ = incoming_tx
                                .send_async(Ok(Bytes::from(buf[0..(read as usize)].to_vec())))
                                .await;
                            if status == ManagedStreamStatus::Ended {
                                let _ = incoming_tx.send_async(Err(StreamError::Closed)).await;
                                break;
                            } else {
                                let read = read as usize;
                                if read < demand {
                                    demand -= read;
                                }
                            }
                        }
                        Err(err) => {
                            let _ = incoming_tx
                                .send_async(Err(StreamError::Trap(anyhow!(err))))
                                .await;
                            break;
                        }
                    }
                }
            }
        });
        Self {
            state: Arc::new(ManagedStdInState {
                incoming: incoming_rx,
                demand: demand_tx,
                remainder_rx,
                remainder_tx,
                handle,
            }),
        }
    }
}

impl Drop for ManagedStdIn {
    fn drop(&mut self) {
        self.state.handle.abort();
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

#[async_trait]
impl Subscribe for ManagedStdIn {
    async fn ready(&mut self) {
        while self.state.incoming.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

#[async_trait]
impl HostInputStream for ManagedStdIn {
    fn read(&mut self, size: usize) -> StreamResult<Bytes> {
        if self.state.incoming.is_empty() && self.state.remainder_rx.is_empty() {
            let _ = self
                .state
                .demand
                .send(size)
                .map_err(|err| StreamError::Trap(anyhow!(err)))?;
            Ok(Bytes::new())
        } else {
            if self.state.remainder_rx.is_empty() {
                match self.state.incoming.recv() {
                    Ok(Ok(bytes)) => {
                        if bytes.len() > size {
                            let (bytes1, bytes2) = bytes.split_at(size);
                            let _ = self.state.remainder_tx.send(Bytes::copy_from_slice(bytes2));
                            Ok(Bytes::from(bytes1.to_vec()))
                        } else {
                            Ok(bytes)
                        }
                    }
                    Ok(Err(err)) => Err(err),
                    Err(err) => Err(StreamError::Trap(anyhow!(err))),
                }
            } else {
                match self.state.remainder_rx.recv() {
                    Ok(bytes) => {
                        if bytes.len() > size {
                            let (bytes1, bytes2) = bytes.split_at(size);
                            let _ = self.state.remainder_tx.send(Bytes::copy_from_slice(bytes2));
                            Ok(Bytes::from(bytes1.to_vec()))
                        } else {
                            Ok(bytes)
                        }
                    }
                    Err(err) => Err(StreamError::Trap(anyhow!(err))),
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct ManagedStdOut {
    state: Arc<ManagedStdOutState>,
}

struct ManagedStdOutState {
    outgoing: flume::Sender<Bytes>,
    consumed: Arc<tokio::sync::Notify>,
    handle: JoinHandle<()>,
}

impl ManagedStdOut {
    pub fn from_standard_io(
        io: ManagedStandardIo,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
    ) -> Self {
        let consumed = Arc::new(tokio::sync::Notify::new());
        consumed.notify_one();
        let (outgoing_tx, outgoing_rx) = flume::unbounded();

        let mut io_clone = io.clone();
        let consumed_clone = consumed.clone();
        let handle = tokio::spawn(async move {
            loop {
                let bytes: Bytes = outgoing_rx.recv_async().await.unwrap();
                let _ = io_clone.write(&bytes).await;
                event_service.emit_stdout(bytes.to_vec());
                let _ = consumed_clone.notify_one();
            }
        });

        Self {
            state: Arc::new(ManagedStdOutState {
                outgoing: outgoing_tx,
                consumed,
                handle,
            }),
        }
    }
}

impl Drop for ManagedStdOut {
    fn drop(&mut self) {
        self.state.handle.abort();
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

#[async_trait]
impl Subscribe for ManagedStdOut {
    async fn ready(&mut self) {
        if !self.state.outgoing.is_empty() {
            self.state.consumed.notified().await;
        }
    }
}

#[async_trait]
impl HostOutputStream for ManagedStdOut {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.state.outgoing.send(bytes).unwrap();
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
        self.state.stderr.stream().ready().await
    }
}

#[async_trait]
impl HostOutputStream for ManagedStdErr {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        let result = self.state.stderr.stream().write(bytes.clone());
        self.state.event_service.emit_stderr(bytes.to_vec());
        result
    }

    fn flush(&mut self) -> StreamResult<()> {
        self.state.stderr.stream().flush()
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        self.state.stderr.stream().check_write()
    }
}

#[derive(Clone)]
pub struct ManagedStdErr {
    state: Arc<crate::host::ManagedStdErrState>,
}

struct ManagedStdErrState {
    stderr: Stderr,
    event_service: Arc<dyn WorkerEventService + Send + Sync>,
}

impl ManagedStdErr {
    pub fn from_stderr(
        stderr: Stderr,
        event_service: Arc<dyn WorkerEventService + Send + Sync>,
    ) -> Self {
        Self {
            state: Arc::new(ManagedStdErrState {
                stderr,
                event_service,
            }),
        }
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
