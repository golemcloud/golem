// Copyright 2024-2026 Golem Cloud
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

use std::cell::RefCell;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

type ResultFuture<T> = Pin<Box<dyn Future<Output = T>>>;
type ResultFactory<T> = Box<dyn FnOnce() -> ResultFuture<T>>;

enum State<T> {
    Initial(Option<ResultFactory<T>>),
    Polling(ResultFuture<T>),
    Ready(T),
}

#[derive(Default)]
struct ResultWake {
    waiters: Mutex<Vec<Waker>>,
}

impl ResultWake {
    fn register(&self, waker: &Waker) {
        let mut waiters = self.waiters.lock().expect("result waiters mutex poisoned");
        if !waiters.iter().any(|waiter| waiter.will_wake(waker)) {
            waiters.push(waker.clone());
        }
    }

    fn wake_waiters(&self) {
        let waiters =
            std::mem::take(&mut *self.waiters.lock().expect("result waiters mutex poisoned"));
        for waiter in waiters {
            waiter.wake();
        }
    }
}

impl Wake for ResultWake {
    fn wake(self: Arc<Self>) {
        self.wake_waiters();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake_waiters();
    }
}

/// Lazily drives one host observation and shares its terminal across all waiters.
pub(crate) struct InvocationResultDriver<T> {
    state: RefCell<State<T>>,
    wake: Arc<ResultWake>,
    source_waker: Waker,
}

impl<T: Clone> InvocationResultDriver<T> {
    pub(crate) fn new(factory: impl FnOnce() -> ResultFuture<T> + 'static) -> Self {
        let wake = Arc::new(ResultWake::default());
        Self {
            state: RefCell::new(State::Initial(Some(Box::new(factory)))),
            source_waker: Waker::from(Arc::clone(&wake)),
            wake,
        }
    }

    pub(crate) fn poll(&self, cx: &mut Context<'_>) -> Poll<T> {
        loop {
            let mut state = self.state.borrow_mut();
            match &mut *state {
                State::Initial(factory) => {
                    let future = factory
                        .take()
                        .expect("tool invocation result driver starts only once")(
                    );
                    *state = State::Polling(future);
                }
                State::Polling(future) => {
                    self.wake.register(cx.waker());
                    let mut source_context = Context::from_waker(&self.source_waker);
                    let Poll::Ready(result) = future.as_mut().poll(&mut source_context) else {
                        return Poll::Pending;
                    };
                    let result_for_caller = result.clone();
                    *state = State::Ready(result);
                    drop(state);
                    self.wake.wake_waiters();
                    return Poll::Ready(result_for_caller);
                }
                State::Ready(result) => return Poll::Ready(result.clone()),
            }
        }
    }

    pub(crate) async fn wait(self: Rc<Self>) -> T {
        poll_fn(|cx| self.poll(cx)).await
    }
}
