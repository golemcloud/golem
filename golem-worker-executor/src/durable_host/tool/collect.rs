// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use crate::preview2::golem::tool::host::{ByteStreamFailure, CollectedOutput};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::oneshot;
use wasmtime::component::{Source, StreamConsumer, StreamResult};
use wasmtime::{AsContextMut, StoreContextMut};

/// Consumes the existing journaled stream rather than reading the raw
/// attachment. Its retained prefix cannot grow beyond the per-channel limit.
pub(super) struct OutputCollector {
    limit: usize,
    output: CollectedOutput,
    completed: Option<oneshot::Sender<CollectedOutput>>,
    cancel: Box<dyn Fn() + Send + 'static>,
}

impl OutputCollector {
    pub(super) fn new(
        limit: usize,
        cancel: impl Fn() + Send + 'static,
    ) -> (Self, oneshot::Receiver<CollectedOutput>) {
        let (completed, receiver) = oneshot::channel();
        (
            Self {
                limit,
                output: CollectedOutput {
                    bytes: Vec::new(),
                    failure: None,
                    overflow: false,
                },
                completed: Some(completed),
                cancel: Box::new(cancel),
            },
            receiver,
        )
    }

    fn accept(&mut self, item: Result<Vec<u8>, ByteStreamFailure>) {
        match item {
            Ok(bytes) => {
                let remaining = self.limit - self.output.bytes.len();
                self.output
                    .bytes
                    .extend_from_slice(&bytes[..remaining.min(bytes.len())]);
                if bytes.len() > remaining && !self.output.overflow {
                    self.output.overflow = true;
                    (self.cancel)();
                }
            }
            Err(failure) => {
                if self.output.failure.is_none() {
                    self.output.failure = Some(failure);
                }
            }
        }
    }
}

impl<D> StreamConsumer<D> for OutputCollector {
    type Item = Result<Vec<u8>, ByteStreamFailure>;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        mut store: StoreContextMut<'_, D>,
        mut source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if finish {
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }
        let mut received = Vec::with_capacity(1);
        if source.remaining(store.as_context_mut()) != 0 {
            source.read(store.as_context_mut(), &mut received)?;
        }
        for item in received {
            self.accept(item);
        }
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

impl Drop for OutputCollector {
    fn drop(&mut self) {
        if let Some(completed) = self.completed.take() {
            let output = CollectedOutput {
                bytes: std::mem::take(&mut self.output.bytes),
                failure: self.output.failure.take(),
                overflow: self.output.overflow,
            };
            let _ = completed.send(output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    #[test]
    fn retains_binary_prefix_and_cancels_once_on_overflow() {
        let cancellations = Arc::new(AtomicUsize::new(0));
        let count = cancellations.clone();
        let (mut collector, mut received) = OutputCollector::new(4, move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        collector.accept(Ok(vec![0, 255]));
        collector.accept(Ok(vec![128, 0]));
        assert_eq!(cancellations.load(Ordering::SeqCst), 0);
        collector.accept(Ok(vec![1, 2]));
        collector.accept(Ok(vec![3]));
        collector.accept(Err(ByteStreamFailure::Cancelled));
        drop(collector);
        let output = received.try_recv().unwrap();
        assert_eq!(output.bytes, [0, 255, 128, 0]);
        assert!(output.overflow);
        assert!(matches!(output.failure, Some(ByteStreamFailure::Cancelled)));
        assert_eq!(cancellations.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn zero_limit_accepts_empty_output_but_not_a_byte() {
        let (mut collector, mut received) = OutputCollector::new(0, || {});
        collector.accept(Ok(vec![]));
        assert!(!collector.output.overflow);
        collector.accept(Ok(vec![0]));
        drop(collector);
        let output = received.try_recv().unwrap();
        assert!(output.bytes.is_empty());
        assert!(output.overflow);
    }
}
