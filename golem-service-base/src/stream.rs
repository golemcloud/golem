// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use futures::stream;
use futures::Stream;
use pin_project::pin_project;
use std::error::Error;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::task::{Context, Poll};
use tracing::error;

pub struct ByteStream {
    inner: Box<dyn Stream<Item = Result<Vec<u8>, anyhow::Error>> + Unpin + Send + Sync>,
    error: AtomicBool,
}

impl Stream for ByteStream {
    type Item = Result<Vec<u8>, anyhow::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if *self.error.get_mut() {
            Poll::Ready(None)
        } else {
            let result = Pin::new(&mut self.inner).poll_next(cx);
            if let Poll::Ready(Some(Err(_))) = result {
                self.error.store(true, std::sync::atomic::Ordering::Relaxed)
            }
            result
        }
    }
}

impl ByteStream {
    pub fn new(
        stream: impl Stream<Item = Result<Vec<u8>, anyhow::Error>> + Unpin + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Box::new(stream),
            error: AtomicBool::new(false),
        }
    }

    pub fn empty() -> Self {
        Self::new(stream::empty())
    }

    pub fn error(error: impl Error + Send + Sync + 'static) -> Self {
        Self::new(stream::iter(vec![Err(anyhow::Error::new(error))]))
    }
}

#[pin_project]
pub struct LoggedByteStream<Inner> {
    #[pin]
    inner: Inner,
}

impl<S: Stream<Item = Result<Vec<u8>, anyhow::Error>>> LoggedByteStream<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S: Stream<Item = Result<Vec<u8>, anyhow::Error>>> Stream for LoggedByteStream<S> {
    type Item = Result<Vec<u8>, anyhow::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let poll_result = this.inner.poll_next(cx);
        if let Poll::Ready(Some(Err(error))) = &poll_result {
            error!("Error in stream: {}", error);
        }
        poll_result
    }
}

pub struct AwsByteStream(aws_sdk_s3::primitives::ByteStream);

impl Stream for AwsByteStream {
    type Item = Result<Vec<u8>, anyhow::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0)
            .poll_next(cx)
            .map_ok(|b| b.to_vec())
            .map_err(|e| e.into())
    }
}

impl From<aws_sdk_s3::primitives::ByteStream> for ByteStream {
    fn from(stream: aws_sdk_s3::primitives::ByteStream) -> Self {
        Self::new(AwsByteStream(stream))
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::stream::ByteStream;
    use anyhow::Error;
    use futures::{stream, StreamExt, TryStreamExt};

    #[test]
    pub async fn test_byte_stream() {
        let stream = ByteStream::new(stream::iter(vec![Ok(vec![1, 2, 3]), Ok(vec![4, 5, 6])]));
        let stream_data: Vec<u8> = stream
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
            .into_iter()
            .flatten()
            .collect();

        assert_eq!(stream_data, vec![1, 2, 3, 4, 5, 6]);

        let stream = ByteStream::new(stream::iter(vec![
            Ok(vec![1, 2, 3]),
            Err(Error::msg("error1")),
            Ok(vec![4, 5, 6]),
            Err(Error::msg("error2")),
        ]));
        let result: Result<Vec<Vec<u8>>, Error> = stream.try_collect::<Vec<_>>().await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "error1".to_string());

        let stream = ByteStream::new(stream::iter(vec![
            Ok(vec![1, 2, 3]),
            Err(Error::msg("error1")),
            Ok(vec![4, 5, 6]),
            Ok(vec![7, 8, 9]),
        ]));

        let result = stream.collect::<Vec<Result<Vec<u8>, Error>>>().await.len();
        assert_eq!(result, 2);
    }
}
