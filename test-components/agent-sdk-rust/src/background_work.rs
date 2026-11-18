use futures::FutureExt;
use golem_rust::agentic::spawn_local;
use golem_rust::{
    PromiseId, agent_definition, agent_implementation, await_promise, create_promise,
};
use std::cell::Cell;
use std::rc::Rc;

/// Counts the amounts other agents add, so tests count effects instead of echoes.
#[agent_definition]
pub trait Tally {
    fn new(name: String) -> Self;

    /// Creates a promise that `add_when_released` can wait for.
    fn create_release(&self) -> PromiseId;

    /// Waits for `release` when it is given, then adds `amount` and returns the new total.
    async fn add_when_released(&mut self, release: Option<PromiseId>, amount: u64) -> u64;

    fn total(&self) -> u64;
}

struct TallyImpl {
    total: u64,
}

#[agent_implementation]
impl Tally for TallyImpl {
    fn new(_name: String) -> Self {
        Self { total: 0 }
    }

    fn create_release(&self) -> PromiseId {
        create_promise()
    }

    async fn add_when_released(&mut self, release: Option<PromiseId>, amount: u64) -> u64 {
        if let Some(release) = release {
            await_promise(&release).await;
        }
        self.total += amount;
        self.total
    }

    fn total(&self) -> u64 {
        self.total
    }
}

/// Methods whose background work outlives their result.
#[agent_definition]
pub trait BackgroundWork {
    fn new(name: String) -> Self;

    /// Creates a promise that background work can wait for.
    fn create_release(&self) -> PromiseId;

    /// Spawns background work and returns at once. The background work waits for `release`,
    /// then adds `amount` on the tally agent named `tally` (which first waits for
    /// `tally_release` when it is given), then counts one completed background run.
    async fn add_after_release(
        &self,
        release: PromiseId,
        tally: String,
        tally_release: Option<PromiseId>,
        amount: u64,
    ) -> String;

    /// Waits for `release` in the method itself, then adds `amount` on the tally agent named
    /// `tally` and returns the new total.
    async fn add_before_return(&self, release: PromiseId, tally: String, amount: u64) -> u64;

    /// Spawns background work and returns at once. The background work waits for `release` and
    /// then panics.
    async fn trap_after_release(&self, release: PromiseId) -> String;

    /// Awaits its own spawned task, which awaits a local channel that nothing completes, so the
    /// method never returns.
    async fn deadlock_before_return(&self) -> String;

    /// Waits for `release` and for a short timer at once. The timer wins, and the method returns
    /// with no further host call, dropping the unfinished wait for `release`. With `background`,
    /// it first spawns background work that waits for `background`.
    async fn drop_unfinished_wait(
        &self,
        release: PromiseId,
        background: Option<PromiseId>,
    ) -> String;

    /// The number of background runs that completed.
    fn completions(&self) -> u64;
}

struct BackgroundWorkImpl {
    completions: Rc<Cell<u64>>,
}

#[agent_implementation]
impl BackgroundWork for BackgroundWorkImpl {
    fn new(_name: String) -> Self {
        Self {
            completions: Rc::default(),
        }
    }

    fn create_release(&self) -> PromiseId {
        create_promise()
    }

    async fn add_after_release(
        &self,
        release: PromiseId,
        tally: String,
        tally_release: Option<PromiseId>,
        amount: u64,
    ) -> String {
        let completions = self.completions.clone();
        spawn_local(async move {
            await_promise(&release).await;
            TallyClient::upsert(tally)
                .add_when_released(tally_release, amount)
                .await;
            completions.set(completions.get() + 1);
        });
        "returned".to_string()
    }

    async fn add_before_return(&self, release: PromiseId, tally: String, amount: u64) -> u64 {
        await_promise(&release).await;
        TallyClient::upsert(tally)
            .add_when_released(None, amount)
            .await
    }

    async fn trap_after_release(&self, release: PromiseId) -> String {
        spawn_local(async move {
            await_promise(&release).await;
            panic!("background work trapped after the result");
        });
        "returned".to_string()
    }

    async fn deadlock_before_return(&self) -> String {
        let (done, finished) = futures::channel::oneshot::channel::<()>();
        let (never, never_completed) = futures::channel::oneshot::channel::<()>();
        spawn_local(async move {
            let _never = never;
            let _ = never_completed.await;
            let _ = done.send(());
        });
        let _ = finished.await;
        "returned".to_string()
    }

    async fn drop_unfinished_wait(
        &self,
        release: PromiseId,
        background: Option<PromiseId>,
    ) -> String {
        if let Some(background) = background {
            let completions = self.completions.clone();
            spawn_local(async move {
                await_promise(&background).await;
                completions.set(completions.get() + 1);
            });
        }
        let released = await_promise(&release).fuse();
        let timer = golem_rust::wasip3::clocks::monotonic_clock::wait_for(10_000_000).fuse();
        futures::pin_mut!(released, timer);
        futures::select! {
            _ = released => "released".to_string(),
            _ = timer => "timed out".to_string(),
        }
    }

    fn completions(&self) -> u64 {
        self.completions.get()
    }
}
