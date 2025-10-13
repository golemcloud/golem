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

#[cfg(feature = "tokio")]
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
