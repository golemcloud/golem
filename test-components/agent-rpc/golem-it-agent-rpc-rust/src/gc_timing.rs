//! Agents whose RPC handles and futures are released at points that differ between the original
//! run and a run restored from an application snapshot. A Rust guest is deterministic, so the
//! snapshot restore is what selects a different release schedule: `new` starts with the live
//! schedule and `load_snapshot` switches to the schedule after restore. This is the same shape as
//! a garbage-collected guest whose restored heap collects handles at different points.

use crate::encode_single_parameter;
use golem_rust::bindings::golem::agent::host::{FutureInvokeResult, WasmRpc};
use golem_rust::bindings::wasi::keyvalue::{cache, types::OutgoingValue};
use golem_rust::{
    FromSchema, FromWire, IntoSchema, IntoWire, PromiseId, SchemaValue, WireSchema,
    agent_definition, agent_implementation, encode_schema_value, read_only,
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

#[agent_definition]
pub trait GcTimingTarget {
    fn new(name: String) -> Self;
    fn increment(&mut self) -> u64;
    fn get(&self) -> u64;
    async fn cache_value(&self, key: String, replacement: Option<String>) -> Option<String>;
}

struct GcTimingTargetImpl {
    count: u64,
}

#[agent_implementation]
impl GcTimingTarget for GcTimingTargetImpl {
    fn new(_name: String) -> Self {
        Self { count: 0 }
    }

    fn increment(&mut self) -> u64 {
        self.count += 1;
        self.count
    }

    fn get(&self) -> u64 {
        self.count
    }

    async fn cache_value(&self, key: String, replacement: Option<String>) -> Option<String> {
        if let Some(replacement) = replacement {
            let value = OutgoingValue::new_outgoing_value();
            value
                .outgoing_value_write_body_sync(replacement.as_bytes())
                .unwrap();
            cache::set(&key, &value, None).get().await.unwrap();
        }
        cache::get(&key)
            .get()
            .await
            .unwrap()
            .map(|value| String::from_utf8(value.incoming_value_consume_sync().unwrap()).unwrap())
    }
}

/// When the caller releases the clients it creates: `0` retains every client until
/// `release_retained`, `n > 0` releases every `n`th client as soon as its call returns and retains
/// the others. `1` releases every client immediately.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Serialize,
    Deserialize,
    IntoSchema,
    FromSchema,
    IntoWire,
    FromWire,
    WireSchema,
)]
pub struct ReleaseSchedule {
    pub every: u32,
    /// `release_retained` releases the newest client first.
    pub reverse: bool,
    /// `save_snapshot` releases the retained clients before it serializes the state.
    pub release_in_save: bool,
}

/// When `abandon` drops the future of its un-awaited call: `0` before the next call, `1` after
/// the next call, `2` never (the future is retained in the agent state).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Serialize,
    Deserialize,
    IntoSchema,
    FromSchema,
    IntoWire,
    FromWire,
    WireSchema,
)]
pub struct AbandonSchedule {
    pub drop_at: u32,
}

#[derive(Debug, Clone, PartialEq, IntoSchema, FromSchema, IntoWire, FromWire, WireSchema)]
pub struct GcTimingReport {
    pub completed: u64,
    pub checksum: u64,
    pub restored: bool,
    pub retained: u32,
    pub retained_futures: u32,
}

#[agent_definition(snapshotting = "enabled")]
pub trait GcTimingCaller {
    fn new(name: String) -> Self;
    fn configure(&mut self, release: ReleaseSchedule, release_after_restore: ReleaseSchedule);
    fn configure_abandon(&mut self, live: AbandonSchedule, after_restore: AbandonSchedule);
    fn create_promise(&self) -> PromiseId;
    /// Creates one client per iteration, increments the target through it and checks that the
    /// target saw exactly the increments of this caller.
    fn run(&mut self, count: u32) -> u64;
    fn release_retained(&mut self) -> u32;
    /// Releases the retained clients in a read-only invocation.
    #[read_only(cache = "no_cache")]
    fn release_retained_read_only(&self) -> u32;
    /// Releases the retained clients when `release_first` is set, runs `count` iterations
    /// (`run_two_targets` when `two_targets` is set), awaits `promise_id`, then runs one more
    /// iteration. An executor restart while it awaits replays the first part with the heap
    /// restored from the last snapshot.
    fn run_then_await(
        &mut self,
        release_first: bool,
        count: u32,
        two_targets: bool,
        promise_id: PromiseId,
    ) -> u64;
    /// Increments the target once and awaits `promise_id`. When the agent was restored from a
    /// snapshot it makes a different call instead: `extra` makes one more call first,
    /// `function` triggers instead of awaiting, `owner` calls another target.
    fn misbehave_then_await(&mut self, kind: String, promise_id: PromiseId) -> u64;
    /// Starts `inc_after_promise(gate)` on the blocking counter without awaiting it, records a
    /// positional `RemoveRetryPolicy` entry, increments the target, drops the un-awaited future at
    /// the scheduled point, then awaits `promise_id`.
    /// `cancel` cancels the future explicitly before the increment.
    fn abandon_then_await(
        &mut self,
        blocking_counter: String,
        gate: PromiseId,
        cancel: bool,
        promise_id: PromiseId,
    ) -> u64;
    /// Drops the futures `abandon_then_await` kept, then awaits `promise_id`.
    fn drop_retained_futures_then_await(&mut self, promise_id: PromiseId) -> u64;
    async fn abandon_cache_then_await(&mut self, promise_id: PromiseId);
    fn drop_retained_cache_then_await(&mut self, promise_id: PromiseId);
    async fn inspect_cache(&self) -> bool;
    async fn fill_cache_then_await(&mut self, promise_id: PromiseId);
    fn drop_retained_fills_then_await(&mut self, promise_id: PromiseId);
    fn inspect(&self) -> GcTimingReport;
}

#[derive(Serialize, Deserialize)]
struct GcTimingSnapshot {
    name: String,
    completed: u64,
    completed_b: u64,
    checksum: u64,
    release_after_restore: ReleaseSchedule,
    abandon_after_restore: AbandonSchedule,
}

struct GcTimingCallerImpl {
    name: String,
    completed: u64,
    completed_b: u64,
    checksum: u64,
    restored: bool,
    created: u32,
    release: ReleaseSchedule,
    release_after_restore: ReleaseSchedule,
    abandon: AbandonSchedule,
    abandon_after_restore: AbandonSchedule,
    retained: RefCell<Vec<WasmRpc>>,
    retained_futures: Vec<(WasmRpc, FutureInvokeResult)>,
    retained_fills: Vec<OutgoingValue>,
    retained_cache: Vec<(
        cache::FutureGetResult,
        cache::FutureExistsResult,
        cache::FutureGetOrSetResult,
        cache::FutureResult,
        cache::FutureResult,
    )>,
}

fn no_input() -> golem_rust::schema::wit::wire::SchemaValueTree {
    encode_schema_value(&SchemaValue::Record { fields: Vec::new() })
        .expect("failed to encode empty RPC input")
}

fn decode_u64(tree: golem_rust::schema::wit::wire::SchemaValueTree) -> u64 {
    u64::from_value(&golem_rust::decode_schema_value(tree).unwrap()).unwrap()
}

fn target(name: &str) -> WasmRpc {
    WasmRpc::new(
        "GcTimingTarget",
        encode_single_parameter(name.to_string()),
        None,
        Vec::new(),
    )
}

fn increment(rpc: &WasmRpc) -> u64 {
    decode_u64(
        rpc.invoke_and_await("increment", no_input(), None)
            .unwrap()
            .result
            .unwrap(),
    )
}

impl GcTimingCallerImpl {
    fn keep_or_release(&mut self, client: WasmRpc) {
        self.created += 1;
        let every = self.release.every;
        if every == 0 || !self.created.is_multiple_of(every) {
            self.retained.borrow_mut().push(client);
        } else {
            drop(client);
        }
    }

    fn release_all(&self) -> u32 {
        let mut retained = self.retained.borrow_mut();
        let released = retained.len() as u32;
        if self.release.reverse {
            while let Some(client) = retained.pop() {
                drop(client);
            }
        } else {
            for client in retained.drain(..) {
                drop(client);
            }
        }
        released
    }

    fn iterate(&mut self, count: u32) {
        for _ in 0..count {
            let client = target(&self.name);
            let observed = increment(&client);
            self.completed += 1;
            assert_eq!(
                observed, self.completed,
                "the target must see exactly the increments of this caller"
            );
            self.checksum += observed;
            self.keep_or_release(client);
        }
    }

    fn iterate_two_targets(&mut self, count: u32) {
        let other = format!("{}-b", self.name);
        for _ in 0..count {
            let a = target(&self.name);
            let b = target(&other);
            let observed_b = increment(&b);
            let observed_a = increment(&a);
            self.completed += 1;
            self.completed_b += 1;
            assert_eq!(observed_a, self.completed);
            assert_eq!(observed_b, self.completed_b);
            self.checksum += observed_a + observed_b;
            self.keep_or_release(a);
            self.keep_or_release(b);
        }
    }
}

#[agent_implementation]
impl GcTimingCaller for GcTimingCallerImpl {
    fn new(name: String) -> Self {
        let retain = ReleaseSchedule {
            every: 0,
            reverse: false,
            release_in_save: false,
        };
        let before_next = AbandonSchedule { drop_at: 0 };
        Self {
            name,
            completed: 0,
            completed_b: 0,
            checksum: 0,
            restored: false,
            created: 0,
            release: retain,
            release_after_restore: retain,
            abandon: before_next,
            abandon_after_restore: before_next,
            retained: RefCell::new(Vec::new()),
            retained_futures: Vec::new(),
            retained_fills: Vec::new(),
            retained_cache: Vec::new(),
        }
    }

    fn configure(&mut self, release: ReleaseSchedule, release_after_restore: ReleaseSchedule) {
        self.release = release;
        self.release_after_restore = release_after_restore;
    }

    fn configure_abandon(&mut self, live: AbandonSchedule, after_restore: AbandonSchedule) {
        self.abandon = live;
        self.abandon_after_restore = after_restore;
    }

    fn create_promise(&self) -> PromiseId {
        golem_rust::create_promise()
    }

    fn run(&mut self, count: u32) -> u64 {
        self.iterate(count);
        self.completed
    }

    fn release_retained(&mut self) -> u32 {
        self.release_all()
    }

    fn release_retained_read_only(&self) -> u32 {
        self.release_all()
    }

    fn run_then_await(
        &mut self,
        release_first: bool,
        count: u32,
        two_targets: bool,
        promise_id: PromiseId,
    ) -> u64 {
        if release_first {
            self.release_all();
        }
        if two_targets {
            self.iterate_two_targets(count);
        } else {
            self.iterate(count);
        }
        golem_rust::blocking_await_promise(&promise_id);
        self.iterate(1);
        self.completed
    }

    fn misbehave_then_await(&mut self, kind: String, promise_id: PromiseId) -> u64 {
        if self.restored {
            match kind.as_str() {
                "extra" => {
                    let client = target(&self.name);
                    increment(&client);
                    increment(&client);
                }
                "function" => {
                    target(&self.name)
                        .invoke("increment", no_input(), None)
                        .unwrap();
                }
                "owner" => {
                    increment(&target(&format!("{}-other", self.name)));
                }
                _ => panic!("unknown divergence: {kind}"),
            }
        } else {
            increment(&target(&self.name));
        }
        golem_rust::blocking_await_promise(&promise_id);
        self.completed += 1;
        self.completed
    }

    fn abandon_then_await(
        &mut self,
        blocking_counter: String,
        gate: PromiseId,
        cancel: bool,
        promise_id: PromiseId,
    ) -> u64 {
        let blocking = WasmRpc::new(
            "RpcBlockingCounter",
            encode_single_parameter(blocking_counter),
            None,
            Vec::new(),
        );
        let future = blocking
            .async_invoke_and_await("inc_after_promise", encode_single_parameter(gate), None)
            .future;
        if cancel {
            future.cancel();
        }
        let mut future = Some(future);
        if self.abandon.drop_at == 0 {
            drop(future.take());
        }
        golem_rust::retry::remove_retry_policy("gc-timing-unused");
        let observed = increment(&target(&self.name));
        self.completed += 1;
        assert_eq!(observed, self.completed);
        self.checksum += observed;
        match (self.abandon.drop_at, future) {
            (2, Some(future)) => self.retained_futures.push((blocking, future)),
            (_, future) => drop(future),
        }
        golem_rust::blocking_await_promise(&promise_id);
        self.completed
    }

    fn drop_retained_futures_then_await(&mut self, promise_id: PromiseId) -> u64 {
        for (rpc, future) in self.retained_futures.drain(..) {
            drop(future);
            drop(rpc);
        }
        golem_rust::blocking_await_promise(&promise_id);
        self.completed
    }

    fn inspect(&self) -> GcTimingReport {
        GcTimingReport {
            completed: self.completed,
            checksum: self.checksum,
            restored: self.restored,
            retained: self.retained.borrow().len() as u32,
            retained_futures: self.retained_futures.len() as u32,
        }
    }

    async fn abandon_cache_then_await(&mut self, promise_id: PromiseId) {
        let key = format!("{}-cache", self.name);
        let value = OutgoingValue::new_outgoing_value();
        value.outgoing_value_write_body_sync(b"original").unwrap();
        cache::set(&key, &value, None).get().await.unwrap();
        let replacement = OutgoingValue::new_outgoing_value();
        replacement
            .outgoing_value_write_body_sync(b"replacement")
            .unwrap();
        let mut futures = Some((
            cache::get(&key),
            cache::exists(&key),
            cache::get_or_set(&key),
            cache::set(&key, &replacement, None),
            cache::delete(&key),
        ));
        if self.abandon.drop_at == 0 {
            drop(futures.take());
        }
        golem_rust::retry::remove_retry_policy("gc-cache-unused");
        match (self.abandon.drop_at, futures) {
            (2, Some(futures)) => self.retained_cache.push(futures),
            (_, futures) => drop(futures),
        }
        golem_rust::blocking_await_promise(&promise_id);
    }

    fn drop_retained_cache_then_await(&mut self, promise_id: PromiseId) {
        self.retained_cache.clear();
        golem_rust::blocking_await_promise(&promise_id);
    }

    async fn inspect_cache(&self) -> bool {
        let key = format!("{}-cache", self.name);
        let value = cache::get(&key).get().await.unwrap().unwrap();
        value.incoming_value_consume_sync().unwrap() == b"original"
    }

    async fn fill_cache_then_await(&mut self, promise_id: PromiseId) {
        let key = format!("{}-fill", self.name);
        let cache::GetOrSetEntry::Vacant(vacancy) = cache::get_or_set(&key).get().await.unwrap()
        else {
            panic!("fill requires a vacant key")
        };
        let mut value = Some(vacancy.vacancy_fill(None));
        value
            .as_ref()
            .unwrap()
            .outgoing_value_write_body_sync(b"filled")
            .unwrap();
        if self.abandon.drop_at == 0 {
            drop(value.take());
        }
        golem_rust::retry::remove_retry_policy("gc-fill-unused");
        match (self.abandon.drop_at, value) {
            (2, Some(value)) => self.retained_fills.push(value),
            (_, value) => drop(value),
        }
        golem_rust::blocking_await_promise(&promise_id);
    }

    fn drop_retained_fills_then_await(&mut self, promise_id: PromiseId) {
        self.retained_fills.clear();
        golem_rust::blocking_await_promise(&promise_id);
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        if self.release.release_in_save {
            self.release_all();
        }
        serde_json::to_vec(&GcTimingSnapshot {
            name: self.name.clone(),
            completed: self.completed,
            completed_b: self.completed_b,
            checksum: self.checksum,
            release_after_restore: self.release_after_restore,
            abandon_after_restore: self.abandon_after_restore,
        })
        .map_err(|err| err.to_string())
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let snapshot: GcTimingSnapshot =
            serde_json::from_slice(&bytes).map_err(|err| err.to_string())?;
        Ok(Self {
            name: snapshot.name,
            completed: snapshot.completed,
            completed_b: snapshot.completed_b,
            checksum: snapshot.checksum,
            restored: true,
            created: 0,
            release: snapshot.release_after_restore,
            release_after_restore: snapshot.release_after_restore,
            abandon: snapshot.abandon_after_restore,
            abandon_after_restore: snapshot.abandon_after_restore,
            retained: RefCell::new(Vec::new()),
            retained_futures: Vec::new(),
            retained_fills: Vec::new(),
            retained_cache: Vec::new(),
        })
    }
}
