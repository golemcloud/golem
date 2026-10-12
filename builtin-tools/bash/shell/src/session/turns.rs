//! Where a running script is: the turns its tasks have taken.
//!
//! A script runs as tasks: the call's own, and one for each pipeline stage, background job and
//! other part of the script that runs beside it. A task takes a turn when it was woken: it runs
//! until it waits or gives way. The interpreter gives way after a fixed number of commands and
//! the commands after a fixed number of lines or reads, never by the clock. So which task takes
//! which turn depends only on what the script runs, as long as nothing from outside wakes a task
//! while another computes, and a second run of the same call finds the same place of the script
//! after the same number of turns. That needs both runs to start alike: the interpreter keeps
//! its own count of commands since it last gave way for the whole thread, and it is zero in a
//! new instance of the component, which every call gets.
//!
//! One script runs at a time on the thread, so the count is kept for the thread.
use std::{
    cell::{Cell, RefCell},
    pin::Pin,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

use futures::future::LocalBoxFuture;

use crate::ExecutionServices;

/// Starts a task of the script.
type SpawnLocal = fn(LocalBoxFuture<'static, ()>);
/// Lets a task give way to the others.
type YieldNow = fn() -> LocalBoxFuture<'static, ()>;

thread_local! {
    /// The embedder's own `spawn_local`.
    static SPAWN_LOCAL: Cell<Option<SpawnLocal>> = const { Cell::new(None) };
    /// The embedder's own `yield_now`.
    static YIELD_NOW: Cell<Option<YieldNow>> = const { Cell::new(None) };
    /// The script that runs. A task left over from an earlier script counts for nothing.
    static SCRIPT: Cell<u64> = const { Cell::new(0) };
    /// The turns the tasks of the script have taken.
    static TURN: Cell<u64> = const { Cell::new(0) };
    /// The tasks of the script that were woken and have not taken their turn.
    static WOKEN: Cell<usize> = const { Cell::new(0) };
    /// The tasks of the script that have not ended.
    static ALIVE: Cell<usize> = const { Cell::new(0) };
    /// The number of turns after which no task takes another before [`release`].
    static DUE: Cell<Option<u64>> = const { Cell::new(None) };
    /// The task that waits in [`reach`].
    static WAITING: RefCell<Option<Waker>> = const { RefCell::new(None) };
    /// The wakes that came to tasks of the script from outside it.
    static OUTSIDE: Cell<u64> = const { Cell::new(0) };
    /// The number of those wakes that a caller of [`reach`] started from, until [`release`].
    static WATCHED: Cell<Option<u64>> = const { Cell::new(None) };
    /// The task that takes its turn now.
    static TAKING: RefCell<Option<Arc<Wakes>>> = const { RefCell::new(None) };
}

/// `services` with every task the script starts counted as a [`Task`].
pub(super) fn counted(services: ExecutionServices) -> ExecutionServices {
    SPAWN_LOCAL.set(Some(services.spawn_local));
    YIELD_NOW.set(Some(services.yield_now));
    ExecutionServices {
        spawn_local: spawn_task,
        yield_now: give_way,
        ..services
    }
}

fn spawn_task(task: LocalBoxFuture<'static, ()>) {
    if let Some(spawn_local) = SPAWN_LOCAL.get() {
        spawn_local(Box::pin(Task::new(task)));
    }
}

fn give_way() -> LocalBoxFuture<'static, ()> {
    // A task that gives way takes another turn. Some executors wake it only later, after the
    // turn: that wake is the task's own.
    TAKING.with_borrow(|taking| {
        if let Some(task) = taking {
            task.note();
            task.gave_way.store(true, Ordering::Relaxed);
        }
    });
    match YIELD_NOW.get() {
        Some(yield_now) => yield_now(),
        None => Box::pin(std::future::ready(())),
    }
}

/// Starts the count for a new script.
pub(super) fn start() {
    SCRIPT.set(SCRIPT.get() + 1);
    TURN.set(0);
    WOKEN.set(0);
    ALIVE.set(0);
    OUTSIDE.set(0);
    release();
}

/// The turns the tasks of the script have taken.
pub(super) fn now() -> u64 {
    TURN.get()
}

/// Whether the script waits: no task of it was woken, so none takes a turn until something
/// from outside the script wakes one.
pub(super) fn waits() -> bool {
    WOKEN.get() == 0
}

/// The number of turns a caller of [`reach`] still waits for, or holds the script at.
pub(super) fn due() -> Option<u64> {
    DUE.get()
}

/// How many wakes have come to tasks of the script from outside it: wakes given while no task
/// of the script took a turn. A task of the script wakes another one in its own turn, so these
/// are the completions of what the script waits for, and the signals sent to it.
pub(super) fn wakes_from_outside() -> u64 {
    OUTSIDE.get()
}

/// What [`reach`] found.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Reached {
    /// The script is held where the caller looks for it.
    There,
    /// The script waits before it is there, and is not met where it waits.
    Waits,
    /// A task of the script was woken from outside before the script was there.
    Woken,
}

/// Looks for the script where its tasks have taken `turn` turns, and holds it there until
/// [`release`]: no task of it takes another turn. A script that is there already is not
/// running now, so it is held at once.
///
/// The script must get there by itself. `wakes` is the number of [`wakes_from_outside`] the
/// caller started from. A wake from outside after that changes what the script does on its
/// way, so this completes with [`Reached::Woken`] and holds the script where it is.
///
/// A script that [`waits`] before it is there takes no turn to get there. `in_a_wait` says
/// that the caller looks for the script in a wait. If the script's own task is then the only
/// one alive, that is where the caller wants it, however many turns it took: the script is
/// there, and the count goes on from `turn`. In every other case this completes with
/// [`Reached::Waits`]: the caller looks for a script that runs, or several tasks are alive and
/// where each of them is cannot be told.
pub(super) async fn reach(turn: u64, in_a_wait: bool, wakes: u64) -> Reached {
    DUE.set(Some(turn));
    WATCHED.set(Some(wakes));
    std::future::poll_fn(|context| {
        if OUTSIDE.get() != wakes {
            Poll::Ready(Reached::Woken)
        } else if TURN.get() >= turn {
            Poll::Ready(Reached::There)
        } else if !waits() {
            WAITING.set(Some(context.waker().clone()));
            Poll::Pending
        } else if in_a_wait && ALIVE.get() == 1 {
            TURN.set(turn);
            Poll::Ready(Reached::There)
        } else {
            Poll::Ready(Reached::Waits)
        }
    })
    .await
}

/// Lets the tasks of the script take turns again.
pub(super) fn release() {
    DUE.set(None);
    WATCHED.set(None);
    WAITING.take();
}

fn held() -> bool {
    DUE.get().is_some_and(|due| TURN.get() >= due)
        || WATCHED.get().is_some_and(|wakes| OUTSIDE.get() != wakes)
}

/// Wakes the task that waits in [`reach`] once the script is held, or no task of it was woken.
fn tell_waiting() {
    if DUE.get().is_some()
        && (held() || waits())
        && let Some(waiting) = WAITING.take()
    {
        waiting.wake();
    }
}

/// The call's own part of the script as a [`Task`].
pub(super) fn task<F: Future + Unpin>(script: F) -> Task<F> {
    Task::new(script)
}

/// A task of the script. It counts the turns it takes, and takes none while the script is held.
pub(super) struct Task<F> {
    future: F,
    wakes: Arc<Wakes>,
}

impl<F> Task<F> {
    fn new(future: F) -> Self {
        let wakes = Arc::new(Wakes {
            script: SCRIPT.get(),
            woken: AtomicBool::new(false),
            over: AtomicBool::new(false),
            gave_way: AtomicBool::new(false),
            executor: Mutex::new(None),
        });
        ALIVE.set(ALIVE.get() + 1);
        // A new task takes its first turn without a wake.
        wakes.note();
        Self { future, wakes }
    }
}

impl<F: Future + Unpin> Future for Task<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.wakes.woken_through(context.waker());
        // Polled for another reason than its own wake, the task has nothing to do.
        if !self.wakes.woken.load(Ordering::Relaxed) {
            return Poll::Pending;
        }
        if held() {
            // Woken again at once, the task keeps its place among the woken tasks.
            context.waker().wake_by_ref();
            return Poll::Pending;
        }
        self.wakes.taken();
        TURN.set(TURN.get() + 1);
        let waker = Waker::from(Arc::clone(&self.wakes));
        let taking = TAKING.replace(Some(Arc::clone(&self.wakes)));
        let poll = Pin::new(&mut self.future).poll(&mut Context::from_waker(&waker));
        TAKING.set(taking);
        if poll.is_ready() {
            self.wakes.end();
        }
        tell_waiting();
        poll
    }
}

impl<F> Drop for Task<F> {
    fn drop(&mut self) {
        self.wakes.end();
        tell_waiting();
    }
}

/// How a [`Task`] is woken: through this, so that the wake is seen.
struct Wakes {
    /// The script the task belongs to.
    script: u64,
    /// Whether the task was woken and has not taken its turn since.
    woken: AtomicBool,
    /// Whether the task is over and takes no further turn.
    over: AtomicBool,
    /// Whether the task gave way and its executor has not woken it for that yet.
    gave_way: AtomicBool,
    /// The task's waker in the embedder's executor.
    executor: Mutex<Option<Waker>>,
}

impl Wakes {
    fn counts(&self) -> bool {
        self.script == SCRIPT.get()
    }

    /// Notes that the task will take a turn.
    fn note(&self) {
        if !self.over.load(Ordering::Relaxed)
            && !self.woken.swap(true, Ordering::Relaxed)
            && self.counts()
        {
            WOKEN.set(WOKEN.get() + 1);
        }
    }

    /// Notes that the task takes its turn, or will take none.
    fn taken(&self) {
        if self.woken.swap(false, Ordering::Relaxed) && self.counts() {
            WOKEN.set(WOKEN.get().saturating_sub(1));
        }
    }

    fn end(&self) {
        if !self.over.swap(true, Ordering::Relaxed) && self.counts() {
            ALIVE.set(ALIVE.get().saturating_sub(1));
        }
        self.taken();
    }

    /// Whether a wake that comes now is the script's own. It is when a task of the script
    /// takes a turn now: that task gives it. It also is when this task gave way and its
    /// executor wakes it for that only after the turn. Every other wake comes from outside.
    ///
    /// The interpreter waits for the future of giving way where it asks for it, and the
    /// component's runtime wakes the task in that poll, so there the mark of giving way does
    /// not outlive the turn and cannot hide a completion.
    fn woken_by_the_script(&self) -> bool {
        let own_turn = TAKING.with_borrow(|taking| {
            taking
                .as_ref()
                .map(|task| std::ptr::eq(Arc::as_ptr(task), self))
        });
        match own_turn {
            Some(true) => {
                self.gave_way.store(false, Ordering::Relaxed);
                true
            }
            Some(false) => true,
            None => self.gave_way.swap(false, Ordering::Relaxed),
        }
    }

    /// Remembers the waker the embedder's executor polls the task with.
    fn woken_through(&self, waker: &Waker) {
        let mut executor = self.executor.lock().unwrap_or_else(PoisonError::into_inner);
        if !executor
            .as_ref()
            .is_some_and(|known| known.will_wake(waker))
        {
            *executor = Some(waker.clone());
        }
    }
}

impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.note();
        if !self.woken_by_the_script() && self.counts() && !self.over.load(Ordering::Relaxed) {
            OUTSIDE.set(OUTSIDE.get() + 1);
            tell_waiting();
        }
        let executor = self
            .executor
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(executor) = executor {
            executor.wake();
        }
    }
}
