// Copyright 2024-2025 Golem Cloud
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

use std::pin::Pin;
use std::task::{Context, Poll};
use std::future::Future;
use crate::{await_promise, PromiseId};

pub struct WebhookHandler {
    promise_id: PromiseId,
    inner: Option<Pin<Box<dyn Future<Output = Vec<u8>>>>>,
}

impl WebhookHandler {
    pub fn new(promise_id: PromiseId) -> Self {
        WebhookHandler {
            promise_id,
            inner: None,
        }
    }
}

impl Future for WebhookHandler {
    type Output = Vec<u8>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.inner.is_none() {
            self.inner = Some(Box::pin(await_promise(&self.promise_id)));
        }

        self.inner.as_mut().unwrap().as_mut().poll(cx)
    }
}