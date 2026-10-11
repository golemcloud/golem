//! A blocking wait that runs no task of the script.
//!
//! The guest runtime keeps each spawned task in one list until the task is polled for the first
//! time, and a blocking wait takes that list: it polls each task of the list, and it ends only
//! when each of them has ended. A command that blocks must do neither: the other tasks of the
//! script must stand still, as they do while a command reads a file.
//!
//! So each task of the script goes to the runtime in a [`ShellTask`]. While a blocking wait
//! runs, a `ShellTask` that is polled gives its task back without a poll and ends at once.
//! After the wait, the tasks that were given back are spawned again, in their order.

use futures::future::LocalBoxFuture;
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

thread_local! {
    /// Whether a blocking wait runs.
    static BLOCKED: Cell<bool> = const { Cell::new(false) };
    /// The tasks that the blocking wait took.
    static TAKEN: RefCell<Vec<LocalBoxFuture<'static, ()>>> = const { RefCell::new(Vec::new()) };
}

/// A task of the script, as the guest runtime holds it.
pub(crate) struct ShellTask(Option<LocalBoxFuture<'static, ()>>);

impl ShellTask {
    pub(crate) fn new(task: LocalBoxFuture<'static, ()>) -> Self {
        Self(Some(task))
    }
}

impl Future for ShellTask {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        // The runtime polls the tasks that run already only between the polls of a command, so
        // only a blocking wait can poll a task while `BLOCKED` is set.
        if BLOCKED.get() {
            if let Some(task) = self.0.take() {
                TAKEN.with_borrow_mut(|taken| taken.push(task));
            }
            return Poll::Ready(());
        }
        match self.0.as_mut() {
            Some(task) => task.as_mut().poll(cx),
            None => Poll::Ready(()),
        }
    }
}

/// Runs `wait`, which blocks, and then gives each task that the wait took to `spawn`.
pub(crate) fn uninterrupted<T>(wait: impl FnOnce() -> T, mut spawn: impl FnMut(ShellTask)) -> T {
    BLOCKED.set(true);
    let result = wait();
    BLOCKED.set(false);
    for task in TAKEN.take() {
        spawn(ShellTask::new(task));
    }
    result
}

#[cfg(test)]
mod tests;
