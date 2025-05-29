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

use bytes::Bytes;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::Stream;
use std::convert::Infallible;
use std::fmt::{Debug, Display};
use std::future::Future;

pub trait ReplayableStream: Send + Sync {
    type Item: 'static;
    type Error;

    fn make_stream(
        &self,
    ) -> impl Future<Output = Result<impl Stream<Item = Self::Item> + Send + 'static, Self::Error>> + Send;

    fn length(&self) -> impl Future<Output = Result<u64, String>> + Send;

    fn erased(self) -> internal::Erased<Self>
    where
        Self: Sized,
    {
        internal::Erased(self)
    }

    fn boxed<'a>(self) -> BoxReplayableStream<'a, Self::Item, Self::Error>
    where
        Self: Sized + 'a,
    {
        Box::new(internal::Erased(self))
    }

    fn map_item<Item2, F>(self, map_item: F) -> internal::MapItem<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item) -> Item2,
    {
        internal::MapItem {
            inner: self,
            map_item,
        }
    }

    fn map_error<E2, F>(self, map_err: F) -> internal::MapError<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Error) -> E2,
    {
        internal::MapError {
            inner: self,
            map_err,
        }
    }
}

/// Blanket impl for all reference types
impl<T: ReplayableStream + ?Sized> ReplayableStream for &'_ T {
    type Item = T::Item;
    type Error = T::Error;

    fn make_stream(
        &self,
    ) -> impl Future<Output = Result<impl Stream<Item = Self::Item> + Send + 'static, Self::Error>> + Send
    {
        <T as ReplayableStream>::make_stream(*self)
    }

    fn length(&self) -> impl Future<Output = Result<u64, String>> + Send {
        <T as ReplayableStream>::length(*self)
    }
}

/// For use in dyn contexts. You should not implement this directly, instead implement ReplayableStream
pub trait ErasedReplayableStream: Send + Sync {
    type Item: 'static;
    type Error;

    fn make_stream_erased(
        &self,
    ) -> BoxFuture<'_, Result<BoxStream<'static, Self::Item>, Self::Error>>;

    fn length_erased(&self) -> BoxFuture<'_, Result<u64, String>>;
}

pub type BoxReplayableStream<'a, Item, Error> =
    Box<dyn ErasedReplayableStream<Item = Item, Error = Error> + 'a>;

/// Specialized impls for the two common ways of using dynsafe objects
impl<Item: 'static, Error> ReplayableStream
    for &'_ dyn ErasedReplayableStream<Item = Item, Error = Error>
{
    type Item = Item;
    type Error = Error;

    async fn make_stream(
        &self,
    ) -> Result<impl Stream<Item = Self::Item> + Send + 'static, Self::Error> {
        self.make_stream_erased().await
    }

    async fn length(&self) -> Result<u64, String> {
        self.length_erased().await
    }
}

/// Specialized impls for the two common ways of using dynsafe objects
impl<Item: 'static, Error> ReplayableStream for BoxReplayableStream<'_, Item, Error> {
    type Item = Item;
    type Error = Error;

    async fn make_stream(
        &self,
    ) -> Result<impl Stream<Item = Self::Item> + Send + 'static, Self::Error> {
        self.make_stream_erased().await
    }

    async fn length(&self) -> Result<u64, String> {
        self.length_erased().await
    }
}

impl ReplayableStream for Bytes {
    type Error = Infallible;
    type Item = Result<Bytes, Infallible>;

    async fn make_stream(
        &self,
    ) -> Result<impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static, Infallible> {
        let data = self.clone();
        Ok(Box::pin(futures::stream::once(async move { Ok(data) })))
    }

    async fn length(&self) -> Result<u64, String> {
        Ok(self.len() as u64)
    }
}

pub trait ContentHash {
    type Error;

    fn content_hash(&self) -> impl Future<Output = Result<String, Self::Error>> + Send;
}

impl<Error, Stream> ContentHash for Stream
where
    Error: Debug + Display + Send + 'static,
    Stream: ReplayableStream<Error = Error, Item = Result<Bytes, Error>>,
{
    type Error = Error;

    async fn content_hash(&self) -> Result<String, Self::Error> {
        let stream = self
            .map_item(|i| i.map(|b| b.to_vec()).map_err(HashingError))
            .make_stream()
            .await?;
        let hash = async_hash::hash_try_stream::<async_hash::Sha256, _, _, _>(stream)
            .await
            .map_err(|HashingError(inner)| inner)?;
        Ok(hex::encode(hash))
    }
}

#[derive(Debug)]
struct HashingError<E>(E);

impl<E: Display> std::fmt::Display for HashingError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Hashing error: {}", self.0)
    }
}

impl<E: Debug + Display> std::error::Error for HashingError<E> {}

pub mod internal {
    use super::{ErasedReplayableStream, ReplayableStream};
    use futures::future::BoxFuture;
    use futures::stream::BoxStream;
    use futures::Stream;
    use futures::StreamExt;

    pub struct Erased<T>(pub(super) T);

    impl<T: ReplayableStream> ErasedReplayableStream for Erased<T> {
        type Error = T::Error;
        type Item = T::Item;

        fn make_stream_erased(
            &self,
        ) -> BoxFuture<'_, Result<BoxStream<'static, Self::Item>, Self::Error>> {
            Box::pin(async move { self.0.make_stream().await.map(|s| s.boxed()) })
        }

        fn length_erased(&self) -> BoxFuture<'_, Result<u64, String>> {
            Box::pin(self.0.length())
        }
    }

    pub struct MapItem<Inner, F> {
        pub(super) inner: Inner,
        pub(super) map_item: F,
    }

    impl<Inner, F, I2> ReplayableStream for MapItem<Inner, F>
    where
        Inner: ReplayableStream,
        F: FnMut(Inner::Item) -> I2 + Send + Sync + Clone + 'static,
        I2: 'static,
    {
        type Error = Inner::Error;
        type Item = I2;

        async fn make_stream(
            &self,
        ) -> Result<impl Stream<Item = I2> + Send + 'static, Inner::Error> {
            let stream = self.inner.make_stream().await?;
            Ok(stream.map(self.map_item.clone()))
        }

        async fn length(&self) -> Result<u64, String> {
            self.inner.length().await
        }
    }

    pub struct MapError<Inner, F> {
        pub(super) inner: Inner,
        pub(super) map_err: F,
    }

    impl<Inner, F, E2> ReplayableStream for MapError<Inner, F>
    where
        Inner: ReplayableStream,
        F: FnMut(Inner::Error) -> E2 + Send + Sync + Clone,
    {
        type Error = E2;
        type Item = Inner::Item;

        async fn make_stream(
            &self,
        ) -> Result<impl Stream<Item = Inner::Item> + Send + 'static, Self::Error> {
            self.inner.make_stream().await.map_err(self.map_err.clone())
        }

        async fn length(&self) -> Result<u64, String> {
            self.inner.length().await
        }
    }
}
