use crate::host::managed_stdio::{ManagedStandardIo, ManagedStreamStatus};
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use wasmtime_wasi::preview2::{
    HostInputStream, HostOutputStream, Stderr, StdinStream, StdoutStream, StreamError,
    StreamResult, Subscribe,
};

pub mod error;
pub mod poll;
pub mod streams;

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
                                break;
                            } else {
                                let read = read as usize;
                                if read < demand {
                                    demand -= read;
                                }
                            }
                        }
                        Err(err) => {
                            let _ = incoming_tx.send_async(Err(StreamError::Trap(err))).await;
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
        if self.state.incoming.is_empty()
            && self.state.demand.is_empty()
            && self.state.remainder_rx.is_empty()
        {
            self.state
                .demand
                .send(128)
                .map_err(|err| StreamError::Trap(anyhow!(err)))
                .expect("failed to send initial demand");
        }
        // TODO: get rid of this poll loop
        while self.state.incoming.is_empty() && self.state.remainder_rx.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

#[async_trait]
impl HostInputStream for ManagedStdIn {
    fn read(&mut self, size: usize) -> StreamResult<Bytes> {
        if self.state.incoming.is_empty() && self.state.remainder_rx.is_empty() {
            self.state
                .demand
                .send(size)
                .map_err(|err| StreamError::Trap(anyhow!(err)))?;
            Ok(Bytes::new())
        } else if self.state.remainder_rx.is_empty() {
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

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Clone)]
pub struct ManagedStdOut {
    state: Arc<ManagedStdOutState>,
}

struct ManagedStdOutState {
    outgoing: flume::Sender<Bytes>,
    consumed: Arc<tokio::sync::Notify>,
    dirty: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl ManagedStdOut {
    pub fn from_standard_io(io: ManagedStandardIo) -> Self {
        let consumed = Arc::new(tokio::sync::Notify::new());
        let (outgoing_tx, outgoing_rx) = flume::unbounded();
        let dirty = Arc::new(AtomicBool::new(false));

        let mut io_clone = io.clone();
        let consumed_clone = consumed.clone();
        let dirty_clone = dirty.clone();
        let handle = tokio::spawn(async move {
            loop {
                let bytes: Bytes = outgoing_rx.recv_async().await.unwrap();
                let _ = io_clone.write(&bytes).await;

                if outgoing_rx.is_empty() {
                    dirty_clone.store(false, Ordering::Relaxed);
                    consumed_clone.notify_waiters();
                }
            }
        });

        Self {
            state: Arc::new(ManagedStdOutState {
                outgoing: outgoing_tx,
                consumed,
                dirty,
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
        if self.state.dirty.load(Ordering::Relaxed) {
            self.state.consumed.notified().await;
        }
    }
}

#[async_trait]
impl HostOutputStream for ManagedStdOut {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.state.outgoing.send(bytes).unwrap();
        self.state.dirty.store(true, Ordering::Relaxed);
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
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        self.state.stderr.stream().write(bytes.clone())
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
    state: Arc<ManagedStdErrState>,
}

struct ManagedStdErrState {
    stderr: Stderr,
}

impl ManagedStdErr {
    pub fn from_stderr(stderr: Stderr) -> Self {
        Self {
            state: Arc::new(ManagedStdErrState { stderr }),
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::golem_host::io::{ManagedStdIn, ManagedStdOut};
    use crate::host::managed_stdio::ManagedStandardIo;
    use crate::services::invocation_key::InvocationKeyServiceDefault;
    use bytes::{BufMut, Bytes};
    use golem_common::model::{InvocationKey, TemplateId, WorkerId};
    use uuid::Uuid;
    use wasmtime_wasi::preview2::{HostInputStream, HostOutputStream, StreamError, Subscribe};

    #[tokio::test]
    async fn enqueue_first_and_read() {
        let instance_id = WorkerId {
            template_id: TemplateId(Uuid::new_v4()),
            worker_name: "test".to_string(),
        };
        let invocation_key_service = Arc::new(InvocationKeyServiceDefault::new());
        let stdio = ManagedStandardIo::new(instance_id, invocation_key_service);

        let msg1 = Bytes::from("hello\n".to_string());
        let key1 = InvocationKey::new("key1".to_string());

        let msg2 = Bytes::from("world\n".to_string());
        let key2 = InvocationKey::new("key2".to_string());

        stdio.enqueue(msg1, key1).await;
        stdio.enqueue(msg2, key2).await;

        let mut input = ManagedStdIn::from_standard_io(stdio.clone()).await;
        let mut output = ManagedStdOut::from_standard_io(stdio.clone());

        let out1 = read_until_newline(&mut input).await;
        output.ready().await;
        output.write("ok\n".as_bytes().into()).unwrap();
        output.flush().unwrap();
        output.ready().await;

        let out2 = read_until_newline(&mut input).await;
        output.ready().await;
        output.write("ok\n".as_bytes().into()).unwrap();
        output.flush().unwrap();
        output.ready().await;

        assert_eq!(out1, "hello\n");
        assert_eq!(out2, "world\n");
    }

    #[tokio::test]
    async fn enqueue_after_first_read() {
        let instance_id = WorkerId {
            template_id: TemplateId(Uuid::new_v4()),
            worker_name: "test".to_string(),
        };
        let invocation_key_service = Arc::new(InvocationKeyServiceDefault::new());
        let stdio = ManagedStandardIo::new(instance_id, invocation_key_service);

        let msg1 = Bytes::from("hello\n".to_string());
        let key1 = InvocationKey::new("key1".to_string());

        let msg2 = Bytes::from("world\n".to_string());
        let key2 = InvocationKey::new("key2".to_string());

        stdio.enqueue(msg1, key1).await;

        let mut input = ManagedStdIn::from_standard_io(stdio.clone()).await;
        let mut output = ManagedStdOut::from_standard_io(stdio.clone());

        let out1 = read_until_newline(&mut input).await;
        output.ready().await;
        output.write("ok\n".as_bytes().into()).unwrap();
        output.flush().unwrap();
        output.ready().await;

        let handle = tokio::spawn(async move {
            println!("sleep..");
            tokio::time::sleep(Duration::from_secs(2)).await;
            println!("awake..");
            stdio.enqueue(msg2, key2).await;
        });

        let out2 = read_until_newline(&mut input).await;
        output.ready().await;
        output.write("ok\n".as_bytes().into()).unwrap();
        output.flush().unwrap();
        output.ready().await;

        handle.await.unwrap();

        assert_eq!(out1, "hello\n");
        assert_eq!(out2, "world\n");
    }

    async fn read_until_newline(stream: &mut ManagedStdIn) -> String {
        let mut result = vec![];

        loop {
            stream.ready().await;
            match stream.read(1) {
                Ok(buf) => {
                    result.put_slice(&buf);
                    if result.ends_with(&[10]) {
                        break;
                    }
                }
                Err(StreamError::Closed) => {
                    break;
                }
                Err(err) => {
                    panic!("unexpected error: {err}")
                }
            }
        }

        String::from_utf8(result).unwrap()
    }
}
