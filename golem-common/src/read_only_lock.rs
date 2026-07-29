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

pub mod std {
    use std::sync::Arc;
    use std::sync::{RwLock, RwLockReadGuard};

    /// Version of std::sync::RwLock that only allows read access to the underlying data.
    /// Useful if you want to limit which parts of the code are allowed to modify certain data.
    pub struct ReadOnlyLock<T> {
        inner: Arc<RwLock<T>>,
    }

    impl<T> ReadOnlyLock<T> {
        pub fn new(underlying: Arc<RwLock<T>>) -> Self {
            Self {
                inner: underlying.clone(),
            }
        }

        pub fn read(&self) -> RwLockReadGuard<'_, T> {
            self.inner.read().unwrap()
        }
    }

    impl<T> Clone for ReadOnlyLock<T> {
        fn clone(&self) -> Self {
            Self {
                inner: self.inner.clone(),
            }
        }
    }
}

pub mod arc_swap {
    use arc_swap::ArcSwap;
    use std::sync::Arc;

    /// Read-only view of an [`ArcSwap`] cell that only allows loading the current value.
    /// Useful if you want to limit which parts of the code are allowed to publish new values.
    pub struct ReadOnlyView<T> {
        inner: Arc<ArcSwap<T>>,
    }

    impl<T> ReadOnlyView<T> {
        pub fn new(underlying: Arc<ArcSwap<T>>) -> Self {
            Self { inner: underlying }
        }

        /// Loads the currently published value. Lock-free; the returned `Arc` is a consistent
        /// snapshot that stays valid even if a new value is published afterwards.
        pub fn get(&self) -> Arc<T> {
            self.inner.load_full()
        }
    }

    impl<T> Clone for ReadOnlyView<T> {
        fn clone(&self) -> Self {
            Self {
                inner: self.inner.clone(),
            }
        }
    }
}

pub mod tokio {
    use std::sync::Arc;
    use tokio::sync::{RwLock, RwLockReadGuard};

    /// Version of tokio::sync::RwLock that only allows read access to the underlying data.
    /// Useful if you want to limit which parts of the code are allowed to modify certain data.
    pub struct ReadOnlyLock<T> {
        inner: Arc<RwLock<T>>,
    }

    impl<T> ReadOnlyLock<T> {
        pub fn new(underlying: Arc<RwLock<T>>) -> Self {
            Self {
                inner: underlying.clone(),
            }
        }

        pub async fn read(&self) -> RwLockReadGuard<'_, T> {
            self.inner.read().await
        }
    }

    impl<T> Clone for ReadOnlyLock<T> {
        fn clone(&self) -> Self {
            Self {
                inner: self.inner.clone(),
            }
        }
    }
}
