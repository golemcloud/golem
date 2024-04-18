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

use futures::stream;
use futures::Stream;
use std::error::Error;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::task::{Context, Poll};

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

#[cfg(test)]
mod tests {
    use crate::stream::ByteStream;
    use anyhow::Error;
    use futures::{stream, StreamExt, TryStreamExt};

    #[tokio::test]
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
