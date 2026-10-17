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

use crate::durable_host::concurrent::{CallReplayOutcome, DurableCallSession, NotCancellable};
use crate::durable_host::io::native_wait::NativeIoWait;
use crate::durable_host::io::{ManagedStdErr, ManagedStdOut};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::model::event::InternalWorkerEvent;
use crate::native_wait_diagnostics::WaitDiagnostic;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions::{
    FilesystemInputStreamRead, FilesystemInputStreamSkip, FilesystemOutputStreamCheckWrite,
};
use golem_common::model::oplog::types::SerializableStreamError;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestNoInput, HostResponseStreamCheckWrite, HostResponseStreamSkip,
};
use wasmtime::component::Resource;
use wasmtime_wasi::StreamError;
use wasmtime_wasi::p2::bindings::io::streams::{
    Host, HostInputStream, HostOutputStream, InputStream, OutputStream, Pollable,
};

// Raw TCP input and raw TCP splice have no durable call to complete.
// Race only their native wait, never file operations or resource deletion.
pub(super) async fn interruptible_tcp_input<T>(
    native: impl Future<Output = Result<T, StreamError>>,
    wait: NativeIoWait,
) -> Result<T, StreamError> {
    wait.wait(native)
        .await
        .unwrap_or_else(|kind| Err(StreamError::Trap(wasmtime::Error::from_anyhow(kind.into()))))
}

impl<Ctx: WorkerCtx> HostInputStream for DurableWorkerCtx<Ctx> {
    async fn read(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<Vec<u8>, StreamError> {
        let handle = self_.rep();
        self.observe_function_call("io::streams::input_stream", "read");
        if self.state.open_filesystem_input_streams.contains(&handle) {
            // Record scheduling-dependent chunk lengths, then re-read the bytes
            // from the restored filesystem during replay.
            let call = DurableCallSession::<FilesystemInputStreamRead, NotCancellable>::start(
                self,
                HostRequestNoInput {},
                DurableFunctionType::ReadLocal,
            )
            .await?;
            let call = if call.is_live() {
                call
            } else {
                match call.replay(self).await? {
                    CallReplayOutcome::Replayed(recorded) => {
                        return match recorded.result {
                            Ok(recorded_len) => {
                                replay_file_stream_read(self, handle, recorded_len).await
                            }
                            Err(err) => Err(StreamError::from(err)),
                        };
                    }
                    CallReplayOutcome::Incomplete(call) => call,
                }
            };
            let result = HostInputStream::read(self.table(), self_, len).await;
            call.complete(
                self,
                HostResponseStreamSkip {
                    result: result
                        .as_ref()
                        .map(|bytes| bytes.len() as u64)
                        .map_err(SerializableStreamError::from),
                },
            )
            .await?;
            result
        } else {
            HostInputStream::read(self.table(), self_, len).await
        }
    }

    async fn blocking_read(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<Vec<u8>, StreamError> {
        self.observe_function_call("io::streams::input_stream", "blocking_read");
        if self.state.open_tcp_input_streams.contains(&self_.rep()) {
            let diagnostic = self.native_wait_diagnostic(
                "blocking_read",
                Some(self_.rep()),
                None,
                std::iter::empty(),
            );
            let interrupt = NativeIoWait::new(self.interrupt_sources());
            let native = WaitDiagnostic::observe(
                diagnostic,
                HostInputStream::blocking_read(self.table(), self_, len),
            );
            interruptible_tcp_input(native, interrupt).await
        } else if self
            .state
            .open_filesystem_input_streams
            .contains(&self_.rep())
        {
            let interrupt = self.create_interrupt_signal();
            observe_file_stream(
                interrupt,
                HostInputStream::blocking_read(self.table(), self_, len),
            )
            .await
        } else {
            HostInputStream::blocking_read(self.table(), self_, len).await
        }
    }

    async fn skip(&mut self, self_: Resource<InputStream>, len: u64) -> Result<u64, StreamError> {
        let handle = self_.rep();
        self.observe_function_call("io::streams::input_stream", "skip");
        if self.state.open_filesystem_input_streams.contains(&handle) {
            let call = DurableCallSession::<FilesystemInputStreamSkip, NotCancellable>::start(
                self,
                HostRequestNoInput {},
                DurableFunctionType::ReadLocal,
            )
            .await?;
            let call = if call.is_live() {
                call
            } else {
                match call.replay(self).await? {
                    CallReplayOutcome::Replayed(recorded) => {
                        return match recorded.result {
                            Ok(recorded_len) => {
                                replay_file_stream_skip(self, handle, recorded_len).await
                            }
                            Err(err) => Err(StreamError::from(err)),
                        };
                    }
                    CallReplayOutcome::Incomplete(call) => call,
                }
            };
            let result = HostInputStream::skip(self.table(), self_, len).await;
            call.complete(
                self,
                HostResponseStreamSkip {
                    result: result
                        .as_ref()
                        .copied()
                        .map_err(SerializableStreamError::from),
                },
            )
            .await?;
            result
        } else {
            HostInputStream::skip(self.table(), self_, len).await
        }
    }

    async fn blocking_skip(
        &mut self,
        self_: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        self.observe_function_call("io::streams::input_stream", "blocking_skip");
        if self.state.open_tcp_input_streams.contains(&self_.rep()) {
            let diagnostic = self.native_wait_diagnostic(
                "blocking_skip",
                Some(self_.rep()),
                None,
                std::iter::empty(),
            );
            let interrupt = NativeIoWait::new(self.interrupt_sources());
            let native = WaitDiagnostic::observe(
                diagnostic,
                HostInputStream::blocking_skip(self.table(), self_, len),
            );
            interruptible_tcp_input(native, interrupt).await
        } else if self
            .state
            .open_filesystem_input_streams
            .contains(&self_.rep())
        {
            let interrupt = self.create_interrupt_signal();
            observe_file_stream(
                interrupt,
                HostInputStream::blocking_skip(self.table(), self_, len),
            )
            .await
        } else {
            HostInputStream::blocking_skip(self.table(), self_, len).await
        }
    }

    fn subscribe(&mut self, self_: Resource<InputStream>) -> wasmtime::Result<Resource<Pollable>> {
        self.observe_function_call("io::streams::input_stream", "subscribe");
        let is_file_stream = self
            .state
            .open_filesystem_input_streams
            .contains(&self_.rep());
        let pollable = HostInputStream::subscribe(self.table(), self_)?;
        if is_file_stream {
            self.state.file_stream_pollables.insert(pollable.rep());
        }
        Ok(pollable)
    }

    async fn drop(&mut self, rep: Resource<InputStream>) -> wasmtime::Result<()> {
        self.observe_function_call("io::streams::input_stream", "drop");
        let stream_rep = rep.rep();
        let result = HostInputStream::drop(self.table(), rep).await;
        if result.is_ok() {
            // Reps can be reused only after a successful drop.
            self.state.open_tcp_input_streams.remove(&stream_rep);
            self.state.open_filesystem_input_streams.remove(&stream_rep);
        }
        result
    }
}

impl<Ctx: WorkerCtx> HostOutputStream for DurableWorkerCtx<Ctx> {
    async fn check_write(&mut self, self_: Resource<OutputStream>) -> Result<u64, StreamError> {
        self.observe_function_call("io::streams::output_stream", "check_write");
        if self
            .state
            .open_filesystem_output_streams
            .contains_key(&self_.rep())
        {
            let call =
                DurableCallSession::<FilesystemOutputStreamCheckWrite, NotCancellable>::start(
                    self,
                    HostRequestNoInput {},
                    DurableFunctionType::ReadLocal,
                )
                .await?;
            let call = if call.is_live() {
                call
            } else {
                match call.replay(self).await? {
                    CallReplayOutcome::Replayed(recorded) => {
                        return match recorded.result {
                            Ok(0) => Ok(0),
                            Ok(budget) => {
                                let interrupt = self.create_interrupt_signal();
                                observe_file_stream(
                                    interrupt,
                                    self.table().get_mut(&self_)?.write_ready(),
                                )
                                .await?;
                                Ok(budget)
                            }
                            Err(err) => Err(StreamError::from(err)),
                        };
                    }
                    CallReplayOutcome::Incomplete(call) => call,
                }
            };
            let result = HostOutputStream::check_write(self.table(), self_).await;
            call.complete(
                self,
                HostResponseStreamCheckWrite {
                    result: result
                        .as_ref()
                        .copied()
                        .map_err(SerializableStreamError::from),
                },
            )
            .await?;
            result
        } else {
            HostOutputStream::check_write(self.table(), self_).await
        }
    }

    async fn write(
        &mut self,
        self_: Resource<OutputStream>,
        contents: Vec<u8>,
    ) -> Result<(), StreamError> {
        self.observe_function_call("io::streams::output_stream", "write");
        let output = self.table().get(&self_)?;
        let event = if output.as_any().downcast_ref::<ManagedStdOut>().is_some() {
            Some(InternalWorkerEvent::stdout(contents.clone()))
        } else if output.as_any().downcast_ref::<ManagedStdErr>().is_some() {
            Some(InternalWorkerEvent::stderr(contents.clone()))
        } else {
            None
        };
        if let Some(event) = event {
            self.emit_log_event(event).await;
            Ok(())
        } else {
            HostOutputStream::write(self.table(), self_, contents).await
        }
    }

    async fn blocking_write_and_flush(
        &mut self,
        self_: Resource<OutputStream>,
        contents: Vec<u8>,
    ) -> Result<(), StreamError> {
        let self2 = Resource::new_borrow(self_.rep());
        HostOutputStream::write(self, self_, contents).await?;
        self.blocking_flush(self2).await
    }

    async fn flush(&mut self, self_: Resource<OutputStream>) -> Result<(), StreamError> {
        self.observe_function_call("io::streams::output_stream", "flush");
        HostOutputStream::flush(self.table(), self_).await
    }

    async fn blocking_flush(&mut self, self_: Resource<OutputStream>) -> Result<(), StreamError> {
        self.observe_function_call("io::streams::output_stream", "blocking_flush");
        if self
            .state
            .open_filesystem_output_streams
            .contains_key(&self_.rep())
        {
            let interrupt = self.create_interrupt_signal();
            observe_file_stream(
                interrupt,
                HostOutputStream::blocking_flush(self.table(), self_),
            )
            .await
        } else {
            HostOutputStream::blocking_flush(self.table(), self_).await
        }
    }

    fn subscribe(&mut self, self_: Resource<OutputStream>) -> wasmtime::Result<Resource<Pollable>> {
        self.observe_function_call("io::streams::output_stream", "subscribe");
        let is_file_stream = self
            .state
            .open_filesystem_output_streams
            .contains_key(&self_.rep());
        let pollable = HostOutputStream::subscribe(self.table(), self_)?;
        if is_file_stream {
            self.state.file_stream_pollables.insert(pollable.rep());
        }
        Ok(pollable)
    }

    async fn write_zeroes(
        &mut self,
        self_: Resource<OutputStream>,
        len: u64,
    ) -> Result<(), StreamError> {
        self.observe_function_call("io::streams::output_stream", "write_zeroes");
        HostOutputStream::write_zeroes(self.table(), self_, len).await
    }

    async fn blocking_write_zeroes_and_flush(
        &mut self,
        self_: Resource<OutputStream>,
        len: u64,
    ) -> Result<(), StreamError> {
        let self2 = Resource::new_borrow(self_.rep());
        self.write_zeroes(self_, len).await?;
        self.blocking_flush(self2).await
    }

    async fn splice(
        &mut self,
        self_: Resource<OutputStream>,
        src: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        self.observe_function_call("io::streams::output_stream", "splice");
        HostOutputStream::splice(self.table(), self_, src, len).await
    }

    async fn blocking_splice(
        &mut self,
        self_: Resource<OutputStream>,
        src: Resource<InputStream>,
        len: u64,
    ) -> Result<u64, StreamError> {
        self.observe_function_call("io::streams::output_stream", "blocking_splice");
        let rep = self_.rep();
        if self.state.open_filesystem_output_streams.contains_key(&rep) {
            let file_pair = self
                .state
                .open_filesystem_input_streams
                .contains(&src.rep());
            let readiness = if file_pair {
                let interrupt = self.create_interrupt_signal();
                observe_file_stream(interrupt, self.table().get_mut(&self_)?.write_ready()).await
            } else {
                self.table().get_mut(&self_)?.write_ready().await
            };
            readiness?;
        }
        let tcp_input = self.state.open_tcp_input_streams.contains(&src.rep());
        let raw_tcp_output = self.state.open_tcp_output_streams.contains(&rep);
        let diagnostic = (tcp_input && raw_tcp_output)
            .then(|| {
                self.native_wait_diagnostic(
                    "blocking_splice",
                    Some(src.rep()),
                    None,
                    std::iter::empty(),
                )
            })
            .flatten();
        let file_pair = self
            .state
            .open_filesystem_input_streams
            .contains(&src.rep())
            && self.state.open_filesystem_output_streams.contains_key(&rep);
        let file_interrupt = file_pair.then(|| self.create_interrupt_signal());
        let interrupt =
            (tcp_input && raw_tcp_output).then(|| NativeIoWait::new(self.interrupt_sources()));
        let native = WaitDiagnostic::observe(
            diagnostic,
            HostOutputStream::blocking_splice(self.table(), self_, src, len),
        );
        match (interrupt, file_interrupt) {
            (Some(interrupt), _) => interruptible_tcp_input(native, interrupt).await,
            (_, Some(interrupt)) => observe_file_stream(interrupt, native).await,
            _ => native.await,
        }
    }

    async fn drop(&mut self, rep: Resource<OutputStream>) -> wasmtime::Result<()> {
        self.observe_function_call("io::streams::output_stream", "drop");
        let handle = rep.rep();
        let result = HostOutputStream::drop(self.table(), rep).await;
        if result.is_ok() {
            self.state.open_tcp_output_streams.remove(&handle);
            self.state.open_filesystem_output_streams.remove(&handle);
        }
        result
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn convert_stream_error(
        &mut self,
        err: StreamError,
    ) -> wasmtime::Result<wasmtime_wasi::p2::bindings::io::streams::StreamError> {
        Host::convert_stream_error(self.table(), err)
    }
}

async fn observe_file_stream<T>(
    interrupt: crate::wasi_filesystem::FilesystemInterruptSignal,
    native: impl std::future::Future<Output = Result<T, StreamError>>,
) -> Result<T, StreamError> {
    crate::wasi_filesystem::observe_filesystem_operation(interrupt, native)
        .await
        .map_err(|kind| StreamError::Trap(wasmtime::Error::from(kind)))?
}

/// Re-reads exactly `recorded_len` bytes from the restored file during replay.
async fn replay_file_stream_read<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
    recorded_len: u64,
) -> Result<Vec<u8>, StreamError> {
    let mut collected = Vec::with_capacity(recorded_len as usize);
    while (collected.len() as u64) < recorded_len {
        let remaining = recorded_len - collected.len() as u64;
        let stream = Resource::<InputStream>::new_borrow(handle);
        let interrupt = ctx.create_interrupt_signal();
        match observe_file_stream(
            interrupt,
            HostInputStream::blocking_read(ctx.table(), stream, remaining),
        )
        .await
        {
            Ok(chunk) if !chunk.is_empty() => collected.extend_from_slice(&chunk),
            Ok(_) | Err(StreamError::Closed) => {
                return Err(file_stream_replay_divergence(
                    collected.len() as u64,
                    recorded_len,
                ));
            }
            Err(err) => return Err(err),
        }
    }
    Ok(collected)
}

async fn replay_file_stream_skip<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
    recorded_len: u64,
) -> Result<u64, StreamError> {
    let mut skipped = 0;
    while skipped < recorded_len {
        let remaining = recorded_len - skipped;
        let stream = Resource::<InputStream>::new_borrow(handle);
        let interrupt = ctx.create_interrupt_signal();
        match observe_file_stream(
            interrupt,
            HostInputStream::blocking_skip(ctx.table(), stream, remaining),
        )
        .await
        {
            Ok(n) if n > 0 => skipped += n,
            Ok(_) | Err(StreamError::Closed) => {
                return Err(file_stream_replay_divergence(skipped, recorded_len));
            }
            Err(err) => return Err(err),
        }
    }
    Ok(skipped)
}

fn file_stream_replay_divergence(got: u64, expected: u64) -> StreamError {
    StreamError::Trap(wasmtime::Error::msg(format!(
        "file stream replay divergence: the file provided {got} bytes but {expected} bytes were recorded"
    )))
}

#[cfg(test)]
mod tcp_input_tests {
    use super::{NativeIoWait, interruptible_tcp_input};
    use golem_common::model::Timestamp;
    use golem_service_base::error::worker_executor::InterruptKind;
    use std::future::{pending, poll_fn, ready};
    use std::task::Poll;
    use test_r::{test, timeout};
    use wasmtime_wasi::StreamError;

    fn assert_interrupt<T: std::fmt::Debug>(
        result: Result<T, StreamError>,
        expected: InterruptKind,
    ) {
        match result {
            Err(StreamError::Trap(error)) => {
                assert_eq!(error.downcast_ref::<InterruptKind>(), Some(&expected))
            }
            other => panic!("expected typed interrupt trap, got {other:?}"),
        }
    }

    #[test]
    #[timeout("10s")]
    async fn prepublished_signal_interrupts_tcp_input() {
        let kind = InterruptKind::Suspend(Timestamp::now_utc());
        assert_interrupt(
            interruptible_tcp_input(
                pending::<Result<u64, StreamError>>(),
                NativeIoWait::with_signal(ready(kind)),
            )
            .await,
            kind,
        );
    }

    #[test]
    #[timeout("10s")]
    async fn signal_interrupts_pending_tcp_input() {
        let kind = InterruptKind::Suspend(Timestamp::now_utc());
        let (pending_tx, pending_rx) = tokio::sync::oneshot::channel();
        let (signal_tx, signal_rx) = tokio::sync::oneshot::channel();
        let mut pending_tx = Some(pending_tx);
        let native = poll_fn(|_| {
            if let Some(tx) = pending_tx.take() {
                let _ = tx.send(());
            }
            Poll::<Result<u64, StreamError>>::Pending
        });
        let input = interruptible_tcp_input(
            native,
            NativeIoWait::with_signal(async { signal_rx.await.unwrap() }),
        );
        let trigger = async {
            pending_rx.await.unwrap();
            signal_tx.send(kind).unwrap();
        };
        let (result, ()) = tokio::join!(input, trigger);
        assert_interrupt(result, kind);
    }

    #[test]
    #[timeout("10s")]
    async fn tcp_input_completion_before_signal_keeps_result() {
        let (signal_tx, signal_rx) = tokio::sync::oneshot::channel();
        let result = interruptible_tcp_input(
            ready(Ok::<_, StreamError>(vec![b'x'])),
            NativeIoWait::with_signal(async { signal_rx.await.unwrap() }),
        )
        .await;
        assert_eq!(result.unwrap(), vec![b'x']);
        assert!(
            signal_tx
                .send(InterruptKind::Suspend(Timestamp::now_utc()))
                .is_err()
        );
    }
}

#[cfg(test)]
mod file_observation_tests {
    use super::*;
    use golem_service_base::error::worker_executor::InterruptKind;
    use test_r::test;

    #[test]
    #[test_r::timeout("30s")]
    async fn pending_file_stream_observation_preserves_typed_stop() {
        let kind = InterruptKind::Suspend(golem_common::model::Timestamp::now_utc());
        let error = observe_file_stream(
            Box::pin(std::future::ready(kind)),
            std::future::pending::<Result<(), StreamError>>(),
        )
        .await
        .unwrap_err();
        let StreamError::Trap(error) = error else {
            panic!("typed trap expected");
        };
        assert_eq!(error.downcast_ref::<InterruptKind>(), Some(&kind));
    }

    #[test]
    async fn file_stream_completion_wins_before_stop() {
        let kind = InterruptKind::Suspend(golem_common::model::Timestamp::now_utc());
        assert_eq!(
            observe_file_stream(
                Box::pin(std::future::ready(kind)),
                std::future::ready(Ok(19))
            )
            .await
            .unwrap(),
            19
        );
    }
}
